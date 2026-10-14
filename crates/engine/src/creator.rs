//! Tools for people publishing video online: YouTube chapters, beat markers, cutting to the beat
//! and a short-form version in one step.
//!
//! | command | menu | what it does |
//! |---|---|---|
//! | `markers.exportChapters` | Markers ▸ Copy YouTube Chapters | the chapter markers as YouTube's chapter list ("0:00 Intro"), checked against YouTube's rules; `path` writes it to a text file |
//! | `markers.addBeats` | Markers ▸ Add Beat Markers | a marker on every (or every `every`-th) beat of the selected music clip |
//! | `sequence.cutToBeats` | Sequence ▸ Cut to the Beat | moves every cut between adjacent clips of a video track to the nearest beat of the music (rolling edits, linked audio follows) |
//! | `sequence.makeShort` | Sequence ▸ Make Short Video… | a vertical version with the silences removed and captions, in one undo step |
//!
//! Beats come from [`filmcraft_audio_dsp::remix::beats`] (onset envelope, tempo by
//! autocorrelation, dynamic-programming beat tracking): music with a steady pulse works, speech
//! and free-tempo music give few or no beats.

use filmcraft_project::{ClipId, Label, Marker, MarkerId, MarkerKind, Sequence, TrackKind};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, f64_p, has_seq, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// Most beat markers one command adds.
const MAX_BEAT_MARKERS: usize = 5_000;
/// Longest stretch of music analysed for beats, seconds (an hour).
const MAX_BEAT_SECONDS: i64 = 3_600;
/// YouTube's rules for chapters: at least three, the first at 0:00, each at least 10 s long.
const YOUTUBE_MIN_CHAPTERS: usize = 3;
const YOUTUBE_MIN_SECONDS: f64 = 10.0;

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "markers.exportChapters",
            label: "Copy YouTube Chapters",
            menu: &["Markers"],
            shortcut: None,
            params: r#"{"path":str?,"firstTitle":str="Intro"}"#,
            enabled: has_markers,
            run: export_chapters,
            journal: false,
        },
        CommandSpec {
            id: "markers.addBeats",
            label: "Add Beat Markers",
            menu: &["Markers"],
            shortcut: None,
            params: r#"{"clip":id?,"every":n=1,"color":label="Cerulean"}"#,
            enabled: has_music,
            run: add_beats,
            journal: true,
        },
        CommandSpec {
            id: "sequence.cutToBeats",
            label: "Cut to the Beat",
            menu: &["Sequence"],
            shortcut: None,
            params: r#"{"clip":id?,"track":"V1"|id?,"every":n=1}"#,
            enabled: has_music,
            run: cut_to_beats,
            journal: true,
        },
        CommandSpec {
            id: "sequence.makeShort",
            label: "Make Short Video…",
            menu: &["Sequence"],
            shortcut: None,
            params: r#"{"aspect":"9:16|1:1|4:5"="9:16","mode":"follow|fill|blur"="follow","removeSilence":bool=true,"thresholdDb":f=-40,"minSeconds":f=0.5,"keepSeconds":f=0.1,"captions":bool=true,"name":str?}"#,
            enabled: has_video,
            run: make_short,
            journal: true,
        },
    ]
}

fn has_markers(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    let q = s.active_sequence().ok_or("no sequence is open")?;
    if q.markers.is_empty() { Err("the sequence has no markers".into()) } else { Ok(()) }
}

fn has_video(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    let q = s.active_sequence().ok_or("no sequence is open")?;
    if q.video_tracks.iter().any(|t| !t.items.is_empty()) { Ok(()) } else { Err("the sequence has no video clips".into()) }
}

fn has_music(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    selected_audio(s).map(|_| ()).ok_or_else(|| "select a music clip on an audio track".into())
}

/// The first selected clip on an audio track.
fn selected_audio(s: &Session) -> Option<ClipId> {
    let seq = s.active_sequence()?;
    s.state.selection.iter().copied().find(|c| seq.find_item(*c).and_then(|(t, _)| seq.track(t)).is_some_and(|t| t.kind == TrackKind::Audio))
}

// ---------------------------------------------------------------------------------------------
// YouTube chapters
// ---------------------------------------------------------------------------------------------

/// A time as YouTube writes it in chapter lists: `M:SS`, or `H:MM:SS` from an hour on.
pub fn youtube_time(t: Tick) -> String {
    let total = (t.seconds().max(0.0).floor() as i64).min(i64::from(u32::MAX));
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// The chapter list of a sequence: its chapter markers (or, without any, its named markers) as
/// `(start, title)`, a first chapter at 0:00 titled `first_title` when the markers start later,
/// and the ways it breaks YouTube's rules.
pub fn chapters(q: &Sequence, first_title: &str) -> (Vec<(Tick, String)>, Vec<String>) {
    let title = |m: &Marker| -> String {
        let t = if m.name.trim().is_empty() { m.comment.trim() } else { m.name.trim() };
        // one line: a chapter title cannot hold a line break
        t.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    let mut marks: Vec<&Marker> = q.markers.iter().filter(|m| m.kind == MarkerKind::Chapter).collect();
    if marks.is_empty() {
        marks = q.markers.iter().filter(|m| !title(m).is_empty()).collect();
    }
    marks.sort_by_key(|m| m.start);
    let mut out: Vec<(Tick, String)> = Vec::new();
    for (i, m) in marks.iter().enumerate() {
        let start = m.start.max(Tick::ZERO);
        // two markers in the same second would give two chapters at the same time: keep the first
        if out.last().is_some_and(|(t, _)| youtube_time(*t) == youtube_time(start)) {
            continue;
        }
        let name = title(m);
        out.push((start, if name.is_empty() { format!("Chapter {}", i + 1) } else { name }));
    }
    if out.first().is_none_or(|(t, _)| youtube_time(*t) != "0:00") {
        let first = first_title.trim();
        out.insert(0, (Tick::ZERO, if first.is_empty() { "Intro".to_string() } else { first.to_string() }));
    }
    let mut warnings = Vec::new();
    if out.len() < YOUTUBE_MIN_CHAPTERS {
        warnings.push(format!("YouTube shows chapters only when there are at least {YOUTUBE_MIN_CHAPTERS} (there are {})", out.len()));
    }
    let end = q.duration();
    for (i, (t, name)) in out.iter().enumerate() {
        let next = out.get(i + 1).map(|(n, _)| *n).unwrap_or(end);
        let len = (next - *t).seconds();
        if len < YOUTUBE_MIN_SECONDS {
            warnings.push(format!("chapter \"{name}\" at {} is {len:.1} s long; YouTube needs at least {YOUTUBE_MIN_SECONDS:.0} s", youtube_time(*t)));
        }
    }
    (out, warnings)
}

fn export_chapters(s: &mut Session, p: &Value) -> Result<Value> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (list, warnings) = chapters(q, str_p(p, "firstTitle").unwrap_or("Intro"));
    let text: String = list.iter().map(|(t, name)| format!("{} {name}\n", youtube_time(*t))).collect();
    if let Some(path) = str_p(p, "path").filter(|x| !x.trim().is_empty()) {
        filmcraft_format::atomic_write(std::path::Path::new(path), text.as_bytes()).map_err(|e| EngineError::Other(format!("writing {path}: {e}")))?;
    }
    Ok(json!({
        "text": text,
        "chapters": list.iter().map(|(t, name)| json!({"time": t.0, "timecode": youtube_time(*t), "title": name})).collect::<Vec<_>>(),
        "warnings": warnings,
    }))
}

// ---------------------------------------------------------------------------------------------
// beats
// ---------------------------------------------------------------------------------------------

/// The music clip a beat command works on: `clip`, or the selected audio clip.
fn music_clip(s: &Session, p: &Value, cmd: &str) -> Result<ClipId> {
    match u64_p(p, "clip") {
        Some(c) => Ok(ClipId(c)),
        None => selected_audio(s).ok_or_else(|| bad(cmd, "select a music clip on an audio track or pass `clip`")),
    }
}

/// The beats of a music clip as sequence times (frame-snapped, increasing, inside the clip),
/// every `every`-th one from the first, and the tempo.
fn clip_beats(s: &Session, c: ClipId, every: usize, cmd: &str) -> Result<(Vec<Tick>, f64)> {
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (tid, it) = seq.find_item(c).ok_or_else(|| bad(cmd, "no such clip"))?;
    if seq.track(tid).is_none_or(|t| t.kind != TrackKind::Audio) {
        return Err(bad(cmd, "beats are found in audio clips"));
    }
    if (it.speed - 1.0).abs() > 1e-9 || it.reverse {
        return Err(bad(cmd, "set the music clip's speed back to 100 % first"));
    }
    let sr = seq.settings.sample_rate.max(1);
    let rate = seq.settings.frame_rate;
    let m0 = it.source_in.to_units_floor(i64::from(sr));
    let len = it.duration.to_units_floor(i64::from(sr)).clamp(0, MAX_BEAT_SECONDS * i64::from(sr));
    let (start, end) = (it.start, it.end());
    let item = it.item;
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let src = filmcraft_render::SourceProvider::source(&provider, item).ok_or_else(|| EngineError::Other("the clip's media is offline".into()))?;
    let buf = src.audio(m0, usize::try_from(len).unwrap_or(0), sr).map_err(|e| EngineError::Other(format!("reading the clip's audio: {e}")))?;
    let refs: Vec<&[f32]> = buf.channels.iter().map(Vec::as_slice).collect();
    let (beats, bpm) = filmcraft_audio_dsp::remix::beats(&refs, sr).map_err(|e| bad(cmd, format!("no beat found in the clip ({e})")))?;
    let mut times: Vec<Tick> =
        beats.iter().map(|b| rate.snap_nearest(start + Tick::from_units(*b, i64::from(sr)))).filter(|t| *t >= start && *t < end).collect();
    times.dedup();
    let times = times.into_iter().step_by(every.max(1)).collect();
    Ok((times, bpm))
}

fn every_p(p: &Value, cmd: &str) -> Result<usize> {
    match p.get("every") {
        None | Some(Value::Null) => Ok(1),
        Some(v) => match v.as_u64() {
            Some(n @ 1..=64) => Ok(n as usize),
            _ => Err(bad(cmd, "`every` must be a whole number from 1 to 64")),
        },
    }
}

fn add_beats(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "markers.addBeats";
    let c = music_clip(s, p, CMD)?;
    let every = every_p(p, CMD)?;
    let color = match str_p(p, "color") {
        Some(n) => Label::from_name(n).ok_or_else(|| bad(CMD, format!("unknown label colour `{n}`")))?,
        None => Label::Cerulean,
    };
    let (times, bpm) = clip_beats(s, c, every, CMD)?;
    let existing: Vec<Tick> = s.active_sequence().map(|q| q.markers.iter().map(|m| m.start).collect()).unwrap_or_default();
    let new: Vec<Tick> = times.into_iter().filter(|t| !existing.contains(t)).take(MAX_BEAT_MARKERS).collect();
    let n = new.len();
    if n > 0 {
        s.edit_sequence("Add Beat Markers", |q, ctx, _| {
            for (k, t) in new.iter().enumerate() {
                let id = MarkerId(ctx.alloc());
                q.markers.push(Marker {
                    id,
                    start: *t,
                    duration: Tick::ZERO,
                    name: format!("Beat {}", k + 1),
                    comment: String::new(),
                    kind: MarkerKind::Comment,
                    color,
                });
            }
            q.markers.sort_by_key(|m| m.start);
            Ok(())
        })?;
    }
    Ok(json!({"added": n, "bpm": bpm, "every": every}))
}

/// The nearest of `beats` (increasing) to `t`.
fn nearest(beats: &[Tick], t: Tick) -> Option<Tick> {
    let i = beats.partition_point(|b| *b < t);
    [i.checked_sub(1), Some(i)].into_iter().flatten().filter_map(|k| beats.get(k).copied()).min_by_key(|b| (*b - t).0.abs())
}

fn video_track_p(s: &Session, p: &Value) -> Option<filmcraft_project::TrackId> {
    let q = s.active_sequence()?;
    match p.get("track") {
        None | Some(Value::Null) => q.video_tracks.first().map(|t| t.id),
        Some(Value::String(name)) => {
            let n: usize = name.trim().trim_start_matches(['V', 'v']).parse().ok()?;
            q.video_tracks.get(n.checked_sub(1)?).map(|t| t.id)
        }
        Some(v) => {
            let id = v.as_u64()?;
            q.video_tracks.iter().find(|t| t.id.0 == id).map(|t| t.id)
        }
    }
}

fn cut_to_beats(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "sequence.cutToBeats";
    let c = music_clip(s, p, CMD)?;
    let every = every_p(p, CMD)?;
    let tid = video_track_p(s, p).ok_or_else(|| bad(CMD, "no such video track"))?;
    let (beats, bpm) = clip_beats(s, c, every, CMD)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let track = q.track(tid).ok_or_else(|| bad(CMD, "no such video track"))?;
    if track.locked {
        return Err(bad(CMD, "the video track is locked"));
    }
    // the cuts: adjacent clips, both edges inside the music
    let (first, last) = (beats.first().copied().unwrap_or_default(), beats.last().copied().unwrap_or_default());
    let mut cuts: Vec<(ClipId, ClipId, Tick)> = Vec::new();
    for w in track.items.windows(2) {
        if let [l, r] = w
            && l.end() == r.start
            && r.start >= first
            && r.start <= last
        {
            cuts.push((l.id, r.id, r.start));
        }
    }
    if cuts.is_empty() {
        return Ok(json!({"moved": 0, "onBeat": 0, "limited": 0, "cuts": 0, "bpm": bpm}));
    }
    let report = s.edit_sequence("Cut to the Beat", |q, ctx, _| {
        let (mut moved, mut on_beat, mut limited) = (0usize, 0usize, 0usize);
        for (l, r, at) in &cuts {
            let Some(b) = nearest(&beats, *at) else { continue };
            let want = b - *at;
            if want == Tick::ZERO {
                on_beat += 1;
                continue;
            }
            // the linked audio of both sides, when it is cut at the same place
            let link_of = |q: &Sequence, c: ClipId| q.find_item(c).and_then(|(_, i)| i.link);
            let partner = |q: &Sequence, link: Option<u64>, edge: Tick, left: bool| -> Option<ClipId> {
                let link = link?;
                q.audio_tracks
                    .iter()
                    .flat_map(|t| &t.items)
                    .find(|i| i.link == Some(link) && if left { i.end() == edge } else { i.start == edge })
                    .map(|i| i.id)
            };
            let audio = partner(q, link_of(q, *l), *at, true).zip(partner(q, link_of(q, *r), *at, false));
            let before = q.clone();
            let got = filmcraft_edit::roll(q, *l, *r, want, ctx)?;
            let audio_ok = match audio {
                Some((al, ar)) if got != Tick::ZERO => filmcraft_edit::roll(q, al, ar, got, ctx).is_ok_and(|a| a == got),
                _ => true,
            };
            if !audio_ok {
                // the sound would come apart from the picture: leave this cut where it was
                *q = before;
                limited += 1;
            } else if got == want {
                moved += 1;
            } else {
                // not enough media beyond the cut to reach the beat
                if got == Tick::ZERO {
                    *q = before;
                }
                limited += 1;
            }
        }
        Ok((moved, on_beat, limited))
    })?;
    let (moved, on_beat, limited) = report;
    Ok(json!({"moved": moved, "onBeat": on_beat, "limited": limited, "cuts": cuts.len(), "bpm": bpm}))
}

// ---------------------------------------------------------------------------------------------
// short video
// ---------------------------------------------------------------------------------------------

/// Caption style for a vertical frame: larger text, above the bottom fifth where the platforms
/// put their buttons and titles.
fn short_caption_style(s: &mut Session) -> Result<()> {
    let tracks: Vec<u64> = s.active_sequence().map(|q| q.caption_tracks.iter().map(|t| t.id.0).collect()).unwrap_or_default();
    for t in tracks {
        s.execute("captions.setStyle", json!({"track": t, "size": 64.0, "anchor": "bottom", "margin": 0.22, "outline": 4.0, "background": false}))?;
    }
    Ok(())
}

fn make_short(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "sequence.makeShort";
    let flag = |k: &str| -> Result<bool> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(true),
            Some(v) => v.as_bool().ok_or_else(|| bad(CMD, format!("`{k}` must be true or false"))),
        }
    };
    let (silence, captions) = (flag("removeSilence")?, flag("captions")?);
    let mut vertical = json!({});
    for k in ["aspect", "mode", "name"] {
        if let Some(v) = p.get(k).filter(|v| !v.is_null()) {
            vertical[k] = v.clone();
        }
    }
    let original = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let n0 = s.history.undo.len();
    let built = (|| -> Result<Value> {
        let v = s.execute("sequence.makeVertical", vertical)?;
        let mut steps = vec![json!({"step": "makeVertical", "sequence": v["sequence"]})];
        let mut skipped: Vec<String> = Vec::new();
        // captions first, from the original's transcript (the nested clip starts at 0, so its times are
        // the vertical sequence's); removing the silences then moves them with the picture
        if captions {
            let words = s.project.sequence(original).map(|q| filmcraft_edit::transcript::sequence_words(q, &s.project.transcripts)).unwrap_or_default();
            if words.is_empty() {
                skipped.push("captions: the sequence has no transcript (Sequence ▸ Transcribe Sequence… first)".to_string());
            } else {
                let r = crate::transcript::captions_from_words(s, &words, &json!({"maxChars": 16, "lines": 2, "name": "Short"}))?;
                short_caption_style(s)?;
                steps.push(json!({"step": "captions", "captions": r["captions"]}));
            }
        }
        if silence {
            let mut q = json!({});
            for k in ["thresholdDb", "minSeconds", "keepSeconds"] {
                if let Some(x) = f64_p(p, k) {
                    q[k] = json!(x);
                }
            }
            match s.execute("sequence.removeSilence", q) {
                Ok(r) => steps.push(json!({"step": "removeSilence", "removed": r["removed"], "seconds": r["seconds"]})),
                // a sequence without sound has no silence to remove; bad parameters are the caller's to fix
                Err(e @ EngineError::BadParams { .. }) => return Err(e),
                Err(e) => skipped.push(format!("removeSilence: {e}")),
            }
        }
        Ok(json!({"sequence": v["sequence"], "name": v["name"], "width": v["width"], "height": v["height"], "steps": steps, "skipped": skipped}))
    })();
    match built {
        Ok(r) => {
            crate::clip_ops::collapse_history(s, n0, "Make Short Video");
            Ok(r)
        }
        Err(e) => {
            while s.history.undo.len() > n0 {
                s.undo();
            }
            Err(e)
        }
    }
}
