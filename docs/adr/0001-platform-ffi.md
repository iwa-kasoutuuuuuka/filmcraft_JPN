# ADR 0001: `unsafe` OS media FFI in `crates/platform`, and nowhere else

- **Status:** accepted (2026-10-05, approved by the project owner; recorded in
  [AGENTS.md](../../AGENTS.md) §0 item 3 and the craftrules never-crash standard)
- **Issue:** #30 (hardware acceleration)

## Context

FilmCraft is pure Rust and the workspace sets `unsafe_code = "forbid"`. Every codec has a
clean-room software decoder, and those remain the reference. But decoding 4K H.264 / HEVC in
software costs 85–120 ms of CPU per frame on an M4 Pro, so 4K playback drops frames under load,
while every current computer has a hardware video decoder the operating system exposes
(VideoToolbox on macOS; Media Foundation / D3D11 on Windows and VA-API on Linux later).

Using it needs FFI into C / Objective-C APIs, which is `unsafe` Rust. The bindings we use (the
`objc2-*` crates, MIT / Apache-2.0 / Zlib) are generated declarations; nothing GPL/LGPL is linked.

## Decision

Allow `unsafe` in exactly one crate, `crates/platform` (`filmcraft-platform`, layer L5), for one
job: OS media FFI (hardware decoding, and hardware H.264 encoding through Media Foundation on Windows: `mfencode.rs`). Audio output, dialogs, menus
and other OS integration do not go in it, with the small read-only OS queries of
[amendment 1](#amendment-1-2026-10-19-read-only-os-queries) and audio plug-in hosting
([amendment 2](#amendment-2-2026-10-20-audio-plug-in-hosting)) as the exceptions.

Containment rules:

1. The crate does not use `lints.workspace = true`. Its own `[lints]` table copies the workspace
   lints except `unsafe_code = "deny"` (not `forbid`), and adds
   `clippy::undocumented_unsafe_blocks = "deny"`. Only the FFI modules (`videotoolbox`) carry
   `#[allow(unsafe_code)]`; the rest of the crate (the fallback logic in `hybrid`) has no `unsafe`.
2. Every `unsafe` block has a `// SAFETY:` comment saying why it is sound.
3. The public API is safe: no `pub unsafe fn`, no raw pointers or FFI types in public signatures;
   failures are `Result`s. The crate keeps the never-crash `deny(clippy::unwrap_used, …)`
   attribute of every clean crate.
4. No panic crosses the FFI boundary: the VideoToolbox output callback runs under `catch_unwind`
   and reports a panic as a failed frame.
5. The crate compiles on every target. OS bindings are target-specific dependencies; elsewhere
   `register()` reports `Unavailable` and does nothing. It is not part of the wasm check (L5).

## The fallback guarantee

Registering a hardware decoder never makes a file undecodable, and never changes a picture:

- The factory declines (the software decoder is used) when Settings ▸ Playback ▸ Hardware
  decoding is Off, for formats the hardware path does not take (field-coded H.264, bit depths
  other than 8 / 10, 4:4:4, mismatched luma / chroma depth), and when the OS cannot create a
  hardware session for the stream (hardware decoding is *required*, so the OS's own software
  decoder is never used instead of ours).
- A hardware decoder that fails mid-stream (decode error, session invalidated by a GPU change or
  sleep, in-band parameter sets that differ from the sample entry) becomes the software decoder
  for that stream without the caller noticing: `HybridDecoder` replays the samples since the last
  restart point through it and drops pictures already returned. It is logged and counted
  (`perf.stats` `decode.hardware.fallbacks`).
- Pictures are interchangeable with the software decoder's: same planes (bit-exact on the H.264
  High, HEVC Main and Main 10 parity fixtures, including after seeks and flushes), colour, pixel
  aspect, pts and presentation order. Tests compare the two on every run on macOS, and the
  fallback logic is tested on every platform with a stand-in hardware decoder.

## Consequences

- 4K H.264 / HEVC decode costs ~4 ms of CPU per frame instead of 85–120 ms
  ([performance.md](../performance.md)).
- Better patent posture for the most encumbered formats: H.264 / HEVC decoding is done by the
  operating system, which carries the licences, wherever hardware decoding is used.
- A new review burden: changes to `crates/platform` need the same scrutiny as any FFI code.
  Anything else that wants `unsafe` needs a new decision, not an extension of this one.

## Amendment 1 (2026-10-19): read-only OS queries

- **Status:** accepted (2026-10-19, approved by the project owner)

Some values only the operating system can answer have no safe Rust binding on Windows: the
user's display language (`locale.rs`: `GetUserDefaultUILanguage`, for the UI language setting
"Auto") and the CPU time used by the process and the calling thread (`cputime.rs`:
`GetProcessTimes` / `GetThreadTimes`, for `perf.stats` `process.cpuS` and the playback profiler;
Unix reads its CPU clocks through `rustix` without `unsafe`).

They go in `crates/platform` too, as their own modules under the same containment rules (1–5
above). The scope stays narrow: a query reads a value about the current process or user, takes
no input from media, projects or the control channel, holds nothing across calls, and its
failure is `None` (the caller has a fallback). Nothing a project produces depends on one.
Anything beyond such queries (audio output, dialogs, windows, files) still needs a new decision.

## Amendment 2 (2026-10-20): audio plug-in hosting

- **Status:** accepted (2026-10-20, approved by the project owner)

Hosting VST3 audio plug-ins (later Audio Units) needs FFI into the plug-in's own code. It goes in
`crates/platform` too (`vst3host`, Windows first), under rules 1–5 above plus the plug-in rules of
[ADR 0002](0002-audio-plugin-hosting.md): callbacks under `catch_unwind`, plug-in data treated as
hostile, modules never unloaded, the format-neutral safe interface in `filmcraft-audio-dsp`. Unlike
the OS media APIs, a plug-in is third-party code in our process: ADR 0002 records what that means
for crashes and the planned out-of-process host.
