//! Media Foundation (Windows) hardware H.264 / HEVC decoding, through DXVA on Direct3D 11.
//!
//! One of the FFI modules of this crate (docs/adr/0001-platform-ffi.md). Rules: every `unsafe`
//! block has a `// SAFETY:` comment; every failure is a `Result` the caller
//! ([`crate::hybrid::HybridDecoder`]) answers by switching to the software decoder.
//!
//! A decoder is the system's H.264 or HEVC decoder transform (`IMFTransform`, found with
//! `MFTEnumEx`) given a Direct3D 11 device manager, which is what makes it decode on the GPU
//! (DXVA). Streams it cannot take that way are declined up front: no Direct3D 11 video device, no
//! decoder transform for the codec (HEVC needs the HEVC Video Extensions), a transform that is not
//! Direct3D 11 aware, a format the transform refuses (4:2:2, 10-bit H.264, field-coded pictures).
//! A transform that accepts the stream but then hands out system-memory pictures is decoding in
//! software; that is reported as a failure, so our own decoder takes over instead.
//!
//! Samples are fed as Annex B access units (the parameter sets of the sample entry go in front of
//! the first one of a run). The transform returns pictures in presentation order as NV12 / P010
//! Direct3D textures; each is read back (`IMF2DBuffer2::Lock2DSize`) and copied into planar
//! [`PixelData::Yuv8`] / [`PixelData::Yuv16`], cropped to the picture's output rectangle.

use std::mem::ManuallyDrop;
use std::sync::{Arc, OnceLock};

use filmcraft_codecs::hw::{NalCodec, NalStreamInfo};
use filmcraft_codecs::{CodecError, DecodedFrame, Result, VideoDecoder};
use filmcraft_frame::{Chroma, PixelData, VideoFrame};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11CreateDevice, ID3D11Device, ID3D11Multithread, ID3D11Texture2D,
};
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer2, IMFActivate, IMFDXGIBuffer, IMFDXGIDeviceManager, IMFMediaType, IMFSample, IMFTransform, MF_E_NOTACCEPTING, MF_E_TRANSFORM_NEED_MORE_INPUT,
    MF_E_TRANSFORM_STREAM_CHANGE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SA_D3D11_AWARE, MF_VERSION,
    MF2DBuffer_LockFlags_Read, MFCreateDXGIDeviceManager, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video, MFSTARTUP_LITE,
    MFStartup, MFT_CATEGORY_VIDEO_DECODER, MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT, MFT_MESSAGE_COMMAND_DRAIN, MFT_MESSAGE_COMMAND_FLUSH,
    MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_MESSAGE_SET_D3D_MANAGER, MFT_OUTPUT_DATA_BUFFER,
    MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFT_REGISTER_TYPE_INFO, MFTEnumEx, MFVideoFormat_H264, MFVideoFormat_HEVC, MFVideoFormat_NV12, MFVideoFormat_P010,
    MFVideoInterlace_Progressive,
};
use windows::Win32::System::Com::{CoIncrementMTAUsage, CoTaskMemFree};
use windows::core::{GUID, Interface};

/// Annex B start code.
const START_CODE: [u8; 4] = [0, 0, 0, 1];
/// Largest access unit handed to the transform (a sample's size comes from the file).
const MAX_SAMPLE: usize = 256 << 20;
/// Pictures taken out of the transform in one call (it holds a reorder buffer's worth at most).
const MAX_OUTPUTS: usize = 64;
/// Decoder transforms tried for a codec.
const MAX_TRANSFORMS: usize = 8;

fn err(what: &str, e: windows::core::Error) -> String {
    format!("{what} failed ({:#010x})", e.code().0)
}

/// COM (multithreaded apartment) and Media Foundation, started once for the process and never
/// stopped: decoders live on any thread until the app exits.
fn runtime() -> std::result::Result<(), String> {
    static STARTED: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    STARTED
        .get_or_init(|| {
            // SAFETY: no arguments. Keeps the process's multithreaded apartment alive, so threads
            // that never initialised COM (ours) use Media Foundation objects in it; the cookie is
            // deliberately never released.
            unsafe { CoIncrementMTAUsage() }.map_err(|e| err("CoIncrementMTAUsage", e))?;
            // SAFETY: plain values; balanced by no MFShutdown on purpose (process lifetime).
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) }.map_err(|e| err("MFStartup", e))
        })
        .clone()
}

/// What the transform outputs and what each picture carries.
#[derive(Clone, Debug)]
struct Layout {
    /// `MFVideoFormat_NV12` or `MFVideoFormat_P010`.
    subtype: GUID,
    /// Output rectangle in the decoded surface (luma samples).
    crop: (u32, u32, u32, u32),
    /// Significant bits per sample (8 = `Yuv8`).
    bits: u32,
    color: filmcraft_color::ColorInfo,
    par: (u32, u32),
}

/// The layout for a stream, or why Media Foundation is not used for it.
fn layout(info: &NalStreamInfo) -> std::result::Result<Layout, String> {
    if info.interlaced {
        return Err("field-coded H.264".into());
    }
    if info.chroma_format_idc != 1 {
        return Err(format!("chroma_format_idc {}", info.chroma_format_idc));
    }
    let subtype = match (info.codec, info.bit_depth_luma, info.bit_depth_chroma) {
        (_, 8, 8) => MFVideoFormat_NV12,
        (NalCodec::Hevc, 10, 10) => MFVideoFormat_P010,
        (_, l, c) => return Err(format!("{l}-bit luma / {c}-bit chroma")),
    };
    let (cx, cy, w, h) = info.crop;
    if w == 0 || h == 0 || w > 8192 || h > 8192 || cx.saturating_add(w) > info.coded.0 || cy.saturating_add(h) > info.coded.1 {
        return Err(format!("picture size {w}x{h}"));
    }
    Ok(Layout { subtype, crop: info.crop, bits: info.bit_depth_luma, color: info.color, par: info.par })
}

/// The Direct3D 11 device every decoder of the process decodes on, with the device manager that
/// shares it between them. Created once: making a device costs far more than decoding a picture.
struct Device {
    manager: IMFDXGIDeviceManager,
    // Kept alive for the manager.
    _device: ID3D11Device,
}

// SAFETY: a Direct3D 11 device with multithread protection on (set in `Device::new`) and the DXGI
// device manager, whose purpose is to share that device between threads, are free-threaded.
unsafe impl Send for Device {}
// SAFETY: as above; `Device` has no interior state of its own.
unsafe impl Sync for Device {}

impl Device {
    fn new() -> std::result::Result<Device, String> {
        let mut device: Option<ID3D11Device> = None;
        // SAFETY: `device` is a valid out pointer for the call; the other out pointers are not
        // requested. No adapter and no software rasterizer module: the default hardware adapter.
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
        }
        .map_err(|e| err("D3D11CreateDevice", e))?;
        let device = device.ok_or("no Direct3D 11 device")?;
        let multithread: ID3D11Multithread = device.cast().map_err(|e| err("ID3D11Multithread", e))?;
        // SAFETY: a valid interface; decoders use the device from their own threads.
        let _ = unsafe { multithread.SetMultithreadProtected(true) };
        let mut token = 0u32;
        let mut manager: Option<IMFDXGIDeviceManager> = None;
        // SAFETY: both are valid out pointers for the call.
        unsafe { MFCreateDXGIDeviceManager(&mut token, &mut manager) }.map_err(|e| err("MFCreateDXGIDeviceManager", e))?;
        let manager = manager.ok_or("no DXGI device manager")?;
        // SAFETY: `device` is a valid Direct3D 11 device and `token` the manager's own reset token.
        unsafe { manager.ResetDevice(&device, token) }.map_err(|e| err("IMFDXGIDeviceManager::ResetDevice", e))?;
        Ok(Device { manager, _device: device })
    }

    /// The process's device, or why there is none (no video-capable adapter).
    fn shared() -> std::result::Result<&'static Device, String> {
        static DEVICE: OnceLock<std::result::Result<Device, String>> = OnceLock::new();
        DEVICE.get_or_init(Device::new).as_ref().map_err(Clone::clone)
    }
}

/// A decoder transform set up for one stream.
struct Session {
    transform: IMFTransform,
}

// SAFETY: a Media Foundation transform is a free-threaded object of the process's multithreaded
// apartment (`runtime`); the decoder owning a `Session` is used by one thread at a time (`&mut
// self`).
unsafe impl Send for Session {}

impl Session {
    fn new(info: &NalStreamInfo, layout: &Layout) -> std::result::Result<Session, String> {
        runtime()?;
        let device = Device::shared()?;

        let mut why = String::from("no decoder transform for the codec");
        for activate in decoder_transforms(info.codec)? {
            // SAFETY: a valid activation object from `MFTEnumEx`.
            let transform: IMFTransform = match unsafe { activate.ActivateObject() } {
                Ok(t) => t,
                Err(e) => {
                    why = err("IMFActivate::ActivateObject", e);
                    continue;
                }
            };
            match configure(&transform, &device.manager, info, layout) {
                Ok(()) => return Ok(Session { transform }),
                Err(e) => why = e,
            }
        }
        Err(why)
    }

    /// Hand one Annex B access unit to the transform. `Ok(false)`: it takes no input until some
    /// output was taken.
    fn input(&self, data: &[u8], time: i64) -> std::result::Result<bool, String> {
        let len = u32::try_from(data.len()).ok().filter(|_| data.len() <= MAX_SAMPLE && !data.is_empty()).ok_or("sample size")?;
        // SAFETY: plain value in, interface out.
        let buffer = unsafe { MFCreateMemoryBuffer(len) }.map_err(|e| err("MFCreateMemoryBuffer", e))?;
        let mut dst: *mut u8 = std::ptr::null_mut();
        let mut capacity = 0u32;
        // SAFETY: valid out pointers; the buffer stays locked until `Unlock` below.
        unsafe { buffer.Lock(&mut dst, Some(&mut capacity), None) }.map_err(|e| err("IMFMediaBuffer::Lock", e))?;
        let fits = !dst.is_null() && capacity >= len;
        if fits {
            // SAFETY: the locked buffer holds at least `capacity >= len` writable bytes at `dst`,
            // `data` holds `len` readable bytes, and a fresh buffer cannot overlap `data`.
            unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len()) };
        }
        // SAFETY: the buffer was locked above.
        unsafe { buffer.Unlock() }.map_err(|e| err("IMFMediaBuffer::Unlock", e))?;
        if !fits {
            return Err("media buffer smaller than asked for".into());
        }
        // SAFETY: `len` bytes of the buffer were written.
        unsafe { buffer.SetCurrentLength(len) }.map_err(|e| err("IMFMediaBuffer::SetCurrentLength", e))?;
        // SAFETY: interface out.
        let sample = unsafe { MFCreateSample() }.map_err(|e| err("MFCreateSample", e))?;
        // SAFETY: valid interfaces and a plain value.
        unsafe {
            sample.AddBuffer(&buffer).map_err(|e| err("IMFSample::AddBuffer", e))?;
            sample.SetSampleTime(time).map_err(|e| err("IMFSample::SetSampleTime", e))?;
        }
        // SAFETY: valid transform and sample; stream 0 is the decoder's only input.
        match unsafe { self.transform.ProcessInput(0, &sample, 0) } {
            Ok(()) => Ok(true),
            Err(e) if e.code() == MF_E_NOTACCEPTING => Ok(false),
            Err(e) => Err(err("IMFTransform::ProcessInput", e)),
        }
    }

    /// Take the next decoded picture. `Ok(None)`: the transform needs more input.
    fn output(&self, layout: &Layout) -> std::result::Result<Option<(i64, VideoFrame)>, String> {
        for _ in 0..4 {
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER::default()];
            let mut status = 0u32;
            // SAFETY: one output buffer for the decoder's only output stream; with a device
            // manager the transform provides the sample (checked in `configure`).
            let r = unsafe { self.transform.ProcessOutput(0, &mut buffers, &mut status) };
            // Whatever happened, what the transform put in the buffer is ours to release.
            let [mut b] = buffers;
            // SAFETY: each field is taken exactly once and not used again.
            let (sample, _events) = unsafe { (ManuallyDrop::take(&mut b.pSample), ManuallyDrop::take(&mut b.pEvents)) };
            match r {
                Ok(()) => {
                    let sample = sample.ok_or("the decoder returned no picture")?;
                    // SAFETY: a valid sample.
                    let time = unsafe { sample.GetSampleTime() }.map_err(|e| err("IMFSample::GetSampleTime", e))?;
                    return Ok(Some((time, copy_out(&sample, layout)?)));
                }
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                // the output format is known only now (or changed): choose it again and retry
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => set_output_type(&self.transform, layout)?,
                Err(e) => return Err(err("IMFTransform::ProcessOutput", e)),
            }
        }
        Err("the decoder keeps changing its output format".into())
    }

    fn message(&self, m: windows::Win32::Media::MediaFoundation::MFT_MESSAGE_TYPE) -> std::result::Result<(), String> {
        // SAFETY: a valid transform; these messages take no parameter.
        unsafe { self.transform.ProcessMessage(m, 0) }.map_err(|e| err("IMFTransform::ProcessMessage", e))
    }
}

/// The system's synchronous decoder transforms for a codec, best first.
fn decoder_transforms(codec: NalCodec) -> std::result::Result<Vec<IMFActivate>, String> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: match codec {
            NalCodec::H264 => MFVideoFormat_H264,
            NalCodec::Hevc => MFVideoFormat_HEVC,
        },
    };
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: `input` outlives the call; `list` and `count` are valid out pointers. On success
    // `list` is a CoTaskMemAlloc'ed array of `count` activation objects that we own.
    unsafe { MFTEnumEx(MFT_CATEGORY_VIDEO_DECODER, MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER, Some(&input), None, &mut list, &mut count) }
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

/// Give the transform the device manager and the stream's input and output formats.
fn configure(transform: &IMFTransform, manager: &IMFDXGIDeviceManager, info: &NalStreamInfo, layout: &Layout) -> std::result::Result<(), String> {
    // SAFETY: a valid transform.
    let attributes = unsafe { transform.GetAttributes() }.map_err(|e| err("IMFTransform::GetAttributes", e))?;
    // SAFETY: a valid attribute store and a pointer to a constant GUID.
    let aware = unsafe { attributes.GetUINT32(&MF_SA_D3D11_AWARE) }.unwrap_or(0) != 0;
    if !aware {
        return Err("the decoder transform is not Direct3D 11 aware".into());
    }
    // SAFETY: the message's parameter is the device manager's interface pointer, alive for the
    // call; the transform takes its own reference to it.
    unsafe { transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize) }.map_err(|e| err("MFT_MESSAGE_SET_D3D_MANAGER", e))?;

    // SAFETY: interface out.
    let input: IMFMediaType = unsafe { MFCreateMediaType() }.map_err(|e| err("MFCreateMediaType", e))?;
    let (_, _, w, h) = info.crop;
    let subtype = match info.codec {
        NalCodec::H264 => MFVideoFormat_H264,
        NalCodec::Hevc => MFVideoFormat_HEVC,
    };
    // SAFETY: valid interfaces; the GUID pointers are to constants and a local that outlive
    // the calls (the media type copies the values).
    unsafe {
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| err("SetGUID", e))?;
        input.SetGUID(&MF_MT_SUBTYPE, &subtype).map_err(|e| err("SetGUID", e))?;
        input.SetUINT64(&MF_MT_FRAME_SIZE, (u64::from(w) << 32) | u64::from(h)).map_err(|e| err("SetUINT64", e))?;
        input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(|e| err("SetUINT32", e))?;
        transform.SetInputType(0, &input, 0).map_err(|e| err("IMFTransform::SetInputType", e))?;
    }
    set_output_type(transform, layout)?;
    // SAFETY: a valid transform; stream 0 is the decoder's only output.
    let stream = unsafe { transform.GetOutputStreamInfo(0) }.map_err(|e| err("IMFTransform::GetOutputStreamInfo", e))?;
    if stream.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 == 0 {
        return Err("the decoder transform does not decode on the Direct3D device".into());
    }
    // SAFETY: a valid transform; these messages take no parameter.
    unsafe {
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).map_err(|e| err("MFT_MESSAGE_NOTIFY_BEGIN_STREAMING", e))?;
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).map_err(|e| err("MFT_MESSAGE_NOTIFY_START_OF_STREAM", e))?;
    }
    Ok(())
}

/// Choose the transform's output type with the layout's pixel format.
fn set_output_type(transform: &IMFTransform, layout: &Layout) -> std::result::Result<(), String> {
    for i in 0..64 {
        // SAFETY: a valid transform; the index ends the loop with MF_E_NO_MORE_TYPES.
        let Ok(ty) = (unsafe { transform.GetOutputAvailableType(0, i) }) else { break };
        // SAFETY: a valid media type and a pointer to a constant GUID.
        if unsafe { ty.GetGUID(&MF_MT_SUBTYPE) }.is_ok_and(|g| g == layout.subtype) {
            // SAFETY: valid interfaces.
            return unsafe { transform.SetOutputType(0, &ty, 0) }.map_err(|e| err("IMFTransform::SetOutputType", e));
        }
    }
    Err("the decoder transform does not output the stream's pixel format".into())
}

/// Unlocks a 2-D buffer when dropped.
struct Locked<'a>(&'a IMF2DBuffer2);

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        // SAFETY: `copy_out` locked the buffer, which is still alive (borrowed).
        let _ = unsafe { self.0.Unlock2D() };
    }
}

/// Copy a decoded NV12 / P010 Direct3D picture into a planar frame (deinterleaving chroma,
/// cropping to the output rectangle, shifting 10-bit samples down from the high bits).
fn copy_out(sample: &IMFSample, l: &Layout) -> std::result::Result<VideoFrame, String> {
    // SAFETY: a valid sample.
    let buffer = unsafe { sample.GetBufferByIndex(0) }.map_err(|e| err("IMFSample::GetBufferByIndex", e))?;
    // A picture that is not a Direct3D texture was decoded in software by the transform.
    let dxgi: IMFDXGIBuffer = buffer.cast().map_err(|_| "the decoder transform decoded in software".to_string())?;
    let mut texture: Option<ID3D11Texture2D> = None;
    // SAFETY: asks for the buffer's texture as the interface whose IID is passed; `texture` is a
    // valid out pointer of that interface type.
    unsafe { dxgi.GetResource(&ID3D11Texture2D::IID, &mut texture as *mut Option<ID3D11Texture2D> as *mut *mut std::ffi::c_void) }
        .map_err(|e| err("IMFDXGIBuffer::GetResource", e))?;
    let texture = texture.ok_or("decoded picture without a texture")?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: a valid texture and out pointer.
    unsafe { texture.GetDesc(&mut desc) };
    let (cx, cy, w, h) = l.crop;
    if cx.saturating_add(w) > desc.Width || cy.saturating_add(h) > desc.Height {
        return Err(format!("decoded picture is {}x{}, the stream's is {w}x{h}", desc.Width, desc.Height));
    }

    let two_d: IMF2DBuffer2 = buffer.cast().map_err(|e| err("IMF2DBuffer2", e))?;
    let (mut scan0, mut start): (*mut u8, *mut u8) = (std::ptr::null_mut(), std::ptr::null_mut());
    let (mut pitch, mut len) = (0i32, 0u32);
    // SAFETY: valid out pointers; the buffer stays locked while `_lock` lives.
    unsafe { two_d.Lock2DSize(MF2DBuffer_LockFlags_Read, &mut scan0, &mut pitch, &mut start, &mut len) }.map_err(|e| err("IMF2DBuffer2::Lock2DSize", e))?;
    let _lock = Locked(&two_d);
    let pitch = usize::try_from(pitch).ok().filter(|p| *p > 0).ok_or("decoded picture with a bottom-up or empty layout")?;
    if start.is_null() || scan0.is_null() || (scan0 as usize) < start as usize {
        return Err("decoded picture is not mapped".into());
    }
    let first = scan0 as usize - start as usize;
    // SAFETY: Media Foundation mapped `len` readable bytes at `start` until the unlock, and the
    // slice does not outlive `_lock` (it is dropped at the end of this function, after `data`).
    let all = unsafe { std::slice::from_raw_parts(start as *const u8, len as usize) };
    let data = all.get(first..).ok_or("decoded picture layout")?;

    // NV12 / P010: `desc.Height` rows of luma, then `desc.Height / 2` rows of interleaved CbCr,
    // every row `pitch` bytes apart.
    let (w, h, cx, cy) = (w as usize, h as usize, cx as usize, cy as usize);
    let bps = if l.bits > 8 { 2 } else { 1 };
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let (cox, coy) = (cx / 2, cy / 2);
    let chroma_at = pitch.checked_mul(desc.Height as usize).ok_or("decoded picture size")?;
    let row = |base: usize, y: usize, x: usize, n: usize| -> std::result::Result<&[u8], String> {
        let at = y.checked_mul(pitch).and_then(|o| o.checked_add(base)).and_then(|o| o.checked_add(x)).ok_or("decoded picture size")?;
        data.get(at..at.saturating_add(n)).ok_or_else(|| "decoded picture row out of range".to_string())
    };
    if pitch < (cx + w) * bps || pitch < (cox + cw) * 2 * bps {
        return Err("decoded picture rows are shorter than the picture".into());
    }
    let pixels = if bps == 1 {
        let mut yp = Vec::with_capacity(w * h);
        for y in 0..h {
            yp.extend_from_slice(row(0, cy + y, cx, w)?);
        }
        let (mut u, mut v) = (Vec::with_capacity(cw * ch), Vec::with_capacity(cw * ch));
        for y in 0..ch {
            for pair in row(chroma_at, coy + y, cox * 2, cw * 2)?.as_chunks::<2>().0 {
                u.push(pair[0]);
                v.push(pair[1]);
            }
        }
        PixelData::Yuv8 { planes: [Arc::new(yp), Arc::new(u), Arc::new(v)], chroma: Chroma::C420, alpha: None }
    } else {
        let shift = 16u32.saturating_sub(l.bits);
        let mut yp = Vec::with_capacity(w * h);
        for y in 0..h {
            yp.extend(row(0, cy + y, cx * 2, w * 2)?.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b) >> shift));
        }
        let (mut u, mut v) = (Vec::with_capacity(cw * ch), Vec::with_capacity(cw * ch));
        for y in 0..ch {
            for q in row(chroma_at, coy + y, cox * 4, cw * 4)?.as_chunks::<4>().0 {
                u.push(u16::from_le_bytes([q[0], q[1]]) >> shift);
                v.push(u16::from_le_bytes([q[2], q[3]]) >> shift);
            }
        }
        PixelData::Yuv16 { planes: [Arc::new(yp), Arc::new(u), Arc::new(v)], chroma: Chroma::C420, bits: l.bits, alpha: None }
    };
    Ok(VideoFrame { width: w as u32, height: h as u32, data: pixels, color: l.color, par: l.par, pts: filmcraft_time::Tick::ZERO })
}

/// The Media Foundation decoder for one `avcC` / `hvcC` stream.
pub struct MfDecoder {
    info: NalStreamInfo,
    layout: Layout,
    /// `None` only after the transform refused to be flushed: a new one is made for the next sample.
    session: Option<Session>,
    /// Presentation times of access units fed whose picture has not come out yet.
    in_flight: Vec<i64>,
    /// The next access unit starts a run: the parameter sets go in front of it.
    first: bool,
    /// The run started at a CRA / BLA picture: its RASL pictures are not decodable and are left
    /// out (as the software decoder does).
    skip_rasl: bool,
    /// Test hook: fail every decode once this many samples were fed.
    fail_after: Option<u64>,
    fed: u64,
    name: &'static str,
}

impl MfDecoder {
    /// A decoder for the stream, or why Media Foundation does not take it (unsupported format, no
    /// Direct3D 11 video device, no hardware-capable decoder transform).
    pub fn new(info: NalStreamInfo) -> std::result::Result<Self, String> {
        let layout = layout(&info)?;
        // Set the transform up now so unsupported streams are declined up front.
        let session = Session::new(&info, &layout)?;
        let name = match info.codec {
            NalCodec::H264 => "Media Foundation H.264",
            NalCodec::Hevc => "Media Foundation HEVC",
        };
        Ok(Self { info, layout, session: Some(session), in_flight: Vec::new(), first: true, skip_rasl: false, fail_after: None, fed: 0, name })
    }

    /// Test hook: from the `n`-th sample on, every decode fails as a lost device would.
    pub fn fail_after(&mut self, n: u64) {
        self.fail_after = Some(n);
    }

    /// The access unit as Annex B, behind the parameter sets when it starts a run.
    fn annex_b(&self, sample: &[u8], with_parameter_sets: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(sample.len().saturating_add(64));
        let sets = if with_parameter_sets { self.info.parameter_sets.as_slice() } else { &[] };
        for nal in sets.iter().map(Vec::as_slice).chain(self.info.nals(sample)) {
            out.extend_from_slice(&START_CODE);
            out.extend_from_slice(nal);
        }
        out
    }

    /// Pictures the transform has ready, in presentation order.
    fn drain(&mut self) -> std::result::Result<Vec<DecodedFrame>, String> {
        let mut out = Vec::new();
        let Some(session) = self.session.as_ref() else { return Ok(out) };
        while out.len() < MAX_OUTPUTS {
            let Some((pts, frame)) = session.output(&self.layout)? else { break };
            // every picture is one we fed, under the time we fed it with
            let Some(i) = self.in_flight.iter().position(|&p| p == pts) else {
                return Err(format!("the decoder returned a picture with an unknown time ({pts})"));
            };
            self.in_flight.swap_remove(i);
            out.push(DecodedFrame { pts, frame, draft: false });
        }
        Ok(out)
    }
}

impl VideoDecoder for MfDecoder {
    fn decode(&mut self, sample: &[u8], pts: i64) -> Result<Vec<DecodedFrame>> {
        self.fed += 1;
        if self.fail_after.is_some_and(|n| self.fed > n) {
            return Err(CodecError::Decode("Media Foundation device lost (test hook)".into()));
        }
        if self.info.codec == NalCodec::Hevc {
            let types = self.info.nal_types(sample);
            let irap = types.iter().any(|t| (16..=23).contains(t));
            if irap {
                // CRA / BLA starting a run: its RASL pictures reference pictures we never decoded.
                self.skip_rasl = self.first && types.iter().any(|t| (16..=21).contains(t) && !(19..=20).contains(t));
            } else if self.skip_rasl && types.iter().any(|t| matches!(t, 8 | 9)) {
                self.first = false;
                return Ok(Vec::new());
            }
        }
        if self.session.is_none() {
            self.session = Some(Session::new(&self.info, &self.layout).map_err(CodecError::Decode)?);
        }
        let data = self.annex_b(sample, self.first);
        self.first = false;
        let mut out = Vec::new();
        // The transform may want pictures taken out before it accepts the next access unit.
        let mut accepted = false;
        for _ in 0..4 {
            let Some(session) = self.session.as_ref() else {
                return Err(CodecError::Decode("no Media Foundation decoder".into()));
            };
            accepted = session.input(&data, pts).map_err(CodecError::Decode)?;
            if accepted {
                break;
            }
            let ready = self.drain().map_err(CodecError::Decode)?;
            if ready.is_empty() {
                break;
            }
            out.extend(ready);
        }
        if !accepted {
            return Err(CodecError::Decode("the Media Foundation decoder accepts no input".into()));
        }
        self.in_flight.push(pts);
        out.extend(self.drain().map_err(CodecError::Decode)?);
        Ok(out)
    }

    fn flush(&mut self) -> Vec<DecodedFrame> {
        let mut out = Vec::new();
        if let Some(s) = self.session.as_ref() {
            match s.message(MFT_MESSAGE_COMMAND_DRAIN) {
                // (bounded by the pictures still inside the transform)
                Ok(()) => {
                    while !self.in_flight.is_empty() {
                        match self.drain() {
                            Ok(v) if v.is_empty() => break,
                            Ok(v) => out.extend(v),
                            Err(e) => {
                                log::warn!("{}: {e} while flushing", self.name);
                                break;
                            }
                        }
                    }
                }
                Err(e) => log::warn!("{}: {e} while flushing", self.name),
            }
        }
        // The drained transform is kept for the next run (setting one up costs far more than a
        // picture): flushed, it takes a new run that starts with the parameter sets.
        if self.session.as_ref().is_some_and(|s| s.message(MFT_MESSAGE_COMMAND_FLUSH).is_err()) {
            self.session = None;
        }
        self.in_flight.clear();
        self.first = true;
        out
    }

    fn reset(&mut self) {
        // No reference pictures survive a seek: flush the transform and start a new run, with the
        // parameter sets in front of the next access unit.
        if let Some(s) = self.session.as_ref()
            && s.message(MFT_MESSAGE_COMMAND_FLUSH).is_err()
        {
            self.session = None;
        }
        self.in_flight.clear();
        self.skip_rasl = false;
        self.first = true;
    }

    fn name(&self) -> &str {
        self.name
    }

    fn is_random_access(&self, sample: &[u8]) -> Option<bool> {
        self.info.is_random_access(sample)
    }

    fn is_disposable(&self, sample: &[u8]) -> bool {
        self.info.is_disposable(sample)
    }
}
