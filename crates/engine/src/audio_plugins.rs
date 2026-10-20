//! Audio plug-in commands (`audioPlugins.*`) and the glue between the plug-in catalogue
//! (`filmcraft_audio_dsp::plugin`, filled by the VST3 host in `filmcraft-platform`) and the effect
//! catalogue (`filmcraft_project::register_plugin_effect`).
//!
//! | command | does |
//! |---|---|
//! | `audioPlugins.scan` | search the plug-in folders, load and describe every plug-in, add the effects to the catalogue |
//! | `audioPlugins.list` | the plug-ins found by the last scan (effect ids, parameters), failures and searched folders |
//!
//! Plug-in effects are then ordinary audio effects: `effects.apply {"effect":"vst3.<uid>"}` (or the
//! plug-in's name), `mixer.addInsert`, `effects.setParam` with `p<id>` parameters (normalised 0…1,
//! keyframable). Applying one creates an instance once, so a plug-in that cannot load is an error
//! right away, and stores the plug-in's initial state in the effect (saved with the project).

use serde_json::{Value, json};

use filmcraft_audio_dsp::plugin::{self, PluginDescriptor, ProcessConfig};
use filmcraft_project::effect::{PluginEffectSpec, PluginParamSpec};
use filmcraft_project::{EffectInstance, Project};

use crate::commands::{CommandSpec, always, bad};
use crate::{Result, Session};

/// The effect-catalogue entry of a scanned plug-in.
fn spec_of(d: &PluginDescriptor) -> PluginEffectSpec {
    PluginEffectSpec {
        effect_id: d.effect_id(),
        format: d.format.id().to_string(),
        uid: d.uid.clone(),
        name: d.name.clone(),
        vendor: d.vendor.clone(),
        folder: d.format.label().to_string(),
        params: d
            .params
            .iter()
            .take(plugin::MAX_PARAMS)
            .map(|p| PluginParamSpec { key: plugin::param_key(p.id), label: p.name.clone(), default: p.default, automatable: p.automatable })
            .collect(),
    }
}

/// Add every plug-in of the catalogue to the effect catalogue.
pub fn register_catalog() -> usize {
    plugin::plugins().iter().filter_map(|d| filmcraft_project::effect::register_plugin_effect(spec_of(d))).count()
}

/// Scan for plug-ins and register them as effects (the desktop app may call this off the UI
/// thread; it is what `audioPlugins.scan` runs).
pub fn scan_and_register() -> plugin::ScanReport {
    let r = plugin::scan();
    register_catalog();
    r
}

fn uses_plugins(p: &Project) -> bool {
    let fx = |v: &[EffectInstance]| v.iter().any(|e| plugin::is_plugin_effect(&e.effect));
    p.sequences()
        .filter_map(|i| i.as_sequence())
        .any(|q| fx(&q.master_effects) || q.all_tracks().chain(q.submix_tracks.iter()).any(|t| fx(&t.effects) || t.items.iter().any(|i| fx(&i.effects))))
}

/// A project was opened: when it uses plug-ins and nothing was scanned yet, scan now so its
/// plug-in effects show their parameters (rendering would scan on first use anyway).
pub(crate) fn on_project_installed(p: &Project) {
    if !plugin::scanned() && !plugin::host_formats().is_empty() && uses_plugins(p) {
        plugin::ensure_scanned();
        register_catalog();
    }
}

/// Make a new effect instance ready: for a plug-in effect, create the plug-in once (so one that
/// cannot load fails now, with its reason) and store its initial state. Other effects are left
/// as they are.
pub(crate) fn prepare_instance(inst: &mut EffectInstance, sample_rate: u32, cmd: &str) -> Result<()> {
    if !plugin::is_plugin_effect(&inst.effect) {
        return Ok(());
    }
    let sr = if sample_rate == 0 { 48000.0 } else { sample_rate as f32 };
    let (_, mut p) = plugin::instantiate(&inst.effect, ProcessConfig { sample_rate: sr, channels: 2, max_block: 1024 }, None).map_err(|e| bad(cmd, e.0))?;
    if let Ok(state) = p.save_state()
        && let Some(r) = inst.plugin.as_mut()
    {
        r.set_state(&state);
    }
    Ok(())
}

fn describe(d: &PluginDescriptor) -> Value {
    json!({
        "effect": d.effect_id(),
        "format": d.format.label(),
        "name": d.name,
        "vendor": d.vendor,
        "version": d.version,
        "categories": d.categories,
        "path": d.path.display().to_string(),
        "params": d.params.iter().take(plugin::MAX_PARAMS).map(|p| json!({
            "id": plugin::param_key(p.id),
            "name": p.name,
            "units": p.units,
            "default": p.default,
            "steps": p.steps,
            "automatable": p.automatable,
        })).collect::<Vec<_>>(),
    })
}

fn report(plugins: &[Value], failures: &[(std::path::PathBuf, String)], folders: &[std::path::PathBuf]) -> Value {
    json!({
        "hosts": plugin::host_formats().iter().map(|f| f.label()).collect::<Vec<_>>(),
        "scanned": plugin::scanned(),
        "plugins": plugins,
        "failures": failures.iter().map(|(p, why)| json!({"path": p.display().to_string(), "error": why})).collect::<Vec<_>>(),
        "folders": folders.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
    })
}

fn scan_cmd(s: &mut Session, _p: &Value) -> Result<Value> {
    if plugin::host_formats().is_empty() {
        return Err(bad("audioPlugins.scan", "audio plug-ins are not supported on this system yet (VST3 hosting is available on Windows)"));
    }
    let r = scan_and_register();
    s.toast(format!(
        "Audio plug-ins: {} found{}",
        r.plugins.len(),
        if r.failures.is_empty() { String::new() } else { format!(", {} could not be loaded", r.failures.len()) }
    ));
    let plugins: Vec<Value> = r.plugins.iter().map(describe).collect();
    Ok(report(&plugins, &r.failures, &r.folders))
}

fn list_cmd(_s: &mut Session, _p: &Value) -> Result<Value> {
    let plugins: Vec<Value> = plugin::plugins().iter().map(|d| describe(d)).collect();
    let (failures, folders) = plugin::last_scan_problems();
    Ok(report(&plugins, &failures, &folders))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "audioPlugins.scan",
            label: "Scan Audio Plug-ins",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: always,
            run: scan_cmd,
            journal: false,
        },
        CommandSpec {
            id: "audioPlugins.list",
            label: "List Audio Plug-ins",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: always,
            run: list_cmd,
            journal: false,
        },
    ]
}
