//! VST3 plug-in host (Windows): loads a plug-in module, reads its factory, creates audio effect
//! instances (`IComponent` + `IAudioProcessor`, with their `IEditController`), runs them on
//! stereo `f32` blocks and saves / restores their state. FFI through the generated `vst3`
//! bindings (MIT / Apache-2.0, from the MIT-licensed VST 3 SDK headers).
//!
//! Containment (docs/adr/0002-audio-plugin-hosting.md):
//!
//! - every `unsafe` block has a `// SAFETY:` comment; the public surface is the safe
//!   [`Vst3Host`] (`filmcraft_audio_dsp::plugin::PluginHost`);
//! - the host objects the plug-in calls back into (stream, parameter changes, host application,
//!   component handler) run their bodies under `catch_unwind`, so no Rust panic unwinds into the
//!   plug-in;
//! - everything a plug-in reports (counts, channel counts, sizes, strings) is treated as hostile:
//!   clamped, length-checked, lossily decoded;
//! - plug-in modules are never unloaded (an instance or a callback the plug-in still holds can
//!   never point into freed code); creation and destruction of instances are serialised.
//!
//! The limit of in-process hosting: a plug-in that crashes (access violation, a C++ exception
//! escaping into the host, stack overflow) takes the process down; Rust cannot catch that. Hosting
//! plug-ins in a separate process is the documented next step.

use std::collections::HashMap;
use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use filmcraft_audio_dsp::plugin::{self as fp, PluginDescriptor, PluginError, PluginFormat, PluginParamInfo, PluginProcessor, ProcessConfig, ScanReport};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper, Interface};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW};
use windows::core::{HSTRING, s};

use crate::audio_plugins::{
    MAX_STATE_PART, c_string, decode_state, encode_state, find_vst3_bundles, is_effect_category, uid_string, vst3_binary, vst3_folders,
};

/// The `PClassInfo::category` of audio processors (`kVstAudioEffectClass`).
const AUDIO_EFFECT_CLASS: &str = "Audio Module Class";
/// Most classes read from one factory.
const MAX_CLASSES: i32 = 1024;
/// Most parameters read from one controller.
const MAX_CONTROLLER_PARAMS: i32 = 8192;
/// Most audio buses / channels per bus set up.
const MAX_BUSES: i32 = 16;
const MAX_BUS_CHANNELS: u32 = 32;

type TResult = tresult;

fn ok(r: TResult) -> bool {
    r == kResultOk
}

/// Run a callback body without letting a panic unwind into the plug-in.
fn guard(f: impl FnOnce() -> TResult) -> TResult {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(kInternalError)
}

/// Serialises instance creation / destruction across threads (factories and `initialize` are not
/// required to be thread-safe).
fn lifecycle_lock() -> std::sync::MutexGuard<'static, ()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(Default::default).lock().unwrap_or_else(PoisonError::into_inner)
}

fn tuid_bytes(t: &TUID) -> [u8; 16] {
    let mut b = [0u8; 16];
    for (d, s) in b.iter_mut().zip(t.iter()) {
        *d = *s as u8;
    }
    b
}

fn chars(buf: &[c_char]) -> String {
    let b: Vec<u8> = buf.iter().map(|&c| c as u8).collect();
    c_string(&b)
}

fn wide(buf: &[TChar]) -> String {
    // TChar is UTF-16 (char16)
    crate::audio_plugins::wide_string(buf)
}

// ------------------------------------------------------------------------------------------------
// Modules
// ------------------------------------------------------------------------------------------------

/// A loaded plug-in module (never unloaded) and its factory.
struct Module {
    factory: ComPtr<IPluginFactory>,
    vendor: String,
}

fn modules() -> &'static Mutex<HashMap<PathBuf, Arc<Module>>> {
    static M: OnceLock<Mutex<HashMap<PathBuf, Arc<Module>>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

type GetFactoryProc = unsafe extern "system" fn() -> *mut IPluginFactory;
type InitDllProc = unsafe extern "system" fn() -> u8;

fn load_module(binary: &Path) -> Result<Arc<Module>, String> {
    let _l = lifecycle_lock();
    if let Some(m) = modules().lock().unwrap_or_else(PoisonError::into_inner).get(binary) {
        return Ok(m.clone());
    }
    let wpath = HSTRING::from(binary.as_os_str());
    // SAFETY: `wpath` is a valid NUL-terminated UTF-16 path. Loading runs the module's DllMain:
    // that is the trust boundary of in-process hosting (ADR 0002). The altered search path lets
    // the module find DLLs next to it. The handle is intentionally never freed.
    let h = unsafe { LoadLibraryExW(&wpath, None, LOAD_WITH_ALTERED_SEARCH_PATH) }.map_err(|e| format!("cannot load the module: {e}"))?;
    // SAFETY: `h` is the module just loaded; the name is a NUL-terminated literal.
    if let Some(init) = unsafe { GetProcAddress(h, s!("InitDll")) } {
        // SAFETY: the VST3 module entry point `bool InitDll()` (stdcall/system ABI, no arguments);
        // a C++ `bool` is one byte, read as `u8` so any value is valid.
        let init: InitDllProc = unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, InitDllProc>(init) };
        // SAFETY: called once, right after loading, as the VST3 module specification requires.
        if unsafe { init() } == 0 {
            return Err("the module's InitDll failed".into());
        }
    }
    // SAFETY: as above.
    let gpf = unsafe { GetProcAddress(h, s!("GetPluginFactory")) }.ok_or("not a VST3 module (no GetPluginFactory)")?;
    // SAFETY: `IPluginFactory* GetPluginFactory()` is the documented VST3 module export.
    let gpf: GetFactoryProc = unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, GetFactoryProc>(gpf) };
    // SAFETY: calling the export; it returns an owned (AddRef'd) factory pointer or null.
    let raw = unsafe { gpf() };
    // SAFETY: a non-null result is a valid IPluginFactory whose reference we now own.
    let factory = unsafe { ComPtr::from_raw(raw) }.ok_or("the module returned no plug-in factory")?;
    let mut fi = PFactoryInfo { vendor: [0; 64], url: [0; 256], email: [0; 128], flags: 0 };
    // SAFETY: `fi` is a valid, writable PFactoryInfo for the call's duration.
    let vendor = if ok(unsafe { factory.getFactoryInfo(&mut fi) }) { chars(&fi.vendor) } else { String::new() };
    let m = Arc::new(Module { factory, vendor });
    modules().lock().unwrap_or_else(PoisonError::into_inner).insert(binary.to_path_buf(), m.clone());
    Ok(m)
}

/// One audio effect class of a factory.
struct ClassEntry {
    cid: TUID,
    name: String,
    vendor: String,
    version: String,
    sub_categories: String,
}

fn effect_classes(m: &Module) -> Vec<ClassEntry> {
    // SAFETY: plain factory query on a valid factory.
    let n = unsafe { m.factory.countClasses() }.clamp(0, MAX_CLASSES);
    let f2: Option<ComPtr<IPluginFactory2>> = m.factory.cast();
    let mut out = Vec::new();
    for i in 0..n {
        let mut info = PClassInfo { cid: [0; 16], cardinality: 0, category: [0; 32], name: [0; 64] };
        // SAFETY: `info` is a valid, writable PClassInfo; `i` is below the class count.
        if !ok(unsafe { m.factory.getClassInfo(i, &mut info) }) || chars(&info.category) != AUDIO_EFFECT_CLASS {
            continue;
        }
        let mut e = ClassEntry { cid: info.cid, name: chars(&info.name), vendor: m.vendor.clone(), version: String::new(), sub_categories: String::new() };
        if let Some(f2) = &f2 {
            let mut i2 = PClassInfo2 {
                cid: [0; 16],
                cardinality: 0,
                category: [0; 32],
                name: [0; 64],
                classFlags: 0,
                subCategories: [0; 128],
                vendor: [0; 64],
                version: [0; 64],
                sdkVersion: [0; 64],
            };
            // SAFETY: as above, for the extended class info.
            if ok(unsafe { f2.getClassInfo2(i, &mut i2) }) {
                e.sub_categories = chars(&i2.subCategories);
                e.version = chars(&i2.version);
                let v = chars(&i2.vendor);
                if !v.is_empty() {
                    e.vendor = v;
                }
            }
        }
        if is_effect_category(&e.sub_categories) {
            out.push(e);
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------
// Host-side COM objects
// ------------------------------------------------------------------------------------------------

/// `IBStream` over memory (plug-in state).
struct MemStream {
    inner: Mutex<(Vec<u8>, usize)>,
}

impl Class for MemStream {
    type Interfaces = (IBStream,);
}

impl MemStream {
    fn new(data: Vec<u8>) -> ComWrapper<MemStream> {
        ComWrapper::new(MemStream { inner: Mutex::new((data, 0)) })
    }
    fn bytes(&self) -> Vec<u8> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner).0.clone()
    }
}

impl IBStreamTrait for MemStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: int32, num_read: *mut int32) -> tresult {
        guard(|| {
            if num_bytes < 0 || (buffer.is_null() && num_bytes > 0) {
                return kInvalidArgument;
            }
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            let (data, pos) = &mut *g;
            let avail = data.len().saturating_sub(*pos);
            let n = (num_bytes as usize).min(avail);
            if n > 0 {
                let Some(src) = data.get(*pos..*pos + n) else { return kInternalError };
                // SAFETY: the plug-in gave a writable buffer of `num_bytes` ≥ `n` bytes; `src` is
                // `n` bytes of our own vector; the regions cannot overlap.
                unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), buffer as *mut u8, n) };
            }
            *pos += n;
            if !num_read.is_null() {
                // SAFETY: non-null out-pointer from the plug-in.
                unsafe { *num_read = n as int32 };
            }
            kResultOk
        })
    }

    unsafe fn write(&self, buffer: *mut c_void, num_bytes: int32, num_written: *mut int32) -> tresult {
        guard(|| {
            if num_bytes < 0 || (buffer.is_null() && num_bytes > 0) {
                return kInvalidArgument;
            }
            let n = num_bytes as usize;
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            let (data, pos) = &mut *g;
            let Some(end) = pos.checked_add(n).filter(|&e| e <= MAX_STATE_PART) else { return kResultFalse };
            if data.len() < end {
                data.resize(end, 0);
            }
            if n > 0 {
                // SAFETY: the plug-in gave a readable buffer of `n` bytes.
                let src = unsafe { std::slice::from_raw_parts(buffer as *const u8, n) };
                let Some(dst) = data.get_mut(*pos..end) else { return kInternalError };
                dst.copy_from_slice(src);
            }
            *pos = end;
            if !num_written.is_null() {
                // SAFETY: non-null out-pointer from the plug-in.
                unsafe { *num_written = n as int32 };
            }
            kResultOk
        })
    }

    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        guard(|| {
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            let (data, cur) = &mut *g;
            let base: i64 = match mode as u32 {
                m if m == IBStream_::IStreamSeekMode_::kIBSeekSet as u32 => 0,
                m if m == IBStream_::IStreamSeekMode_::kIBSeekCur as u32 => *cur as i64,
                m if m == IBStream_::IStreamSeekMode_::kIBSeekEnd as u32 => data.len() as i64,
                _ => return kInvalidArgument,
            };
            let Some(np) = base.checked_add(pos).filter(|&p| (0..=MAX_STATE_PART as i64).contains(&p)) else { return kInvalidArgument };
            *cur = np as usize;
            if !result.is_null() {
                // SAFETY: non-null out-pointer from the plug-in.
                unsafe { *result = np };
            }
            kResultOk
        })
    }

    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        guard(|| {
            if pos.is_null() {
                return kInvalidArgument;
            }
            let cur = self.inner.lock().unwrap_or_else(PoisonError::into_inner).1;
            // SAFETY: non-null out-pointer from the plug-in.
            unsafe { *pos = cur as int64 };
            kResultOk
        })
    }
}

/// One parameter change for a block (`IParamValueQueue` with a single point at offset 0).
struct ParamQueue {
    id: AtomicU32,
    value: AtomicU64,
}

impl Class for ParamQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.load(Ordering::Relaxed)
    }
    unsafe fn getPointCount(&self) -> int32 {
        1
    }
    unsafe fn getPoint(&self, index: int32, sample_offset: *mut int32, value: *mut ParamValue) -> tresult {
        guard(|| {
            if index != 0 || sample_offset.is_null() || value.is_null() {
                return kInvalidArgument;
            }
            // SAFETY: non-null out-pointers from the plug-in.
            unsafe {
                *sample_offset = 0;
                *value = f64::from_bits(self.value.load(Ordering::Relaxed));
            }
            kResultOk
        })
    }
    unsafe fn addPoint(&self, _sample_offset: int32, _value: ParamValue, _index: *mut int32) -> tresult {
        kResultFalse
    }
}

/// The block's parameter changes (`IParameterChanges`): the first `count` queues.
struct ParamChanges {
    queues: Vec<ComWrapper<ParamQueue>>,
    count: AtomicI32,
}

impl Class for ParamChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        self.count.load(Ordering::Relaxed)
    }
    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        let n = self.count.load(Ordering::Relaxed);
        if index < 0 || index >= n {
            return std::ptr::null_mut();
        }
        // a borrowed pointer (the host keeps the queue alive for the block), as the API specifies
        self.queues.get(index as usize).and_then(|q| q.as_com_ref::<IParamValueQueue>()).map_or(std::ptr::null_mut(), |r| r.as_ptr())
    }
    unsafe fn addParameterData(&self, _id: *const ParamID, _index: *mut int32) -> *mut IParamValueQueue {
        std::ptr::null_mut()
    }
}

/// The host context passed to `initialize` (`IHostApplication`).
struct HostApp;

impl Class for HostApp {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApp {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        guard(|| {
            if name.is_null() {
                return kInvalidArgument;
            }
            // SAFETY: non-null pointer to the plug-in's String128.
            let out = unsafe { &mut *name };
            out.fill(0);
            for (d, c) in out.iter_mut().zip("FilmCraft".encode_utf16()) {
                *d = c;
            }
            kResultOk
        })
    }
    unsafe fn createInstance(&self, _cid: *mut TUID, _iid: *mut TUID, obj: *mut *mut c_void) -> tresult {
        guard(|| {
            if !obj.is_null() {
                // SAFETY: non-null out-pointer from the plug-in.
                unsafe { *obj = std::ptr::null_mut() };
            }
            kResultFalse
        })
    }
}

/// Edits the controller reports (from its own GUI, not shown yet): acknowledged.
struct Handler;

impl Class for Handler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for Handler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn performEdit(&self, _id: ParamID, _value: ParamValue) -> tresult {
        kResultOk
    }
    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn restartComponent(&self, _flags: int32) -> tresult {
        kResultOk
    }
}

// ------------------------------------------------------------------------------------------------
// Instances
// ------------------------------------------------------------------------------------------------

/// A created and initialised plug-in (component, processor, controller), torn down on drop.
struct Instance {
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    /// The controller is its own object (initialised and terminated separately).
    separate: bool,
    connection: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    active: bool,
    processing: bool,
    _host: ComWrapper<HostApp>,
    _handler: ComWrapper<Handler>,
}

impl Instance {
    fn new(m: &Module, cid: &TUID) -> Result<Instance, String> {
        let _l = lifecycle_lock();
        let host = ComWrapper::new(HostApp);
        let handler = ComWrapper::new(Handler);
        let ctx = host.as_com_ref::<IHostApplication>().map(|r| r.as_ptr() as *mut FUnknown).ok_or("host context")?;
        let mut obj: *mut c_void = std::ptr::null_mut();
        // SAFETY: `cid` is a 16-byte class id, the iid a 16-byte interface id, `obj` a valid
        // out-pointer; on success it receives an owned IComponent reference.
        let r = unsafe { m.factory.createInstance(cid.as_ptr(), IComponent::IID.as_ptr() as *const c_char, &mut obj) };
        if !ok(r) {
            return Err("the factory could not create the plug-in".into());
        }
        // SAFETY: on success `obj` is a valid IComponent pointer whose reference we own.
        let component = unsafe { ComPtr::from_raw(obj as *mut IComponent) }.ok_or("the factory returned no component")?;
        // SAFETY: `ctx` points to our live IHostApplication (kept alive by `host`).
        if !ok(unsafe { component.initialize(ctx) }) {
            return Err("the plug-in failed to initialise".into());
        }
        let mut inst = Instance {
            processor: match component.cast::<IAudioProcessor>() {
                Some(p) => p,
                None => {
                    // SAFETY: balance the successful initialize above.
                    unsafe { component.terminate() };
                    return Err("not an audio processor".into());
                }
            },
            component,
            controller: None,
            separate: false,
            connection: None,
            active: false,
            processing: false,
            _host: host,
            _handler: handler,
        };
        // the controller: the component itself (single-component plug-ins) or its own class
        if let Some(c) = inst.component.cast::<IEditController>() {
            inst.controller = Some(c);
        } else {
            let mut ccid: TUID = [0; 16];
            // SAFETY: `ccid` is a writable 16-byte class id.
            if ok(unsafe { inst.component.getControllerClassId(&mut ccid) }) && ccid != [0; 16] {
                let mut obj: *mut c_void = std::ptr::null_mut();
                // SAFETY: as for the component.
                let r = unsafe { m.factory.createInstance(ccid.as_ptr(), IEditController::IID.as_ptr() as *const c_char, &mut obj) };
                // SAFETY: on success `obj` is an owned IEditController reference.
                if let Some(c) = ok(r).then(|| unsafe { ComPtr::from_raw(obj as *mut IEditController) }).flatten() {
                    // SAFETY: `ctx` is our live host context.
                    if ok(unsafe { c.initialize(ctx) }) {
                        inst.controller = Some(c);
                        inst.separate = true;
                    }
                }
            }
        }
        if inst.separate {
            let a = inst.component.cast::<IConnectionPoint>();
            let b = inst.controller.as_ref().and_then(|c| c.cast::<IConnectionPoint>());
            if let (Some(a), Some(b)) = (a, b) {
                // SAFETY: both are live connection points of this instance; they are
                // disconnected before either is released (Drop).
                unsafe {
                    a.connect(b.as_ptr());
                    b.connect(a.as_ptr());
                }
                inst.connection = Some((a, b));
            }
        }
        if let (Some(c), Some(h)) = (&inst.controller, inst._handler.as_com_ref::<IComponentHandler>()) {
            // SAFETY: `h` is our live component handler (kept alive by the instance).
            unsafe { c.setComponentHandler(h.as_ptr()) };
        }
        Ok(inst)
    }

    /// The controller's parameters (hidden, read-only, bypass and program-change ones left out).
    fn params(&self) -> Vec<PluginParamInfo> {
        let Some(c) = &self.controller else { return Vec::new() };
        // SAFETY: plain controller query.
        let n = unsafe { c.getParameterCount() }.clamp(0, MAX_CONTROLLER_PARAMS);
        let mut out = Vec::new();
        for i in 0..n {
            let mut pi =
                ParameterInfo { id: 0, title: [0; 128], shortTitle: [0; 128], units: [0; 128], stepCount: 0, defaultNormalizedValue: 0.0, unitId: 0, flags: 0 };
            // SAFETY: `pi` is a valid, writable ParameterInfo; `i` is below the count.
            if !ok(unsafe { c.getParameterInfo(i, &mut pi) }) {
                continue;
            }
            use ParameterInfo_::ParameterFlags_::*;
            if pi.flags & (kIsHidden | kIsReadOnly | kIsBypass | kIsProgramChange) != 0 {
                continue;
            }
            let mut name = wide(&pi.title);
            if name.is_empty() {
                name = wide(&pi.shortTitle);
            }
            if name.is_empty() {
                name = format!("Parameter {}", pi.id);
            }
            let d = pi.defaultNormalizedValue;
            out.push(PluginParamInfo {
                id: pi.id,
                name,
                units: wide(&pi.units),
                default: if d.is_finite() { d.clamp(0.0, 1.0) } else { 0.0 },
                steps: pi.stepCount.max(0),
                automatable: pi.flags & kCanAutomate != 0,
            });
            if out.len() >= fp::MAX_PARAMS {
                break;
            }
        }
        out
    }

    /// The component's state, and the controller's.
    fn save_state(&self) -> Result<Vec<u8>, String> {
        let s = MemStream::new(Vec::new());
        let p = s.as_com_ref::<IBStream>().ok_or("stream")?.as_ptr();
        // SAFETY: `p` is our live stream for the call's duration.
        let r = unsafe { self.component.getState(p) };
        if !ok(r) && r != kNotImplemented {
            return Err("the plug-in did not save its state".into());
        }
        let comp = s.bytes();
        let mut ctrl = Vec::new();
        if let (true, Some(c)) = (self.separate, &self.controller) {
            let s2 = MemStream::new(Vec::new());
            let p2 = s2.as_com_ref::<IBStream>().ok_or("stream")?.as_ptr();
            // SAFETY: as above.
            if ok(unsafe { c.getState(p2) }) {
                ctrl = s2.bytes();
            }
        }
        Ok(encode_state(&comp, &ctrl))
    }

    /// Restore a saved state (`None`: sync the controller with the component's defaults).
    fn load_state(&self, blob: Option<&[u8]>) -> Result<(), String> {
        let (comp, ctrl) = match blob {
            Some(b) if !b.is_empty() => {
                let (c, k) = decode_state(b).ok_or("the saved plug-in state is damaged")?;
                let s = MemStream::new(c.to_vec());
                let p = s.as_com_ref::<IBStream>().ok_or("stream")?.as_ptr();
                // SAFETY: `p` is our live stream.
                let r = unsafe { self.component.setState(p) };
                if !ok(r) && r != kNotImplemented && !c.is_empty() {
                    return Err("the plug-in refused its saved state".into());
                }
                (c.to_vec(), k.to_vec())
            }
            _ => {
                let s = MemStream::new(Vec::new());
                let p = s.as_com_ref::<IBStream>().ok_or("stream")?.as_ptr();
                // SAFETY: `p` is our live stream.
                unsafe { self.component.getState(p) };
                (s.bytes(), Vec::new())
            }
        };
        if let (true, Some(c)) = (self.separate, &self.controller) {
            let s = MemStream::new(comp);
            let p = s.as_com_ref::<IBStream>().ok_or("stream")?.as_ptr();
            // SAFETY: `p` is our live stream.
            unsafe { c.setComponentState(p) };
            if !ctrl.is_empty() {
                let s2 = MemStream::new(ctrl);
                let p2 = s2.as_com_ref::<IBStream>().ok_or("stream")?.as_ptr();
                // SAFETY: as above.
                unsafe { c.setState(p2) };
            }
        }
        Ok(())
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _l = lifecycle_lock();
        // SAFETY: tear down in the order the VST3 workflow requires, on objects this instance
        // owns; each call is balanced with the setup call that succeeded.
        unsafe {
            if self.processing {
                self.processor.setProcessing(0);
            }
            if self.active {
                self.component.setActive(0);
            }
            if let Some((a, b)) = &self.connection {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
            }
            if let Some(c) = &self.controller {
                c.setComponentHandler(std::ptr::null_mut());
                if self.separate {
                    c.terminate();
                }
            }
            self.component.terminate();
        }
    }
}

fn channel_count(arr: SpeakerArrangement) -> u32 {
    arr.count_ones().min(MAX_BUS_CHANNELS)
}

/// Audio buffers of one bus direction, with the pointer arrays the plug-in reads.
struct Buses {
    /// per bus, per channel
    bufs: Vec<Vec<Vec<f32>>>,
    /// keeps the channel-pointer arrays `abb` points into alive
    _ptrs: Vec<Vec<*mut f32>>,
    abb: Vec<AudioBusBuffers>,
}

impl Buses {
    fn new(channels: &[u32], max_block: usize) -> Buses {
        let mut bufs: Vec<Vec<Vec<f32>>> = channels.iter().map(|&c| (0..c).map(|_| vec![0.0f32; max_block]).collect()).collect();
        let mut ptrs: Vec<Vec<*mut f32>> = bufs.iter_mut().map(|b| b.iter_mut().map(|c| c.as_mut_ptr()).collect()).collect();
        let abb = ptrs
            .iter_mut()
            .map(|p| AudioBusBuffers { numChannels: p.len() as int32, silenceFlags: 0, __field0: AudioBusBuffers__type0 { channelBuffers32: p.as_mut_ptr() } })
            .collect();
        Buses { bufs, _ptrs: ptrs, abb }
    }
}

/// A plug-in instance set up for processing.
struct Vst3Processor {
    inst: Instance,
    ins: Buses,
    outs: Buses,
    max_block: usize,
    sample_rate: f64,
    params: Vec<PluginParamInfo>,
    values: HashMap<u32, f64>,
    pending: Vec<(u32, f64)>,
    changes: ComWrapper<ParamChanges>,
    context: ProcessContext,
    latency: usize,
    failed_blocks: u64,
}

// SAFETY: the raw pointers in the bus arrays point into heap buffers this value owns (never
// resized after construction), and the COM objects are only used through `&mut self` (one thread
// at a time). Moving the value to another thread moves that exclusive ownership with it.
unsafe impl Send for Vst3Processor {}

impl Vst3Processor {
    fn new(inst: Instance, config: ProcessConfig) -> Result<Vst3Processor, String> {
        let mut inst = inst;
        let max_block = config.max_block.clamp(16, 8192);
        let audio = MediaTypes_::kAudio as MediaType;
        let (din, dout) = (BusDirections_::kInput as BusDirection, BusDirections_::kOutput as BusDirection);
        // SAFETY: plain component queries.
        let (n_in, n_out) =
            unsafe { (inst.component.getBusCount(audio, din).clamp(0, MAX_BUSES), inst.component.getBusCount(audio, dout).clamp(0, MAX_BUSES)) };
        if n_in < 1 || n_out < 1 {
            return Err("not an audio effect (it has no audio input or output)".into());
        }
        let arrangement = |dir: BusDirection, i: int32| -> SpeakerArrangement {
            let mut a: SpeakerArrangement = 0;
            // SAFETY: `a` is a writable arrangement; `i` is below the bus count.
            unsafe { inst.processor.getBusArrangement(dir, i, &mut a) };
            a
        };
        let want = if config.channels >= 2 { SpeakerArr::kStereo } else { SpeakerArr::kMono };
        let mut ins: Vec<SpeakerArrangement> = (0..n_in).map(|i| arrangement(din, i)).collect();
        let mut outs: Vec<SpeakerArrangement> = (0..n_out).map(|i| arrangement(dout, i)).collect();
        if let (Some(a), Some(b)) = (ins.first_mut(), outs.first_mut()) {
            *a = want;
            *b = want;
        }
        // SAFETY: the arrays hold `n_in` / `n_out` arrangements and outlive the call. A plug-in
        // that refuses keeps its own arrangement, read back below.
        unsafe { inst.processor.setBusArrangements(ins.as_mut_ptr(), n_in, outs.as_mut_ptr(), n_out) };
        let in_ch: Vec<u32> = (0..n_in).map(|i| channel_count(arrangement(din, i))).collect();
        let out_ch: Vec<u32> = (0..n_out).map(|i| channel_count(arrangement(dout, i))).collect();
        if in_ch.first().copied().unwrap_or(0) == 0 || out_ch.first().copied().unwrap_or(0) == 0 {
            return Err("the plug-in has no usable main audio bus".into());
        }
        for (dir, n) in [(din, n_in), (dout, n_out)] {
            for i in 0..n {
                // SAFETY: activate the main buses, deactivate side-chains (`i` below the count).
                unsafe { inst.component.activateBus(audio, dir, i, (i == 0) as TBool) };
            }
        }
        let sample32 = SymbolicSampleSizes_::kSample32 as int32;
        // SAFETY: plain processor query.
        if !ok(unsafe { inst.processor.canProcessSampleSize(sample32) }) {
            return Err("the plug-in does not process 32-bit float audio".into());
        }
        let mut setup = ProcessSetup {
            processMode: ProcessModes_::kRealtime as int32,
            symbolicSampleSize: sample32,
            maxSamplesPerBlock: max_block as int32,
            sampleRate: config.sample_rate as f64,
        };
        // SAFETY: `setup` is a valid ProcessSetup for the call.
        if !ok(unsafe { inst.processor.setupProcessing(&mut setup) }) {
            return Err(format!("the plug-in does not run at {} Hz", config.sample_rate));
        }
        // SAFETY: activation after setup, as the VST3 workflow requires.
        if !ok(unsafe { inst.component.setActive(1) }) {
            return Err("the plug-in could not be activated".into());
        }
        inst.active = true;
        // SAFETY: start processing (plug-ins may answer kNotImplemented; that is fine).
        unsafe { inst.processor.setProcessing(1) };
        inst.processing = true;
        // SAFETY: plain processor query.
        let latency = (unsafe { inst.processor.getLatencySamples() } as usize).min(1 << 20);
        let params = inst.params();
        let mut values = HashMap::new();
        if let Some(c) = &inst.controller {
            for p in &params {
                // SAFETY: plain controller query for a listed parameter.
                let v = unsafe { c.getParamNormalized(p.id) };
                values.insert(p.id, if v.is_finite() { v.clamp(0.0, 1.0) } else { p.default });
            }
        }
        let queues = (0..params.len().max(1)).map(|_| ComWrapper::new(ParamQueue { id: AtomicU32::new(0), value: AtomicU64::new(0) })).collect();
        let context = ProcessContext {
            state: (ProcessContext_::StatesAndFlags_::kTempoValid
                | ProcessContext_::StatesAndFlags_::kTimeSigValid
                | ProcessContext_::StatesAndFlags_::kPlaying) as uint32,
            sampleRate: config.sample_rate as f64,
            projectTimeSamples: 0,
            systemTime: 0,
            continousTimeSamples: 0,
            projectTimeMusic: 0.0,
            barPositionMusic: 0.0,
            cycleStartMusic: 0.0,
            cycleEndMusic: 0.0,
            tempo: 120.0,
            timeSigNumerator: 4,
            timeSigDenominator: 4,
            chord: Chord { keyNote: 0, rootNote: 0, chordMask: 0 },
            smpteOffsetSubframes: 0,
            frameRate: FrameRate { framesPerSecond: 0, flags: 0 },
            samplesToNextClock: 0,
        };
        Ok(Vst3Processor {
            inst,
            ins: Buses::new(&in_ch, max_block),
            outs: Buses::new(&out_ch, max_block),
            max_block,
            sample_rate: config.sample_rate as f64,
            params,
            values,
            pending: Vec::new(),
            changes: ComWrapper::new(ParamChanges { queues, count: AtomicI32::new(0) }),
            context,
            latency,
            failed_blocks: 0,
        })
    }

    fn block(&mut self, channels: &mut [&mut [f32]], pos: usize, len: usize) {
        // main input: our channels in order (a mono source feeds every plug-in channel)
        for (c, buf) in self.ins.bufs.iter_mut().enumerate() {
            for (k, ch) in buf.iter_mut().enumerate() {
                let dst = ch.get_mut(..len).unwrap_or_default();
                let src = if c == 0 { channels.get(k).or(channels.first()).and_then(|s| s.get(pos..pos + len)) } else { None };
                match src {
                    Some(s) => dst.copy_from_slice(s),
                    None => dst.fill(0.0),
                }
            }
        }
        // parameter changes for this block
        let mut n = 0usize;
        for &(id, v) in &self.pending {
            if let Some(q) = self.changes.queues.get(n) {
                q.id.store(id, Ordering::Relaxed);
                q.value.store(v.to_bits(), Ordering::Relaxed);
                n += 1;
            }
        }
        self.pending.clear();
        self.changes.count.store(n as i32, Ordering::Relaxed);
        let changes = if n > 0 { self.changes.as_com_ref::<IParameterChanges>().map_or(std::ptr::null_mut(), |r| r.as_ptr()) } else { std::ptr::null_mut() };
        let mut data = ProcessData {
            processMode: ProcessModes_::kRealtime as int32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
            numSamples: len as int32,
            numInputs: self.ins.abb.len() as int32,
            numOutputs: self.outs.abb.len() as int32,
            inputs: self.ins.abb.as_mut_ptr(),
            outputs: self.outs.abb.as_mut_ptr(),
            inputParameterChanges: changes,
            outputParameterChanges: std::ptr::null_mut(),
            inputEvents: std::ptr::null_mut(),
            outputEvents: std::ptr::null_mut(),
            processContext: &mut self.context,
        };
        // SAFETY: every pointer in `data` refers to buffers / objects owned by `self` that stay
        // alive and unmoved for the call: bus buffers of `max_block` ≥ `len` samples per channel,
        // the parameter-change list and the process context.
        let r = unsafe { self.inst.processor.process(&mut data) };
        self.changes.count.store(0, Ordering::Relaxed);
        self.context.projectTimeSamples = self.context.projectTimeSamples.saturating_add(len as i64);
        self.context.continousTimeSamples = self.context.projectTimeSamples;
        if !ok(r) {
            // the block passes through unchanged
            self.failed_blocks = self.failed_blocks.saturating_add(1);
            return;
        }
        let Some(out) = self.outs.bufs.first() else { return };
        for (k, ch) in out.iter().enumerate() {
            if let (Some(dst), Some(src)) = (channels.get_mut(k).and_then(|c| c.get_mut(pos..pos + len)), ch.get(..len)) {
                for (d, s) in dst.iter_mut().zip(src) {
                    // a plug-in's NaN / inf must not reach the mix
                    *d = if s.is_finite() { *s } else { 0.0 };
                }
            }
        }
    }
}

impl PluginProcessor for Vst3Processor {
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        let mut pos = 0;
        while pos < n {
            let len = (n - pos).min(self.max_block);
            self.block(channels, pos, len);
            pos += len;
        }
    }

    fn set_param(&mut self, id: u32, normalized: f64) -> bool {
        if !self.values.contains_key(&id) && !self.params.iter().any(|p| p.id == id) {
            return false;
        }
        let v = if normalized.is_finite() { normalized.clamp(0.0, 1.0) } else { return false };
        self.values.insert(id, v);
        match self.pending.iter_mut().find(|(p, _)| *p == id) {
            Some(e) => e.1 = v,
            None => self.pending.push((id, v)),
        }
        if let Some(c) = &self.inst.controller {
            // SAFETY: keep the controller's copy in sync (its state and display text).
            unsafe { c.setParamNormalized(id, v) };
        }
        true
    }

    fn param(&self, id: u32) -> Option<f64> {
        self.values.get(&id).copied()
    }

    fn reset(&mut self) {
        // SAFETY: deactivate / reactivate (the documented way to clear a plug-in's state).
        unsafe {
            self.inst.processor.setProcessing(0);
            self.inst.component.setActive(0);
            self.inst.component.setActive(1);
            self.inst.processor.setProcessing(1);
        }
        // re-send every parameter on the next block
        self.pending = self.values.iter().map(|(&k, &v)| (k, v)).collect();
        self.context.projectTimeSamples = 0;
        let _ = self.sample_rate;
    }

    fn latency(&self) -> usize {
        self.latency
    }

    fn save_state(&mut self) -> Result<Vec<u8>, PluginError> {
        self.inst.save_state().map_err(PluginError)
    }

    fn param_text(&self, id: u32, normalized: f64) -> Option<String> {
        let c = self.inst.controller.as_ref()?;
        let mut s: String128 = [0; 128];
        // SAFETY: `s` is a writable String128.
        ok(unsafe { c.getParamStringByValue(id, normalized.clamp(0.0, 1.0), &mut s) }).then(|| wide(&s))
    }
}

// ------------------------------------------------------------------------------------------------
// The host
// ------------------------------------------------------------------------------------------------

/// The VST3 host registered by `register_audio_plugins`.
#[derive(Default)]
pub struct Vst3Host {
    /// Search only these folders instead of the standard ones (tests, portable setups).
    folders: Option<Vec<PathBuf>>,
}

impl Vst3Host {
    /// A host that searches the standard VST3 folders ([`vst3_folders`]).
    pub fn standard() -> Vst3Host {
        Vst3Host { folders: None }
    }
    /// A host that searches only `folders`.
    pub fn with_folders(folders: Vec<PathBuf>) -> Vst3Host {
        Vst3Host { folders: Some(folders) }
    }
}

/// The class id of a uid string (inverse of `uid_string` for the COM layout).
fn tuid_of(uid: &str, m: &Module) -> Option<TUID> {
    effect_classes(m).into_iter().find(|c| uid_string(tuid_bytes(&c.cid), true) == uid).map(|c| c.cid)
}

/// Describe the plug-ins of one module (creating each effect once to read its parameters).
pub fn describe_module(bundle: &Path) -> Result<Vec<PluginDescriptor>, String> {
    let bin = vst3_binary(bundle).ok_or("no module for this system in the bundle")?;
    let m = load_module(&bin)?;
    let classes = effect_classes(&m);
    if classes.is_empty() {
        return Err("no audio effects in the module".into());
    }
    let mut out = Vec::new();
    let mut last_err = None;
    for c in classes {
        let params = match Instance::new(&m, &c.cid) {
            Ok(inst) => {
                let _ = inst.load_state(None);
                inst.params()
            }
            Err(e) => {
                last_err = Some(format!("{}: {e}", c.name));
                continue;
            }
        };
        out.push(PluginDescriptor {
            format: PluginFormat::Vst3,
            uid: uid_string(tuid_bytes(&c.cid), true),
            name: if c.name.is_empty() { bundle.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default() } else { c.name },
            vendor: c.vendor,
            version: c.version,
            categories: c.sub_categories,
            path: bundle.to_path_buf(),
            params,
        });
    }
    match (out.is_empty(), last_err) {
        (true, Some(e)) => Err(e),
        _ => Ok(out),
    }
}

impl fp::PluginHost for Vst3Host {
    fn format(&self) -> PluginFormat {
        PluginFormat::Vst3
    }

    fn scan(&self) -> ScanReport {
        let folders: Vec<PathBuf> = match &self.folders {
            Some(f) => f.iter().filter(|d| d.is_dir()).cloned().collect(),
            None => vst3_folders(),
        };
        let mut r = ScanReport { folders: folders.clone(), ..Default::default() };
        let mut seen = std::collections::HashSet::new();
        for f in &folders {
            for b in find_vst3_bundles(f) {
                let key = b.canonicalize().unwrap_or_else(|_| b.clone());
                if !seen.insert(key) || r.plugins.len() >= crate::audio_plugins::MAX_BUNDLES {
                    continue;
                }
                match describe_module(&b) {
                    Ok(v) => r.plugins.extend(v),
                    Err(e) => {
                        log::info!("VST3 scan: {}: {e}", b.display());
                        r.failures.push((b, e));
                    }
                }
            }
        }
        r
    }

    fn instantiate(&self, desc: &PluginDescriptor, config: ProcessConfig, state: Option<&[u8]>) -> Result<Box<dyn PluginProcessor>, PluginError> {
        let bin = vst3_binary(&desc.path).ok_or_else(|| PluginError(format!("{}: the plug-in file is gone", desc.path.display())))?;
        let m = load_module(&bin).map_err(PluginError)?;
        let cid = tuid_of(&desc.uid, &m).ok_or_else(|| PluginError(format!("{} no longer contains this plug-in", desc.path.display())))?;
        let inst = Instance::new(&m, &cid).map_err(PluginError)?;
        inst.load_state(state).map_err(PluginError)?;
        let p = Vst3Processor::new(inst, config).map_err(|e| PluginError(format!("{}: {e}", desc.name)))?;
        Ok(Box::new(p))
    }
}
