//! Tests of [`crate::narration_credits`] against the fake engine of `narration_tests`.

use serde_json::json;

use crate::Session;
use crate::narration_credits::{CREDIT_KEY, ROLL_NAME, VOICE_KEY};
use crate::narration_tests::{Mode, demo, fake, out_dir};

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
    assert_eq!(roll_text(&rl[0]), "Voices\n\nVOICEVOX:ずんだもん\nVOICEVOX:四国めたん");
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
    // a Japanese interface titles it 「音声」; hostile parameters are errors
    s.japanese_text = true;
    let u = s.execute("tts.addCreditRoll", json!({"seconds": 4.0})).unwrap();
    assert_eq!(u["updated"], true);
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
