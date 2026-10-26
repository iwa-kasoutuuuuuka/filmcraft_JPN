//! Wondershare Filmora projects (`.wfp`, Filmora 12–16), read only.
//!
//! A `.wfp` is a ZIP archive. `ProjectFolder/project_info.json` names the timeline document
//! (`timeline_mediaId`) and the sequence format; `ProjectFolder/Medias/<id>/timeline.wesproj` is
//! that document, JSON:
//!
//! - `resources[]`: the media files (`filename` as a `file:/` URL, `mediaLength` and stream info);
//! - `timelineInfos[]`: timelines (the main one is `currentTimelineId`; compound clips and titles
//!   have their own), each with `trackInfos[]` (bottom to top; `trackType` 1 video, 2 audio) of
//!   `clipList[]`.
//!
//! Times are in 100 ns units. A clip has its timeline range (`tlBegin`..=`tlEnd`), its source
//! range (`inPoint`..`outPoint`) and `type`: 1 video / image, 2 audio, 6 / 16 the video / audio of a
//! compound clip (a nested timeline), 7 a title (a nested timeline whose clip carries the text in
//! `scriptBuf`), 26 a drawn graphic.
//!
//! Read: tracks, media clips (speed, reverse; fitted to the frame with Filmora's position, scale
//! and rotation as Motion), compound clips as nested sequences, titles as text graphics (text,
//! font, alignment, position, size, colour, background), drawn graphics as shape layers
//! (rectangles, arrows and pen strokes with their stroke and fill, placed by their transform),
//! links between a file's video and audio.
//! Clip settings: audio volume (Volume level) and fades (Constant Gain transitions from / to
//! silence), the crop (a Crop effect that fills the frame).
//! Reported, not read: volume keyframes and other effects that carry settings (Filmora writes no
//! parameters for effects left at their defaults; those need nothing), pen curves (read as
//! straight segments), speed ramps (read as the average speed over the same duration).
//!
//! Media paths are kept as the project wrote them; the engine looks for missing files relative to
//! the folder the `.wfp` was opened from ([`original_project_dir`]).

use std::collections::{BTreeMap, HashMap, HashSet};

use filmcraft_geom::Vec2;
use filmcraft_media::MediaKind;
use filmcraft_project::{ItemId, ItemKind, Label, Param, ParamValue, Sequence, TrackItem, TrackKind};
use filmcraft_time::{FrameRate, Tick};
use serde_json::Value;

use crate::common::{Builder, MediaSpec, empty_sequence, file_name, kind_from_ext, settings_for};
use crate::{Error, ImportOptions, Imported, Report, Result};

/// Filmora's time unit: 100 ns.
const UNITS_PER_SECOND: i64 = 10_000_000;
/// Largest archive member read (the timeline document of a long project is a few MB).
const MAX_MEMBER: usize = 256 << 20;
/// Most central-directory entries walked.
const MAX_ENTRIES: usize = 200_000;
/// Deepest nesting of compound clips followed.
const MAX_DEPTH: usize = 32;
/// Most clips read from one timeline.
const MAX_CLIPS: usize = 100_000;

const PROJECT_INFO: &str = "ProjectFolder/project_info.json";

// ---------------------------------------------------------------------------------------------
// ZIP (stored and deflated members; no ZIP64, no encryption)
// ---------------------------------------------------------------------------------------------

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([*s.first()?, *s.get(1)?]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?]))
}

/// One archive member: where its data starts, how it is stored.
#[derive(Clone, Debug)]
struct Member {
    method: u16,
    flags: u16,
    compressed: usize,
    size: usize,
    local_header: usize,
}

/// The members of a ZIP archive by name.
fn members(b: &[u8]) -> Result<HashMap<String, Member>> {
    // end of central directory: within the last 64 KiB + 22 bytes
    let from = b.len().saturating_sub(65_557);
    let tail = b.get(from..).unwrap_or(&[]);
    let eocd = tail
        .windows(4)
        .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
        .map(|p| from + p)
        .ok_or_else(|| Error::Other("not a Filmora project (no ZIP directory)".into()))?;
    let count = usize::from(u16_at(b, eocd + 10).unwrap_or(0));
    let cd_offset = u32_at(b, eocd + 16).ok_or_else(|| Error::Other("damaged Filmora project (ZIP directory)".into()))?;
    if cd_offset == u32::MAX || count == usize::from(u16::MAX) {
        return Err(Error::Other("Filmora projects larger than 4 GB (ZIP64) are not supported".into()));
    }
    let mut out = HashMap::new();
    let mut at = cd_offset as usize;
    for _ in 0..count.min(MAX_ENTRIES) {
        if u32_at(b, at) != Some(0x0201_4b50) {
            break;
        }
        let field = |o: usize| u16_at(b, at + o).map(usize::from).unwrap_or(0);
        let word = |o: usize| u32_at(b, at + o).map(|v| v as usize).unwrap_or(0);
        let (name_len, extra_len, comment_len) = (field(28), field(30), field(32));
        let name_start = at + 46;
        let name = b.get(name_start..name_start.saturating_add(name_len)).map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
        out.insert(
            name,
            Member {
                method: u16_at(b, at + 10).unwrap_or(0),
                flags: u16_at(b, at + 8).unwrap_or(0),
                compressed: word(20),
                size: word(24),
                local_header: word(42),
            },
        );
        at = name_start.saturating_add(name_len).saturating_add(extra_len).saturating_add(comment_len);
    }
    Ok(out)
}

/// The bytes of member `name`.
fn read_member(b: &[u8], members: &HashMap<String, Member>, name: &str) -> Result<Vec<u8>> {
    let m = members.get(name).ok_or_else(|| Error::Other(format!("Filmora project: `{name}` is missing")))?;
    if m.flags & 1 != 0 {
        return Err(Error::Other("encrypted Filmora projects are not supported".into()));
    }
    if m.size > MAX_MEMBER {
        return Err(Error::Other(format!("Filmora project: `{name}` is too large")));
    }
    let lh = m.local_header;
    if u32_at(b, lh) != Some(0x0403_4b50) {
        return Err(Error::Other(format!("damaged Filmora project (`{name}`)")));
    }
    let skip = usize::from(u16_at(b, lh + 26).unwrap_or(0)) + usize::from(u16_at(b, lh + 28).unwrap_or(0));
    let start = lh.saturating_add(30).saturating_add(skip);
    let data = b.get(start..start.saturating_add(m.compressed)).ok_or_else(|| Error::Other(format!("damaged Filmora project (`{name}` is cut short)")))?;
    match m.method {
        0 => Ok(data.to_vec()),
        8 => miniz_oxide::inflate::decompress_to_vec_with_limit(data, MAX_MEMBER)
            .map_err(|_| Error::Other(format!("damaged Filmora project (`{name}` does not inflate)"))),
        other => Err(Error::Other(format!("Filmora project: `{name}` uses ZIP method {other}, which is not supported"))),
    }
}

fn read_json(b: &[u8], members: &HashMap<String, Member>, name: &str) -> Result<Value> {
    let bytes = read_member(b, members, name)?;
    let text = String::from_utf8_lossy(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes)).into_owned();
    serde_json::from_str(&text).map_err(|e| Error::Other(format!("Filmora project: `{name}`: {e}")))
}

/// Whether `bytes` look like a Filmora project (a ZIP with `ProjectFolder/project_info.json`).
pub fn sniff(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x50, 0x4b, 0x03, 0x04]) && members(bytes).is_ok_and(|m| m.contains_key(PROJECT_INFO))
}

// ---------------------------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------------------------

/// A Filmora `file:/…` reference as a path (`file:/E:/a/b.mp4` → `E:/a/b.mp4`,
/// `file:/Users/x.mov` → `/Users/x.mov`); other forms through [`crate::file_url_to_path`].
pub fn path_of(reference: &str) -> String {
    let lower = reference.get(..7).map(str::to_ascii_lowercase);
    if lower.as_deref() == Some("file://") {
        return crate::file_url_to_path(reference);
    }
    match reference.get(..6).map(str::to_ascii_lowercase).as_deref() {
        Some("file:/") => {
            let rest = reference.get(6..).unwrap_or("");
            let drive = rest.as_bytes().get(1) == Some(&b':') && rest.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
            if drive { rest.to_string() } else { format!("/{}", rest.trim_start_matches('/')) }
        }
        _ => reference.to_string(),
    }
}

/// The folder the project was saved in (`proj_zip_save_path`'s parent), when it says. Media under
/// it can be looked for relative to wherever the `.wfp` is now.
pub fn original_project_dir(bytes: &[u8]) -> Option<String> {
    let m = members(bytes).ok()?;
    let info = read_json(bytes, &m, PROJECT_INFO).ok()?;
    let saved = info.get("proj_zip_save_path")?.as_str()?.replace('\\', "/");
    let cut = saved.rfind('/')?;
    saved.get(..cut).map(str::to_string).filter(|d| !d.is_empty())
}

// ---------------------------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------------------------

fn tick(units: i64) -> Tick {
    Tick::from_units(units, UNITS_PER_SECOND)
}

fn int(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(|x| x.as_i64().or_else(|| x.as_f64().filter(|f| f.is_finite()).map(|f| f as i64)))
}

fn rate_of(v: Option<&Value>) -> Option<FrameRate> {
    let v = v?;
    let (num, den) = (int(v, "num")?, int(v, "den")?);
    (num > 0 && den > 0 && num / den <= 1000).then(|| FrameRate::new(num, den))
}

/// What a resource (media file) says about itself.
fn spec_of(r: &Value, path: &str) -> MediaSpec {
    let stream_type = int(r, "streamType").unwrap_or(0);
    let length = int(r, "mediaLength").filter(|l| *l > 0).map(tick);
    let vid = r.get("vidStreamInfo").and_then(Value::as_array).and_then(|a| a.first());
    let aud = r.get("audStreamInfo").and_then(Value::as_array).and_then(|a| a.first());
    let video = vid.and_then(|v| {
        let (w, h) = (int(v, "width")?, int(v, "height")?);
        let rate = rate_of(v.get("frameRate")).unwrap_or(FrameRate::FPS_30);
        (w > 0 && h > 0 && w <= 65_536 && h <= 65_536).then_some((w as u32, h as u32, rate))
    });
    let audio = aud.and_then(|a| {
        let (sr, ch) = (int(a, "sampleRate")?, int(a, "channels")?);
        (sr > 0 && sr <= 768_000 && ch > 0 && ch <= 64).then_some((sr as u32, ch as u32))
    });
    let kind = match stream_type {
        5 => Some(MediaKind::Still),
        3 => Some(MediaKind::AudioOnly),
        _ => match kind_from_ext(path) {
            MediaKind::Still => Some(MediaKind::Still),
            _ => None,
        },
    };
    MediaSpec { duration: length, video: video.filter(|_| stream_type != 3), audio, start_tc: None, kind }
}

/// A clip's constant speed and reverse flag. A speed ramp becomes the average speed that plays its
/// source range (`speed.offset`..`offsetEnd`, seconds) in the clip's `dur`, with that range's start
/// as the source in point (the third value).
fn speed_of(c: &Value, dur: Tick, report: &mut Report) -> (f64, bool, Option<Tick>) {
    let Some(sp) = c.get("speed") else { return (1.0, false, None) };
    let reverse = sp.get("reverse").and_then(Value::as_bool).unwrap_or(false);
    let keys: Vec<f64> = sp
        .get("speedParam")
        .and_then(Value::as_str)
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.get("keyframeSets").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|k| k.get("_value").and_then(Value::as_f64))
        .filter(|v| v.is_finite() && *v > 0.0)
        .collect();
    let secs = |k: &str| sp.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0 && *v < 1e7);
    let mut src_in = None;
    let speed = match keys.as_slice() {
        [] => 1.0,
        [first, rest @ ..] => {
            if rest.iter().any(|v| (v - first).abs() > 1e-6) {
                report.warn("speed ramps were read as the average speed (same duration, constant rate)");
                let clip = dur.seconds();
                match (secs("offset"), secs("offsetEnd")) {
                    (Some(a), Some(b)) if b > a && clip > 0.0 => {
                        src_in = Some(Tick::from_seconds_f64(a));
                        (b - a) / clip
                    }
                    _ => keys.iter().sum::<f64>() / keys.len() as f64,
                }
            } else {
                *first
            }
        }
    };
    (speed.clamp(0.01, 100.0), reverse, src_in)
}

/// Linear fades (seconds) at a clip's start and end.
struct Fades {
    fade_in: f64,
    fade_out: f64,
}

/// Read the settings FilmCraft has a place for: audio volume (dB) and fades, the video crop (a
/// Crop effect that fills the frame). Returns the audio fades to add as transitions.
fn apply_settings(ti: &mut TrackItem, c: &Value, kind: TrackKind, report: &mut Report) -> Option<Fades> {
    let effects = effects_of(c);
    let find = |id: &str| effects.iter().copied().find(|e| effect_id(e) == id && enabled(e) && has_params(e));
    if kind == TrackKind::Audio {
        if let Some(db) = find("audio/effect/volume").and_then(|e| fx_f64(e, "VolumeGain")).filter(|v| v.is_finite()) {
            crate::common::set_param(ti, "volume", "level", Param::new(ParamValue::Float(db.clamp(-287.5, 15.0))));
        }
        let fades = find("audio/effect/fade").map(|e| {
            let s = |k: &str| fx_f64(e, k).filter(|v| v.is_finite()).unwrap_or(0.0).clamp(0.0, 3600.0);
            Fades { fade_in: s("FadeInTime"), fade_out: s("FadeOutTime") }
        });
        return fades.filter(|f| f.fade_in > 0.0 || f.fade_out > 0.0);
    }
    if let Some(script) = find("video/effect/crop-pan-zoom").and_then(|e| fx_param(e, "SettingScript")).and_then(Value::as_str) {
        let keys: Vec<Value> = serde_json::from_str::<Value>(script).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
        if keys.len() > 1 {
            report.warn("Filmora pan & zoom animations were read as their first crop");
        }
        let g = |k: &Value, n: &str| k.get(n).and_then(Value::as_f64).filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1.0));
        if let Some(k) = keys.first()
            && let (Some(x), Some(y), Some(w), Some(h)) = (g(k, "X"), g(k, "Y"), g(k, "Width"), g(k, "Height"))
            && w > 1e-3
            && h > 1e-3
        {
            let pct = |v: f64| Param::new(ParamValue::Float((v * 100.0).clamp(0.0, 100.0)));
            crate::common::set_param(ti, "crop", "left", pct(x));
            crate::common::set_param(ti, "crop", "top", pct(y));
            crate::common::set_param(ti, "crop", "right", pct(1.0 - x - w));
            crate::common::set_param(ti, "crop", "bottom", pct(1.0 - y - h));
            // Filmora shows the cropped part full frame; Crop's zoom stretches each axis to the
            // frame, so it only matches when the crop keeps the frame's shape
            if (w - h).abs() < 0.01 {
                crate::common::set_param(ti, "crop", "zoom", Param::new(ParamValue::Bool(true)));
            } else {
                report.warn("Filmora crops of another shape than the frame were not enlarged to fill it");
            }
        }
    }
    None
}

/// Every effect of a clip, in chain order.
fn effects_of(c: &Value) -> Vec<&Value> {
    c.get("effectChainList")
        .and_then(Value::as_array)
        .map(|chains| chains.iter().flat_map(|ch| ch.get("effectList").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])).collect())
        .unwrap_or_default()
}

fn effect_id(e: &Value) -> &str {
    e.get("id").and_then(Value::as_str).unwrap_or("")
}

/// `paramList[name].fxParam.unValue` of an effect.
fn fx_param<'v>(e: &'v Value, name: &str) -> Option<&'v Value> {
    e.get("paramList")?.as_array()?.iter().find(|p| p.get("name").and_then(Value::as_str) == Some(name))?.get("fxParam")?.get("unValue")
}

fn fx_f64(e: &Value, name: &str) -> Option<f64> {
    fx_param(e, name).and_then(Value::as_f64).filter(|v| v.is_finite())
}

fn has_params(e: &Value) -> bool {
    e.get("paramList").and_then(Value::as_array).is_some_and(|l| !l.is_empty())
}

/// Effects can be switched off (`"enable": false`); absent means on.
fn enabled(e: &Value) -> bool {
    e.get("enable").and_then(Value::as_bool) != Some(false)
}

/// A drawn graphic's colour: 0xRRGGBB.
fn rgb_of(c: i64) -> [f32; 3] {
    let c = (c as u32) & 0x00ff_ffff;
    [((c >> 16) & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, (c & 0xff) as f32 / 255.0]
}

/// Filmora's stroke width unit in sequence pixels (measured on its renders: a 4 is about 7 px at
/// 1920 × 984, whatever the shape's scale).
const STROKE_PX: f64 = 1.8;

/// A drawn graphic's canvas (the frame) placed by its transform: canvas pixel `q` lands at
/// `position + R(rotation) · ((q − anchor) ⊙ scale)`.
struct Placement {
    position: (f64, f64),
    anchor: (f64, f64),
    scale: (f64, f64),
    /// Radians, clockwise (y down).
    rotation: f64,
}

impl Placement {
    fn of(c: &Value, (w, h): (f64, f64)) -> Self {
        let t = effects_of(c).into_iter().find(|e| effect_id(e) == "video/effect/transform");
        let get = |k: &str, d: f64| t.and_then(|e| fx_f64(e, k)).unwrap_or(d);
        Placement {
            position: (get("Position_x", 0.5).clamp(-10.0, 10.0) * w, get("Position_y", 0.5).clamp(-10.0, 10.0) * h),
            anchor: (get("_Anchor_x", 0.5).clamp(-10.0, 10.0) * w, get("_Anchor_y", 0.5).clamp(-10.0, 10.0) * h),
            scale: (get("Scale_x", 100.0).clamp(0.0, 10_000.0) / 100.0, get("Scale_y", 100.0).clamp(0.0, 10_000.0) / 100.0),
            rotation: get("Rotation", 0.0).clamp(-36_000.0, 36_000.0).to_radians(),
        }
    }

    fn map(&self, (x, y): (f64, f64)) -> [f32; 2] {
        let (dx, dy) = ((x - self.anchor.0) * self.scale.0, (y - self.anchor.1) * self.scale.1);
        let (sin, cos) = self.rotation.sin_cos();
        [(self.position.0 + dx * cos - dy * sin) as f32, (self.position.1 + dx * sin + dy * cos) as f32]
    }
}

/// The outlines of a drawn graphic in canvas pixels: (points, closed).
fn outlines(c: &Value, (w, h): (f64, f64), report: &mut Report) -> Vec<(Vec<(f64, f64)>, bool)> {
    let mut out = Vec::new();
    for e in effects_of(c).into_iter().filter(|e| enabled(e)) {
        let bbox = || {
            let g = |k: &str| fx_f64(e, k).unwrap_or(0.0).clamp(-10.0, 10.0);
            (g("start_x") * w, g("start_y") * h, g("width") * w, g("height") * h)
        };
        match effect_id(e) {
            "graphic/effect/path_graphic_rect" => {
                let (x, y, bw, bh) = bbox();
                out.push((vec![(x, y), (x + bw, y), (x + bw, y + bh), (x, y + bh)], true));
            }
            "graphic/effect/path_graphic_arrow" => {
                // a right-pointing block arrow filling the box: shaft 40 % of its height, head 35 %
                // of its length (Filmora's arrow, measured on its renders)
                let (x, y, bw, bh) = bbox();
                let (cy, head) = (y + bh / 2.0, x + bw - 0.35 * bw.abs().min(bh.abs() * 2.5));
                let (shaft, half) = (0.2 * bh, 0.5 * bh);
                out.push((
                    vec![(x, cy - shaft), (head, cy - shaft), (head, cy - half), (x + bw, cy), (head, cy + half), (head, cy + shaft), (x, cy + shaft)],
                    true,
                ));
            }
            "graphic/effect/path" => {
                let mask = e.get("effectMask").and_then(|m| m.get("custom_mask")).and_then(Value::as_str).and_then(|m| serde_json::from_str::<Value>(m).ok());
                for path in mask.as_ref().and_then(|m| m.get("paths")).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
                    if path.get("enable").and_then(Value::as_bool) == Some(false) {
                        continue;
                    }
                    let pts: Vec<(f64, f64)> = path
                        .get("points")
                        .and_then(Value::as_array)
                        .map(Vec::as_slice)
                        .unwrap_or(&[])
                        .iter()
                        .take(10_000)
                        .filter_map(|q| {
                            let (x, y) = (q.get("value_x")?.as_f64()?, q.get("value_y")?.as_f64()?);
                            (x.is_finite() && y.is_finite()).then(|| (x.clamp(-10.0, 10.0) * w, y.clamp(-10.0, 10.0) * h))
                        })
                        .collect();
                    if pts
                        .iter()
                        .zip(path.get("points").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]))
                        .any(|(_, q)| q.get("type").and_then(Value::as_i64).is_some_and(|t| t != 0))
                    {
                        report.warn("curved pen strokes were read as straight segments");
                    }
                    if pts.len() >= 2 {
                        out.push((pts, path.get("is_closed").and_then(Value::as_bool).unwrap_or(false)));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// A thick segment `a`–`b` (`width` px, ends extended by half the width so joints close) as a
/// quadrilateral.
fn segment(a: [f32; 2], b: [f32; 2], width: f32) -> Vec<[f32; 2]> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len.is_nan() || len <= 1e-3 {
        return Vec::new();
    }
    let (ux, uy) = (dx / len, dy / len);
    let (hx, hy) = (-uy * width / 2.0, ux * width / 2.0);
    let (a, b) = ([a[0] - ux * width / 2.0, a[1] - uy * width / 2.0], [b[0] + ux * width / 2.0, b[1] + uy * width / 2.0]);
    vec![[a[0] + hx, a[1] + hy], [b[0] + hx, b[1] + hy], [b[0] - hx, b[1] - hy], [a[0] - hx, a[1] - hy]]
}

/// An arrowhead at `tip` pointing away from `from` for a `width` px stroke (about 7 widths long and
/// 5 wide, measured on Filmora's renders): the triangle, and where the line should stop.
fn arrowhead(from: [f32; 2], tip: [f32; 2], width: f32) -> Option<(Vec<[f32; 2]>, [f32; 2])> {
    let (dx, dy) = (tip[0] - from[0], tip[1] - from[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len.is_nan() || len <= 1e-3 {
        return None;
    }
    let (ux, uy) = (dx / len, dy / len);
    let (head, half) = ((7.0 * width).min(len), 2.6 * width);
    let base = [tip[0] - ux * head, tip[1] - uy * head];
    Some((vec![tip, [base[0] - uy * half, base[1] + ux * half], [base[0] + uy * half, base[1] - ux * half]], base))
}

/// Filmora's transform of a clip (`video/effect/transform`): centre position as fractions of the
/// frame, scale in percent of the fitted size, rotation in degrees. Absent values are Filmora's
/// defaults (centred, 100 %, no rotation).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Transform {
    position: (f64, f64),
    scale: (f64, f64),
    rotation: f64,
}

fn transform_of(c: &Value) -> Option<Transform> {
    let params = c
        .get("effectChainList")
        .and_then(Value::as_array)?
        .iter()
        .flat_map(|ch| ch.get("effectList").and_then(Value::as_array).cloned().unwrap_or_default())
        .find(|e| e.get("id").and_then(Value::as_str) == Some("video/effect/transform"))?
        .get("paramList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let get = |name: &str| {
        params
            .iter()
            .find(|p| p.get("name").and_then(Value::as_str) == Some(name))
            .and_then(|p| p.get("fxParam").and_then(|f| f.get("unValue")).and_then(Value::as_f64))
            .filter(|v| v.is_finite())
    };
    let t = Transform {
        position: (get("Position_x").unwrap_or(0.5).clamp(-10.0, 10.0), get("Position_y").unwrap_or(0.5).clamp(-10.0, 10.0)),
        scale: (get("Scale_x").unwrap_or(100.0).clamp(0.0, 10_000.0), get("Scale_y").unwrap_or(100.0).clamp(0.0, 10_000.0)),
        rotation: get("Rotation").unwrap_or(0.0).clamp(-36_000.0, 36_000.0),
    };
    (t != Transform { position: (0.5, 0.5), scale: (100.0, 100.0), rotation: 0.0 }).then_some(t)
}

/// Fit the clip to the frame (Filmora's default) and apply its transform as Motion.
fn apply_transform(ti: &mut TrackItem, c: &Value, (w, h): (u32, u32)) {
    ti.scale_to_frame = true;
    let Some(t) = transform_of(c) else { return };
    let set = |ti: &mut TrackItem, k: &str, v: ParamValue| crate::common::set_param(ti, "motion", k, Param::new(v));
    set(ti, "position", ParamValue::Vec2(Vec2::new(t.position.0 * f64::from(w), t.position.1 * f64::from(h))));
    if (t.scale.0 - t.scale.1).abs() < 1e-6 {
        set(ti, "scale", ParamValue::Float(t.scale.0));
    } else {
        set(ti, "uniform_scale", ParamValue::Bool(false));
        set(ti, "scale", ParamValue::Float(t.scale.1));
        set(ti, "scale_width", ParamValue::Float(t.scale.0));
    }
    if t.rotation != 0.0 {
        set(ti, "rotation", ParamValue::Float(t.rotation));
    }
}

/// A title's text and look, from its `scriptBuf`.
struct Title {
    text: String,
    font: Option<String>,
    /// The text layer's anchor (first line's baseline, at the box's alignment edge), in pixels of
    /// the sequence frame.
    position: Vec2,
    /// Font size in pixels.
    size: f64,
    /// Extra space between lines in pixels (Filmora's line pitch minus the font's own).
    leading: f64,
    /// Index into the text layer's Alignment choices (Left, Center, Right).
    align: u32,
    bold: bool,
    italic: bool,
    color: [f32; 4],
    /// The box behind the text: colour (with opacity) and padding in pixels.
    background: Option<([f32; 4], f64)>,
}

/// A Filmora colour: 0xAABBGGRR in a (possibly negative) 32-bit integer; the alpha byte is ignored
/// (Filmora keeps opacity in a separate field).
fn color_of(c: i64) -> [f32; 3] {
    let c = (c as u32) & 0x00ff_ffff;
    [(c & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, ((c >> 16) & 0xff) as f32 / 255.0]
}

fn title_of(script: &str, w: u32, h: u32) -> Option<Title> {
    let v: Value = serde_json::from_str(script).ok()?;
    let raw = v.get("Text").and_then(Value::as_str)?.replace("\r\n", "\n").replace('\r', "\n");
    let data = v.get("TextData").and_then(Value::as_array).and_then(|a| a.first());
    let basic = data.and_then(|d| d.get("Basic"));
    let f = |o: Option<&Value>, k: &str| o.and_then(|x| x.get(k)).and_then(Value::as_f64).filter(|x| x.is_finite());
    let (fw, fh) = (f64::from(w.max(1)), f64::from(h.max(1)));
    // The box (centre PosX / PosY, ScaleX × ScaleY of the frame) holds every line of the text,
    // the empty ones Filmora keeps at the end included, at one pitch each. Measured against
    // Filmora's own renders: the font size is about pitch / 1.45, the first line centred half a
    // pitch below the top of the box.
    let lines = raw.split('\n').count().max(1) as f64;
    let text = raw.trim_end_matches('\n').to_string();
    let (cx, cy) = (f(Some(&v), "PosX").unwrap_or(0.5).clamp(-1.0, 2.0) * fw, f(Some(&v), "PosY").unwrap_or(0.5).clamp(-1.0, 2.0) * fh);
    let (bw, bh) = (f(Some(&v), "ScaleX").unwrap_or(0.5).clamp(0.001, 4.0) * fw, f(Some(&v), "ScaleY").unwrap_or(0.1).clamp(0.001, 4.0) * fh);
    let pitch = (bh / lines).clamp(4.0, 4000.0);
    let size = (pitch / 1.45).clamp(4.0, 2000.0);
    // Meiryo / most UI fonts set lines about 1.36 font sizes apart
    let leading = pitch - 1.36 * size;
    let align = match v.get("Layout").and_then(|l| l.get("TextAlign")).and_then(Value::as_str) {
        Some("left") => 0,
        Some("right") => 2,
        _ => 1,
    };
    let x = match align {
        0 => cx - bw / 2.0,
        2 => cx + bw / 2.0,
        _ => cx,
    };
    // first baseline: line centre plus about a third of the font size
    let pos = Vec2::new(x, cy - bh / 2.0 + pitch / 2.0 + 0.36 * size);
    // colours are 0xBBGGRR
    let color = basic
        .and_then(|b| b.get("TextColor"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|c| c.get("Color"))
        .and_then(Value::as_i64)
        .map(|c| {
            let [r, g, b] = color_of(c);
            [r, g, b, (f(basic, "TextAlpha").unwrap_or(255.0).clamp(0.0, 255.0) / 255.0) as f32]
        })
        .unwrap_or([1.0, 1.0, 1.0, 1.0]);
    let bg = v.get("Background");
    let background = bg.filter(|b| b.get("Enable").and_then(Value::as_i64) == Some(1)).map(|b| {
        let [r, g, bl] = color_of(b.get("Color").and_then(Value::as_i64).unwrap_or(0));
        let alpha = (f(Some(b), "Alpha").unwrap_or(255.0).clamp(0.0, 255.0) / 255.0) as f32;
        // Filmora pads its box by a few of its own units (a 24-unit font ≈ `size` pixels)
        let pad = ["Left", "Right", "Top", "Bottom"].iter().filter_map(|k| f(Some(b), k)).fold(0.0f64, f64::max).clamp(0.0, 100.0);
        ([r, g, bl, alpha], (pad * size / 24.0 + 0.15 * size).clamp(0.0, 1000.0))
    });
    Some(Title {
        text,
        font: basic.and_then(|b| b.get("FontName")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string),
        position: pos,
        size,
        leading,
        align,
        bold: f(basic, "FontBold").unwrap_or(0.0) != 0.0,
        italic: f(basic, "FontItalic").unwrap_or(0.0) != 0.0,
        color,
        background,
    })
}

struct Importer<'a> {
    b: Builder,
    report: &'a mut Report,
    timelines: HashMap<i64, &'a Value>,
    resources: HashMap<String, &'a Value>,
    /// Nested timelines already turned into sequences.
    done: HashMap<i64, ItemId>,
    /// Timelines being built (cycle guard).
    building: HashSet<i64>,
    /// Graphic source items by (width, height, rate).
    graphics: HashMap<(u32, u32, FrameRate), ItemId>,
    main_settings: (FrameRate, u32, u32, u32),
}

impl Importer<'_> {
    fn settings_of(&self, t: &Value) -> filmcraft_project::SequenceSettings {
        let (rate0, w0, h0, sr0) = self.main_settings;
        let rate = rate_of(t.get("frameRate")).unwrap_or(rate0);
        let w = int(t, "resolutionWidth").filter(|v| *v > 0 && *v <= 16_384).map_or(w0, |v| v as u32);
        let h = int(t, "resolutionHeight").filter(|v| *v > 0 && *v <= 16_384).map_or(h0, |v| v as u32);
        let mut s = settings_for(rate, w, h, false);
        s.sample_rate = int(t, "sampleRate").filter(|v| *v > 0 && *v <= 768_000).map_or(sr0, |v| v as u32);
        s
    }

    fn media(&mut self, reference: &str) -> ItemId {
        let path = path_of(reference);
        let name = file_name(&path).to_string();
        let spec = self.resources.get(reference).map(|r| spec_of(r, &path)).unwrap_or_else(|| MediaSpec {
            kind: match kind_from_ext(&path) {
                MediaKind::Still => Some(MediaKind::Still),
                k => Some(k),
            },
            ..Default::default()
        });
        self.b.file_media(&path, &name, &path, &spec, None)
    }

    fn graphic_item(&mut self, w: u32, h: u32, rate: FrameRate) -> ItemId {
        if let Some(id) = self.graphics.get(&(w, h, rate)) {
            return *id;
        }
        let id = self.b.p.add_item("Graphic", Label::Rose, ItemKind::Graphic { width: w, height: h, rate }, None);
        self.graphics.insert((w, h, rate), id);
        id
    }

    /// The sequence for timeline `id` (built once; None for a cycle or a missing timeline).
    fn timeline(&mut self, id: i64, name: &str, depth: usize) -> Option<ItemId> {
        if let Some(s) = self.done.get(&id) {
            return Some(*s);
        }
        if depth > MAX_DEPTH || !self.building.insert(id) {
            self.report.warn("compound clips nested too deeply (or in a loop) were skipped");
            return None;
        }
        let t = *self.timelines.get(&id)?;
        let settings = self.settings_of(t);
        let seq_id = self.b.reserve_sequence(name, settings.clone(), None);
        self.done.insert(id, seq_id);
        let seq = self.build(t, settings, depth);
        self.b.put_sequence(seq_id, seq);
        self.building.remove(&id);
        Some(seq_id)
    }

    fn build(&mut self, t: &Value, settings: filmcraft_project::SequenceSettings, depth: usize) -> Sequence {
        let rate = settings.frame_rate;
        let (w, h) = (settings.width, settings.height);
        let mut seq = empty_sequence(settings);
        seq.video_tracks.clear();
        seq.audio_tracks.clear();
        let tracks = t.get("trackInfos").and_then(Value::as_array).cloned().unwrap_or_default();
        // pre-create the tracks in Filmora's order (bottom to top), empty ones dropped at the end
        let (mut vi, mut ai) = (0usize, 0usize);
        let mut slots: Vec<(TrackKind, usize)> = Vec::new();
        for tr in &tracks {
            let kind = if int(tr, "trackType") == Some(1) { TrackKind::Video } else { TrackKind::Audio };
            let idx = if kind == TrackKind::Video { &mut vi } else { &mut ai };
            slots.push((kind, *idx));
            *idx += 1;
        }
        self.b.ensure_tracks(&mut seq, TrackKind::Video, vi);
        self.b.ensure_tracks(&mut seq, TrackKind::Audio, ai);
        // (key, start, end) → link id: a file's (or compound clip's) video and audio at the same place
        let mut links: BTreeMap<(String, i64, i64), u64> = BTreeMap::new();
        let mut placed: Vec<(TrackKind, usize, TrackItem, Option<Fades>)> = Vec::new();
        let mut clips_seen = 0usize;
        for (tr, (kind, idx)) in tracks.iter().zip(slots) {
            for c in tr.get("clipList").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
                clips_seen += 1;
                if clips_seen > MAX_CLIPS {
                    self.report.warn(format!("timelines with more than {MAX_CLIPS} clips were cut short"));
                    break;
                }
                let (Some(b), Some(e)) = (int(c, "tlBegin"), int(c, "tlEnd")) else { continue };
                // tlEnd is the last unit inside the clip
                let start = rate.snap_nearest(tick(b.max(0)));
                let end = rate.snap_nearest(tick(e.saturating_add(1).max(b)));
                let dur = (end - start).max(rate.frame_duration());
                let ty = int(c, "type").unwrap_or(0);
                let (speed, reverse, ramp_in) = speed_of(c, dur, self.report);
                let src_in = ramp_in.unwrap_or_else(|| tick(int(c, "inPoint").unwrap_or(0).max(0)));
                let ti = match ty {
                    1 | 2 => {
                        let Some(reference) = c.get("filename").and_then(Value::as_str).filter(|f| !f.is_empty()) else {
                            self.report.warn("clips without a media file were skipped");
                            continue;
                        };
                        let item = self.media(reference);
                        let still = self.b.p.item(item).and_then(|i| i.as_media()).is_some_and(|m| m.info.kind == MediaKind::Still);
                        let name = file_name(&path_of(reference)).to_string();
                        let mut ti = self.b.clip(item, kind, &name, start, dur, if still { Tick::ZERO } else { src_in });
                        if !still {
                            ti.speed = speed;
                            ti.reverse = reverse;
                        }
                        if kind == TrackKind::Video {
                            apply_transform(&mut ti, c, (w, h));
                        }
                        let key = c.get("sourceUuid").and_then(Value::as_str).unwrap_or(reference).to_string();
                        Some((ti, Some(key)))
                    }
                    6 | 16 => match int(c, "timelineId").and_then(|id| self.timeline(id, "Compound Clip", depth + 1).map(|s| (id, s))) {
                        Some((id, nested)) => {
                            let mut ti = self.b.clip(nested, kind, "Compound Clip", start, dur, src_in);
                            ti.speed = speed;
                            ti.reverse = reverse;
                            if kind == TrackKind::Video {
                                apply_transform(&mut ti, c, (w, h));
                            }
                            Some((ti, Some(format!("timeline:{id}"))))
                        }
                        None => None,
                    },
                    7 => self.title_clip(c, start, dur, (w, h, rate)).map(|ti| (ti, None)),
                    26 => self.drawing_clip(c, start, dur, (w, h, rate)).map(|ti| (ti, None)),
                    other => {
                        self.report.warn(format!("clips of Filmora type {other} are not supported and were skipped"));
                        None
                    }
                };
                let Some((mut ti, key)) = ti else { continue };
                let fades = if matches!(ty, 1 | 2 | 6 | 16) { apply_settings(&mut ti, c, kind, self.report) } else { None };
                if let Some(key) = key {
                    let entry = links.entry((key, b, e)).or_insert(0);
                    if *entry == 0 {
                        *entry = self.b.link_id();
                    }
                    ti.link = Some(*entry);
                }
                self.report_settings(c);
                placed.push((kind, idx, ti, fades));
            }
        }
        // a link with only one side is no link
        let mut sides: HashMap<u64, u32> = HashMap::new();
        for (_, _, ti, _) in &placed {
            if let Some(l) = ti.link {
                *sides.entry(l).or_default() += 1;
            }
        }
        for (kind, idx, mut ti, fades) in placed {
            if ti.link.is_some_and(|l| sides.get(&l).copied().unwrap_or(0) < 2) {
                ti.link = None;
            }
            let (clip, start, dur) = (ti.id, ti.start, ti.duration);
            let i = self.b.place(&mut seq, kind, idx, ti);
            // audio fades: Constant Gain (linear) from / to silence at the clip's edges
            let Some(f) = fades else { continue };
            let secs = dur.seconds();
            let total = f.fade_in + f.fade_out;
            let k = if total > secs && total > 0.0 { secs / total } else { 1.0 };
            for (len, at_start) in [(f.fade_in * k, true), (f.fade_out * k, false)] {
                let len = rate.snap_nearest(Tick::from_seconds_f64(len)).min(dur);
                if len <= Tick::ZERO {
                    continue;
                }
                let Some(effect) = filmcraft_project::find_effect("constant_gain").map(|d| d.instance()) else { continue };
                let id = filmcraft_project::TransitionId(self.b.alloc());
                let (t_start, from, to, align) = if at_start {
                    (start, None, Some(clip), filmcraft_project::TransitionAlign::StartAtCut)
                } else {
                    (start + dur - len, Some(clip), None, filmcraft_project::TransitionAlign::EndAtCut)
                };
                if let Some(track) = seq.tracks_mut(kind).get_mut(i) {
                    track.transitions.push(filmcraft_project::Transition { id, effect, start: t_start, duration: len, from, to, align, reverse: false });
                    track.sort();
                }
            }
        }
        // drop empty tracks (Filmora keeps many), keeping at least one of each kind
        for kind in [TrackKind::Video, TrackKind::Audio] {
            let tracks = seq.tracks_mut(kind);
            let keep: Vec<_> = tracks.iter().filter(|t| !t.items.is_empty()).cloned().collect();
            if !keep.is_empty() {
                *tracks = keep;
            } else {
                tracks.truncate(1);
            }
            for (i, t) in tracks.iter_mut().enumerate() {
                t.name = match kind {
                    TrackKind::Video => format!("Video {}", i + 1),
                    TrackKind::Audio => format!("Audio {}", i + 1),
                };
            }
        }
        seq
    }

    /// Report what a clip sets that is not read. Filmora writes no parameters for effects left at
    /// their defaults (volume 0 dB, no fade, flat EQ, no crop), so only effects with parameters count.
    fn report_settings(&mut self, c: &Value) {
        for e in effects_of(c).into_iter().filter(|e| enabled(e) && has_params(e)) {
            match effect_id(e) {
                "video/effect/transform" => {}
                id if id.starts_with("graphic/effect/") => {}
                "video/effect/crop-pan-zoom" | "audio/effect/volume" | "audio/effect/fade" => {}
                id if id.starts_with("audio/effect/") => self.report.warn("Filmora audio effects (EQ, denoise…) were not carried over"),
                _ => self.report.warn("Filmora effects were not carried over"),
            }
        }
        let keyed = |k: &str| {
            c.get(k)
                .and_then(|v| v.get("parameter"))
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .and_then(|v| v.get("keyframeSets").and_then(Value::as_array).map(|a| !a.is_empty()))
                .unwrap_or(false)
        };
        if keyed("volumeKeyframe") || keyed("audioDuckingframe") {
            self.report.warn("Filmora volume keyframes were not carried over");
        }
    }

    /// A drawn graphic (`type` 26): a graphic clip with one shape layer per outline (pen strokes:
    /// one per segment), in sequence pixels.
    fn drawing_clip(&mut self, c: &Value, start: Tick, dur: Tick, (w, h, rate): (u32, u32, FrameRate)) -> Option<TrackItem> {
        let (fw, fh) = (f64::from(w.max(1)), f64::from(h.max(1)));
        let place = Placement::of(c, (fw, fh));
        let effects = effects_of(c);
        let find = |id: &str| effects.iter().copied().find(|e| effect_id(e) == id && enabled(e));
        let stroke = find("graphic/effect/path_stroke").map(|e| {
            let [r, g, b] = rgb_of(fx_param(e, "color").and_then(Value::as_i64).unwrap_or(0));
            ([r, g, b, 1.0f32], (fx_f64(e, "lineSize").unwrap_or(4.0).clamp(0.0, 500.0) * STROKE_PX) as f32)
        });
        let fill = find("graphic/effect/path_fill").and_then(|e| {
            let alpha = fx_f64(e, "alpha").unwrap_or(255.0).clamp(0.0, 255.0) as f32 / 255.0;
            let color = fx_param(e, "color").and_then(Value::as_i64)?;
            let [r, g, b] = rgb_of(color);
            (alpha > 0.0).then_some([r, g, b, alpha])
        });
        // shape_render: end_shape / start_shape 2 = arrowhead
        let ends: [i64; 2] = find("graphic/effect/shape_render").map_or([0, 0], |e| {
            [fx_param(e, "end_shape").and_then(Value::as_i64).unwrap_or(0), fx_param(e, "start_shape").and_then(Value::as_i64).unwrap_or(0)]
        });
        if find("graphic/effect/path_trim").is_some_and(|e| fx_f64(e, "start").unwrap_or(0.0) > 0.0 || fx_f64(e, "end").unwrap_or(100.0) < 100.0) {
            self.report.warn("partly drawn pen strokes (trim) were imported whole");
        }
        let mut layers = Vec::new();
        for (pts, closed) in outlines(c, (fw, fh), self.report) {
            let mapped: Vec<[f32; 2]> = pts.iter().map(|q| place.map(*q)).collect();
            let mut shapes: Vec<(Vec<[f32; 2]>, Option<[f32; 4]>, Option<([f32; 4], f32)>)> = Vec::new();
            if closed && mapped.len() >= 3 {
                shapes.push((mapped, fill, stroke));
            } else if let Some((color, width)) = stroke {
                // an open stroke: each segment filled with the stroke colour, arrowheads at the
                // ends Filmora's shape_render asks for (the line stops at the head's base)
                let width = width.max(1.0);
                let mut mapped = mapped;
                for at_end in [true, false] {
                    if ends.get(usize::from(!at_end)).copied() != Some(2) || mapped.len() < 2 {
                        continue;
                    }
                    let n = mapped.len();
                    let (tip_i, from_i) = if at_end { (n - 1, n - 2) } else { (0, 1) };
                    let (Some(tip), Some(from)) = (mapped.get(tip_i).copied(), mapped.get(from_i).copied()) else { continue };
                    if let Some((head, base)) = arrowhead(from, tip, width) {
                        shapes.push((head, Some(color), None));
                        if let Some(p) = mapped.get_mut(tip_i) {
                            *p = base;
                        }
                    }
                }
                for pair in mapped.windows(2) {
                    if let (Some(a), Some(b)) = (pair.first(), pair.get(1)) {
                        let quad = segment(*a, *b, width);
                        if !quad.is_empty() {
                            shapes.push((quad, Some(color), None));
                        }
                    }
                }
            }
            for (points, fill, stroke) in shapes {
                let mut layer = filmcraft_project::graphic::new_shape_layer(3, Vec2::new(0.0, 0.0), Vec2::new(fw, fh), points);
                let mut set = |k: &str, v: ParamValue| {
                    layer.params.insert(k.to_string(), Param::new(v));
                };
                match fill {
                    Some(c) => set("fill_color", ParamValue::Color(c)),
                    None => set("fill", ParamValue::Bool(false)),
                }
                if let Some((color, width)) = stroke {
                    set("stroke", ParamValue::Bool(true));
                    set("stroke_color", ParamValue::Color(color));
                    set("stroke_width", ParamValue::Float(f64::from(width)));
                    set("stroke_type", ParamValue::Choice(1));
                }
                layers.push(layer);
            }
        }
        if layers.is_empty() {
            self.report.warn("drawn graphics without a readable outline were skipped");
            return None;
        }
        let item = self.graphic_item(w, h, rate);
        let mut ti = self.b.clip(item, TrackKind::Video, "Drawing", start, dur, Tick::ZERO);
        ti.effects.extend(layers);
        self.report.info("drawn graphics were imported as shape layers");
        Some(ti)
    }

    /// A title (`type` 7): a graphic clip with one text layer.
    fn title_clip(&mut self, c: &Value, start: Tick, dur: Tick, (w, h, rate): (u32, u32, FrameRate)) -> Option<TrackItem> {
        let script = int(c, "timelineId")
            .and_then(|id| self.timelines.get(&id).copied())
            .and_then(|t| t.get("trackInfos").and_then(Value::as_array).cloned())
            .unwrap_or_default()
            .iter()
            .flat_map(|tr| tr.get("clipList").and_then(Value::as_array).cloned().unwrap_or_default())
            .find_map(|cl| cl.get("scriptBuf").and_then(Value::as_str).map(str::to_string));
        let Some(title) = script.as_deref().and_then(|s| title_of(s, w, h)) else {
            self.report.warn("titles without readable text were skipped");
            return None;
        };
        let item = self.graphic_item(w, h, rate);
        let name = title.text.lines().next().filter(|l| !l.trim().is_empty()).unwrap_or("Title").to_string();
        let mut ti = self.b.clip(item, TrackKind::Video, &name, start, dur, Tick::ZERO);
        let mut layer = filmcraft_project::graphic::new_text_layer(&title.text, title.position, title.size);
        let mut set = |k: &str, v: ParamValue| {
            layer.params.insert(k.to_string(), Param::new(v));
        };
        if let Some(font) = &title.font {
            set("font", ParamValue::Text(font.clone()));
        }
        set("align", ParamValue::Choice(title.align));
        set("leading", ParamValue::Float(title.leading));
        set("fill_color", ParamValue::Color(title.color));
        if let Some(([r, g, b, a], pad)) = title.background {
            set("background", ParamValue::Bool(true));
            set("background_color", ParamValue::Color([r, g, b, 1.0]));
            set("background_opacity", ParamValue::Float(f64::from(a) * 100.0));
            set("background_size", ParamValue::Float(pad));
        }
        if title.bold {
            set("faux_bold", ParamValue::Bool(true));
        }
        if title.italic {
            set("faux_italic", ParamValue::Bool(true));
        }
        ti.effects.push(layer);
        self.report.info("titles were imported as text with their font, size, colour and position; Filmora title styles and animations were not");
        Some(ti)
    }
}

/// Import a Filmora project.
pub(crate) fn import(bytes: &[u8], opts: &ImportOptions) -> Result<(Imported, Report)> {
    let m = members(bytes)?;
    let info = read_json(bytes, &m, PROJECT_INFO)?;
    let media_id = info.get("timeline_mediaId").and_then(Value::as_str).ok_or_else(|| Error::Other("Filmora project: no timeline".into()))?;
    let doc = read_json(bytes, &m, &format!("ProjectFolder/Medias/{media_id}/timeline.wesproj"))?;
    let mut report = Report::default();
    let pair = |k: &str| -> Option<(i64, i64)> {
        let a = info.get(k)?.as_array()?;
        Some((a.first()?.as_i64()?, a.get(1)?.as_i64()?))
    };
    let rate = pair("project_timeline_framerate").filter(|(n, d)| *n > 0 && *d > 0 && n / d <= 1000).map_or(FrameRate::FPS_30, |(n, d)| FrameRate::new(n, d));
    let (w, h) = pair("project_timeline_resolution")
        .filter(|(w, h)| *w > 0 && *h > 0 && *w <= 16_384 && *h <= 16_384)
        .map_or((1920, 1080), |(w, h)| (w as u32, h as u32));
    let sr = int(&info, "project_sample_rate").filter(|v| *v > 0 && *v <= 768_000).map_or(48_000, |v| v as u32);
    let name = opts
        .name
        .clone()
        .or_else(|| info.get("project_file_name").and_then(Value::as_str).map(str::to_string))
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "Filmora Project".into());
    let timelines: HashMap<i64, &Value> = doc
        .get("timelineInfos")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|t| int(t, "timelineId").map(|id| (id, t))).collect())
        .unwrap_or_default();
    let resources: HashMap<String, &Value> = doc
        .get("resources")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|r| r.get("filename").and_then(Value::as_str).map(|f| (f.to_string(), r))).collect())
        .unwrap_or_default();
    let current = int(&doc, "currentTimelineId").filter(|id| timelines.contains_key(id)).or_else(|| timelines.keys().min().copied());
    let Some(current) = current else { return Err(Error::Other("Filmora project: the timeline is empty".into())) };
    let mut imp = Importer {
        b: Builder::new(&name),
        report: &mut report,
        timelines,
        resources,
        done: HashMap::new(),
        building: HashSet::new(),
        graphics: HashMap::new(),
        main_settings: (rate, w, h, sr),
    };
    let main = imp.timeline(current, &name, 0).ok_or_else(|| Error::Other("Filmora project: the main timeline could not be read".into()))?;
    imp.b.top.push(main);
    let imported = imp.b.finish();
    Ok((imported, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ZIP of stored members.
    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cd = Vec::new();
        for (name, data) in files {
            let off = out.len() as u32;
            let crc = 0u32; // not checked by the reader
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&[10, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            cd.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            cd.extend_from_slice(&[20, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            cd.extend_from_slice(&crc.to_le_bytes());
            cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            cd.extend_from_slice(&off.to_le_bytes());
            cd.extend_from_slice(name.as_bytes());
        }
        let cd_off = out.len() as u32;
        out.extend_from_slice(&cd);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    const SECOND: i64 = UNITS_PER_SECOND;

    fn project(timeline: &str) -> Vec<u8> {
        let info = r#"{"timeline_mediaId":"{T}","project_timeline_framerate":[30,1],"project_timeline_resolution":[1920,1080],
            "project_sample_rate":48000,"project_file_name":"Demo","proj_zip_save_path":"E:/Old/Demo.wfp"}"#;
        zip(&[(PROJECT_INFO, info.as_bytes()), ("ProjectFolder/Medias/{T}/timeline.wesproj", timeline.as_bytes())])
    }

    fn timeline() -> String {
        let clip = |ty: i64, file: &str, b: i64, e: i64, inp: i64, extra: &str| {
            let chains = if extra.contains("effectChainList") { "" } else { r#","effectChainList":[]"# };
            format!(
                r#"{{"type":{ty},"filename":"{file}","sourceUuid":"{file}","tlBegin":{b},"tlEnd":{e},"inPoint":{inp},"outPoint":{}{chains}{extra}}}"#,
                inp + e - b + 1
            )
        };
        let title = format!(r#"{{"type":7,"timelineId":2,"tlBegin":{},"tlEnd":{},"inPoint":0,"outPoint":{}}}"#, 2 * SECOND, 3 * SECOND - 1, SECOND);
        // a red-stroked rectangle in the middle of its canvas, moved to the top-left quarter at half size
        let graphic = format!(
            r#"{{"type":26,"tlBegin":0,"tlEnd":{},"effectChainList":[
                {{"name":"Basic","effectList":[{{"id":"video/effect/transform","paramList":[{{"name":"Position_x","fxParam":{{"unValue":0.25}}}},{{"name":"Position_y","fxParam":{{"unValue":0.25}}}},{{"name":"Scale_x","fxParam":{{"unValue":50.0}}}},{{"name":"Scale_y","fxParam":{{"unValue":50.0}}}}]}}]}},
                {{"name":"graphic","effectList":[
                    {{"id":"graphic/effect/path_graphic_rect","paramList":[{{"name":"start_x","fxParam":{{"unValue":0.4}}}},{{"name":"start_y","fxParam":{{"unValue":0.4}}}},{{"name":"width","fxParam":{{"unValue":0.2}}}},{{"name":"height","fxParam":{{"unValue":0.2}}}}]}},
                    {{"id":"graphic/effect/path_stroke","paramList":[{{"name":"lineSize","fxParam":{{"unValue":4}}}},{{"name":"color","fxParam":{{"unValue":16711680}}}}]}},
                    {{"id":"graphic/effect/path_fill","paramList":[{{"name":"alpha","fxParam":{{"unValue":0}}}}]}}]}}]}}"#,
            SECOND - 1
        );
        let script = r#"{"Text":"Hello\rWorld","PosX":0.5,"PosY":0.9,"ScaleY":0.2,"Layout":{"TextAlign":"center"},"Background":{"Enable":1,"Color":-1027028792,"Alpha":194,"Left":2.0},"TextData":[{"Basic":{"FontName":"Meiryo UI","FontBold":1,"TextColor":[{"Color":-16776961}],"TextAlpha":255}}]}"#;
        format!(
            r#"{{"currentTimelineId":1,"resources":[
                {{"filename":"file:/E:/Old/a.mp4","streamType":2,"mediaLength":{len},"vidStreamInfo":[{{"width":1280,"height":720,"frameRate":{{"num":30,"den":1}}}}],"audStreamInfo":[{{"sampleRate":48000,"channels":2}}]}},
                {{"filename":"file:/E:/Old/p.png","streamType":5,"mediaLength":0,"vidStreamInfo":[{{"width":1920,"height":1080}}]}}],
              "timelineInfos":[
                {{"timelineId":1,"frameRate":{{"num":30,"den":1}},"resolutionWidth":1920,"resolutionHeight":1080,"sampleRate":48000,"trackInfos":[
                    {{"trackType":2,"clipList":[{a}]}},
                    {{"trackType":1,"clipList":[{p}]}},
                    {{"trackType":1,"clipList":[{v},{t}]}},
                    {{"trackType":1,"clipList":[]}},
                    {{"trackType":1,"clipList":[{g}]}}]}},
                {{"timelineId":2,"trackInfos":[{{"trackType":1,"clipList":[{{"type":4,"tlBegin":0,"tlEnd":{tl2},"scriptBuf":{script}}}]}}]}}]}}"#,
            len = 20 * SECOND,
            a = clip(2, "file:/E:/Old/a.mp4", 0, 2 * SECOND - 1, 5 * SECOND, ""),
            p = clip(1, "file:/E:/Old/p.png", 0, 4 * SECOND - 1, 0, ""),
            v = clip(
                1,
                "file:/E:/Old/a.mp4",
                0,
                2 * SECOND - 1,
                5 * SECOND,
                r#","effectChainList":[{"name":"Basic","effectList":[{"id":"video/effect/transform","paramList":[{"name":"Position_x","fxParam":{"unValue":0.9}},{"name":"Position_y","fxParam":{"unValue":0.1}},{"name":"Scale_x","fxParam":{"unValue":10.0}},{"name":"Scale_y","fxParam":{"unValue":10.0}}]}]}]"#
            ),
            t = title,
            g = graphic,
            tl2 = SECOND - 1,
            script = serde_json::to_string(script).unwrap(),
        )
    }

    /// Volume, fades, crop and a speed ramp as Filmora 16 saved them (values from a project made
    /// in Filmora for this test).
    #[test]
    fn reads_volume_fades_crop_and_speed_ramps() {
        let ramp = r#"{\"keyframeSets\": [{\"_time\": 0.0,\"Interpolation\": 9,\"_value\": 0.9},{\"_time\": 5.0,\"Interpolation\": 9,\"_value\": 6.8},{\"_time\": 10.0,\"Interpolation\": 9,\"_value\": 1.0}],\"_totalTime\": 10.0}"#;
        let crop = r#"[{\"OriTime\": 0,\"Time\": 0.0,\"X\": 0.125,\"Y\": 0.1336,\"Width\": 0.5,\"Height\": 0.5}]"#;
        let tl = format!(
            r#"{{"currentTimelineId":1,"resources":[
                {{"filename":"file:/E:/Old/a.mp4","streamType":2,"mediaLength":{len},"vidStreamInfo":[{{"width":1920,"height":1080,"frameRate":{{"num":30,"den":1}}}}],"audStreamInfo":[{{"sampleRate":48000,"channels":2}}]}}],
              "timelineInfos":[{{"timelineId":1,"frameRate":{{"num":30,"den":1}},"resolutionWidth":1920,"resolutionHeight":1080,"trackInfos":[
                {{"trackType":2,"clipList":[{{"type":2,"filename":"file:/E:/Old/a.mp4","tlBegin":0,"tlEnd":{ten},"inPoint":0,"outPoint":{ten},
                    "effectChainList":[{{"name":"a","effectList":[
                        {{"id":"audio/effect/volume","paramList":[{{"fxParam":{{"paramType":2,"unValue":-6.76}},"name":"VolumeGain"}}]}},
                        {{"id":"audio/effect/fade","paramList":[{{"fxParam":{{"unValue":1.0}},"name":"FadeInTime"}},{{"fxParam":{{"unValue":2.0}},"name":"FadeOutTime"}}]}}]}}]}}]}},
                {{"trackType":1,"clipList":[
                    {{"type":1,"filename":"file:/E:/Old/a.mp4","tlBegin":0,"tlEnd":{ten},"inPoint":0,"outPoint":{ten},
                      "effectChainList":[{{"name":"v","effectList":[{{"id":"video/effect/crop-pan-zoom","paramList":[{{"fxParam":{{"paramType":6,"unValue":"{crop}"}},"name":"SettingScript"}}]}}]}}]}},
                    {{"type":1,"filename":"file:/E:/Old/a.mp4","tlBegin":{s20},"tlEnd":{ramp_end},"inPoint":0,"outPoint":{five},
                      "speed":{{"offset":2.0,"offsetEnd":12.0,"reverse":false,"speedParam":"{ramp}"}},"effectChainList":[]}}]}}]}}]}}"#,
            len = 20 * SECOND,
            ten = 10 * SECOND - 1,
            s20 = 20 * SECOND,
            five = 5 * SECOND,
            ramp_end = 25 * SECOND - 1,
        );
        let (imp, report) = import(&project(&tl), &ImportOptions::default()).unwrap();
        let q = imp.project.sequence(imp.sequences[0]).unwrap();
        // audio: -6.76 dB, 1 s fade-in and 2 s fade-out as Constant Gain transitions from / to silence
        let a = &q.audio_tracks[0];
        assert_eq!(a.items[0].effect("volume").unwrap().f64_at("level", Tick::ZERO), -6.76);
        let fades: Vec<_> = a.transitions.iter().map(|t| (t.effect.effect.as_str(), t.start, t.duration, t.from.is_some(), t.to.is_some())).collect();
        let sec = |s: i64| Tick(s * filmcraft_time::TICKS_PER_SECOND);
        assert_eq!(fades, vec![("constant_gain", sec(0), sec(1), false, true), ("constant_gain", sec(8), sec(2), true, false)]);
        // video: the crop (fractions → percent per side), enlarged to fill the frame
        let v = &q.video_tracks[0].items;
        let crop = v[0].effect("crop").unwrap();
        let close = |id: &str, want: f64| assert!((crop.f64_at(id, Tick::ZERO) - want).abs() < 1e-9, "{id}: {}", crop.f64_at(id, Tick::ZERO));
        close("left", 12.5);
        close("top", 13.36);
        close("right", 37.5);
        close("bottom", 36.64);
        assert_eq!(crop.param("zoom").unwrap().value, ParamValue::Bool(true));
        // the ramp plays source 2..12 s in 5 s: twice the speed, from 2 s
        assert!((v[1].speed - 2.0).abs() < 1e-9, "{}", v[1].speed);
        assert_eq!(v[1].source_in, sec(2));
        let warnings: Vec<_> = report.entries.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(warnings, ["speed ramps were read as the average speed (same duration, constant rate)"]);
    }

    #[test]
    fn reads_tracks_clips_titles_and_links() {
        let bytes = project(&timeline());
        assert!(sniff(&bytes));
        assert_eq!(original_project_dir(&bytes).as_deref(), Some("E:/Old"));
        let (imp, report) = import(&bytes, &ImportOptions::default()).unwrap();
        let p = &imp.project;
        let q = p.sequence(imp.sequences[0]).unwrap();
        assert_eq!((q.settings.width, q.settings.height, q.settings.frame_rate), (1920, 1080, FrameRate::FPS_30));
        // empty tracks dropped; bottom to top: still, movie + title, drawing
        assert_eq!(q.video_tracks.len(), 3, "{:?}", q.video_tracks.iter().map(|t| t.items.len()).collect::<Vec<_>>());
        // the drawing: one rectangle outline, centred at (480, 270), 192 × 108, red, 7.2 px centre stroke
        let d = &q.video_tracks[2].items[0];
        let shape = d.effects.last().unwrap();
        let pts = match &shape.param("points").unwrap().value {
            ParamValue::Curve(p) => p.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(pts, vec![[384.0, 216.0], [576.0, 216.0], [576.0, 324.0], [384.0, 324.0]]);
        assert_eq!(shape.param("fill").unwrap().value, ParamValue::Bool(false));
        assert_eq!(shape.param("stroke_color").unwrap().value, ParamValue::Color([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(shape.param("stroke_type").unwrap().value, ParamValue::Choice(1));
        let still = &q.video_tracks[0].items[0];
        assert_eq!(p.item(still.item).unwrap().as_media().unwrap().info.kind, MediaKind::Still);
        assert_eq!(still.duration, Tick::from_units(4, 1));
        let v = &q.video_tracks[1].items[0];
        assert_eq!((v.start, v.duration, v.source_in), (Tick::ZERO, Tick::from_units(2, 1), Tick::from_units(5, 1)));
        // fitted to the frame, with Filmora's position and scale as Motion
        assert!(v.scale_to_frame && still.scale_to_frame);
        let motion = |k: &str| v.effect("motion").and_then(|e| e.param(k)).map(|p| p.value.clone());
        assert_eq!(motion("position"), Some(ParamValue::Vec2(Vec2::new(0.9 * 1920.0, 0.1 * 1080.0))));
        assert_eq!(motion("scale"), Some(ParamValue::Float(10.0)));
        let a = &q.audio_tracks[0].items[0];
        assert_eq!(a.item, v.item, "one media item for the file's video and audio");
        assert!(v.link.is_some() && v.link == a.link, "video and audio linked");
        // the title: a graphic with the text, font and colour
        let t = &q.video_tracks[1].items[1];
        assert!(matches!(p.item(t.item).unwrap().kind, ItemKind::Graphic { .. }));
        assert_eq!(t.start, Tick::from_units(2, 1));
        let layer = t.effects.last().unwrap();
        assert_eq!(layer.param("text").unwrap().value, ParamValue::Text("Hello\nWorld".into()));
        assert_eq!(layer.param("font").unwrap().value, ParamValue::Text("Meiryo UI".into()));
        assert_eq!(layer.param("fill_color").unwrap().value, ParamValue::Color([1.0, 0.0, 0.0, 1.0]), "0xFF0000FF: red, the alpha byte ignored");
        assert_eq!(layer.param("background").unwrap().value, ParamValue::Bool(true));
        let grey = 200.0 / 255.0;
        assert_eq!(layer.param("background_color").unwrap().value, ParamValue::Color([grey, grey, grey, 1.0]));
        assert!(!report.has_warnings(), "nothing set is dropped: {report}");
        // media paths as written
        assert_eq!(crate::common::item_path(p, v.item), Some("E:/Old/a.mp4"));
    }

    #[test]
    fn placement_scales_and_rotates_about_the_anchor() {
        let c: Value = serde_json::from_str(
            r#"{"effectChainList":[{"effectList":[{"id":"video/effect/transform","paramList":[
                {"name":"Position_x","fxParam":{"unValue":0.5}},{"name":"Position_y","fxParam":{"unValue":0.5}},
                {"name":"_Anchor_x","fxParam":{"unValue":0.0}},{"name":"_Anchor_y","fxParam":{"unValue":0.0}},
                {"name":"Rotation","fxParam":{"unValue":90.0}}]}]}]}"#,
        )
        .unwrap();
        let p = Placement::of(&c, (100.0, 50.0));
        // the anchor (canvas origin) lands at the centre; +x turns to +y (clockwise on screen)
        let near = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3;
        assert!(near(p.map((0.0, 0.0)), [50.0, 25.0]));
        assert!(near(p.map((10.0, 0.0)), [50.0, 35.0]), "{:?}", p.map((10.0, 0.0)));
        // a thick segment and a degenerate one
        assert_eq!(segment([0.0, 0.0], [10.0, 0.0], 2.0), vec![[-1.0, 1.0], [11.0, 1.0], [11.0, -1.0], [-1.0, -1.0]]);
        assert!(segment([3.0, 3.0], [3.0, 3.0], 2.0).is_empty());
        // an arrowhead pointing +x: tip, then the two base corners; the line stops 7 widths back
        let (head, base) = arrowhead([0.0, 0.0], [100.0, 0.0], 5.0).unwrap();
        assert_eq!(base, [65.0, 0.0]);
        assert_eq!(head, vec![[100.0, 0.0], [65.0, 13.0], [65.0, -13.0]]);
        assert!(arrowhead([1.0, 1.0], [1.0, 1.0], 5.0).is_none());
        assert_eq!(rgb_of(0xff2f00), [1.0, 47.0 / 255.0, 0.0]);
    }

    #[test]
    fn paths_from_filmora_references() {
        assert_eq!(path_of("file:/E:/a b/c.mp4"), "E:/a b/c.mp4");
        assert_eq!(path_of("file:/Users/x/c.mov"), "/Users/x/c.mov");
        assert_eq!(path_of("file:///C:/x%20y.mp4"), "C:/x y.mp4");
        assert_eq!(path_of("D:/plain.wav"), "D:/plain.wav");
    }

    #[test]
    fn hostile_archives_are_errors_not_panics() {
        for bytes in [Vec::new(), b"PK\x03\x04".to_vec(), vec![0u8; 100], zip(&[]), zip(&[(PROJECT_INFO, b"{not json")]), project("[]"), project("{}")] {
            assert!(import(&bytes, &ImportOptions::default()).is_err(), "{bytes:?}");
            let _ = sniff(&bytes);
            let _ = original_project_dir(&bytes);
        }
        // a timeline that refers to itself as a compound clip
        let looped = r#"{"currentTimelineId":1,"timelineInfos":[{"timelineId":1,"trackInfos":[{"trackType":1,"clipList":[
            {"type":6,"timelineId":1,"tlBegin":0,"tlEnd":9999999}]}]}]}"#;
        let (imp, _) = import(&project(looped), &ImportOptions::default()).unwrap();
        assert_eq!(imp.sequences.len(), 1);
        // hostile numbers
        let wild = r#"{"currentTimelineId":1,"timelineInfos":[{"timelineId":1,"frameRate":{"num":0,"den":0},"resolutionWidth":-5,"trackInfos":[{"trackType":1,"clipList":[
            {"type":1,"filename":"file:/x.mp4","tlBegin":-9223372036854775807,"tlEnd":9223372036854775807,"inPoint":-1,"speed":{"speedParam":"{\"keyframeSets\":[{\"_value\":1e308},{\"_value\":-1}]}"}}]}]}]}"#;
        assert!(import(&project(wild), &ImportOptions::default()).is_ok());
        // a member that claims to be huge
        let mut z = zip(&[(PROJECT_INFO, b"{}")]);
        if let Some(p) = z.windows(4).rposition(|w| w == [0x50, 0x4b, 0x01, 0x02]) {
            z[p + 24..p + 28].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        assert!(import(&z, &ImportOptions::default()).is_err());
    }
}
