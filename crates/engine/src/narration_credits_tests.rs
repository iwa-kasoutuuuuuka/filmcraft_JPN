//! Tests of [`crate::narration_credits`] against the fake engine of `narration_tests`.

use serde_json::json;

use crate::Session;
use crate::narration_credits::{CREDIT_KEY, ROLL_NAME, ROLL_TAG, VOICE_KEY};
use crate::narration_tests::{Mode, fake, out_dir};

/// The demo project with the interface language set to English (the heading follows
/// Settings ▸ Appearance ▸ Language; Auto with an unknown system language is Japanese).
fn demo() -> Session {
    let mut s = crate::narration_tests::demo();
    s.prefs.appearance.language = "en".into();
    s
}

fn rolls(s: &Session) -> Vec<filmcraft_project::TrackItem> {
    let q = s.active_sequence().unwrap();
    q.video_tracks.iter().flat_map(|t| &t.items).filter(|i| i.name == ROLL_NAME).cloned().collect()
}

fn roll_text(it: &filmcraft_project::TrackItem) -> String {
    let l = filmcraft_project::graphic::layer_indices(&it.effects)[0];
    match &it.effects[l].params["text"].value {
        filmcraft_project::ParamValue::Text(t) => t.clone(),
        other => panic!("{other:?}"),
    }
}

/// Two synthesize calls with different voices (and engines); the credits accumulate, survive
/// save / load, and the narration a user deleted from the timeline is no longer credited.
#[test]
fn credits_accumulate_over_calls_and_survive_save_and_load() {
    let vv = fake(Mode::Good, false);
    let av = fake(Mode::Good, true);
    let mut s = demo();
    s.prefs.narration.voicevox_url = vv;
    s.prefs.narration.aivisspeech_url = av;
    let dir = out_dir("credits-acc");
    let undo0 = s.history.undo.len();
    let r1 = s
        .execute("tts.synthesize", json!({"engine": "voicevox", "lines": [{"text": "あい", "voice": "ずんだもん"}, {"text": "うえ", "voice": "ずんだもん/あまあま"}], "dir": dir, "time": 0, "captions": false}))
        .unwrap();
    // the voice is remembered on the item, inside the same undo step
    let item = r1["items"][0].as_u64().unwrap();
    let it = s.project.item(filmcraft_project::ItemId(item)).unwrap();
    assert_eq!(it.metadata[CREDIT_KEY], "VOICEVOX:ずんだもん");
    assert!(it.metadata[VOICE_KEY].contains("uuid-zunda"));
    assert_eq!(s.history.undo.len(), undo0 + 1);
    assert!(r1.get("creditRoll").is_none(), "no roll by default");
    let r2 = s
        .execute("tts.synthesize", json!({"engine": "aivisspeech", "text": "おか", "voice": "ずんだもん", "dir": dir, "seconds": 5.0, "captions": false}))
        .unwrap();
    s.execute("tts.synthesize", json!({"engine": "voicevox", "text": "きく", "voice": "四国めたん", "dir": dir, "seconds": 8.0, "captions": false})).unwrap();
    let c = s.execute("tts.credits", json!({})).unwrap();
    // timeline order, one per voice (the same speaker on two engines is two credits)
    assert_eq!(c["creditText"], "VOICEVOX:ずんだもん\nAivisSpeech:ずんだもん\nVOICEVOX:四国めたん");
    assert_eq!(c["credits"][1]["license"], "ACML 1.0");
    assert_eq!(c["credits"][0]["termsCommand"]["command"], "tts.voiceTerms");

    // save and load
    let proj = std::path::Path::new(&dir).join("credits.fcproj").to_string_lossy().into_owned();
    s.execute("file.saveAs", json!({"path": proj})).unwrap();
    let mut t = Session::default();
    t.execute("file.open", json!({"path": proj})).unwrap();
    let c2 = t.execute("tts.credits", json!({})).unwrap();
    assert_eq!(c2["creditText"], c["creditText"]);
    assert_eq!(c2["credits"][1]["modelUrl"], c["credits"][1]["modelUrl"]);

    // a deleted clip is no longer credited in the sequence, but still in the project
    let clip = r2["clips"][0].as_u64().unwrap();
    t.state.selection = vec![filmcraft_project::ClipId(clip)];
    t.execute("edit.clear", json!({})).unwrap();
    assert_eq!(t.execute("tts.credits", json!({})).unwrap()["creditText"], "VOICEVOX:ずんだもん\nVOICEVOX:四国めたん");
    let all = t.execute("tts.credits", json!({"scope": "project"})).unwrap();
    assert_eq!(all["credits"].as_array().unwrap().len(), 3);
    // editing the Narration Credit metadata changes the credit line
    let first = r1["items"][0].as_u64().unwrap();
    t.execute("metadata.set", json!({"item": first, "field": CREDIT_KEY, "value": "VOICEVOX:ずんだもん(東北ずん子・ずんだもんプロジェクト)"})).unwrap();
    assert!(t.execute("tts.credits", json!({})).unwrap()["creditText"].as_str().unwrap().starts_with("VOICEVOX:ずんだもん(東北"));
    assert!(t.execute("tts.credits", json!({"scope": "galaxy"})).is_err());
}

/// The roll is added once at the sequence end and updated (text, position) by later calls; it
/// is never duplicated, and each add / update is one undo step.
#[test]
fn credit_roll_is_added_once_then_updated() {
    let vv = fake(Mode::Good, false);
    let mut s = demo();
    s.prefs.narration.voicevox_url = vv;
    let dir = out_dir("credits-roll");
    // nothing to credit yet
    let e = s.execute("tts.addCreditRoll", json!({})).unwrap_err().to_string();
    assert!(e.contains("no narration"), "{e}");
    s.execute("tts.synthesize", json!({"engine": "voicevox", "text": "あい", "voice": "ずんだもん", "dir": dir, "captions": false})).unwrap();
    let undo0 = s.history.undo.len();
    let end0 = s.active_sequence().unwrap().all_tracks().flat_map(|t| &t.items).map(|i| i.end()).max().unwrap();
    let a = s.execute("tts.addCreditRoll", json!({"extra": ["Music: someone"]})).unwrap();
    assert_eq!(a["updated"], false);
    assert_eq!(s.history.undo.len(), undo0 + 1);
    assert_eq!(s.history.undo.last().unwrap().0, "Narration Credit Roll");
    let rl = rolls(&s);
    assert_eq!(rl.len(), 1);
    let roll = &rl[0];
    assert_eq!(roll.id.0, a["clip"].as_u64().unwrap());
    assert!(roll.start >= end0, "at the end: {:?} vs {:?}", roll.start, end0);
    assert!(roll.duration.seconds() >= 5.0 - 1e-9);
    assert_eq!(roll_text(roll), "Voices\n\nVOICEVOX:ずんだもん\n\nMusic: someone");
    let g = roll.graphic.as_ref().unwrap();
    assert_eq!(g.roll.mode, filmcraft_project::graphic_design::RollMode::Roll);
    assert!(g.roll.start_off_screen && g.roll.end_off_screen);
    // Japanese names render: the Japanese face on Automatic
    let l = filmcraft_project::graphic::layer_indices(&roll.effects)[0];
    assert_eq!(roll.effects[l].params["font"].value, filmcraft_project::ParamValue::Text(filmcraft_text::fonts::JAPANESE_FAMILY.into()));
    // the text is centred in the frame
    let (w, _) = {
        let q = s.active_sequence().unwrap();
        (q.settings.width, q.settings.height)
    };
    let specs = filmcraft_render::graphic_clip::item_layer_specs(roll, roll.source_in, (w, 1080));
    let quad = filmcraft_render::graphic_clip::layer_quad(&specs[0].1);
    let cx = (quad[0].x + quad[1].x) / 2.0;
    assert!((cx - w as f64 / 2.0).abs() < w as f64 * 0.05, "centred: {cx}");

    // more narration after the roll (another voice), with creditRoll: the same clip is updated
    // and moved to the new end
    let roll_start = roll.start.seconds();
    let r2 = s
        .execute(
            "tts.synthesize",
            json!({"engine": "voicevox", "text": "うえお", "voice": "四国めたん", "dir": dir, "captions": false, "seconds": roll_start + 10.0, "creditRoll": true}),
        )
        .unwrap();
    assert_eq!(r2["creditRoll"]["updated"], true, "{r2}");
    assert_eq!(s.history.undo.len(), undo0 + 2, "narration + roll update are one step");
    let rl = rolls(&s);
    assert_eq!(rl.len(), 1, "not duplicated");
    assert_eq!(rl[0].id, roll.id);
    // the extra lines given before are kept
    assert_eq!(roll_text(&rl[0]), "Voices\n\nVOICEVOX:ずんだもん\nVOICEVOX:四国めたん\n\nMusic: someone");
    let narr_end = s.active_sequence().unwrap().all_tracks().flat_map(|t| &t.items).filter(|i| i.id != roll.id).map(|i| i.end()).max().unwrap();
    assert!(rl[0].start >= narr_end, "moved to the end");
    // the selection stays on the narration
    assert_eq!(
        s.state.selection.iter().map(|c| c.0).collect::<Vec<_>>(),
        r2["clips"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect::<Vec<_>>()
    );
    // creditRoll: false leaves the roll alone (and never adds a second one)
    s.execute(
        "tts.synthesize",
        json!({"engine": "voicevox", "text": "か", "voice": "ずんだもん/あまあま", "dir": dir, "captions": false, "seconds": 1.0, "creditRoll": false}),
    )
    .unwrap();
    assert_eq!(rolls(&s).len(), 1);
    // undo the roll update: the previous text comes back
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(roll_text(&rolls(&s)[0]), "Voices\n\nVOICEVOX:ずんだもん\n\nMusic: someone");
    // the heading stays the one the roll was made with, whatever the interface says now; a
    // new title replaces it. Hostile parameters are errors
    s.japanese_text = true;
    let u = s.execute("tts.addCreditRoll", json!({"seconds": 4.0})).unwrap();
    assert_eq!(u["updated"], true);
    assert!(roll_text(&rolls(&s)[0]).starts_with("Voices\n\nVOICEVOX:ずんだもん"));
    s.execute("tts.addCreditRoll", json!({"title": "音声"})).unwrap();
    assert!(roll_text(&rolls(&s)[0]).starts_with("音声\n\nVOICEVOX:ずんだもん"));
    for p in [json!({"seconds": -1.0}), json!({"extra": [1, 2]}), json!({"extra": vec!["x"; 60]}), json!({"at": "soon"}), json!({"extra": 5})] {
        assert!(s.execute("tts.addCreditRoll", p.clone()).is_err(), "{p}");
    }
    assert_eq!(rolls(&s).len(), 1);
}

/// `creditRoll: true` (or the preference) on the first synthesis adds the roll in the same undo
/// step.
#[test]
fn synthesize_with_the_credit_roll_preference() {
    let vv = fake(Mode::Good, false);
    let mut s = demo();
    s.prefs.narration.voicevox_url = vv;
    s.prefs.narration.credit_roll = true;
    let undo0 = s.history.undo.len();
    let items0 = s.project.items.len();
    let r = s
        .execute("tts.synthesize", json!({"engine": "voicevox", "text": "あい", "voice": "ずんだもん", "dir": out_dir("credits-pref"), "captions": false}))
        .unwrap();
    assert_eq!(r["creditRoll"]["updated"], false, "{r}");
    assert_eq!(rolls(&s).len(), 1);
    assert_eq!(s.history.undo.len(), undo0 + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(rolls(&s).is_empty());
    assert!(s.project.items.len() <= items0 + 1, "the narration item is gone (a graphic source item may stay)");
}

/// `tts.exportCredits` writes the description text: credit lines, the AivisSpeech model and its
/// licence under its voice.
#[test]
fn export_credits_writes_the_description_text() {
    let vv = fake(Mode::Good, false);
    let av = fake(Mode::Good, true);
    let mut s = demo();
    s.prefs.narration.voicevox_url = vv;
    s.prefs.narration.aivisspeech_url = av;
    let dir = out_dir("credits-export");
    assert!(s.execute("tts.exportCredits", json!({"path": format!("{dir}/none.txt")})).is_err(), "nothing to credit");
    s.execute("tts.synthesize", json!({"engine": "voicevox", "text": "あい", "voice": "四国めたん", "dir": dir, "time": 0, "captions": false})).unwrap();
    s.execute("tts.synthesize", json!({"engine": "aivisspeech", "text": "うえ", "voice": "ずんだもん", "dir": dir, "seconds": 3.0, "captions": false}))
        .unwrap();
    let path = std::path::Path::new(&dir).join("description.txt").to_string_lossy().into_owned();
    let r = s.execute("tts.exportCredits", json!({"path": path})).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text, r["text"].as_str().unwrap());
    assert!(text.starts_with("Voices:\nVOICEVOX:四国めたん\nAivisSpeech:ずんだもん\n  Model: Zunda Model / Licence: ACML 1.0 / https://"), "{text}");
    assert!(text.trim_end().ends_with("/aivm-models/m1"), "{text}");
    // Japanese interface; default path next to the saved project
    s.japanese_text = true;
    let proj = std::path::Path::new(&dir).join("Movie.fcproj").to_string_lossy().into_owned();
    s.execute("file.saveAs", json!({"path": proj})).unwrap();
    let r = s.execute("tts.exportCredits", json!({})).unwrap();
    let p = r["path"].as_str().unwrap();
    assert!(p.ends_with("Movie_credits.txt"), "{p}");
    let text = std::fs::read_to_string(p).unwrap();
    assert!(text.starts_with("音声:\nVOICEVOX:四国めたん\nAivisSpeech:ずんだもん\n  モデル: Zunda Model / ライセンス: ACML 1.0"), "{text}");
}

// ------------------------------------------------------------------------------------- helpers

fn roll_layer(it: &filmcraft_project::TrackItem) -> usize {
    filmcraft_project::graphic::layer_indices(&it.effects)[0]
}

fn layer_text(it: &filmcraft_project::TrackItem, key: &str) -> String {
    match &it.effects[roll_layer(it)].params[key].value {
        filmcraft_project::ParamValue::Text(t) => t.clone(),
        other => panic!("{other:?}"),
    }
}

/// Change the roll clip in one edit (as the user would through a panel).
fn change_roll(s: &mut Session, f: impl FnOnce(&mut filmcraft_project::TrackItem)) {
    let id = rolls(s)[0].id;
    s.edit_sequence("user edit", |q, _, _| {
        let (_, it) = q.find_item_mut(id).unwrap();
        f(it);
        Ok(())
    })
    .unwrap();
}

fn set_layer_text(it: &mut filmcraft_project::TrackItem, key: &str, v: &str) {
    let l = roll_layer(it);
    it.effects[l].params.insert(key.into(), filmcraft_project::Param::new(filmcraft_project::ParamValue::Text(v.into())));
}

fn end_without(s: &Session, skip: filmcraft_project::ClipId) -> filmcraft_time::Tick {
    s.active_sequence().unwrap().all_tracks().flat_map(|t| &t.items).filter(|i| i.id != skip).map(|i| i.end()).max().unwrap()
}

/// A session with one VOICEVOX narration line and a credit roll at the end.
fn with_roll(name: &str, roll: serde_json::Value) -> (Session, String) {
    let vv = fake(Mode::Good, false);
    let mut s = demo();
    s.prefs.narration.voicevox_url = vv;
    let dir = out_dir(name);
    s.execute("tts.synthesize", json!({"engine": "voicevox", "text": "あい", "voice": "ずんだもん", "dir": dir, "captions": false})).unwrap();
    s.execute("tts.addCreditRoll", roll).unwrap();
    (s, dir)
}

fn synth_more(s: &mut Session, dir: &str, voice: &str, seconds: f64) -> serde_json::Value {
    s.execute("tts.synthesize", json!({"engine": "voicevox", "text": "うえお", "voice": voice, "dir": dir, "captions": false, "seconds": seconds})).unwrap()
}

// ------------------------------------------------------------------------------------- tests

/// Regression: credits of a sequence nesting 200 clips of a sequence nesting 200 clips of a
/// sequence with 200 narration clips used to walk 200³ clips (and keep 8 million entries).
/// Each nested sequence is walked once now.
#[test]
fn deeply_repeated_nesting_is_walked_once_per_sequence() {
    use filmcraft_project::{ItemKind, Label, Project, SequenceSettings, TrackKind};
    use filmcraft_time::{Tick, TimeRange};
    let mut p = Project::default();
    let rate = SequenceSettings::default().frame_rate;
    let narr = |p: &mut Project, credit: &str| {
        let id = p.add_item(credit, Label::Violet, ItemKind::AdjustmentLayer { width: 16, height: 16, rate, duration: Tick::from_seconds_f64(1.0) }, None);
        p.item_mut(id).unwrap().metadata.insert(CREDIT_KEY.into(), credit.into());
        id
    };
    let a = narr(&mut p, "VOICEVOX:ずんだもん");
    let b = narr(&mut p, "VOICEVOX:四国めたん");
    let one = Tick::from_seconds_f64(1.0);
    let fill = |p: &mut Project, seq: filmcraft_project::ItemId, items: &[filmcraft_project::ItemId]| {
        let mut clips = Vec::new();
        for k in 0..200usize {
            let item = items[k % items.len()];
            clips.push(p.make_track_item(item, TrackKind::Audio, Tick(one.0 * k as i64), TimeRange::new(Tick::ZERO, one), rate).unwrap());
        }
        p.sequence_mut(seq).unwrap().audio_tracks[0].items = clips;
    };
    let leaf = p.new_sequence("leaf", SequenceSettings::default(), 1, 1, None);
    // the later voice comes first in the leaf: timeline order is kept
    fill(&mut p, leaf, &[b, a]);
    let mid = p.new_sequence("mid", SequenceSettings::default(), 1, 1, None);
    fill(&mut p, mid, &[leaf]);
    let top = p.new_sequence("top", SequenceSettings::default(), 1, 1, None);
    fill(&mut p, top, &[mid]);
    // a cycle back to the top is ignored
    let back = p.make_track_item(top, TrackKind::Audio, Tick(one.0 * 300), TimeRange::new(Tick::ZERO, one), rate).unwrap();
    p.sequence_mut(leaf).unwrap().audio_tracks[0].items.push(back);
    assert!(matches!(p.item(top).unwrap().kind, ItemKind::Sequence(_)));
    let t0 = std::time::Instant::now();
    let c = crate::narration_credits::collect(&p, Some(top));
    assert!(t0.elapsed() < std::time::Duration::from_secs(5), "took {:?}", t0.elapsed());
    assert_eq!(crate::narration_credits::credit_text(&c), "VOICEVOX:四国めたん\nVOICEVOX:ずんだもん");
}

/// An automatic update after Generate keeps the custom title and extra lines; text the user
/// edited is left alone (`skipped: "edited"`) until an explicit `tts.addCreditRoll`, which still
/// keeps the stored title and extra lines.
#[test]
fn generate_keeps_the_roll_title_extra_lines_and_user_edits() {
    let (mut s, dir) = with_roll("credits-keep", json!({"title": "Cast", "extra": ["Music: someone"]}));
    let id = rolls(&s)[0].id;
    assert_eq!(roll_text(&rolls(&s)[0]), "Cast\n\nVOICEVOX:ずんだもん\n\nMusic: someone");
    let end = end_without(&s, id).seconds();
    let r = synth_more(&mut s, &dir, "四国めたん", end + 1.0);
    assert_eq!(r["creditRoll"]["changed"], true, "{r}");
    assert_eq!(roll_text(&rolls(&s)[0]), "Cast\n\nVOICEVOX:ずんだもん\nVOICEVOX:四国めたん\n\nMusic: someone");

    // the user types into the roll: Generate leaves it alone (one undo step, the narration's)
    change_roll(&mut s, |it| set_layer_text(it, "text", "My own credits"));
    let undo0 = s.history.undo.len();
    let r = synth_more(&mut s, &dir, "ずんだもん/あまあま", 1.0);
    assert_eq!(r["creditRoll"]["skipped"], "edited", "{r}");
    assert_eq!(roll_text(&rolls(&s)[0]), "My own credits");
    assert_eq!(s.history.undo.len(), undo0 + 1);
    // an explicit update regenerates it, with the stored title and extra lines
    let u = s.execute("tts.addCreditRoll", json!({})).unwrap();
    assert_eq!(u["changed"], true);
    let t = roll_text(&rolls(&s)[0]);
    assert!(t.starts_with("Cast\n\nVOICEVOX:ずんだもん\n") && t.ends_with("\n\nMusic: someone"), "{t}");
    // new extra lines replace the stored ones
    s.execute("tts.addCreditRoll", json!({"extra": ["Edit: me"]})).unwrap();
    assert!(roll_text(&rolls(&s)[0]).ends_with("\n\nEdit: me"));
    assert_eq!(rolls(&s).len(), 1);
}

/// A font the user picked for the roll stays; the font the command chose follows the text.
#[test]
fn generate_keeps_the_users_roll_font() {
    let (mut s, dir) = with_roll("credits-font", json!({}));
    assert_eq!(layer_text(&rolls(&s)[0], "font"), filmcraft_text::fonts::JAPANESE_FAMILY);
    change_roll(&mut s, |it| set_layer_text(it, "font", "Some Display Face"));
    synth_more(&mut s, &dir, "四国めたん", 1.0);
    let roll = &rolls(&s)[0];
    assert!(roll_text(roll).contains("四国めたん"));
    assert_eq!(layer_text(roll, "font"), "Some Display Face");
}

/// The roll moves with the end only when it sat at the end; a roll the user placed stays; an
/// explicit `at` moves it. A chosen length is kept; a string `at` is clamped.
#[test]
fn generate_keeps_a_placed_roll() {
    let (mut s, dir) = with_roll("credits-place", json!({}));
    let id = rolls(&s)[0].id;
    // the user moves it 30 s past the end (an end card in between, say)
    let end = end_without(&s, id);
    let placed = filmcraft_time::Tick(end.0 + filmcraft_time::Tick::from_seconds_f64(30.0).0);
    change_roll(&mut s, |it| it.start = placed);
    // narration after the old end, still before the roll: the roll stays
    let r = synth_more(&mut s, &dir, "四国めたん", end.seconds() + 0.5);
    assert_eq!(r["creditRoll"]["moved"], false, "{r}");
    assert_eq!(rolls(&s)[0].start, placed);
    assert!(roll_text(&rolls(&s)[0]).contains("四国めたん"));
    // an explicit `at` moves it
    let u = s.execute("tts.addCreditRoll", json!({"at": {"seconds": 100.0}})).unwrap();
    assert_eq!(u["moved"], true, "{u}");
    assert!((rolls(&s)[0].start.seconds() - 100.0).abs() < 0.1);
    // a chosen length is kept by later updates
    s.execute("tts.addCreditRoll", json!({"seconds": 4.0})).unwrap();
    let d4 = rolls(&s)[0].duration;
    assert!((d4.seconds() - 4.0).abs() < 0.1);
    for (k, v) in ["ずんだもん/あまあま", "四国めたん"].into_iter().enumerate() {
        synth_more(&mut s, &dir, v, 1.0 + k as f64);
    }
    assert_eq!(rolls(&s)[0].duration, d4);
    // a string `at` far in the future is clamped to an hour past the media end
    s.execute("tts.addCreditRoll", json!({"at": "1e12"})).unwrap();
    let e = end_without(&s, id).seconds();
    assert!(rolls(&s)[0].start.seconds() <= e + 3600.0 + 1.0, "{}", rolls(&s)[0].start.seconds());
}

/// The automatic duration grows when a later voice adds lines.
#[test]
fn automatic_duration_grows_with_the_lines() {
    let (mut s, dir) = with_roll("credits-grow", json!({"extra": ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"]}));
    let id = rolls(&s)[0].id;
    let d0 = rolls(&s)[0].duration;
    let end = end_without(&s, id).seconds();
    synth_more(&mut s, &dir, "四国めたん", end + 0.5);
    assert!(rolls(&s)[0].duration > d0, "{:?} vs {d0:?}", rolls(&s)[0].duration);
}

/// A roll on a locked track is not touched: Generate reports it and succeeds; an explicit update
/// changes nothing and adds no undo step. An update that changes nothing adds no undo step.
#[test]
fn locked_or_unchanged_roll_adds_no_undo_step() {
    let (mut s, dir) = with_roll("credits-lock", json!({}));
    let undo0 = s.history.undo.len();
    let u = s.execute("tts.addCreditRoll", json!({})).unwrap();
    assert_eq!(u["changed"], false, "{u}");
    assert_eq!(s.history.undo.len(), undo0, "nothing changed: no undo step");
    let id = rolls(&s)[0].id;
    s.edit_sequence("lock", |q, _, _| {
        let (tid, _) = q.find_item(id).unwrap();
        q.track_mut(tid).unwrap().locked = true;
        Ok(())
    })
    .unwrap();
    let before = roll_text(&rolls(&s)[0]);
    let undo1 = s.history.undo.len();
    let r = synth_more(&mut s, &dir, "四国めたん", 1.0);
    assert_eq!(r["creditRoll"]["skipped"], "locked", "{r}");
    assert!(r.get("creditRollError").is_none());
    assert_eq!(s.history.undo.len(), undo1 + 1);
    assert_eq!(roll_text(&rolls(&s)[0]), before);
    let undo2 = s.history.undo.len();
    let u = s.execute("tts.addCreditRoll", json!({})).unwrap();
    assert_eq!(u["skipped"], "locked");
    assert_eq!(s.history.undo.len(), undo2);
}

/// The roll is found by its tag after renaming; a copy makes two (the first is updated and the
/// count reported); a roll of an earlier version (name only) is adopted, not duplicated.
#[test]
fn the_roll_is_found_by_its_tag_and_by_its_old_name() {
    let tagged = |s: &Session| {
        s.active_sequence().unwrap().video_tracks.iter().flat_map(|t| &t.items).filter(|i| i.effects.iter().any(|e| e.params.contains_key(ROLL_TAG))).count()
    };
    let (mut s, dir) = with_roll("credits-tag", json!({}));
    let id = rolls(&s)[0].id;
    change_roll(&mut s, |it| it.name = "End credits".into());
    let r = synth_more(&mut s, &dir, "四国めたん", 1.0);
    assert_eq!(r["creditRoll"]["updated"], true, "{r}");
    assert_eq!(r["creditRoll"]["clip"], id.0);
    assert_eq!(tagged(&s), 1);
    // a copy far after the end, on the same track
    s.edit_sequence("copy", |q, ctx, _| {
        let (tid, it) = q.find_item(id).unwrap();
        let mut c = it.clone();
        c.id = filmcraft_project::ClipId(*ctx.next_id);
        *ctx.next_id += 1;
        c.start = filmcraft_time::Tick(it.end().0 + filmcraft_time::Tick::from_seconds_f64(60.0).0);
        let t = q.track_mut(tid).unwrap();
        t.items.push(c);
        t.sort();
        Ok(())
    })
    .unwrap();
    let u = s.execute("tts.addCreditRoll", json!({"title": "Cast"})).unwrap();
    assert_eq!(u["rolls"], 2, "{u}");
    assert_eq!(tagged(&s), 2);

    // an untagged roll named like the old version's is adopted
    let (mut s, dir) = with_roll("credits-old", json!({}));
    change_roll(&mut s, |it| {
        for e in &mut it.effects {
            e.params.remove(ROLL_TAG);
        }
    });
    assert_eq!(tagged(&s), 0);
    let r = synth_more(&mut s, &dir, "四国めたん", 1.0);
    assert_eq!(r["creditRoll"]["updated"], true, "{r}");
    assert_eq!(rolls(&s).len(), 1);
    assert_eq!(tagged(&s), 1);
}

/// The heading follows Settings ▸ Appearance ▸ Language without the UI (CLI / MCP): `en`, `ja`,
/// and `auto` by the system language (Japanese when unknown).
#[test]
fn heading_language_comes_from_the_language_setting() {
    let vv = fake(Mode::Good, false);
    let dir = out_dir("credits-lang");
    for (pref, system, want) in
        [("en", None, "Voices"), ("ja", Some("en-US"), "音声"), ("auto", Some("en-GB"), "Voices"), ("auto", Some("ja-JP"), "音声"), ("auto", None, "音声")]
    {
        let mut s = crate::narration_tests::demo();
        s.prefs.narration.voicevox_url = vv.clone();
        s.prefs.appearance.language = pref.into();
        s.system_language = system.map(str::to_string);
        s.execute("tts.synthesize", json!({"engine": "voicevox", "text": "あい", "voice": "ずんだもん", "dir": dir, "captions": false})).unwrap();
        s.execute("tts.addCreditRoll", json!({})).unwrap();
        assert!(roll_text(&rolls(&s)[0]).starts_with(want), "{pref} {system:?}");
        let r = s.execute("tts.exportCredits", json!({"path": format!("{dir}/lang-{pref}-{}.txt", system.unwrap_or("none"))})).unwrap();
        assert!(r["text"].as_str().unwrap().starts_with(want), "{pref} {system:?}");
    }
}

/// Clearing the `Narration Credit` field removes the credit (it does not fall back to the JSON).
#[test]
fn a_cleared_credit_field_removes_the_credit() {
    let vv = fake(Mode::Good, false);
    let mut s = demo();
    s.prefs.narration.voicevox_url = vv;
    let r = s
        .execute("tts.synthesize", json!({"engine": "voicevox", "text": "あい", "voice": "ずんだもん", "dir": out_dir("credits-clear"), "captions": false}))
        .unwrap();
    let item = r["items"][0].as_u64().unwrap();
    s.execute("metadata.set", json!({"item": item, "field": CREDIT_KEY, "value": ""})).unwrap();
    assert_eq!(s.execute("tts.credits", json!({})).unwrap()["credits"].as_array().unwrap().len(), 0);
}

/// `tts.exportCredits`: a relative path is under the project folder (the narration folder while
/// unsaved), `.txt` is added, the open project and existing non-text files are never
/// overwritten, and `scope: "project"` works with no sequence open.
#[test]
fn export_credits_path_rules() {
    let vv = fake(Mode::Good, false);
    let mut s = demo();
    s.prefs.narration.voicevox_url = vv;
    let dir = out_dir("credits-paths");
    s.execute("tts.synthesize", json!({"engine": "voicevox", "text": "あい", "voice": "ずんだもん", "dir": dir, "captions": false})).unwrap();
    // unsaved: relative to the narration folder
    let r = s.execute("tts.exportCredits", json!({"path": "rel-unsaved"})).unwrap();
    let p = std::path::PathBuf::from(r["path"].as_str().unwrap());
    assert_eq!(p, std::path::Path::new(&crate::narration::narration_dir(&s, &serde_json::Value::Null)).join("rel-unsaved.txt"));
    assert!(p.exists());
    // saved: relative to the project folder
    let proj = std::path::Path::new(&dir).join("Paths.fcproj");
    s.execute("file.saveAs", json!({"path": proj.to_string_lossy()})).unwrap();
    let r = s.execute("tts.exportCredits", json!({"path": "sub/desc"})).unwrap();
    let p = std::path::PathBuf::from(r["path"].as_str().unwrap());
    assert_eq!(p, std::path::Path::new(&dir).join("sub").join("desc.txt"));
    assert!(p.exists());
    // the open project and other existing files are refused, and left as they were
    let before = std::fs::read(&proj).unwrap();
    for bad in [proj.to_string_lossy().into_owned(), "Paths.fcproj".into()] {
        let e = s.execute("tts.exportCredits", json!({"path": bad})).unwrap_err().to_string();
        assert!(e.contains("project") || e.contains("not a .txt"), "{e}");
    }
    assert_eq!(std::fs::read(&proj).unwrap(), before);
    let other = std::path::Path::new(&dir).join("notes.md");
    std::fs::write(&other, "keep").unwrap();
    assert!(s.execute("tts.exportCredits", json!({"path": other.to_string_lossy()})).is_err());
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "keep");
    // an existing .txt is replaced
    assert!(s.execute("tts.exportCredits", json!({"path": "sub/desc.txt"})).is_ok());
    // no sequence open: the project scope still works
    s.state.active_sequence = None;
    assert!(s.execute("tts.exportCredits", json!({"path": "proj-scope", "scope": "project"})).is_ok());
    assert!(s.execute("tts.exportCredits", json!({"path": "seq-scope"})).is_err());
}
