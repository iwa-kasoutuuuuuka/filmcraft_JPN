//! Audio plug-in hosting interface (VST3 now, Audio Units later): the plug-in formats' common
//! shape, the process-wide catalogue of installed plug-ins, and [`PluginEffect`], which makes a
//! plug-in instance an [`AudioEffect`] like the built-in ones.
//!
//! This crate stays pure Rust and `unsafe`-free: the format hosts (loading plug-in binaries, the
//! VST3 COM interfaces) live in `filmcraft-platform` (docs/adr/0002-audio-plugin-hosting.md),
//! which registers a [`PluginHost`] at startup with [`register_host`]. Without one (the web build,
//! systems without a host yet), the catalogue is empty and plug-in effects in a project pass their
//! audio through unchanged, so a project never fails to play or export because a plug-in is
//! missing.
//!
//! Plug-in effects use the effect id `<format>.<uid>` (`vst3.6E33225254224A00AA69301AF318797D`) and
//! the parameter ids `p<id>` (the plug-in's own parameter id), with normalised values 0…1.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};

use crate::{AudioEffect, ParamSpec, Unit};

/// A plug-in format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PluginFormat {
    /// Steinberg VST 3.
    Vst3,
    /// Apple Audio Units (v2 / v3); no host yet.
    AudioUnit,
}

impl PluginFormat {
    /// The effect-id prefix (`vst3`, `au`).
    pub fn id(self) -> &'static str {
        match self {
            PluginFormat::Vst3 => "vst3",
            PluginFormat::AudioUnit => "au",
        }
    }
    /// The Effects-panel folder name (`VST3`, `Audio Units`).
    pub fn label(self) -> &'static str {
        match self {
            PluginFormat::Vst3 => "VST3",
            PluginFormat::AudioUnit => "Audio Units",
        }
    }
    /// The format of an effect-id prefix.
    pub fn from_id(s: &str) -> Option<PluginFormat> {
        match s {
            "vst3" => Some(PluginFormat::Vst3),
            "au" => Some(PluginFormat::AudioUnit),
            _ => None,
        }
    }
}

/// Longest uid accepted in an effect id (VST3 uids are 32 hex digits).
const MAX_UID_LEN: usize = 64;

/// Split a plug-in effect id (`vst3.<uid>`) into its format and uid; `None` for every other id.
/// The uid must be 1–64 ASCII letters / digits.
pub fn parse_effect_id(id: &str) -> Option<(PluginFormat, &str)> {
    let (fmt, uid) = id.split_once('.')?;
    let fmt = PluginFormat::from_id(fmt)?;
    (!uid.is_empty() && uid.len() <= MAX_UID_LEN && uid.bytes().all(|b| b.is_ascii_alphanumeric())).then_some((fmt, uid))
}

/// Whether `id` names a plug-in effect (`vst3.…`).
pub fn is_plugin_effect(id: &str) -> bool {
    parse_effect_id(id).is_some()
}

/// The parameter id of plug-in parameter `id` (`p<id>`).
pub fn param_key(id: u32) -> String {
    format!("p{id}")
}

/// The plug-in parameter id of a parameter key (`p12` → 12).
pub fn parse_param_key(key: &str) -> Option<u32> {
    key.strip_prefix('p')?.parse().ok()
}

/// One automatable parameter of a plug-in (values are normalised 0…1).
#[derive(Clone, Debug, PartialEq)]
pub struct PluginParamInfo {
    pub id: u32,
    pub name: String,
    /// The plug-in's unit label ("dB", "ms"), for display.
    pub units: String,
    /// Default normalised value.
    pub default: f64,
    /// 0 for continuous parameters, otherwise the number of steps (1 = on / off).
    pub steps: i32,
    /// The plug-in allows automation.
    pub automatable: bool,
}

/// An installed plug-in effect.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginDescriptor {
    pub format: PluginFormat,
    /// Stable class id (VST3: the processor class id as 32 hex digits, the same on every OS).
    pub uid: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    /// The plug-in's own categories (VST3 sub-categories, `Fx|Delay`).
    pub categories: String,
    /// The plug-in file or bundle.
    pub path: PathBuf,
    /// Parameters the host shows (hidden, read-only and bypass parameters are left out).
    pub params: Vec<PluginParamInfo>,
}

impl PluginDescriptor {
    /// The effect id (`vst3.<uid>`).
    pub fn effect_id(&self) -> String {
        format!("{}.{}", self.format.id(), self.uid)
    }
}

/// An error from a plug-in host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginError(pub String);

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PluginError {}

impl From<String> for PluginError {
    fn from(s: String) -> Self {
        PluginError(s)
    }
}

impl From<&str> for PluginError {
    fn from(s: &str) -> Self {
        PluginError(s.to_string())
    }
}

/// What a scan found.
#[derive(Clone, Debug, Default)]
pub struct ScanReport {
    pub plugins: Vec<PluginDescriptor>,
    /// Plug-in files that could not be loaded or described, with the reason.
    pub failures: Vec<(PathBuf, String)>,
    /// The folders that were searched.
    pub folders: Vec<PathBuf>,
}

/// How an instance will be driven.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcessConfig {
    pub sample_rate: f32,
    /// Channels of the audio passed to `process` (2, or 6 for 5.1).
    pub channels: usize,
    /// Largest block passed to the plug-in at once (`process` splits longer blocks).
    pub max_block: usize,
}

/// A running plug-in instance (safe interface; the host keeps every FFI detail inside).
pub trait PluginProcessor: Send {
    /// Process one block in place (planar). Channels the plug-in does not take pass through.
    fn process(&mut self, channels: &mut [&mut [f32]]);
    /// Set a parameter (normalised 0…1), from the next processed sample. `false` for unknown ids.
    fn set_param(&mut self, id: u32, normalized: f64) -> bool;
    /// Current normalised value of a parameter.
    fn param(&self, id: u32) -> Option<f64>;
    /// Clear the plug-in's internal state (tails, envelopes).
    fn reset(&mut self);
    /// Processing latency in samples.
    fn latency(&self) -> usize;
    /// The plug-in's state (a host-specific blob that [`PluginHost::instantiate`] restores).
    fn save_state(&mut self) -> Result<Vec<u8>, PluginError>;
    /// The plug-in's text for a parameter value ("-6.0 dB"), when it provides one.
    fn param_text(&self, _id: u32, _normalized: f64) -> Option<String> {
        None
    }
}

/// A plug-in format host (implemented in `filmcraft-platform`).
pub trait PluginHost: Send + Sync {
    fn format(&self) -> PluginFormat;
    /// Find and describe the installed plug-ins (slow: loads every plug-in).
    fn scan(&self) -> ScanReport;
    /// Create an instance of `desc`, restoring `state` (from [`PluginProcessor::save_state`]).
    fn instantiate(&self, desc: &PluginDescriptor, config: ProcessConfig, state: Option<&[u8]>) -> Result<Box<dyn PluginProcessor>, PluginError>;
}

#[derive(Default)]
struct Catalog {
    scanned: bool,
    entries: Vec<(Arc<dyn PluginHost>, Arc<PluginDescriptor>)>,
    failures: Vec<(PathBuf, String)>,
    folders: Vec<PathBuf>,
}

fn hosts() -> &'static RwLock<Vec<Arc<dyn PluginHost>>> {
    static H: OnceLock<RwLock<Vec<Arc<dyn PluginHost>>>> = OnceLock::new();
    H.get_or_init(Default::default)
}

fn catalog() -> &'static RwLock<Catalog> {
    static C: OnceLock<RwLock<Catalog>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Serialises scans (a lazy scan from a render thread and a user's scan).
fn scan_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(Default::default)
}

/// Register a plug-in host (call once at startup; a second host for the same format is added too,
/// and its plug-ins are merged into the catalogue).
pub fn register_host(host: Arc<dyn PluginHost>) {
    hosts().write().unwrap_or_else(PoisonError::into_inner).push(host);
}

/// The formats that have a host on this system.
pub fn host_formats() -> Vec<PluginFormat> {
    let mut v: Vec<PluginFormat> = hosts().read().unwrap_or_else(PoisonError::into_inner).iter().map(|h| h.format()).collect();
    v.dedup();
    v
}

/// Scan every host and replace the catalogue. A host that panics is reported as a failure (no
/// panic escapes). Plug-ins with the same effect id keep the first one found.
pub fn scan() -> ScanReport {
    let _guard = scan_lock().lock().unwrap_or_else(PoisonError::into_inner);
    scan_locked()
}

fn scan_locked() -> ScanReport {
    let hs: Vec<Arc<dyn PluginHost>> = hosts().read().unwrap_or_else(PoisonError::into_inner).clone();
    let mut report = ScanReport::default();
    let mut entries: Vec<(Arc<dyn PluginHost>, Arc<PluginDescriptor>)> = Vec::new();
    for h in hs {
        let r = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h.scan())) {
            Ok(r) => r,
            Err(_) => {
                report.failures.push((PathBuf::new(), format!("the {} scan failed unexpectedly", h.format().label())));
                continue;
            }
        };
        for d in r.plugins {
            let id = d.effect_id();
            if entries.iter().any(|(_, e)| e.effect_id() == id) {
                continue;
            }
            entries.push((h.clone(), Arc::new(d.clone())));
            report.plugins.push(d);
        }
        report.failures.extend(r.failures);
        report.folders.extend(r.folders);
    }
    let mut c = catalog().write().unwrap_or_else(PoisonError::into_inner);
    *c = Catalog { scanned: true, entries, failures: report.failures.clone(), folders: report.folders.clone() };
    report
}

/// Whether a scan has run in this process.
pub fn scanned() -> bool {
    catalog().read().unwrap_or_else(PoisonError::into_inner).scanned
}

/// Scan once if no scan has run yet (for projects that use plug-ins before the user scanned).
/// Does nothing without a registered host.
pub fn ensure_scanned() {
    if scanned() || hosts().read().unwrap_or_else(PoisonError::into_inner).is_empty() {
        return;
    }
    let _guard = scan_lock().lock().unwrap_or_else(PoisonError::into_inner);
    if !scanned() {
        scan_locked();
    }
}

/// The plug-ins found by the last scan.
pub fn plugins() -> Vec<Arc<PluginDescriptor>> {
    catalog().read().unwrap_or_else(PoisonError::into_inner).entries.iter().map(|(_, d)| d.clone()).collect()
}

/// The last scan's failures and searched folders.
pub fn last_scan_problems() -> (Vec<(PathBuf, String)>, Vec<PathBuf>) {
    let c = catalog().read().unwrap_or_else(PoisonError::into_inner);
    (c.failures.clone(), c.folders.clone())
}

/// The installed plug-in with effect id `effect_id` (scanning first if nothing was scanned yet).
pub fn find(effect_id: &str) -> Option<Arc<PluginDescriptor>> {
    parse_effect_id(effect_id)?;
    ensure_scanned();
    catalog().read().unwrap_or_else(PoisonError::into_inner).entries.iter().find(|(_, d)| d.effect_id() == effect_id).map(|(_, d)| d.clone())
}

/// Create a running instance of the installed plug-in `effect_id`.
pub fn instantiate(effect_id: &str, config: ProcessConfig, state: Option<&[u8]>) -> Result<(Arc<PluginDescriptor>, Box<dyn PluginProcessor>), PluginError> {
    if parse_effect_id(effect_id).is_none() {
        return Err(format!("`{effect_id}` is not a plug-in effect").into());
    }
    ensure_scanned();
    let found = catalog().read().unwrap_or_else(PoisonError::into_inner).entries.iter().find(|(_, d)| d.effect_id() == effect_id).cloned();
    let Some((host, desc)) = found else {
        return Err(format!("the plug-in `{effect_id}` is not installed (scan for plug-ins, or install it)").into());
    };
    let config = ProcessConfig { sample_rate: config.sample_rate, channels: config.channels.clamp(1, 32), max_block: config.max_block.clamp(16, 8192) };
    if !(config.sample_rate.is_finite() && (1000.0..=1_000_000.0).contains(&config.sample_rate)) {
        return Err(format!("unsupported sample rate {}", config.sample_rate).into());
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| host.instantiate(&desc, config, state))) {
        Ok(Ok(p)) => Ok((desc, p)),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(format!("creating `{}` failed unexpectedly", desc.name).into()),
    }
}

/// Most distinct plug-in effect ids given static names in one process (project files are
/// untrusted: an id outside this budget is treated as a missing plug-in).
const MAX_INTERNED: usize = 4096;

/// Most parameters exposed per plug-in (the rest stay at the plug-in's own values).
pub const MAX_PARAMS: usize = 512;

#[derive(Default)]
struct Interned {
    ids: HashMap<String, &'static str>,
    /// effect id → (parameter ids it was built from, specs)
    specs: HashMap<String, (Vec<u32>, &'static [ParamSpec])>,
}

fn interned() -> &'static Mutex<Interned> {
    static I: OnceLock<Mutex<Interned>> = OnceLock::new();
    I.get_or_init(Default::default)
}

/// A `'static` copy of a plug-in effect id (the DSP registry uses `'static` ids). `None` for ids
/// that are not plug-in effect ids, or once [`MAX_INTERNED`] distinct ids were seen.
pub fn intern_effect_id(id: &str) -> Option<&'static str> {
    parse_effect_id(id)?;
    let mut i = interned().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(s) = i.ids.get(id) {
        return Some(s);
    }
    if i.ids.len() >= MAX_INTERNED {
        return None;
    }
    let s: &'static str = Box::leak(id.to_string().into_boxed_str());
    i.ids.insert(id.to_string(), s);
    Some(s)
}

/// The parameter specs of a plug-in (`p<id>`, 0…1), built once per plug-in and parameter set.
fn specs_for(effect_id: &str, params: &[PluginParamInfo]) -> &'static [ParamSpec] {
    let ids: Vec<u32> = params.iter().take(MAX_PARAMS).map(|p| p.id).collect();
    let mut i = interned().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((have, specs)) = i.specs.get(effect_id)
        && *have == ids
    {
        return specs;
    }
    if i.specs.len() >= MAX_INTERNED {
        return &[];
    }
    let specs: Vec<ParamSpec> = params
        .iter()
        .take(MAX_PARAMS)
        .map(|p| {
            let id: &'static str = Box::leak(param_key(p.id).into_boxed_str());
            let name: &'static str = Box::leak(p.name.clone().into_boxed_str());
            let default = if p.default.is_finite() { p.default.clamp(0.0, 1.0) as f32 } else { 0.0 };
            ParamSpec::new(id, name, 0.0, 1.0, default, Unit::None)
        })
        .collect();
    let specs: &'static [ParamSpec] = Box::leak(specs.into_boxed_slice());
    i.specs.insert(effect_id.to_string(), (ids, specs));
    specs
}

/// A plug-in instance as an [`AudioEffect`]. When the plug-in is missing or fails to load it
/// passes audio through unchanged ([`PluginEffect::error`] says why), so a project with a missing
/// plug-in still plays and exports.
pub struct PluginEffect {
    id: &'static str,
    specs: &'static [ParamSpec],
    inner: Option<Box<dyn PluginProcessor>>,
    error: Option<String>,
}

impl PluginEffect {
    /// An instance of plug-in effect `effect_id` restoring `state`; a pass-through stand-in (see
    /// [`PluginEffect::error`]) when it cannot be created.
    pub fn create(effect_id: &str, sample_rate: f32, channels: usize, state: Option<&[u8]>) -> PluginEffect {
        let id = intern_effect_id(effect_id).unwrap_or("vst3.invalid");
        match instantiate(effect_id, ProcessConfig { sample_rate, channels, max_block: 1024 }, state) {
            Ok((desc, p)) => PluginEffect { id, specs: specs_for(effect_id, &desc.params), inner: Some(p), error: None },
            Err(e) => PluginEffect { id, specs: &[], inner: None, error: Some(e.0) },
        }
    }

    /// Wrap a running instance (tests, and hosts that create instances themselves).
    pub fn from_processor(effect_id: &str, params: &[PluginParamInfo], p: Box<dyn PluginProcessor>) -> PluginEffect {
        PluginEffect { id: intern_effect_id(effect_id).unwrap_or("vst3.invalid"), specs: specs_for(effect_id, params), inner: Some(p), error: None }
    }

    /// Why the plug-in is not running (`None` when it is).
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The plug-in's state blob (`None` when the plug-in is not running or refuses).
    pub fn save_state(&mut self) -> Option<Vec<u8>> {
        self.inner.as_mut().and_then(|p| p.save_state().ok())
    }

    /// The plug-in's display text for a parameter value.
    pub fn param_text(&self, key: &str, normalized: f64) -> Option<String> {
        self.inner.as_ref()?.param_text(parse_param_key(key)?, normalized)
    }
}

impl AudioEffect for PluginEffect {
    fn id(&self) -> &'static str {
        self.id
    }
    fn params(&self) -> &'static [ParamSpec] {
        self.specs
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(pid) = parse_param_key(id) else { return false };
        let Some(p) = self.inner.as_mut() else { return false };
        let v = if value.is_finite() { value.clamp(0.0, 1.0) as f64 } else { self.specs.iter().find(|s| s.id == id).map_or(0.0, |s| s.default as f64) };
        p.set_param(pid, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.as_ref()?.param(parse_param_key(id)?).map(|v| v as f32)
    }
    fn reset(&mut self) {
        if let Some(p) = self.inner.as_mut() {
            p.reset();
        }
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        if let Some(p) = self.inner.as_mut() {
            p.process(channels);
        }
    }
    fn latency(&self) -> usize {
        self.inner.as_ref().map_or(0, |p| p.latency()).min(1 << 20)
    }
}

/// A stand-in plug-in host for tests on every platform (no plug-in binaries): one "plug-in", a
/// stereo gain with one parameter.
pub mod fake {
    use super::*;

    /// The fake plug-in's uid.
    pub const UID: &str = "F11C0AF7000000000000000000000001";

    /// A host whose single plug-in multiplies by `2 × gain` (gain 0.5 = unity, the default).
    pub struct FakeHost;

    impl FakeHost {
        pub fn descriptor() -> PluginDescriptor {
            PluginDescriptor {
                format: PluginFormat::Vst3,
                uid: UID.to_string(),
                name: "Fake Gain".into(),
                vendor: "FilmCraft tests".into(),
                version: "1.0".into(),
                categories: "Fx".into(),
                path: PathBuf::from("fake.vst3"),
                params: vec![PluginParamInfo { id: 7, name: "Gain".into(), units: String::new(), default: 0.5, steps: 0, automatable: true }],
            }
        }
    }

    impl PluginHost for FakeHost {
        fn format(&self) -> PluginFormat {
            PluginFormat::Vst3
        }
        fn scan(&self) -> ScanReport {
            ScanReport { plugins: vec![Self::descriptor()], failures: Vec::new(), folders: vec![PathBuf::from("fake")] }
        }
        fn instantiate(&self, _desc: &PluginDescriptor, _config: ProcessConfig, state: Option<&[u8]>) -> Result<Box<dyn PluginProcessor>, PluginError> {
            let gain = match state {
                None | Some([]) => 0.5,
                Some(s) => {
                    let b: [u8; 8] = s.try_into().map_err(|_| PluginError::from("bad fake state"))?;
                    f64::from_le_bytes(b)
                }
            };
            Ok(Box::new(FakeGain { gain }))
        }
    }

    struct FakeGain {
        gain: f64,
    }

    impl PluginProcessor for FakeGain {
        fn process(&mut self, channels: &mut [&mut [f32]]) {
            let g = (2.0 * self.gain) as f32;
            for c in channels.iter_mut().take(2) {
                for v in c.iter_mut() {
                    *v *= g;
                }
            }
        }
        fn set_param(&mut self, id: u32, normalized: f64) -> bool {
            if id != 7 {
                return false;
            }
            self.gain = normalized.clamp(0.0, 1.0);
            true
        }
        fn param(&self, id: u32) -> Option<f64> {
            (id == 7).then_some(self.gain)
        }
        fn reset(&mut self) {}
        fn latency(&self) -> usize {
            0
        }
        fn save_state(&mut self) -> Result<Vec<u8>, PluginError> {
            Ok(self.gain.to_le_bytes().to_vec())
        }
        fn param_text(&self, id: u32, normalized: f64) -> Option<String> {
            (id == 7).then(|| format!("{:.1} dB", 20.0 * (2.0 * normalized).max(1e-9).log10()))
        }
    }

    /// Register the fake host once (tests).
    pub fn install() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| register_host(Arc::new(FakeHost)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_ids_parse_strictly() {
        assert_eq!(parse_effect_id("vst3.ABC123"), Some((PluginFormat::Vst3, "ABC123")));
        assert_eq!(parse_effect_id("au.x1"), Some((PluginFormat::AudioUnit, "x1")));
        for bad in ["vst3.", "vst3", "reverb", "vst2.AB", "vst3.AB/..", "vst3.A B", ""] {
            assert!(parse_effect_id(bad).is_none(), "{bad}");
        }
        assert!(parse_effect_id(&format!("vst3.{}", "A".repeat(65))).is_none());
        assert_eq!(parse_param_key("p12"), Some(12));
        assert_eq!(parse_param_key("p-1"), None);
        assert_eq!(parse_param_key("gain"), None);
        assert_eq!(param_key(4_000_000_000), "p4000000000");
    }

    #[test]
    fn fake_plugin_runs_as_an_audio_effect() {
        fake::install();
        let report = scan();
        assert!(report.plugins.iter().any(|d| d.uid == fake::UID));
        let id = fake::FakeHost::descriptor().effect_id();
        assert!(find(&id).is_some());
        let mut fx = PluginEffect::create(&id, 48000.0, 2, None);
        assert!(fx.error().is_none());
        assert_eq!(fx.id(), id);
        assert_eq!(fx.params().len(), 1);
        assert_eq!(fx.params()[0].id, "p7");
        assert_eq!(fx.param("p7"), Some(0.5));
        let (mut l, mut r) = (vec![0.25f32; 64], vec![-0.25f32; 64]);
        fx.process(&mut [&mut l, &mut r]);
        assert_eq!(l[0], 0.25);
        assert!(fx.set_param("p7", 0.25));
        assert!(!fx.set_param("p8", 0.25));
        assert!(!fx.set_param("gain", 0.25));
        // hostile values clamp / fall back to the default
        assert!(fx.set_param("p7", f32::NAN));
        assert_eq!(fx.param("p7"), Some(0.5));
        assert!(fx.set_param("p7", 9.0));
        assert_eq!(fx.param("p7"), Some(1.0));
        fx.set_param("p7", 0.25);
        fx.process(&mut [&mut l, &mut r]);
        assert_eq!(l[0], 0.125);
        // state round trip
        let state = fx.save_state().unwrap();
        let fx2 = PluginEffect::create(&id, 48000.0, 2, Some(&state));
        assert_eq!(fx2.param("p7"), Some(0.25));
        assert_eq!(fx.param_text("p7", 0.5).as_deref(), Some("0.0 dB"));
    }

    #[test]
    fn missing_plugins_pass_audio_through() {
        fake::install();
        for id in ["vst3.0000000000000000000000000000DEAD", "reverb", "vst3.bad id"] {
            let mut fx = PluginEffect::create(id, 48000.0, 2, None);
            assert!(fx.error().is_some(), "{id}");
            let (mut l, mut r) = (vec![0.5f32; 16], vec![0.5f32; 16]);
            fx.process(&mut [&mut l, &mut r]);
            fx.process(&mut []);
            assert!(l.iter().chain(&r).all(|&v| v == 0.5));
            assert!(!fx.set_param("p7", 1.0));
            assert!(fx.save_state().is_none());
            assert_eq!(fx.latency(), 0);
        }
        // a corrupt state blob is an error, not a crash
        let id = fake::FakeHost::descriptor().effect_id();
        let fx = PluginEffect::create(&id, 48000.0, 2, Some(&[1, 2, 3]));
        assert!(fx.error().is_some());
        // hostile sample rates
        assert!(instantiate(&id, ProcessConfig { sample_rate: f32::NAN, channels: 2, max_block: 0 }, None).is_err());
        assert!(instantiate(&id, ProcessConfig { sample_rate: 0.0, channels: usize::MAX, max_block: usize::MAX }, None).is_err());
    }

    #[test]
    fn interning_is_bounded_to_plugin_ids() {
        assert!(intern_effect_id("reverb").is_none());
        let a = intern_effect_id("vst3.ABCD").unwrap();
        let b = intern_effect_id("vst3.ABCD").unwrap();
        assert!(std::ptr::eq(a, b));
    }

    #[test]
    fn a_panicking_host_is_a_scan_failure() {
        struct Boom;
        impl PluginHost for Boom {
            fn format(&self) -> PluginFormat {
                PluginFormat::AudioUnit
            }
            fn scan(&self) -> ScanReport {
                panic!("boom")
            }
            fn instantiate(&self, _: &PluginDescriptor, _: ProcessConfig, _: Option<&[u8]>) -> Result<Box<dyn PluginProcessor>, PluginError> {
                Err("no".into())
            }
        }
        fake::install();
        register_host(Arc::new(Boom));
        let r = scan();
        assert!(r.failures.iter().any(|(_, why)| why.contains("Audio Units")));
        assert!(r.plugins.iter().any(|d| d.uid == fake::UID), "other hosts still scanned");
    }
}
