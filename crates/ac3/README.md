# filmcraft-ac3

Clean-room AC-3 (Dolby Digital) and E-AC-3 (Dolby Digital Plus) audio decoder in pure Rust. Layer L0: depends only on
`thiserror`. Builds for `wasm32-unknown-unknown`; no `unsafe`. No GPL/LGPL code (FFmpeg,
liba52) was consulted; FFmpeg is used only as an external fixture generator and test oracle.

## Specification

ATSC A/52:2012 "Digital Audio Compression (AC-3, E-AC-3) Standard" (17 December 2012), the
publicly available standard: §5 bit stream syntax and semantics, §6 decoding overview, §7.1
exponents, §7.2 parametric bit allocation (Tables 7.6-7.16 extracted mechanically from the
standard's text), §7.3 mantissas and dither, §7.4 coupling, §7.5 rematrixing, §7.7.1 dynamic
range, §7.9 transforms and block switching. The window is computed as the Kaiser-Bessel derived
window (α = 5) and checked against Table 7.33 to 6·10⁻⁶. E-AC-3: Annex E (E.1 syntax and
semantics, E.2 tables, E.3 decoding: spectral extension). The frame exponent strategy table
(E2.10) is generated from its structure (every split of six blocks into runs, a run's strategy
following from its length) and spot-checked against the printed table.

## API

```rust
let mut dec = filmcraft_ac3::Decoder::new();
let h = filmcraft_ac3::parse_header(frame)?;   // rate, channels, frame size
let out = dec.decode(frame)?;                  // h.samples per channel, WAV channel order
dec.reset();                                    // before decoding from another position
```

The same decoder takes AC-3 and E-AC-3 syncframes (by bsid). `Header::samples` is 1536 for
AC-3 and 256 × 1/2/3/6 blocks for E-AC-3; a dependent E-AC-3 substream or a second program
(`!Header::decodes_audio()`) decodes to no channels.

## Decoder

- Every audio coding mode (1+1, 1/0, 2/0, 3/0, 2/1, 3/1, 2/2, 3/2) with or without LFE; 32 /
  44.1 / 48 kHz and every frame size; bsid ≤ 8 (9 and 10, the half / quarter-rate variants, are
  accepted).
- D15 / D25 / D45 exponents with reuse across blocks; the parametric bit allocation in the
  standard's fixed-point steps, delta bit allocation, the all-zero SNR offset special case;
  grouped 3-, 5- and 11-level mantissas (groups shared across exponent sets), 7- and 15-level
  and asymmetric mantissas up to 16 bits.
- Channel coupling (coupling bands, master coordinates, phase flags in 2/0), rematrixing (all
  four banding cases), dynamic range control (`dynrng`, `dynrng2`; applied at full scale), dither
  for zero-bit mantissas (uniform ±0.707, §7.3.4).
- 512-sample IMDCT and the pair of 256-sample IMDCTs for block-switched blocks, with radix-2
  inverse FFTs, KBD window and overlap-add.
- Output: f32 at full scale ±1; channels reordered from the coded order (L C R Ls Rs LFE) to the
  WAV / SMPTE order (L R C LFE Ls Rs). No downmix (the mixer handles layouts).
- Dither is reseeded per syncframe from the frame's CRC words, so a frame decodes to the same
  samples however it was reached (seeking, render caches).
- Errors (bad exponents, reserved codes, blocks running past the frame) return `Err`; garbage
  never panics.

### E-AC-3 (bsid 11-16)

- Syncframe header: strmtyp, substreamid, frmsiz, fscod and fscod2 (24 / 22.05 / 16 kHz),
  1, 2, 3 or 6 audio blocks per syncframe; the whole bsi (mixing, informational, converter and
  additional metadata parsed and skipped) and audfrm (per-block and frame exponent strategies,
  SNR offset strategies 0-2, block switch / dither / bit allocation / fast gain / delta bit
  allocation / skip field enables, converter exponent strategies, block start information).
- Audio blocks: spectral extension (strategy, default and coded banding, coordinates, blending
  factors, translation of the base band with wrap-around, noise blending, E.3.6), channel
  coupling with the E-AC-3 rules (default banding structure, first coordinates and leak values
  always sent, coupling ending where spectral extension begins), rematrixing (including its
  spectral extension banding), the bit allocation defaults (`bamode`, fast gain code 4), then the
  AC-3 exponents, bit allocation, mantissas, dither and transforms.
- Substreams: independent substream 0 is decoded (the 5.1 core of a 7.1 program). Dependent
  substreams (strmtyp 1) and further independent substreams decode to no audio; containers
  (TS) give their units no duration, MP4 / Matroska report the core's channel count.
- Refused with `Error::Unsupported` (the syncframe fails; players substitute silence): the
  adaptive hybrid transform (AHT, E.3.4, used by some very low bit rate encoders) and enhanced
  coupling (E.3.5, very rare).
- Not applied (audio decodes without them): transient pre-noise processing (E.3.7, a time-domain
  pre-echo refinement) and spectral extension attenuation (a notch at translation borders); their
  parameters are parsed and skipped.

## Accuracy (vs FFmpeg as an external oracle)

AC-3 output is not bit-exact across decoders: zero-bit mantissas are filled with
decoder-specific dither. `tests/oracle.rs` decodes FFmpeg-encoded streams and compares with
FFmpeg's decode (worst channel SNR):

| Stream | SNR vs FFmpeg | SNR between two of our dither seeds |
|---|---|---|
| 2/0 48 kHz 192 kb/s (tones, chirp) | 66.1 dB | 76.0 dB |
| 2/0 44.1 kHz 96 kb/s with coupling and rematrixing | 84.5 dB | 81.7 dB |
| 1/0 32 kHz 96 kb/s | 94.1 dB | 95.7 dB |
| 3/2 + LFE 48 kHz 448 kb/s | 81.4 dB | 90.2 dB |
| 2/0 pink noise 128 kb/s (many zero-bit mantissas) | 27.7 dB | 27.9 dB |

The noise row shows the difference is dither: two of our own decodes with different dither
sequences differ as much as we differ from FFmpeg. In MPEG-2 TS (AVCHD-style `.mts`) and VOB the
maximum sample difference to FFmpeg is 2.3·10⁻³.

### E-AC-3

`tests/eac3_oracle.rs` (ffmpeg's E-AC-3 encoder: frame exponent strategies, coupling, the
default coupling banding, mixing / informational metadata) — worst channel SNR:

| Stream | SNR vs FFmpeg | SNR between two of our dither seeds |
|---|---|---|
| 2/0 48 kHz 192 kb/s (tones, chirp) | 66.1 dB | 75.7 dB |
| 2/0 44.1 kHz 96 kb/s with coupling | 83.8 dB | 85.7 dB |
| 1/0 32 kHz 64 kb/s | 89.9 dB | 92.4 dB |
| 3/2 + LFE 384 kb/s, coupling, downmix and informational metadata | 81.0 dB | 86.3 dB |
| 3/2 + LFE 640 kb/s, per-frame metadata | 72.5 dB | 77.0 dB |
| 2/0 + LFE 160 kb/s | 92.1 dB | 92.6 dB |
| 2/0 pink noise 128 kb/s | 28.7 dB | 28.9 dB |

In MP4 (`ec-3`), Matroska (`A_EAC3`) and MPEG-2 TS (stream type 0x87) through
`filmcraft-codecs` the decode is 71.6-100.7 dB from FFmpeg's (`codecs/tests/eac3_oracle.rs`;
Matroska aligned on a 16-sample lag: the 256-sample encoder priming is stored as a rounded −5 ms
timestamp).

## Gaps

- FFmpeg's AC-3 encoder never block-switches, so the 256-sample transforms are implemented from
  §7.9.4.2 but not checked against an oracle.
- No downmixing, no heavy compression (`compr`) or dialogue normalisation (both are metadata
  for playback systems), no CRC check (corrupt frames are caught by consistency checks).
- E-AC-3: ffmpeg's encoder writes six-block independent frames without spectral extension,
  AHT, block switching, per-block exponent strategies, SNR offset strategies 1-2, coded bit
  allocation parameters / fast gains or dependent substreams. Those paths are implemented from
  Annex E and covered by hostile-input tests but not checked against an oracle (spectral
  extension noise is random in any case, so only its level could be compared).
- E-AC-3 7.1 (a dependent substream) plays as its 5.1 core; AHT and enhanced coupling frames
  fail with `Unsupported`; no transient pre-noise processing or spectral extension attenuation.
