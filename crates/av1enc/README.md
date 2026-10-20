# filmcraft-av1enc

AV1 encoding for FilmCraft's export path (Export ▸ Format ▸ AV1, the "Match Source – AV1" and
"AV1 1080p Full HD" presets). Layer L0.

Unlike FilmCraft's other codecs this crate does not implement the format itself: it is a thin,
safe wrapper around [rav1e](https://github.com/xiph/rav1e) 0.8 (BSD-2-Clause, by the Xiph.Org
Foundation and contributors; AOMedia Patent License 1.0 for the AV1 format), used unmodified as a
crates.io dependency. No rav1e source is copied here.

- **Pure Rust.** rav1e is built with `default-features = false`: no `asm` feature (no nasm, no C
  compiler), no binaries, no `git2` build dependency. Feature `threads` (on by default) enables
  rav1e's rayon threading. Every crate in the dependency tree is MIT / Apache-2.0 / BSD-2-Clause /
  Unicode; none needs a C toolchain.
- **Never crash.** Settings are validated up front (16..=65535 per side, 0.01..=1000 fps, keyframe
  interval 1..=10000, non-zero bitrate and aspect ratio) and every call into rav1e runs under
  `catch_unwind`: a panic inside rav1e becomes `Error::Encoder` and the encoder refuses further
  work. Input planes are bounds-checked before they are copied.
- **Output.** One `Packet` per temporal unit, in presentation order, temporal delimiters removed
  (AV1-ISOBMFF §2.4 samples). `Encoder::av1c()` is the `AV1CodecConfigurationRecord` (§2.3) with
  the sequence header OBU as `configOBUs`; profile, level and tier are read from that OBU (AV1
  specification §5.5.1).
- **Speed.** Exports use the fastest preset (speed 10) and about one tile per 640×360 area so the
  tiles encode in parallel. Without rav1e's assembly the kernels are several times slower than a
  stock rav1e build.

Input is planar 8-bit 4:2:0 (limited or full range, BT.709 / BT.2020 colour description).
10-bit and HDR AV1 export are not wired yet.

Tests (`src/tests.rs`) decode the output with FilmCraft's own AV1 decoder (`filmcraft-av1`,
dev-dependency only) and check PSNR, keyframe placement and the `av1C`; hostile settings and
mutation-fuzzed OBU parsing must not panic. The export crate adds an MP4 round trip with
ffprobe as oracle.
