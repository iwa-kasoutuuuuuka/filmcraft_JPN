# filmcraft-platform

OS media integration for FilmCraft (layer L5): hardware video decoding through the operating
system's codecs, behind `filmcraft_codecs::VideoDecoder`. It holds OS media FFI and nothing else.
It is the one crate of the workspace allowed to contain `unsafe`, under the rules of
[ADR 0001](../../docs/adr/0001-platform-ffi.md) and [AGENTS.md](../../AGENTS.md) §0.3.

```rust
// at startup (the desktop app, filmcraft-cli, the bench)
let availability = filmcraft_platform::register(); // Available("VideoToolbox") on macOS, Available("Media Foundation") on Windows
```

## What it does

- **macOS: VideoToolbox H.264 (`avcC`) and HEVC (`hvcC`)**, 8- and 10-bit, 4:2:0 and 4:2:2
  (`videotoolbox.rs`). The session is created from the sample entry's parameter sets with a
  hardware decoder *required*; samples go in as `CMSampleBuffer`s with asynchronous decompression
  (two access units in flight); the output callback copies each NV12 / P010-style biplanar
  `CVPixelBuffer` into planar `Yuv8` / `Yuv16` (chroma deinterleaved, 10-bit samples shifted down
  from the high bits, cropped to the conformance window when the buffer is the coded size). A
  reorder buffer of the stream's own depth (`max_num_reorder_frames` /
  `sps_max_num_reorder_pics`) restores presentation order; a run starting at an HEVC CRA leaves
  out its RASL pictures, as our decoder does. Each seek (`reset`) starts a fresh session.
- **Windows: Media Foundation H.264 (`avcC`) and HEVC (`hvcC`) on Direct3D 11 (DXVA)**, 4:2:0,
  8-bit H.264 and 8- / 10-bit HEVC (`mediafoundation.rs`). The decoder is the system's decoder
  transform (`MFTEnumEx`; HEVC needs the HEVC Video Extensions) given a DXGI device manager, which
  is what makes it decode on the GPU. One Direct3D device serves every decoder of the process, and
  a transform is kept across seeks and flushes (setting one up costs far more than a picture).
  Samples go in as Annex B access units, the sample entry's parameter sets in front of the first
  one of a run; pictures come back in presentation order as NV12 / P010 textures and are read back
  (`IMF2DBuffer2::Lock2DSize`) into planar `Yuv8` / `Yuv16`, cropped to the output rectangle. A
  transform that accepts the stream but returns system-memory pictures is decoding in software:
  that is reported as a failure, so our own decoder takes over.
- **`locale`** (`locale.rs`): the operating system's display language as a BCP 47 tag, for the UI's
  "Auto" language (`GetUserDefaultUILanguage` on Windows, the POSIX locale variables elsewhere;
  None for a macOS app started from the Finder).
- **Other systems:** `register()` does nothing and returns `Availability::Unavailable`.
- **`HybridDecoder`** (`hybrid.rs`, safe code): the hardware decoder plus the means to build our
  software decoder for the same `SampleEntry` (`filmcraft_codecs::software_video_decoder`). On a
  mid-stream failure (decode error, invalidated session, changed in-band parameter sets) it replays
  the samples since the last restart point (IDR / IRAP; for an HEVC CRA the one before, so its RASL
  pictures decode) through the software decoder, drops pictures already returned, keeps the ones
  the hardware had decoded but not returned, and stays in software for that instance. The replay
  log is bounded (600 samples / 256 MB); beyond it one error is returned and the next seek restarts
  in software. Streams our decoders cannot decode (HEVC 4:2:2) have no fallback: the error stands.

## Guarantees

- **Never undecodable:** the factory declines (returns `None`, so the software decoder is used)
  when Settings ▸ Playback ▸ Hardware decoding is Off, for formats it does not take (field-coded
  H.264, bit depths other than 8 / 10, 4:4:4 or monochrome, luma / chroma depth mismatch, larger
  than 8192×8192; on Windows also 4:2:2 and 10-bit H.264) and when the OS cannot decode the
  stream in hardware (VideoToolbox cannot create a hardware session; no Direct3D 11 video device,
  no decoder transform for the codec, or one that is not Direct3D 11 aware).
- **Interchangeable:** colour, pixel aspect, pts, presentation order, `is_random_access` and
  `is_disposable` come from the software decoders' own helpers (`filmcraft_codecs::hw`,
  `video::vui_color`, `sar_par`).
- **Never crash:** no `unwrap` / `expect` / `panic!` outside tests; the output callback runs under
  `catch_unwind`; every `unsafe` block has a `// SAFETY:` comment; the public API is safe.
- **Counted:** `perf.stats` `decode.hardware` (frames, software frames, sessions, declined,
  fallbacks; `filmcraft_codecs::hw::hw_stats`).

## Tests

| test | what |
|---|---|
| `tests/videotoolbox.rs` (macOS) | H.264 High, HEVC Main (open GOP: CRA + RASL) and HEVC Main 10, 640×360 (coded 368: cropping) with B-frames: every picture **bit-exact** with our software decoder, same pts order, count, colour and aspect, also after `reset` + reseek to every later sync sample, mid-stream `flush`, and a full pass after resets; forced mid-stream failures (`VtDecoder::fail_after`) at five points continue with the software decoder's exact output; seeded mutation of samples and parameter sets (bit flips, truncation, corrupt length prefixes) never panics or hangs; HEVC 4:2:2 10-bit is bit-exact with ffmpeg's decode |
| `tests/mediafoundation.rs` (Windows) | H.264 High with B-frames from our own encoder (no external tool), 640×360 (coded 368: cropping), 1280×720 and 322×242, several GOPs: every picture **bit-exact** with our software decoder, same pts order and count, also after `reset` + reseek to every later key picture; through the factory; forced mid-stream failures (`MfDecoder::fail_after`) continue in software with the same pictures; damaged samples and parameter sets never crash or hang; unsupported formats are declined. Skips on a machine without a hardware decoder. `throughput` (ignored) prints fps and CPU per frame for both decoders. |
| `tests/fallback.rs` (every OS) | `HybridDecoder` with a stand-in hardware decoder failing after N samples (every sync sample ± a few, first / last sample, after a seek): output identical to the software decoder; in-band parameter sets identical to the sample entry's stay in hardware, different ones switch to software |
| `tests/setting.rs` | Hardware decoding Off gives the software decoder through `make_video_decoder` and the media stack (no hardware frames); Auto gives VideoToolbox where available |

Fixtures are made with ffmpeg into `target/fixtures/platform/` (generator only, never linked);
tests skip without ffmpeg or without a hardware decoder.

## Performance

M4 Pro, load 150–190 (`cargo xtask bench --hw off|auto`): CPU per decoded frame H.264 2160p
119 → 4.2 ms, HEVC 2160p 86 → 3.7 ms; decode 35 → 107 fps and 49 → 217 fps; 4K H.264 and HEVC
playback with no dropped frames at Full, 1/2 and 1/4. Details in
[docs/performance.md](../../docs/performance.md).

Windows 11, NVIDIA GPU, `cargo test -p filmcraft-platform --test mediafoundation throughput --
--ignored --nocapture` (dev profile, 48-picture H.264 High streams from our encoder, decode and
flush four times; measured once, machine otherwise idle): 1080p 103 fps at 1.9 ms CPU per frame
(software 204 fps at 39 ms), 2160p 74 fps at 5.7 ms (software 67 fps at 154 ms). So on Windows
hardware decoding costs about 20–27× less CPU; at 1080p the all-core software decoder is still the
faster of the two in pictures per second, at 2160p the hardware is. Not yet measured: playback in
the app, HEVC, other GPU vendors.

## Not yet

Zero-copy upload of decoded pictures (`CVPixelBuffer`s, Direct3D textures) into wgpu textures;
hardware encoding; VA-API (Linux) decoders; field-coded H.264; HEVC on Windows is implemented but
untested (no HEVC stream without an external encoder, and it needs the HEVC Video Extensions).
