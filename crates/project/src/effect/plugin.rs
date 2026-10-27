//! Audio plug-in effects (VST3) in the effect catalogue.
//!
//! Built-in effects are a static table; plug-in effects are known only after the engine scans the
//! installed plug-ins. [`register_plugin_effect`] adds one: it then appears in the Effects panel
//! (Audio Effects ▸ VST3), `effects.list`, Effect Controls and `find_effect` like any other audio
//! effect. Its parameters are the plug-in's, keyed `p<id>`, as normalised values 0…1.
//!
//! Effect instances of a plug-in carry a [`PluginRef`]: which plug-in it is (so a project opened
//! where the plug-in is missing still names it) and its saved state. A plug-in that is not
//! installed leaves the instance untouched: it is kept, saved back unchanged and passes audio
//! through.

use std::sync::{OnceLock, PoisonError, RwLock};

use serde::{Deserialize, Serialize};

use super::{EffectDef, EffectKind, ParamDef, ParamKind};
use crate::keyframe::ParamValue;

/// The plug-in behind an effect instance, saved in the project (schema v13).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRef {
    /// `vst3` (later `au`).
    pub format: String,
    /// The plug-in class id (32 hex digits for VST3).
    pub uid: String,
    /// Display name and vendor when the instance was made (shown when the plug-in is missing).
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vendor: String,
    /// The plug-in's state blob (base64), restored before the parameters are applied.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub state: String,
}

/// Largest plug-in state kept in a project (bytes, before base64).
pub const MAX_STATE_BYTES: usize = 64 << 20;

impl PluginRef {
    /// The saved state blob (`None` when there is none, it is not valid base64 or is too large).
    pub fn state_bytes(&self) -> Option<Vec<u8>> {
        if self.state.is_empty() || self.state.len() > MAX_STATE_BYTES / 3 * 4 + 4 {
            return None;
        }
        crate::gtemplate::base64_decode(&self.state)
    }
    /// Store a state blob (an over-large one is not stored: the plug-in then starts from its
    /// defaults plus the instance's parameters). Returns whether it was stored.
    pub fn set_state(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() > MAX_STATE_BYTES {
            return false;
        }
        self.state = crate::gtemplate::base64_encode(bytes);
        true
    }
}

/// One parameter of a plug-in effect.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginParamSpec {
    /// The parameter key (`p<id>`).
    pub key: String,
    pub label: String,
    /// Default normalised value (0…1).
    pub default: f64,
    pub automatable: bool,
}

/// A plug-in effect to add to the catalogue.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginEffectSpec {
    /// `vst3.<uid>`.
    pub effect_id: String,
    pub format: String,
    pub uid: String,
    pub name: String,
    pub vendor: String,
    /// The Effects-panel sub-folder of Audio Effects (`VST3`).
    pub folder: String,
    pub params: Vec<PluginParamSpec>,
}

/// Most plug-in effects in the catalogue (registration past this is refused).
const MAX_PLUGIN_EFFECTS: usize = 4096;

struct Entry {
    spec: PluginEffectSpec,
    def: &'static EffectDef,
}

fn registry() -> &'static RwLock<Vec<Entry>> {
    static R: OnceLock<RwLock<Vec<Entry>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// Add (or update) a plug-in effect in the catalogue and return its definition. Registering the
/// same effect again with the same description returns the existing definition. `None` when the
/// id is not a plug-in effect id or the catalogue is full.
pub fn register_plugin_effect(spec: PluginEffectSpec) -> Option<&'static EffectDef> {
    if !is_plugin_effect_id(&spec.effect_id) {
        return None;
    }
    let mut r = registry().write().unwrap_or_else(PoisonError::into_inner);
    if let Some(e) = r.iter().find(|e| e.spec == spec) {
        return Some(e.def);
    }
    if r.len() >= MAX_PLUGIN_EFFECTS {
        return None;
    }
    let category: &'static [&'static str] = Box::leak(vec!["Audio Effects", leak(&spec.folder)].into_boxed_slice());
    let params = spec
        .params
        .iter()
        .map(|p| {
            let d = if p.default.is_finite() { p.default.clamp(0.0, 1.0) } else { 0.0 };
            ParamDef {
                id: leak(&p.key),
                label: leak(&p.label),
                kind: ParamKind::Float { min: 0.0, max: 1.0, soft_min: 0.0, soft_max: 1.0, unit: "", decimals: 3 },
                default: ParamValue::Float(d),
                animatable: p.automatable,
                group: None,
            }
        })
        .collect();
    let def: &'static EffectDef = Box::leak(Box::new(EffectDef {
        id: leak(&spec.effect_id),
        name: leak(&spec.name),
        kind: EffectKind::Audio,
        category,
        params,
        intrinsic: false,
        accelerated: false,
        float32: true,
        yuv: false,
    }));
    // an updated plug-in (new parameters) replaces its old entry; the old definition stays valid
    r.retain(|e| e.spec.effect_id != spec.effect_id);
    r.push(Entry { spec, def });
    Some(def)
}

/// Whether `id` has the shape of a plug-in effect id (`vst3.<uid>` / `au.<uid>`).
pub fn is_plugin_effect_id(id: &str) -> bool {
    let Some((fmt, uid)) = id.split_once('.') else { return false };
    matches!(fmt, "vst3" | "au") && !uid.is_empty() && uid.len() <= 64 && uid.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The registered plug-in effect `id`.
pub(super) fn find_plugin_effect(id: &str) -> Option<&'static EffectDef> {
    if !is_plugin_effect_id(id) {
        return None;
    }
    registry().read().unwrap_or_else(PoisonError::into_inner).iter().find(|e| e.def.id == id).map(|e| e.def)
}

/// Every registered plug-in effect, in registration order.
pub fn plugin_effect_defs() -> Vec<&'static EffectDef> {
    registry().read().unwrap_or_else(PoisonError::into_inner).iter().map(|e| e.def).collect()
}

/// The [`PluginRef`] a new instance of plug-in effect `id` starts with (no state yet).
pub(super) fn plugin_ref_for(id: &str) -> Option<Box<PluginRef>> {
    if !is_plugin_effect_id(id) {
        return None;
    }
    let r = registry().read().unwrap_or_else(PoisonError::into_inner);
    let e = r.iter().find(|e| e.def.id == id)?;
    Some(Box::new(PluginRef {
        format: e.spec.format.clone(),
        uid: e.spec.uid.clone(),
        name: e.spec.name.clone(),
        vendor: e.spec.vendor.clone(),
        state: String::new(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::{all_effect_defs, find_effect};

    fn spec(id: &str, n: usize) -> PluginEffectSpec {
        PluginEffectSpec {
            effect_id: id.into(),
            format: "vst3".into(),
            uid: id.trim_start_matches("vst3.").into(),
            name: "Test Delay".into(),
            vendor: "Acme".into(),
            folder: "VST3".into(),
            params: (0..n).map(|i| PluginParamSpec { key: format!("p{i}"), label: format!("Param {i}"), default: 0.25, automatable: true }).collect(),
        }
    }

    #[test]
    fn plugin_effects_join_the_catalogue() {
        let d = register_plugin_effect(spec("vst3.00AA", 2)).unwrap();
        assert_eq!(d.kind, EffectKind::Audio);
        assert_eq!(d.category, &["Audio Effects", "VST3"]);
        assert!(std::ptr::eq(find_effect("vst3.00AA").unwrap(), d));
        assert!(all_effect_defs().iter().any(|e| e.id == "vst3.00AA"));
        // same description: same definition; new parameters: a new one
        assert!(std::ptr::eq(register_plugin_effect(spec("vst3.00AA", 2)).unwrap(), d));
        let d2 = register_plugin_effect(spec("vst3.00AA", 3)).unwrap();
        assert_eq!(d2.params.len(), 3);
        assert_eq!(find_effect("vst3.00AA").unwrap().params.len(), 3);
        let inst = d2.instance();
        let pr = inst.plugin.as_ref().unwrap();
        assert_eq!((pr.format.as_str(), pr.uid.as_str(), pr.name.as_str()), ("vst3", "00AA", "Test Delay"));
        assert_eq!(inst.f64_at("p1", filmcraft_time::Tick::ZERO), 0.25);
        // not plug-in ids
        assert!(register_plugin_effect(spec("reverb", 1)).is_none());
        assert!(register_plugin_effect(spec("vst3.../x", 1)).is_none());
        assert!(find_effect("vst3.FFFF").is_none());
    }

    #[test]
    fn plugin_state_round_trips_and_is_optional() {
        let d = register_plugin_effect(spec("vst3.00BB", 1)).unwrap();
        let mut inst = d.instance();
        assert!(inst.plugin.as_mut().unwrap().set_state(&[0, 1, 2]));
        assert_eq!(inst.plugin.as_ref().unwrap().state_bytes().unwrap(), vec![0, 1, 2]);
        let json = serde_json::to_string(&inst).unwrap();
        let back: crate::EffectInstance = serde_json::from_str(&json).unwrap();
        assert_eq!(back, inst);
        // built-in effects carry no plug-in field
        let plain = find_effect("reverb").map(|d| serde_json::to_string(&d.instance()).unwrap());
        if let Some(j) = plain {
            assert!(!j.contains("plugin"));
        }
    }
}
