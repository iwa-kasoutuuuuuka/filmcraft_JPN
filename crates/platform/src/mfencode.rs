//! Media Foundation (Windows) hardware H.264 encoding: the GPU's media engine (NVENC, Quick Sync,
//! AMF) through its encoder transform.
//!
//! One of the FFI modules of this crate (docs/adr/0001-platform-ffi.md). Rules: every `unsafe`
//! block has a `// SAFETY:` comment; every failure is a `Result`. Settings the hardware path does
//! not take are declined up front (the factory in `lib.rs`), and an export whose hardware encoder
//! fails on its first frames carries on with our encoder (`filmcraft_export::hw`).
//!
//! An encoder is the system's hardware H.264 encoder transform (`MFTEnumEx` with
//! `MFT_ENUM_FLAG_HARDWARE`), an asynchronous transform driven by its events: it asks for input
//! (`METransformNeedInput`) and announces output (`METransformHaveOutput`). Pictures go in as NV12
//! in system memory (BT.709 limited range, as our encoder converts them); the transform gets the
//! process's Direct3D 11 device manager when it is Direct3D aware. B-frames are off, so the
//! output is in presentation order and needs no composition offsets. The Annex B output is turned
//! into length-prefixed samples, with the parameter sets moved into the `avcC`.

use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};

use filmcraft_export::{BitrateMode, ColorSignal, EncodedPacket, EncoderFrame, ExportError, ExportSettings, H264Profile, Result, VideoEncoder};
use filmcraft_isobmff::{AvcConfig, SampleEntry};
use filmcraft_time::FrameRate;
use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncCommonMaxBitRate, CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVEncMPVDefaultBPictureCount,
    CODECAPI_AVEncMPVGOPSize, ICodecAPI, IMFActivate, IMFMediaEventGenerator, IMFMediaType, IMFSample, IMFShutdown, IMFTransform, METransformDrainComplete,
    METransformHaveOutput, METransformNeedInput, MF_E_NO_EVENTS_AVAILABLE, MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE, MF_EVENT_FLAG_NO_WAIT,
    MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG2_LEVEL,
    MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_MT_TRANSFER_FUNCTION, MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_VIDEO_PRIMARIES, MF_MT_YUV_MATRIX,
    MF_SA_D3D11_AWARE, MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video,
    MFNominalRange_16_235, MFSampleExtension_CleanPoint, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER,
    MFT_MESSAGE_COMMAND_DRAIN, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
    MFT_MESSAGE_SET_D3D_MANAGER, MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFT_REGISTER_TYPE_INFO, MFTEnumEx, MFVideoFormat_H264,
    MFVideoFormat_NV12, MFVideoInterlace_Progressive, MFVideoPrimaries_BT709, MFVideoTransFunc_709, MFVideoTransferMatrix_BT709,
    eAVEncCommonRateControlMode_CBR, eAVEncCommonRateControlMode_PeakConstrainedVBR,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Variant::VARIANT;
use windows::core::Interface;

use crate::mediafoundation::{Device, err, runtime};

/// Encoder transforms tried.
const MAX_TRANSFORMS: usize = 8;
/// Largest picture side the hardware path takes (the media engines' common limit is 4096).
const MAX_SIDE: u32 = 4096;
/// How long the transform may go without asking for input or giving output before the encode
/// counts as failed (a hung driver must not hang the export).
const TIMEOUT: Duration = Duration::from_secs(10);
/// Pause between polls of the transform's event queue.
const POLL: Duration = Duration::from_micros(100);
/// Largest encoded picture accepted from the transform.
const MAX_PACKET: usize = 64 << 20;

fn enc_err(s: String) -> ExportError {
    ExportError::Encode(format!("hardware H.264 encoder: {s}"))
}

/// A hardware H.264 encoder for one export.
pub struct MfEncoder {
    transform: IMFTransform,
    events: IMFMediaEventGenerator,
    name: String,
    w: u32,
    h: u32,
    rate: FrameRate,
    signal: ColorSignal,
    /// The transform allocates its output samples (else: `out_size` bytes per sample, ours).
    provides_samples: bool,
    out_size: u32,
    /// Input requests not answered yet.
    credits: u32,
    /// Encoded pictures not handed out yet (in output order).
    ready: VecDeque<EncodedPacket>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    /// Sample time of the last output (presentation order is checked: no B-frames).
    last_time: Option<i64>,
}

// SAFETY: a Media Foundation transform and its event generator are free-threaded objects of the
// process's multithreaded apartment (`runtime`); the encoder is used by one thread at a time
// (`&mut self`).
unsafe impl Send for MfEncoder {}

impl MfEncoder {
    /// A hardware encoder for `w`×`h` at `rate` with the export's bitrate, keyframe distance and
    /// profile, or why there is none.
    pub fn new(w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> std::result::Result<MfEncoder, String> {
        if w == 0 || h == 0 || !w.is_multiple_of(2) || !h.is_multiple_of(2) || w > MAX_SIDE || h > MAX_SIDE {
            return Err(format!("picture size {w}x{h}"));
        }
        if rate.num <= 0 || rate.den <= 0 || rate.num > u32::MAX as i64 || rate.den > u32::MAX as i64 {
            return Err("frame rate".into());
        }
        runtime()?;
        let mut why = String::from("no hardware H.264 encoder on this system");
        for activate in encoder_transforms()? {
            // SAFETY: a valid activation object from `MFTEnumEx`.
            let transform: IMFTransform = match unsafe { activate.ActivateObject() } {
                Ok(t) => t,
                Err(e) => {
                    why = err("IMFActivate::ActivateObject", e);
                    continue;
                }
            };
            let name = transform_name(&activate);
            match Self::configure(transform, name, w, h, rate, s) {
                Ok(enc) => return Ok(enc),
                Err(e) => why = e,
            }
        }
        Err(why)
    }

    fn configure(transform: IMFTransform, name: String, w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> std::result::Result<MfEncoder, String> {
        // SAFETY: a valid transform.
        let attributes = unsafe { transform.GetAttributes() }.map_err(|e| err("IMFTransform::GetAttributes", e))?;
        // SAFETY: a valid attribute store and pointers to constant GUIDs.
        unsafe {
            if attributes.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) == 0 {
                return Err(format!("{name}: not an asynchronous (hardware) transform"));
            }
            attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1).map_err(|e| err("MF_TRANSFORM_ASYNC_UNLOCK", e))?;
        }
        let events: IMFMediaEventGenerator = transform.cast().map_err(|e| err("IMFMediaEventGenerator", e))?;
        // SAFETY: a valid attribute store and a pointer to a constant GUID.
        if unsafe { attributes.GetUINT32(&MF_SA_D3D11_AWARE) }.unwrap_or(0) != 0
            && let Ok(device) = Device::shared()
        {
            // SAFETY: the message's parameter is the device manager's interface pointer, alive
            // for the call; the transform takes its own reference to it.
            unsafe { transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, device.manager.as_raw() as usize) }
                .map_err(|e| err("MFT_MESSAGE_SET_D3D_MANAGER", e))?;
        }

        // rate control, GOP and no B-frames, before the media types (they can depend on them)
        let kbps = s.bitrate_kbps.max(100);
        let max_kbps = s.max_bitrate_kbps.filter(|m| *m >= kbps).unwrap_or(kbps * 3 / 2);
        let keyint = s.keyframe_distance.filter(|k| *k > 0).unwrap_or_else(|| (rate.num as f64 / rate.den as f64 * 2.0).round().max(1.0) as u32);
        if let Ok(api) = transform.cast::<ICodecAPI>() {
            let mode = if s.bitrate_mode == BitrateMode::Cbr { eAVEncCommonRateControlMode_CBR } else { eAVEncCommonRateControlMode_PeakConstrainedVBR };
            set_codec_value(&api, &CODECAPI_AVEncCommonRateControlMode, mode.0 as u32);
            set_codec_value(&api, &CODECAPI_AVEncCommonMeanBitRate, kbps.saturating_mul(1000));
            if s.bitrate_mode != BitrateMode::Cbr {
                set_codec_value(&api, &CODECAPI_AVEncCommonMaxBitRate, max_kbps.saturating_mul(1000));
            }
            set_codec_value(&api, &CODECAPI_AVEncMPVGOPSize, keyint);
            set_codec_value(&api, &CODECAPI_AVEncMPVDefaultBPictureCount, 0);
        }

        let profile = match s.h264_profile {
            H264Profile::Baseline => 66u32,
            H264Profile::Main => 77,
            H264Profile::High => 100,
        };
        let size = (u64::from(w) << 32) | u64::from(h);
        let fps = ((rate.num as u64) << 32) | rate.den as u64;
        // SAFETY: interface out.
        let output: IMFMediaType = unsafe { MFCreateMediaType() }.map_err(|e| err("MFCreateMediaType", e))?;
        // SAFETY: valid interfaces; the GUID pointers are to constants that outlive the calls (the
        // media type copies the values).
        unsafe {
            output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| err("SetGUID", e))?;
            output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(|e| err("SetGUID", e))?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, kbps.saturating_mul(1000)).map_err(|e| err("SetUINT32", e))?;
            output.SetUINT64(&MF_MT_FRAME_SIZE, size).map_err(|e| err("SetUINT64", e))?;
            output.SetUINT64(&MF_MT_FRAME_RATE, fps).map_err(|e| err("SetUINT64", e))?;
            output.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1).map_err(|e| err("SetUINT64", e))?;
            output.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(|e| err("SetUINT32", e))?;
            output.SetUINT32(&MF_MT_MPEG2_PROFILE, profile).map_err(|e| err("SetUINT32", e))?;
            if let Some(level) = s.h264_level {
                output.SetUINT32(&MF_MT_MPEG2_LEVEL, u32::from(level)).map_err(|e| err("SetUINT32", e))?;
            }
            set_bt709(&output);
            transform.SetOutputType(0, &output, 0).map_err(|e| err(&format!("{name}: IMFTransform::SetOutputType"), e))?;
        }
        // SAFETY: interface out.
        let input: IMFMediaType = unsafe { MFCreateMediaType() }.map_err(|e| err("MFCreateMediaType", e))?;
        // SAFETY: as above.
        unsafe {
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| err("SetGUID", e))?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).map_err(|e| err("SetGUID", e))?;
            input.SetUINT64(&MF_MT_FRAME_SIZE, size).map_err(|e| err("SetUINT64", e))?;
            input.SetUINT64(&MF_MT_FRAME_RATE, fps).map_err(|e| err("SetUINT64", e))?;
            input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1).map_err(|e| err("SetUINT64", e))?;
            input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(|e| err("SetUINT32", e))?;
            input.SetUINT32(&MF_MT_DEFAULT_STRIDE, w).map_err(|e| err("SetUINT32", e))?;
            set_bt709(&input);
            transform.SetInputType(0, &input, 0).map_err(|e| err(&format!("{name}: IMFTransform::SetInputType"), e))?;
        }
        // SAFETY: a valid transform; stream 0 is the encoder's only output.
        let stream = unsafe { transform.GetOutputStreamInfo(0) }.map_err(|e| err("IMFTransform::GetOutputStreamInfo", e))?;
        let provides_samples = stream.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
        let out_size = stream.cbSize.max(w.saturating_mul(h)).max(1 << 16);
        // SAFETY: a valid transform; these messages take no parameter.
        unsafe {
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).map_err(|e| err("MFT_MESSAGE_NOTIFY_BEGIN_STREAMING", e))?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).map_err(|e| err("MFT_MESSAGE_NOTIFY_START_OF_STREAM", e))?;
        }
        Ok(MfEncoder {
            transform,
            events,
            name,
            w,
            h,
            rate,
            signal: s.signal,
            provides_samples,
            out_size,
            credits: 0,
            ready: VecDeque::new(),
            sps: None,
            pps: None,
            last_time: None,
        })
    }

    /// The transform's name (diagnostics).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 100-ns units of frame `index`.
    fn time_of(&self, index: u64) -> i64 {
        let t = index as i128 * 10_000_000 * self.rate.den as i128 / self.rate.num as i128;
        t.clamp(0, i64::MAX as i128) as i64
    }

    /// An NV12 sample of the picture (BT.709 limited range, as our H.264 encoder converts).
    fn input_sample(&self, f: &EncoderFrame) -> std::result::Result<IMFSample, String> {
        let (w, h) = (self.w as usize, self.h as usize);
        if f.width != self.w || f.height != self.h || f.rgba.len() < w * h * 4 {
            return Err("picture size changed".into());
        }
        let len = w * h + w * (h / 2);
        let len32 = u32::try_from(len).map_err(|_| "picture too large")?;
        // SAFETY: plain value in, interface out.
        let buffer = unsafe { MFCreateMemoryBuffer(len32) }.map_err(|e| err("MFCreateMemoryBuffer", e))?;
        let mut dst: *mut u8 = std::ptr::null_mut();
        let mut capacity = 0u32;
        // SAFETY: valid out pointers; the buffer stays locked until `Unlock` below.
        unsafe { buffer.Lock(&mut dst, Some(&mut capacity), None) }.map_err(|e| err("IMFMediaBuffer::Lock", e))?;
        let fits = !dst.is_null() && capacity as usize >= len;
        if fits {
            // SAFETY: the locked buffer holds at least `capacity >= len` writable bytes at `dst`,
            // and a fresh buffer cannot overlap the picture.
            let out = unsafe { std::slice::from_raw_parts_mut(dst, len) };
            match f.prepared.filter(|p| p.len() == len) {
                // converted while rendering (`input_converter`)
                Some(p) => out.copy_from_slice(p),
                None => {
                    let (luma, chroma) = out.split_at_mut(w * h);
                    filmcraft_export::rgba_to_nv12(f.rgba, w, h, luma, chroma);
                }
            }
        }
        // SAFETY: the buffer was locked above.
        unsafe { buffer.Unlock() }.map_err(|e| err("IMFMediaBuffer::Unlock", e))?;
        if !fits {
            return Err("media buffer smaller than asked for".into());
        }
        let (time, next) = (self.time_of(f.index), self.time_of(f.index.saturating_add(1)));
        // SAFETY: `len` bytes of the buffer were written; valid interfaces and plain values.
        unsafe {
            buffer.SetCurrentLength(len32).map_err(|e| err("IMFMediaBuffer::SetCurrentLength", e))?;
            let sample = MFCreateSample().map_err(|e| err("MFCreateSample", e))?;
            sample.AddBuffer(&buffer).map_err(|e| err("IMFSample::AddBuffer", e))?;
            sample.SetSampleTime(time).map_err(|e| err("IMFSample::SetSampleTime", e))?;
            sample.SetSampleDuration(next.saturating_sub(time).max(1)).map_err(|e| err("IMFSample::SetSampleDuration", e))?;
            Ok(sample)
        }
    }

    /// The next event of the transform without waiting: `Some(type)` or `None` (queue empty).
    fn poll_event(&self) -> std::result::Result<Option<u32>, String> {
        // SAFETY: a valid event generator; the no-wait flag returns at once.
        match unsafe { self.events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
            // SAFETY: a valid event.
            Ok(ev) => unsafe { ev.GetType() }.map(Some).map_err(|e| err("IMFMediaEvent::GetType", e)),
            Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => Ok(None),
            Err(e) => Err(err("IMFMediaEventGenerator::GetEvent", e)),
        }
    }

    /// Handle every queued event: input requests are counted, outputs collected. Returns whether
    /// `stop` (an event type) was seen.
    fn pump(&mut self, stop: Option<u32>) -> std::result::Result<bool, String> {
        let mut seen = false;
        while let Some(ty) = self.poll_event()? {
            if ty == METransformNeedInput.0 as u32 {
                self.credits = self.credits.saturating_add(1);
            } else if ty == METransformHaveOutput.0 as u32 {
                self.take_output()?;
            }
            if Some(ty) == stop {
                seen = true;
            }
        }
        Ok(seen)
    }

    /// Take one encoded picture from the transform.
    fn take_output(&mut self) -> std::result::Result<(), String> {
        for _ in 0..4 {
            let ours = if self.provides_samples {
                None
            } else {
                // SAFETY: plain value in, interfaces out.
                unsafe {
                    let buffer = MFCreateMemoryBuffer(self.out_size).map_err(|e| err("MFCreateMemoryBuffer", e))?;
                    let sample = MFCreateSample().map_err(|e| err("MFCreateSample", e))?;
                    sample.AddBuffer(&buffer).map_err(|e| err("IMFSample::AddBuffer", e))?;
                    Some(sample)
                }
            };
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER { pSample: ManuallyDrop::new(ours), ..Default::default() }];
            let mut status = 0u32;
            // SAFETY: one output buffer for the encoder's only output stream, holding our sample
            // when the transform does not provide its own.
            let r = unsafe { self.transform.ProcessOutput(0, &mut buffers, &mut status) };
            let [mut b] = buffers;
            // SAFETY: each field is taken exactly once and not used again.
            let (sample, _events) = unsafe { (ManuallyDrop::take(&mut b.pSample), ManuallyDrop::take(&mut b.pEvents)) };
            match r {
                Ok(()) => return self.packet(&sample.ok_or("the encoder returned no sample")?),
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    // the output type was renegotiated: take the transform's first offer again
                    // SAFETY: a valid transform.
                    unsafe {
                        let ty = self.transform.GetOutputAvailableType(0, 0).map_err(|e| err("IMFTransform::GetOutputAvailableType", e))?;
                        self.transform.SetOutputType(0, &ty, 0).map_err(|e| err("IMFTransform::SetOutputType", e))?;
                    }
                }
                Err(e) => return Err(err("IMFTransform::ProcessOutput", e)),
            }
        }
        Err("the encoder keeps changing its output format".into())
    }

    /// An encoded sample → a length-prefixed packet (parameter sets kept for the `avcC`).
    fn packet(&mut self, sample: &IMFSample) -> std::result::Result<(), String> {
        // SAFETY: a valid sample.
        let time = unsafe { sample.GetSampleTime() }.unwrap_or(0);
        if self.last_time.is_some_and(|t| time < t) {
            return Err("the encoder reorders pictures (B-frames)".into());
        }
        self.last_time = Some(time);
        // SAFETY: a valid sample and a pointer to a constant GUID.
        let clean = unsafe { sample.GetUINT32(&MFSampleExtension_CleanPoint) }.unwrap_or(0) != 0;
        // SAFETY: a valid sample.
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }.map_err(|e| err("IMFSample::ConvertToContiguousBuffer", e))?;
        let mut src: *mut u8 = std::ptr::null_mut();
        let mut len = 0u32;
        // SAFETY: valid out pointers; the buffer stays locked until `Unlock` below.
        unsafe { buffer.Lock(&mut src, None, Some(&mut len)) }.map_err(|e| err("IMFMediaBuffer::Lock", e))?;
        let ok = !src.is_null() && (len as usize) <= MAX_PACKET;
        let data = if ok {
            // SAFETY: the locked buffer holds `len` valid bytes at `src` until it is unlocked; they
            // are copied out before that.
            unsafe { std::slice::from_raw_parts(src, len as usize) }.to_vec()
        } else {
            Vec::new()
        };
        // SAFETY: the buffer was locked above.
        unsafe { buffer.Unlock() }.map_err(|e| err("IMFMediaBuffer::Unlock", e))?;
        if !ok {
            return Err("encoded picture too large".into());
        }
        let mut out = Vec::with_capacity(data.len() + 16);
        let mut idr = false;
        for nal in annex_b_units(&data) {
            let Some(&head) = nal.first() else { continue };
            match head & 0x1f {
                7 => self.sps = Some(nal.to_vec()),
                8 => self.pps = Some(nal.to_vec()),
                // access unit delimiters and filler data stay out of MP4 samples
                9 | 12 => {}
                t => {
                    idr |= t == 5;
                    out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                    out.extend_from_slice(nal);
                }
            }
        }
        if !out.is_empty() {
            self.ready.push_back(EncodedPacket { data: out, key: idr || clean, duration: self.rate.den as u32, composition_offset: 0 });
        }
        Ok(())
    }

    fn message(&self, m: windows::Win32::Media::MediaFoundation::MFT_MESSAGE_TYPE) -> std::result::Result<(), String> {
        // SAFETY: a valid transform; these messages take no parameter.
        unsafe { self.transform.ProcessMessage(m, 0) }.map_err(|e| err("IMFTransform::ProcessMessage", e))
    }

    fn encode_inner(&mut self, f: &EncoderFrame) -> std::result::Result<Vec<EncodedPacket>, String> {
        let sample = self.input_sample(f)?;
        let first = self.sps.is_none();
        let mut idle = Instant::now();
        loop {
            let before = (self.credits, self.ready.len());
            self.pump(None)?;
            if self.credits > 0 {
                break;
            }
            if (self.credits, self.ready.len()) != before {
                idle = Instant::now();
            }
            if idle.elapsed() > TIMEOUT {
                return Err(format!("{} stopped asking for pictures", self.name));
            }
            std::thread::sleep(POLL);
        }
        self.credits -= 1;
        // SAFETY: valid transform and sample; stream 0 is the encoder's only input.
        unsafe { self.transform.ProcessInput(0, &sample, 0) }.map_err(|e| err("IMFTransform::ProcessInput", e))?;
        // the first picture: wait for the parameter sets (until the transform wants more input)
        if first {
            let idle = Instant::now();
            while self.sps.is_none() && self.credits == 0 {
                self.pump(None)?;
                if idle.elapsed() > TIMEOUT {
                    return Err(format!("{} gave no output", self.name));
                }
                std::thread::sleep(POLL);
            }
        } else {
            self.pump(None)?;
        }
        Ok(self.ready.drain(..).collect())
    }

    fn flush_inner(&mut self) -> std::result::Result<Vec<EncodedPacket>, String> {
        self.message(MFT_MESSAGE_NOTIFY_END_OF_STREAM)?;
        self.message(MFT_MESSAGE_COMMAND_DRAIN)?;
        let mut idle = Instant::now();
        loop {
            let before = self.ready.len();
            if self.pump(Some(METransformDrainComplete.0 as u32))? {
                break;
            }
            if self.ready.len() != before {
                idle = Instant::now();
            }
            if idle.elapsed() > TIMEOUT {
                return Err(format!("{} did not finish draining", self.name));
            }
            std::thread::sleep(POLL);
        }
        Ok(self.ready.drain(..).collect())
    }
}

impl VideoEncoder for MfEncoder {
    fn sample_entry(&self) -> SampleEntry {
        let cfg = AvcConfig::new(self.sps.iter().cloned().collect(), self.pps.iter().cloned().collect(), 4);
        let mut e = SampleEntry::avc(cfg, self.w as u16, self.h as u16);
        self.signal.apply_to(&mut e, false);
        e
    }
    fn timescale(&self) -> u32 {
        self.rate.num as u32
    }
    fn encode(&mut self, frame: &EncoderFrame) -> Result<Vec<EncodedPacket>> {
        self.encode_inner(frame).map_err(enc_err)
    }
    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        self.flush_inner().map_err(enc_err)
    }
    fn is_hardware(&self) -> bool {
        true
    }
    fn input_converter(&self) -> Option<filmcraft_export::InputConverter> {
        Some(filmcraft_export::InputConverter { id: "nv12", convert: filmcraft_export::rgba_to_nv12_vec })
    }
}

impl Drop for MfEncoder {
    fn drop(&mut self) {
        // an asynchronous transform holds worker threads and GPU resources until shut down
        if let Ok(s) = self.transform.cast::<IMFShutdown>() {
            // SAFETY: a valid interface; nothing uses the transform afterwards.
            let _ = unsafe { s.Shutdown() };
        }
    }
}

/// Set a codec property; transforms that do not support one keep their default.
fn set_codec_value(api: &ICodecAPI, key: &windows::core::GUID, value: u32) {
    let v = VARIANT::from(value);
    // SAFETY: a valid interface; the GUID and the variant outlive the call (it copies the value).
    if let Err(e) = unsafe { api.SetValue(key, &v) } {
        log::debug!("hardware encoder: codec property {key:?} not set ({:#010x})", e.code().0);
    }
}

/// BT.709 limited-range signalling on a media type (what the encoder writes into the VUI).
fn set_bt709(ty: &IMFMediaType) {
    // SAFETY: a valid media type and pointers to constant GUIDs; failures leave the encoder's
    // defaults, and the MP4 sample entry signals the colour anyway.
    unsafe {
        let _ = ty.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32);
        let _ = ty.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32);
        let _ = ty.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32);
        let _ = ty.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32);
    }
}

/// The NAL units of an Annex B byte stream (start codes removed).
fn annex_b_units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let mut end = starts.get(k + 1).map(|n| n - 3).unwrap_or(data.len());
        // a four-byte start code leaves a zero at the end of the previous unit
        while end > s && data.get(end - 1) == Some(&0) && k + 1 < starts.len() {
            end -= 1;
        }
        if let Some(nal) = data.get(s..end).filter(|n| !n.is_empty()) {
            out.push(nal);
        }
    }
    out
}

/// The transform's friendly name, for diagnostics.
fn transform_name(activate: &IMFActivate) -> String {
    use windows::Win32::Media::MediaFoundation::MFT_FRIENDLY_NAME_Attribute;
    let mut buf = [0u16; 128];
    let mut len = 0u32;
    // SAFETY: a valid activation object, a constant GUID, and a buffer with its length.
    match unsafe { activate.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buf, Some(&mut len)) } {
        Ok(()) => String::from_utf16_lossy(buf.get(..len as usize).unwrap_or(&[])),
        Err(_) => "hardware H.264 encoder".into(),
    }
}

/// The system's hardware H.264 encoder transforms, best first.
fn encoder_transforms() -> std::result::Result<Vec<IMFActivate>, String> {
    let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: `output` outlives the call; `list` and `count` are valid out pointers. On success
    // `list` is a CoTaskMemAlloc'ed array of `count` activation objects that we own.
    unsafe { MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER, None, Some(&output), &mut list, &mut count) }
        .map_err(|e| err("MFTEnumEx", e))?;
    let mut out = Vec::new();
    if !list.is_null() {
        for i in 0..count as usize {
            // SAFETY: `i < count`, so the element is inside the array; `read` moves the reference
            // out (each element exactly once), and the array itself is freed below without
            // dropping its elements again.
            if let Some(a) = unsafe { list.add(i).read() }
                && out.len() < MAX_TRANSFORMS
            {
                out.push(a);
            }
        }
        // SAFETY: the array was allocated by `MFTEnumEx` with CoTaskMemAlloc and is no longer used.
        unsafe { CoTaskMemFree(Some(list as *const std::ffi::c_void)) };
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annex_b_units_split_three_and_four_byte_start_codes() {
        let data = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 4, 5, 0];
        let units = annex_b_units(&data);
        assert_eq!(units, vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 4, 5, 0][..]]);
        assert!(annex_b_units(&[]).is_empty());
        assert!(annex_b_units(&[0, 0, 1]).is_empty());
    }
}
