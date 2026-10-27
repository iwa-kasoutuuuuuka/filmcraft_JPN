# filmcraft-riff

A clean-room RIFF chunk walker and AVI demuxer, written from Microsoft's published AVI RIFF file
reference and the OpenDML AVI File Format Extensions 1.02. Layer L0: no dependencies beyond `std`,
no `unsafe`, builds for `wasm32-unknown-unknown`. No FFmpeg, libavformat or other AVI implementation
was read; ffmpeg and ffprobe are used only as external **test oracles**.

## API

```rust
use filmcraft_riff::{open, StreamKind};

let file = open(&bytes)?;                    // any ByteSource: a slice, a file, a web Blob…
for (i, s) in file.streams.iter().enumerate() {
    match s.kind {
        StreamKind::Video => println!("{:?} {} fps", s.video.as_ref().map(|b| b.compression), s.units_per_second()),
        StreamKind::Audio => println!("format 0x{:04x}", s.audio.as_ref().map_or(0, |w| w.format_tag)),
        _ => {}
    }
    let starts = s.chunk_starts();               // chunk times in the stream's scale / rate units
    let first = file.read_chunk(&bytes, i, 0)?;   // frame data, on demand
}
// any RIFF file: the chunks of a range (a LIST's children start after its 4-byte type)
let top = filmcraft_riff::chunks(&bytes, 0, bytes.len() as u64);
```

## What it reads

| Part | |
|---|---|
| Headers | `avih`; per stream `strh`, `strf` (`BITMAPINFOHEADER`, `WAVEFORMATEX` / `WAVEFORMATEXTENSIBLE`), `strn` |
| Index | OpenDML `indx` super index → `ix##` standard indexes (field indexes too), else `idx1` (offsets relative to `movi` or absolute), else a scan of the `movi` lists |
| Large files | `RIFF AVIX` extensions: through the OpenDML index, or scanned after `idx1`, which only covers the first RIFF |
| Key frames | from the index (`AVIIF_KEYFRAME`, bit 31 of an `ix##` size); a scan marks every chunk and says so (`keyframes_known`) |
| Timing | `dwRate / dwScale`; a chunk per frame for video and variable-size audio (VBR MP3), bytes over `dwSampleSize` for fixed-size samples (PCM) |
| Damage | sizes clamped to the file (a cut-off recording opens with what is there), `rec ` lists nested at most 3 deep, a standard index read once however often it is referenced, OpenDML index bytes and per-stream tables capped, zero rates read as 1 / 1 |

`filmcraft-codecs` maps the streams onto decoders (`AviSource`): Motion JPEG, H.264, HEVC and
uncompressed RGB / YUV video; PCM, MP3, MP2, AC-3 and AAC-LC audio.

## Tests

`cargo test -p filmcraft-riff`: hand-built files for every index path (`idx1` relative and absolute,
`rec ` lists, OpenDML standard indexes, a scan), truncation, hostile headers, and every single-byte
mutation and truncation of them (never a panic); `tests/robustness.rs` adds 20 000 random
mutations, splices and truncations. `tests/ffmpeg_oracle.rs` checks ffmpeg-made files (Motion JPEG,
H.264 with B-frames, BGR24, YUY2; PCM and MP3) against ffprobe (packet counts, sizes, key flags); the
ignored `opendml_file_past_1_gb` checks a 1.16 GB OpenDML file with `AVIX` extensions.
