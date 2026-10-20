//! VST3 hosting against a real plug-in (Windows).
//!
//! The plug-in is `$FILMCRAFT_VST3_TEST_PLUGIN` (a `.vst3` bundle or single-file module with a
//! stereo effect whose first parameter is a linear gain, like the plug-in below) or, by default,
//! the `gain` example of the `vst3` bindings crate (MIT / Apache-2.0), built from the copy in the
//! cargo registry into `target/fixtures/platform/vst3/` — a test fixture only, never shipped.
//! Without either the tests are skipped.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use filmcraft_audio_dsp::AudioEffect;
use filmcraft_audio_dsp::plugin::{self, PluginEffect, PluginHost, ProcessConfig};
use filmcraft_platform::vst3host::{Vst3Host, describe_module};

/// The class id of the vst3-rs `gain` example's processor.
const GAIN_UID: &str = "6E33225254224A00AA69301AF318797D";

fn build_gain_example() -> Result<PathBuf, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let root = filmcraft_testkit::fixtures::workspace_root();
    // this platform only: other platforms' crates may not be downloaded (offline)
    let triple = format!("{}-pc-windows-{}", std::env::consts::ARCH, if cfg!(target_env = "gnu") { "gnu" } else { "msvc" });
    let md = std::process::Command::new(&cargo)
        .args(["metadata", "--format-version", "1", "--offline", "--filter-platform", &triple, "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !md.status.success() {
        return Err(format!("cargo metadata: {}", String::from_utf8_lossy(&md.stderr)));
    }
    let v: serde_json::Value = serde_json::from_slice(&md.stdout).map_err(|e| e.to_string())?;
    let manifest = v["packages"]
        .as_array()
        .and_then(|a| a.iter().find(|p| p["name"] == "vst3"))
        .and_then(|p| p["manifest_path"].as_str())
        .map(PathBuf::from)
        .ok_or("the vst3 crate is not in the dependency graph")?;
    let fixtures = filmcraft_testkit::fixtures::fixtures_dir("platform/vst3");
    let target = fixtures.join("build");
    let out = std::process::Command::new(&cargo)
        .args(["build", "--example", "gain", "--offline", "--locked", "--manifest-path"])
        .arg(&manifest)
        .arg("--target-dir")
        .arg(&target)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .output()
        .map_err(|e| format!("cargo build: {e}"))?;
    if !out.status.success() {
        return Err(format!("building the gain example failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let dll = target.join("debug").join("examples").join("gain.dll");
    // lay it out as a VST 3 bundle, as installers do
    let bundle = fixtures.join("plugins").join("FilmCraftTestGain.vst3");
    let bin_dir = bundle.join("Contents").join(if cfg!(target_arch = "aarch64") { "arm64-win" } else { "x86_64-win" });
    std::fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;
    let bin = bin_dir.join("FilmCraftTestGain.vst3");
    if !bin.is_file() {
        let tmp = filmcraft_testkit::fixtures::temp_path(&bin);
        std::fs::copy(&dll, &tmp).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &bin).or_else(|e| if bin.is_file() { Ok(()) } else { Err(e.to_string()) })?;
    }
    Ok(bundle)
}

/// The test plug-in bundle, or `None` (skip).
fn test_plugin() -> Option<&'static Path> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        if let Some(p) = std::env::var_os("FILMCRAFT_VST3_TEST_PLUGIN").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(p));
        }
        match build_gain_example() {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("skipping the VST3 hosting tests: {e}");
                None
            }
        }
    })
    .as_deref()
}

fn custom_plugin() -> bool {
    std::env::var_os("FILMCRAFT_VST3_TEST_PLUGIN").is_some_and(|v| !v.is_empty())
}

fn sine(n: usize, f: f32) -> Vec<f32> {
    (0..n).map(|i| 0.5 * (i as f32 * f * std::f32::consts::TAU / 48000.0).sin()).collect()
}

const CFG: ProcessConfig = ProcessConfig { sample_rate: 48000.0, channels: 2, max_block: 256 };

#[test]
fn describes_the_plugin() {
    let Some(bundle) = test_plugin() else { return };
    let d = describe_module(bundle).unwrap();
    assert!(!d.is_empty());
    let p = &d[0];
    assert_eq!(p.format, plugin::PluginFormat::Vst3);
    assert_eq!(p.uid.len(), 32);
    assert!(!p.name.is_empty());
    if !custom_plugin() {
        assert_eq!(p.uid, GAIN_UID);
        assert_eq!(p.name, "Gain (vst3-rs example plugin)");
        assert_eq!(p.vendor, "Vendor");
        assert_eq!(p.params.len(), 1);
        assert_eq!((p.params[0].id, p.params[0].name.as_str(), p.params[0].default), (0, "Gain", 1.0));
        assert!(p.params[0].automatable);
    }
}

#[test]
fn processes_with_parameters_and_restores_state() {
    let Some(bundle) = test_plugin() else { return };
    let desc = describe_module(bundle).unwrap().remove(0);
    let host = Vst3Host::standard();
    let mut p = host.instantiate(&desc, CFG, None).unwrap();
    let gain_id = desc.params[0].id;
    let src_l = sine(1000, 440.0);
    let src_r = sine(1000, 660.0);
    // unity at the default (gain 1.0), in blocks larger than max_block
    let (mut l, mut r) = (src_l.clone(), src_r.clone());
    p.set_param(gain_id, 1.0);
    p.process(&mut [&mut l, &mut r]);
    if !custom_plugin() {
        assert_eq!(l, src_l);
        assert_eq!(r, src_r);
    }
    // half gain from the next block
    assert!(p.set_param(gain_id, 0.5));
    assert_eq!(p.param(gain_id), Some(0.5));
    let (mut l, mut r) = (src_l.clone(), src_r.clone());
    p.process(&mut [&mut l, &mut r]);
    if !custom_plugin() {
        for (a, b) in l.iter().zip(&src_l).chain(r.iter().zip(&src_r)) {
            assert!((a - b * 0.5).abs() < 1e-6);
        }
    }
    // unknown and hostile parameters
    assert!(!p.set_param(0xFFFF_FFF0, 0.5));
    assert!(!p.set_param(gain_id, f64::NAN));
    assert_eq!(p.param(gain_id), Some(0.5));
    // zero-length, mono, 5.1 and ragged blocks
    p.process(&mut []);
    p.process(&mut [&mut [], &mut []]);
    p.process(&mut [&mut [0.1f32; 7]]);
    let mut six: Vec<Vec<f32>> = (0..6).map(|_| vec![0.25f32; 300]).collect();
    let mut refs: Vec<&mut [f32]> = six.iter_mut().map(|c| c.as_mut_slice()).collect();
    p.process(&mut refs);
    assert!(six[2..].iter().flatten().all(|&v| v == 0.25), "channels beyond the plug-in's pass through");
    let (mut a, mut b) = (vec![0.5f32; 10], vec![0.5f32; 3]);
    p.process(&mut [&mut a, &mut b]);
    // state: save, restore into a new instance
    let state = p.save_state().unwrap();
    let mut p2 = host.instantiate(&desc, CFG, Some(&state)).unwrap();
    let (mut l2, mut r2) = (src_l.clone(), src_r.clone());
    p2.process(&mut [&mut l2, &mut r2]);
    // damaged state is an error, not a crash
    assert!(host.instantiate(&desc, CFG, Some(b"garbage")).is_err());
    let mut cut = state.clone();
    cut.truncate(state.len().saturating_sub(1));
    assert!(host.instantiate(&desc, CFG, Some(&cut)).is_err());
    // reset keeps the parameters
    p.reset();
    let (mut l, mut r) = (src_l.clone(), src_r.clone());
    p.process(&mut [&mut l, &mut r]);
    if !custom_plugin() {
        assert!((l[10] - src_l[10] * 0.5).abs() < 1e-6, "gain kept after reset");
    }
    assert!(p.latency() < 1 << 20);
    // other sample rates and a mono configuration
    for (sr, ch) in [(44100.0, 2), (96000.0, 2), (48000.0, 1), (48000.0, 6)] {
        let mut q = host.instantiate(&desc, ProcessConfig { sample_rate: sr, channels: ch, max_block: 512 }, None).unwrap();
        let mut bufs: Vec<Vec<f32>> = (0..ch).map(|_| sine(700, 300.0)).collect();
        let mut refs: Vec<&mut [f32]> = bufs.iter_mut().map(|c| c.as_mut_slice()).collect();
        q.process(&mut refs);
        assert!(bufs.iter().flatten().all(|v| v.is_finite()));
    }
}

#[test]
fn many_instances_across_threads() {
    let Some(bundle) = test_plugin() else { return };
    let desc = Arc::new(describe_module(bundle).unwrap().remove(0));
    let workers: Vec<_> = (0..4)
        .map(|k| {
            let desc = desc.clone();
            std::thread::spawn(move || {
                for _ in 0..5 {
                    let mut p = Vst3Host::standard().instantiate(&desc, CFG, None).unwrap();
                    p.set_param(desc.params[0].id, 0.25 * (k as f64 + 1.0) / 4.0);
                    let mut l = sine(512, 100.0);
                    let mut r = l.clone();
                    p.process(&mut [&mut l, &mut r]);
                    assert!(l.iter().all(|v| v.is_finite()));
                }
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
}

#[test]
fn a_scanned_plugin_renders_as_a_project_effect() {
    let Some(bundle) = test_plugin() else { return };
    let folder = bundle.parent().unwrap().to_path_buf();
    plugin::register_host(Arc::new(Vst3Host::with_folders(vec![folder.clone()])));
    let report = plugin::scan();
    assert!(report.folders.contains(&folder), "{:?}", report.folders);
    let d = report.plugins.iter().find(|d| d.path == bundle).unwrap_or_else(|| panic!("{:?} / {:?}", report.plugins, report.failures)).clone();
    let id = d.effect_id();
    // as an AudioEffect (what the render chains hold)
    let mut fx = PluginEffect::create(&id, 48000.0, 2, None);
    assert!(fx.error().is_none(), "{:?}", fx.error());
    let key = plugin::param_key(d.params[0].id);
    assert_eq!(fx.params()[0].id, key);
    assert!(fx.set_param(&key, 0.25));
    let src = sine(2000, 440.0);
    let (mut l, mut r) = (src.clone(), src.clone());
    fx.process(&mut [&mut l, &mut r]);
    if !custom_plugin() {
        assert!((l[100] - src[100] * 0.25).abs() < 1e-6);
    }
    // through the project-effect pipeline: an effect instance with a keyed parameter and state
    let mut inst = filmcraft_project::EffectInstance {
        effect: id.clone(),
        enabled: true,
        params: Default::default(),
        masks: Vec::new(),
        post_fader: false,
        essential: false,
        layer: None,
        plugin: Some(Box::new(filmcraft_project::PluginRef {
            format: "vst3".into(),
            uid: d.uid.clone(),
            name: d.name.clone(),
            vendor: d.vendor.clone(),
            state: String::new(),
        })),
    };
    let state = fx.save_state().unwrap();
    assert!(inst.plugin.as_mut().unwrap().set_state(&state));
    inst.params.insert(key.clone(), filmcraft_project::Param::new(filmcraft_project::ParamValue::Float(0.5)));
    let mut dsp = filmcraft_render::audio_fx::configured(&inst, filmcraft_time::Tick::ZERO, 48000).unwrap();
    let (mut l, mut r) = (src.clone(), src.clone());
    dsp.process(&mut [&mut l, &mut r]);
    if !custom_plugin() {
        assert!((l[100] - src[100] * 0.5).abs() < 1e-6);
    }
    assert!(filmcraft_render::audio_fx::supported(&id));
}
