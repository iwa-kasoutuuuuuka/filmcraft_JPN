//! Audio plug-in effects through engine commands, with the stand-in plug-in host
//! (`filmcraft_audio_dsp::plugin::fake`): scan, list, apply to clips and tracks, parameters by
//! id, rendering, saving and reopening, and hostile parameters.

use super::*;
use filmcraft_audio_dsp::plugin::fake;
use serde_json::json;

fn demo() -> Session {
    fake::install();
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn fake_id() -> String {
    fake::FakeHost::descriptor().effect_id()
}

fn a1_clip(s: &Session) -> u64 {
    s.active_sequence().unwrap().audio_tracks[0].items[0].id.0
}

fn clip_peak(s: &Session, clip: u64) -> f32 {
    let it = s.active_sequence().unwrap().find_item(ClipId(clip)).unwrap().1.clone();
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let a0 = it.start.to_units_floor(48000);
    let sig = filmcraft_render::audio::clip_signal(&it, a0, 4800, 48000, &provider).unwrap();
    sig.iter().flatten().fold(0.0f32, |a, v| a.max(v.abs()))
}

#[test]
fn scan_lists_plugins_as_vst3_effects() {
    let mut s = demo();
    let r = s.execute("audioPlugins.scan", json!({})).unwrap();
    let id = fake_id();
    let p = r["plugins"].as_array().unwrap().iter().find(|p| p["effect"] == id.as_str()).unwrap_or_else(|| panic!("{r}"));
    assert_eq!(p["name"], "Fake Gain");
    assert_eq!(p["params"][0]["id"], "p7");
    assert!(r["hosts"].as_array().unwrap().iter().any(|h| h == "VST3"));
    let l = s.execute("audioPlugins.list", json!({})).unwrap();
    assert!(l["plugins"].as_array().unwrap().iter().any(|p| p["effect"] == id.as_str()));
    let fx = s.execute("effects.list", json!({"folder": "Audio Effects/VST3", "detail": true})).unwrap();
    let e = fx.as_array().unwrap().iter().find(|e| e["id"] == id.as_str()).unwrap_or_else(|| panic!("{fx}"));
    assert_eq!(e["kind"], "Audio");
    assert_eq!(e["folder"], "VST3");
    assert_eq!(e["params"][0], "p7");
}

#[test]
fn plugin_effect_applies_renders_and_survives_save() {
    let mut s = demo();
    s.execute("audioPlugins.scan", json!({})).unwrap();
    let clip = a1_clip(&s);
    let before = clip_peak(&s, clip);
    assert!(before > 1e-3);
    // by name, like any effect
    s.execute("effects.apply", json!({"clips": [clip], "effect": "Fake Gain"})).unwrap();
    let it = s.active_sequence().unwrap().find_item(ClipId(clip)).unwrap().1.clone();
    let idx = it.effects.iter().position(|e| e.effect == fake_id()).unwrap();
    let pr = it.effects[idx].plugin.as_ref().expect("plug-in reference");
    assert_eq!(pr.name, "Fake Gain");
    assert_eq!(pr.state_bytes().unwrap(), 0.5f64.to_le_bytes().to_vec(), "initial state captured at apply time");
    // unity at the default
    assert!((clip_peak(&s, clip) - before).abs() < 1e-4);
    // gain 0.25 → ×0.5
    s.execute("effects.setParam", json!({"clip": clip, "effect": idx, "param": "p7", "value": 0.25})).unwrap();
    let half = clip_peak(&s, clip);
    assert!((half - before * 0.5).abs() < 1e-3, "{half} vs {before}");
    // keyframable like any parameter
    s.execute("effects.toggleAnimation", json!({"clip": clip, "effect": idx, "param": "p7"})).unwrap();
    // save, reopen: the effect, its parameter and its plug-in state come back
    let dir = std::env::temp_dir().join(format!("fc-plugins-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plugins.filmcraft");
    let path = path.to_str().unwrap();
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains("\"plugin\"") && text.contains(&fake_id()), "plug-in saved");
    let mut s2 = demo();
    s2.execute("file.open", json!({"path": path})).unwrap();
    let it2 = s2.active_sequence().unwrap().find_item(ClipId(clip)).unwrap().1.clone();
    assert_eq!(it2.effects[idx].effect, fake_id());
    assert_eq!(it2.effects[idx].plugin, it.effects[idx].plugin);
    assert!((clip_peak(&s2, clip) - half).abs() < 1e-3);
    let _ = std::fs::remove_dir_all(&dir);
    // a track insert
    s.execute("mixer.addInsert", json!({"strip": "A1", "effect": fake_id()})).unwrap();
    let t = &s.active_sequence().unwrap().audio_tracks[0];
    assert!(t.effects.iter().any(|e| e.effect == fake_id() && e.plugin.is_some()));
}

#[test]
fn missing_or_hostile_plugins_never_crash() {
    let mut s = demo();
    let clip = a1_clip(&s);
    // an id that is not installed is an unknown effect
    let e = s.execute("effects.apply", json!({"clips": [clip], "effect": "vst3.00000000000000000000000000000BAD"}));
    assert!(e.is_err());
    for bad in ["vst3.", "vst3.../../x", "vst3.\u{0}", &"vst3.A".repeat(100)] {
        assert!(s.execute("effects.apply", json!({"clips": [clip], "effect": bad})).is_err(), "{bad}");
    }
    // a project whose clip has a plug-in that is not installed (hand-edited file) still renders
    let before = clip_peak(&s, clip);
    s.edit_sequence("test", |q, _, _| {
        for t in q.all_tracks_mut() {
            for i in t.items.iter_mut().filter(|i| i.id.0 == clip) {
                let mut e = filmcraft_project::EffectInstance {
                    effect: "vst3.0000000000000000000000000000DEAD".into(),
                    enabled: true,
                    params: Default::default(),
                    masks: Vec::new(),
                    post_fader: false,
                    essential: false,
                    layer: None,
                    plugin: Some(Box::new(filmcraft_project::PluginRef {
                        format: "vst3".into(),
                        uid: "DEAD".into(),
                        name: "Gone".into(),
                        vendor: String::new(),
                        state: "!!not base64!!".into(),
                    })),
                };
                e.params.insert("p1".into(), filmcraft_project::Param::new(filmcraft_project::ParamValue::Float(f64::NAN)));
                e.params.insert("p99999999999".into(), filmcraft_project::Param::new(filmcraft_project::ParamValue::Float(1e300)));
                i.effects.push(e);
            }
        }
        Ok(())
    })
    .unwrap();
    assert!((clip_peak(&s, clip) - before).abs() < 1e-4, "missing plug-in passes audio through");
    // hostile values on a real plug-in parameter are refused or clamped, never a crash
    s.execute("audioPlugins.scan", json!({})).unwrap();
    s.execute("effects.apply", json!({"clips": [clip], "effect": fake_id()})).unwrap();
    let it = s.active_sequence().unwrap().find_item(ClipId(clip)).unwrap().1.clone();
    let idx = it.effects.iter().position(|e| e.effect == fake_id()).unwrap();
    for v in [json!(1e308), json!(-5), json!("x"), json!(null)] {
        let _ = s.execute("effects.setParam", json!({"clip": clip, "effect": idx, "param": "p7", "value": v}));
        assert!(clip_peak(&s, clip).is_finite());
    }
}
