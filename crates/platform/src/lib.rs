//! OS media integration (layer L5): hardware video decoding and encoding through the operating
//! system's codecs.
//!
//! [`register`] puts the platform's hardware decoder factory in front of FilmCraft's own decoders
//! (`filmcraft_codecs::register_video_decoder`), for H.264 (`avcC`) and HEVC (`hvcC`) streams:
//!
//! - macOS: VideoToolbox, 8- and 10-bit, 4:2:0 and 4:2:2;
//! - Windows: Media Foundation on Direct3D 11 (DXVA), 4:2:0, 8-bit H.264 and 8- / 10-bit HEVC
//!   (HEVC when the system has the HEVC Video Extensions);
//! - other systems: registration does nothing and reports [`Availability::Unavailable`].
//!
//! Hardware decoding never makes a file undecodable:
//!
//! - the factory declines (falls through to the software decoder) when Settings ▸ Playback ▸
//!   Hardware decoding is Off (`filmcraft_codecs::hw::set_hardware_decoding`), when the stream's
//!   format is one the hardware path does not take, or when the OS cannot create a hardware
//!   session for it (profile, size, no hardware decoder);
//! - a decoder that fails mid-stream switches to the software decoder transparently
//!   ([`HybridDecoder`]) and logs it.
//!
//! Pictures are the software decoder's: same planes (bit-exact on the parity fixtures), colour,
//! pixel aspect, pts and presentation order, so the two are interchangeable.
//!
//! On Windows, [`register`] also puts Media Foundation hardware encoders (the GPU's media engine)
//! in front of ours for MP4 / MOV exports (`filmcraft_export::register_encoder`; Export ▸
//! Performance, `filmcraft_export::hw`): H.264, and HEVC, which has no encoder of our own. They
//! decline two-pass, MXF and non-square-pixel exports and HDR H.264 (HDR HEVC is Main 10 from
//! P010), and an H.264 export whose hardware encoder fails on its first frames finishes with our
//! encoder.
//!
//! This is the one crate allowed to use `unsafe` (OS FFI), and only in its FFI modules
//! (docs/adr/0001-platform-ffi.md).

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod hybrid;
pub mod locale;
#[cfg(windows)]
#[allow(unsafe_code)]
pub mod mediafoundation;
#[cfg(windows)]
#[allow(unsafe_code)]
pub mod mfencode;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub mod videotoolbox;

pub use hybrid::HybridDecoder;

/// What [`register`] made available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// A hardware decoder factory was registered (its name).
    Available(&'static str),
    /// Nothing to register on this system (why).
    Unavailable(&'static str),
}

/// Register the platform's hardware video decoders (call once at startup; repeated calls are
/// harmless). Streams they do not take, and every stream while hardware decoding is Off, keep
/// using FilmCraft's own decoders.
pub fn register() -> Availability {
    #[cfg(target_os = "macos")]
    {
        filmcraft_codecs::register_video_decoder(videotoolbox_factory);
        Availability::Available("VideoToolbox")
    }
    #[cfg(windows)]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            filmcraft_codecs::register_video_decoder(mediafoundation_factory);
            filmcraft_export::register_encoder(mediafoundation_encoder_factory);
        });
        Availability::Available("Media Foundation")
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Availability::Unavailable("no hardware video decoder for this system yet")
    }
}

/// Whether this system's hardware decoder takes the stream of `entry` (whatever the Hardware
/// decoding setting says): diagnostics and tests.
pub fn hardware_decoder_for(entry: &filmcraft_isobmff::SampleEntry) -> bool {
    #[cfg(target_os = "macos")]
    {
        filmcraft_codecs::hw::NalStreamInfo::from_entry(entry).and_then(|r| r.ok()).is_some_and(|info| videotoolbox::VtDecoder::new(info).is_ok())
    }
    #[cfg(windows)]
    {
        filmcraft_codecs::hw::NalStreamInfo::from_entry(entry).and_then(|r| r.ok()).is_some_and(|info| mediafoundation::MfDecoder::new(info).is_ok())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = entry;
        false
    }
}

/// The VideoToolbox factory: a [`HybridDecoder`] around [`videotoolbox::VtDecoder`] for `avcC` /
/// `hvcC` streams VideoToolbox can decode in hardware, `None` otherwise.
#[cfg(target_os = "macos")]
pub fn videotoolbox_factory(entry: &filmcraft_isobmff::SampleEntry) -> Option<filmcraft_codecs::Result<Box<dyn filmcraft_codecs::VideoDecoder>>> {
    if !filmcraft_codecs::hw::hardware_decoding() {
        return None;
    }
    let info = filmcraft_codecs::hw::NalStreamInfo::from_entry(entry)?.ok()?;
    match videotoolbox::VtDecoder::new(info.clone()) {
        Ok(vt) => Some(Ok(Box::new(HybridDecoder::new(Box::new(vt), entry.clone(), info)))),
        Err(why) => {
            log::info!("hardware decoding declined for {} video: {why}", entry.codec.name());
            filmcraft_codecs::hw::note_hw_declined_because(entry.codec.name(), &why);
            None
        }
    }
}

/// The Media Foundation factory: a [`HybridDecoder`] around [`mediafoundation::MfDecoder`] for
/// `avcC` / `hvcC` streams the system decodes on the GPU, `None` otherwise.
#[cfg(windows)]
pub fn mediafoundation_factory(entry: &filmcraft_isobmff::SampleEntry) -> Option<filmcraft_codecs::Result<Box<dyn filmcraft_codecs::VideoDecoder>>> {
    if !filmcraft_codecs::hw::hardware_decoding() {
        return None;
    }
    let info = filmcraft_codecs::hw::NalStreamInfo::from_entry(entry)?.ok()?;
    match mediafoundation::MfDecoder::new(info.clone()) {
        Ok(mf) => Some(Ok(Box::new(HybridDecoder::new(Box::new(mf), entry.clone(), info)))),
        Err(why) => {
            log::info!("hardware decoding declined for {} video: {why}", entry.codec.name());
            filmcraft_codecs::hw::note_hw_declined_because(entry.codec.name(), &why);
            None
        }
    }
}

/// The Media Foundation hardware H.264 / HEVC encoder for MP4 / MOV exports, `None` (our encoder;
/// for HEVC: none) when hardware encoding is off, for settings it does not take, or without a
/// hardware encoder. HEVC ignores the export's Performance choice: there is no software HEVC.
#[cfg(windows)]
pub fn mediafoundation_encoder_factory(
    format: filmcraft_export::Format,
    w: u32,
    h: u32,
    rate: filmcraft_time::FrameRate,
    s: &filmcraft_export::ExportSettings,
) -> Option<filmcraft_export::Result<Box<dyn filmcraft_export::VideoEncoder>>> {
    let codec = match format {
        filmcraft_export::Format::H264 if s.hardware_encoding => mfencode::Codec::H264,
        filmcraft_export::Format::Hevc => mfencode::Codec::Hevc,
        _ => return None,
    };
    if !filmcraft_export::hw::hardware_encoding()
        || s.format.is_mxf()
        // HDR: HEVC Main 10 only (HDR H.264 stays with our encoder)
        || (s.signal.is_hdr() && codec != mfencode::Codec::Hevc)
        || !matches!(s.h264_pass, filmcraft_export::H264Pass::Single)
        || s.pixel_aspect.is_some_and(|(n, d)| n != d)
    {
        return None;
    }
    match mfencode::MfEncoder::new(codec, w, h, rate, s) {
        Ok(e) => {
            log::info!("hardware {codec:?} encoding with {}", e.name());
            filmcraft_export::hw::note_hw_encode_session();
            Some(Ok(Box::new(e)))
        }
        Err(why) => {
            log::info!("hardware {codec:?} encoding declined: {why}");
            None
        }
    }
}
