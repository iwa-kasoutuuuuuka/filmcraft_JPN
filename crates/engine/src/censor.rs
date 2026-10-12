//! Clip ▸ Censor Area: a mosaic or blur over one area of a clip, ready to follow what it covers
//! (a face, a number plate).
//!
//! `clip.censor {clip?, style="mosaic"|"blur", shape="ellipse"|"rectangle", center:[x,y]?,
//! size:[w,h]?, strength=1..100=50, feather:px?, track="none"|"forward"|"backward"|"both",
//! wait=false}` does in one step what otherwise takes four: it applies the effect (Mosaic or
//! Gaussian Blur), sets its strength, draws a mask of that shape on it and selects the mask, so it
//! can be moved on the Program monitor right away. That is one undo step ("Censor Area").
//!
//! `center` and `size` are in clip pixels; without them the area is in the middle of the picture
//! (the mask command's default), to be dragged where it belongs. `strength` is the block size
//! (4 to 64 pixels) for a mosaic and the blurriness (2 to 200) for a blur.
//!
//! With `track`, the mask is then tracked from the playhead like Track Selected Mask
//! (`masks.track`): a background job (`jobs.list`; `wait: true` waits for it), each direction its
//! own undo step. `both` tracks forward and then backward and therefore always waits. Tracking
//! that fails (nothing to follow inside the area) does not undo the effect: the reply reports it
//! under `tracking`.

use filmcraft_project::ClipId;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, clip_p, f64_p, has_seq, str_p};
use crate::{EngineError, Result, Session};

const CMD: &str = "clip.censor";

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Censor Area (Mosaic / Blur)",
        menu: &["Clip"],
        shortcut: None,
        params: r#"{"clip":id?,"style":"mosaic|blur"="mosaic","shape":"ellipse|rectangle"="ellipse","center":[x,y]?,"size":[w,h]?,"strength":1..100=50,"feather":px?,"track":"none|forward|backward|both"="none","wait":bool=false}"#,
        enabled: has_video_clip,
        run: censor,
        journal: true,
    }]
}

/// The clip the command works on: `clip`, else the first selected video clip.
fn target(s: &Session, p: &Value) -> Option<ClipId> {
    let q = s.active_sequence()?;
    let is_video = |c: ClipId| q.video_tracks.iter().any(|t| t.items.iter().any(|i| i.id == c));
    match clip_p(p, "clip") {
        Some(c) => is_video(c).then_some(c),
        None => s.state.selection.iter().copied().find(|c| is_video(*c)),
    }
}

fn has_video_clip(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    if target(s, &Value::Null).is_some() { Ok(()) } else { Err("select a video clip first".into()) }
}

fn pair(v: Option<&Value>) -> Option<Option<[f64; 2]>> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Some(None) };
    let a = v.as_array().filter(|a| a.len() == 2)?;
    let (x, y) = (a.first()?.as_f64()?, a.get(1)?.as_f64()?);
    (x.is_finite() && y.is_finite()).then_some(Some([x, y]))
}

fn censor(s: &mut Session, p: &Value) -> Result<Value> {
    let clip = target(s, p).ok_or_else(|| bad(CMD, "select a video clip, or pass `clip`"))?;
    let (effect, mosaic) = match str_p(p, "style").unwrap_or("mosaic") {
        "mosaic" => ("mosaic", true),
        "blur" => ("gaussian_blur", false),
        _ => return Err(bad(CMD, "`style` must be `mosaic` or `blur`")),
    };
    let shape = match str_p(p, "shape").unwrap_or("ellipse") {
        "ellipse" => "ellipse",
        "rectangle" => "polygon",
        _ => return Err(bad(CMD, "`shape` must be `ellipse` or `rectangle`")),
    };
    let strength = f64_p(p, "strength").unwrap_or(50.0);
    if !(1.0..=100.0).contains(&strength) {
        return Err(bad(CMD, "`strength` must be between 1 and 100"));
    }
    let center = pair(p.get("center")).ok_or_else(|| bad(CMD, "`center` must be [x, y]"))?;
    let size = pair(p.get("size")).ok_or_else(|| bad(CMD, "`size` must be [width, height]"))?;
    if size.is_some_and(|[w, h]| w <= 0.0 || h <= 0.0) {
        return Err(bad(CMD, "`size` must be larger than zero"));
    }
    let feather = f64_p(p, "feather");
    if feather.is_some_and(|f| !(0.0..=1000.0).contains(&f)) {
        return Err(bad(CMD, "`feather` must be between 0 and 1000"));
    }
    let directions: &[&str] = match str_p(p, "track").unwrap_or("none") {
        "none" => &[],
        "forward" => &["forward"],
        "backward" => &["backward"],
        "both" => &["forward", "backward"],
        _ => return Err(bad(CMD, "`track` must be `none`, `forward`, `backward` or `both`")),
    };
    let wait = bool_p(p, "wait").unwrap_or(false) || directions.len() > 1;
    let (w, h) = {
        let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
        let (_, it) = q.find_item(clip).ok_or_else(|| bad(CMD, "no such clip"))?;
        filmcraft_render::source_size(&s.project, it.item).map(|(w, h)| (w as f64, h as f64)).unwrap_or((q.settings.width as f64, q.settings.height as f64))
    };

    // ---- the effect, its strength and the mask: one undo step
    let n0 = s.history.undo.len();
    let built = (|| -> Result<(usize, Value)> {
        s.execute("effects.apply", json!({"clips": [clip.0], "effect": effect}))?;
        // a new standard effect goes right before the clip's intrinsic effects
        let index = {
            let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
            let (_, it) = q.find_item(clip).ok_or_else(|| bad(CMD, "no such clip"))?;
            let first_intrinsic = it.effects.iter().position(|e| e.def().is_some_and(|d| d.intrinsic)).unwrap_or(it.effects.len());
            first_intrinsic
                .checked_sub(1)
                .filter(|i| it.effects.get(*i).is_some_and(|e| e.effect == effect))
                .ok_or_else(|| EngineError::Other("the effect was not applied".into()))?
        };
        let set =
            |s: &mut Session, param: &str, value: f64| s.execute("effects.setParam", json!({"clip": clip.0, "effect": index, "param": param, "value": value}));
        if mosaic {
            // strength 1..100 → blocks of 4..64 pixels
            let block = 4.0 + (strength - 1.0) * 60.0 / 99.0;
            set(s, "horizontal", (w / block).round().clamp(1.0, 4000.0))?;
            set(s, "vertical", (h / block).round().clamp(1.0, 4000.0))?;
        } else {
            set(s, "blurriness", strength * 2.0)?;
        }
        let mut m = json!({"clip": clip.0, "effect": index, "shape": shape});
        if let Some(c) = center {
            m["center"] = json!(c);
        }
        if let Some(z) = size {
            m["size"] = json!(z);
        }
        let mask = s.execute("masks.add", m)?;
        if let Some(f) = feather {
            s.execute("masks.set", json!({"clip": clip.0, "effect": index, "mask": mask["mask"], "feather": f}))?;
        }
        Ok((index, mask))
    })();
    let (index, mask) = match built {
        Ok(v) => v,
        Err(e) => {
            // leave nothing half done behind
            while s.history.undo.len() > n0 {
                s.undo();
            }
            return Err(e);
        }
    };
    crate::clip_ops::collapse_history(s, n0, "Censor Area");
    let mask_index = mask["mask"].clone();

    // ---- tracking: Track Selected Mask's own job and undo steps
    let mut tracking = Vec::new();
    for dir in directions {
        let r = s.execute("masks.track", json!({"clip": clip.0, "effect": index, "mask": mask_index, "direction": dir, "wait": wait}));
        tracking.push(match r {
            Ok(v) => json!({"direction": dir, "result": v}),
            Err(e) => json!({"direction": dir, "error": e.to_string()}),
        });
    }
    Ok(json!({"clip": clip.0, "effect": index, "effectId": effect, "mask": mask_index, "name": mask["name"], "tracking": tracking}))
}
