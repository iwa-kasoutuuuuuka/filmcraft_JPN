//! Sequence ▸ Make Vertical Version: a 9:16 (or square, or 4:5) sequence for short-form video
//! from the open sequence.
//!
//! `sequence.makeVertical {aspect="9:16"|"1:1"|"4:5", mode="follow"|"fill"|"blur", name?}` makes a
//! new sequence of that shape, with the same frame rate and audio settings, holding the open
//! sequence as one nested clip, and opens it. The original is untouched, and because the new
//! sequence nests it, later edits to the original show up in the vertical version.
//!
//! - `follow` (default): the picture fills the frame and the Auto Reframe effect, set to the
//!   frame's shape, keeps the subject in it (`filmcraft_render::vfx`: a saliency centroid,
//!   smoothed over time; an approximation, not subject recognition);
//! - `fill`: the picture fills the frame, centred (a plain centre crop);
//! - `blur`: the whole picture fits the width, over a blurred, enlarged copy of itself that fills
//!   the rest of the frame.
//!
//! The new frame keeps the original's shorter side: 1920×1080 gives 1080×1920, 1080×1080 or
//! 1080×1350. One undo step ("Make Vertical Version") removes the new sequence again.

use filmcraft_project::{ClipId, ItemId};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, has_seq, str_p};
use crate::{EngineError, Result, Session};

const CMD: &str = "sequence.makeVertical";
/// Blurriness of the background copy in `blur` mode, as a fraction of the frame height.
const BACKGROUND_BLUR: f64 = 0.04;

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Make Vertical Version",
        menu: &["Sequence"],
        shortcut: None,
        params: r#"{"aspect":"9:16|1:1|4:5"="9:16","mode":"follow|fill|blur"="follow","name":str?}"#,
        enabled: has_content,
        run: make_vertical,
        journal: true,
    }]
}

fn has_content(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    let q = s.active_sequence().ok_or("no sequence is open")?;
    if q.video_tracks.iter().any(|t| !t.items.is_empty()) { Ok(()) } else { Err("the sequence has no video clips".into()) }
}

/// Frame size for an aspect (width : height) that keeps `short`, the original's shorter side, as
/// its width; both even, as video encoders want.
pub fn target_size(short: u32, aspect: (u32, u32)) -> (u32, u32) {
    let even = |v: u64| (v.clamp(2, 16_384) as u32) & !1;
    let w = even(u64::from(short));
    let h = even((u64::from(w) * u64::from(aspect.1)).checked_div(u64::from(aspect.0)).unwrap_or(u64::from(w)));
    (w, h)
}

fn make_vertical(s: &mut Session, p: &Value) -> Result<Value> {
    // a value of the wrong type is refused, not replaced by the default
    let text = |k: &str, default: &'static str| match p.get(k) {
        None | Some(Value::Null) => Some(default.to_string()),
        Some(v) => v.as_str().map(str::to_string),
    };
    // (ratio, name suffix, Auto Reframe's Target Aspect choice)
    let (aspect, label, reframe_aspect) = match text("aspect", "9:16").as_deref() {
        Some("9:16") => ((9, 16), "Vertical 9x16", 1),
        Some("1:1") => ((1, 1), "Square 1x1", 2),
        Some("4:5") => ((4, 5), "Vertical 4x5", 3),
        _ => return Err(bad(CMD, "`aspect` must be `9:16`, `1:1` or `4:5`")),
    };
    let mode = text("mode", "follow")
        .filter(|m| matches!(m.as_str(), "follow" | "fill" | "blur"))
        .ok_or_else(|| bad(CMD, "`mode` must be `follow`, `fill` or `blur`"))?;
    let mode = mode.as_str();
    let src: ItemId = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let (name, sw, sh, fps, sample_rate, duration) = {
        let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
        let name = s.project.item(src).map(|i| i.name.clone()).unwrap_or_else(|| "Sequence".into());
        let r = q.settings.frame_rate;
        (name, q.settings.width, q.settings.height, r.num as f64 / (r.den.max(1)) as f64, q.settings.sample_rate, q.duration())
    };
    if duration.0 <= 0 {
        return Err(bad(CMD, "the sequence is empty"));
    }
    let (w, h) = target_size(sw.min(sh), aspect);
    let name = match str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.chars().take(200).collect::<String>(),
        None => format!("{name} ({label})"),
    };

    let n0 = s.history.undo.len();
    let built = (|| -> Result<(ItemId, Vec<ClipId>)> {
        let layers = if mode == "blur" { 2 } else { 1 };
        let r =
            s.execute("file.newSequence", json!({"name": name, "width": w, "height": h, "fps": fps, "sampleRate": sample_rate, "video": layers, "audio": 1}))?;
        let seq = ItemId(r["sequence"].as_u64().ok_or_else(|| EngineError::Other("no sequence was made".into()))?);
        // the picture's scale: `fill` covers the frame, `fit` shows all of it across the width
        let fill = (w as f64 / sw.max(1) as f64).max(h as f64 / sh.max(1) as f64) * 100.0;
        let fit = (w as f64 / sw.max(1) as f64).min(h as f64 / sh.max(1) as f64) * 100.0;
        let place = |s: &mut Session, track: &str, with_audio: bool| -> Result<ClipId> {
            let mut q = json!({"item": src.0, "track": track, "seconds": 0.0});
            if with_audio {
                q["audioTrack"] = json!("A1");
            }
            let r = s.execute("timeline.place", q)?;
            // the video clip of the pair placed
            let ids: Vec<ClipId> = r["clips"].as_array().map(|a| a.iter().filter_map(Value::as_u64).map(ClipId).collect()).unwrap_or_default();
            let seq_now = s.active_sequence().ok_or(EngineError::NoSequence)?;
            ids.into_iter()
                .find(|c| seq_now.video_tracks.iter().any(|t| t.items.iter().any(|i| i.id == *c)))
                .ok_or_else(|| EngineError::Other("the nested clip was not placed".into()))
        };
        let scale =
            |s: &mut Session, c: ClipId, pct: f64| s.execute("effects.setParam", json!({"clip": c.0, "effect": "motion", "param": "scale", "value": pct}));
        let main = place(s, "V1", true)?;
        let mut clips = vec![main];
        match mode {
            "fill" => {
                scale(s, main, fill)?;
            }
            "follow" => {
                // Motion makes the picture cover the frame; Auto Reframe, told the frame's shape,
                // slides the picture so the part that shows is the part with the subject
                scale(s, main, fill)?;
                s.execute("effects.apply", json!({"clips": [main.0], "effect": "auto_reframe"}))?;
                let index = {
                    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
                    let (_, it) = q.find_item(main).ok_or_else(|| EngineError::Other("the nested clip is gone".into()))?;
                    it.effects.iter().position(|e| e.effect == "auto_reframe").ok_or_else(|| EngineError::Other("Auto Reframe was not applied".into()))?
                };
                s.execute("effects.setParam", json!({"clip": main.0, "effect": index, "param": "aspect", "value": reframe_aspect}))?;
            }
            _ => {
                // background: the picture enlarged to fill, blurred; foreground: all of it, on V2
                scale(s, main, fill)?;
                s.execute("effects.apply", json!({"clips": [main.0], "effect": "gaussian_blur"}))?;
                let blur_index = {
                    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
                    let (_, it) = q.find_item(main).ok_or_else(|| EngineError::Other("the nested clip is gone".into()))?;
                    it.effects.iter().position(|e| e.effect == "gaussian_blur").ok_or_else(|| EngineError::Other("the blur was not applied".into()))?
                };
                s.execute(
                    "effects.setParam",
                    json!({"clip": main.0, "effect": blur_index, "param": "blurriness", "value": (h as f64 * BACKGROUND_BLUR).round()}),
                )?;
                s.execute("effects.setParam", json!({"clip": main.0, "effect": blur_index, "param": "repeat_edge", "value": true}))?;
                let front = place(s, "V2", false)?;
                scale(s, front, fit)?;
                clips.push(front);
            }
        }
        Ok((seq, clips))
    })();
    let (seq, clips) = match built {
        Ok(v) => v,
        Err(e) => {
            // leave no half-made sequence behind
            while s.history.undo.len() > n0 {
                s.undo();
            }
            return Err(e);
        }
    };
    crate::clip_ops::collapse_history(s, n0, "Make Vertical Version");
    Ok(json!({"sequence": seq.0, "name": name, "width": w, "height": h, "mode": mode, "source": src.0, "clips": clips.iter().map(|c| c.0).collect::<Vec<_>>()}))
}
