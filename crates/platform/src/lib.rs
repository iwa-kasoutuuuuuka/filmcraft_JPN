//! OS media integration (layer L5): hardware video decoding through the operating system's codecs.
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
//! This is the one crate allowed to use `unsafe` (OS FFI), and only in its FFI modules
//! (docs/adr/0001-platform-ffi.md).

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod hybrid;
pub mod locale;
#[cfg(windows)]
#[allow(unsafe_code)]
pub mod mediafoundation;
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
        filmcraft_codecs::register_video_decoder(mediafoundation_factory);
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

/// Media Foundation hands every picture back through a synchronous GPU read-back, which on a
/// many-core CPU is slower than FilmCraft's own frame-threaded H.264 decoder: on a 16-core
/// Ryzen 9 (32 threads), H.264 playback at 1080p dropped 17–95 % of the frames through Media
/// Foundation and none in software, and at 2160p 47–92 % against none (docs/performance.md).
/// From this many threads Auto keeps H.264 in software; below it, or with the setting On, the
/// hardware decodes it.
#[cfg(windows)]
const SOFTWARE_H264_THREADS: usize = 12;

#[cfg(windows)]
fn software_h264_is_faster() -> bool {
    std::thread::available_parallelism().is_ok_and(|n| n.get() >= SOFTWARE_H264_THREADS)
}

/// The Media Foundation factory: a [`HybridDecoder`] around [`mediafoundation::MfDecoder`] for
/// `avcC` / `hvcC` streams the system decodes on the GPU, `None` otherwise.
#[cfg(windows)]
pub fn mediafoundation_factory(entry: &filmcraft_isobmff::SampleEntry) -> Option<filmcraft_codecs::Result<Box<dyn filmcraft_codecs::VideoDecoder>>> {
    if !filmcraft_codecs::hw::hardware_decoding() {
        return None;
    }
    let info = filmcraft_codecs::hw::NalStreamInfo::from_entry(entry)?.ok()?;
    if info.codec == filmcraft_codecs::hw::NalCodec::H264
        && filmcraft_codecs::hw::hardware_mode() == filmcraft_codecs::hw::HwMode::Auto
        && software_h264_is_faster()
    {
        log::info!("H.264 video decoded in software: faster than Media Foundation on this CPU");
        return None;
    }
    match mediafoundation::MfDecoder::new(info.clone()) {
        Ok(mf) => Some(Ok(Box::new(HybridDecoder::new(Box::new(mf), entry.clone(), info)))),
        Err(why) => {
            log::info!("hardware decoding declined for {} video: {why}", entry.codec.name());
            filmcraft_codecs::hw::note_hw_declined_because(entry.codec.name(), &why);
            None
        }
    }
}
