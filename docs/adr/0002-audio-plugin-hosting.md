# ADR 0002: Audio plug-in hosting (VST3) in `crates/platform`

- **Status:** accepted (2026-10-20, approved by the project owner: "start plugin hosting", the
  owner decision ROADMAP.md "Where we are lacking" item 7 asked for)
- **Extends:** [ADR 0001](0001-platform-ffi.md) (the only crate allowed `unsafe`)

## Context

Professional editors rely on third-party audio plug-ins (reverbs, restoration, loudness, mastering
chains). Premiere hosts VST3 on every OS and Audio Units on macOS. Without plug-in hosting many
editors cannot use FilmCraft at all (ROADMAP: "Plugins and pro ecosystem ~0–10%").

A plug-in is native code in a shared library that speaks a C++ ABI (VST3: COM-style interfaces).
Hosting it needs FFI: loading the library, calling through virtual tables, handing it pointers to
our buffers and objects that it calls back. That is `unsafe` Rust, which the workspace forbids
everywhere except `crates/platform`.

### Licence and clean-room check

- **The VST 3 SDK is MIT-licensed.** Steinberg relicensed the VST 3 SDK, including the
  `pluginterfaces` headers the bindings are generated from, under the MIT licence with VST 3.8
  (2025). Checked against the licence text of `steinbergmedia/vst3_pluginterfaces` (`LICENSE.txt`:
  MIT, © Steinberg Media Technologies GmbH) and of the `coupler-rs/vst3_pluginterfaces` copy the
  bindings are generated from (MIT, © Steinberg Media Technologies GmbH, 2025). No GPL / dual
  licence and no proprietary SDK agreement is involved.
- **Bindings:** the [`vst3`](https://crates.io/crates/vst3) crate 0.3 (coupler-rs/vst3-rs,
  MIT OR Apache-2.0) and its only dependency `com-scrape-types` 0.1 (MIT OR Apache-2.0). Since 0.3
  the bindings are pre-generated Rust source shipped in the crate (`build = false`, no build
  script): no C/C++ toolchain, no libclang, no copy of the SDK at build time.
- **No SDK code is used.** We host plug-ins through the interface declarations only; the SDK's C++
  helper classes are not used or ported. The `gain` example of the `vst3` crate (MIT / Apache-2.0)
  is built only as a test fixture (see Testing); it is never linked into or shipped with FilmCraft.
- Dynamic loading uses the `windows` crate (already a dependency of `platform`, MIT OR Apache-2.0).

## Decision

Host audio plug-ins in `crates/platform` under the containment rules of ADR 0001, as a second
amendment of its scope rather than a new crate. A separate crate would duplicate the lint set,
the review rule and the "no other crate has `unsafe`" invariant for no gain; `platform` is already
L5, compiled per OS and excluded from the wasm check.

Layering:

| Layer | Crate | Part |
|---|---|---|
| L1 | `audio-dsp` | `plugin`: the format-neutral interface (`PluginHost`, `PluginProcessor`), the process-wide catalogue (`scan`, `find`, `instantiate`), and `PluginEffect`, a plug-in instance as an `AudioEffect` (pass-through when the plug-in is missing). Safe code, no FFI, wasm-clean. |
| L1 | `project` | plug-in effect definitions registered at run time (`register_plugin_effect`; Audio Effects ▸ VST3) and `EffectInstance::plugin` (`PluginRef`: format, class uid, name, vendor, base64 state), schema v13. |
| L3 | `render` | `audio_fx`: plug-in effect ids (`vst3.<uid>`) map to a `PluginEffect`; parameters `p<id>` (normalised, keyframable) are applied per block like built-in effects, for clip effects and track / submix / master inserts, in playback and export. |
| L4 | `engine` | `audioPlugins.scan` / `audioPlugins.list`; `effects.apply` and `mixer.addInsert` create the plug-in once (an error if it cannot load) and store its initial state. |
| L5 | `platform` | `audio_plugins` (safe: folders, bundle layout, class-id text, state container; tested on every OS) and `vst3host` (the FFI; Windows). `register_audio_plugins()` registers the host at startup. |

Containment rules (ADR 0001 rules 1–5 apply unchanged; in addition):

1. **FFI only in `vst3host`** (and later an `audiounit` module), with `#[allow(unsafe_code)]` on the
   module and a `// SAFETY:` comment on every `unsafe` block. The rest of the hosting code is safe.
2. **Safe public API returning `Result`.** Nothing outside `vst3host` sees a raw pointer, a COM type
   or an `unsafe fn`. The engine and renderer see `PluginHost` / `PluginProcessor` from
   `audio-dsp`.
3. **No panic crosses the FFI boundary.** Every host object the plug-in calls into (`IBStream`,
   `IParameterChanges`, `IParamValueQueue`, `IHostApplication`, `IComponentHandler`) runs its body
   under `catch_unwind` and answers `kInternalError`. Calls into the host are made from Rust code
   that returns `Result`, and the catalogue runs every host's `scan` / `instantiate` under
   `catch_unwind` too.
4. **Plug-in data is hostile.** Counts (classes, parameters, buses, channels), string buffers,
   state sizes and sample values a plug-in returns are clamped, length-checked and lossily
   decoded; non-finite output samples become 0; saved state is capped (64 MiB in a project) and a
   damaged blob is an error. Project files are hostile too: effect ids are validated, interned
   names are bounded, a missing plug-in passes audio through and its instance (with its state) is
   kept unchanged.
5. **Lifetime.** Plug-in modules are never unloaded (a plug-in may keep threads or callbacks
   alive); instance creation and destruction are serialised behind one lock; teardown follows the
   VST3 order (setProcessing(false), setActive(false), disconnect, terminate).

### The limit of in-process hosting

A plug-in runs in FilmCraft's process. `catch_unwind` handles Rust panics only: a plug-in that
dereferences a bad pointer, overflows its stack or lets a C++ exception escape terminates the
process, and no in-process host can prevent that. What we do instead:

- nothing loads a plug-in until the user (or an agent) scans, or opens a project that uses one;
  startup never scans, so a broken plug-in cannot stop FilmCraft from starting;
- Auto Save and crash recovery keep the user's work when a plug-in takes the process down.

**Out-of-process hosting** (a `filmcraft-plugin-host` helper process per plug-in or per sandbox
group, audio over shared memory, the scan done in the helper so a crashing plug-in is blacklisted
instead of crashing the app) is the planned next step. It needs its own owner decision: it adds a
second shipped executable and an IPC protocol.

## Scope of this pass and next steps

Implemented (Windows): scanning the standard folders (`%CommonProgramFiles%\VST3`,
`%LOCALAPPDATA%\Programs\Common\VST3`, plus `FILMCRAFT_VST3_PATH`), bundles and single-file
modules; audio effects (`Audio Module Class`, instruments skipped) with one main input and output
bus (stereo, or the plug-in's own arrangement; side-chain buses inactive); 32-bit float
processing in blocks of up to 1024 samples; parameters (normalised values, automatable through
the normal keyframes); component + controller state saved in the project.

Next:

- the plug-in's editor window (`IPlugView` in a native child window: an L6 concern, the window
  handle comes from the app);
- parameter edits from that editor (`IComponentHandler::performEdit`) recorded as keyframes;
- Audio Units on macOS (`audiounit` module behind the same `PluginHost` trait), VST3 on macOS and
  Linux (bundle entry points `bundleEntry` / `ModuleEntry`);
- a scan cache (no reload at every start), per-plug-in latency compensation reported to the
  mixer, side-chain inputs, offline (`kOffline`) processing for export, `IMessage` /
  `IAttributeList` for plug-ins that message between processor and controller;
- out-of-process hosting (above).

## Testing

- `audio-dsp`: the catalogue, `PluginEffect`, hostile ids / values / state with a stand-in host
  (`plugin::fake`), on every platform.
- `platform` (`audio_plugins`): folder walking, bundle layouts, class-id text form, state
  container; every platform.
- `engine` (`audio_plugins_tests`): scan, list, apply to clips and tracks, render, save and reopen,
  missing plug-ins and hostile parameters, with the stand-in host.
- `platform/tests/vst3_host.rs` (Windows): a real VST3 plug-in, `$FILMCRAFT_VST3_TEST_PLUGIN` or by
  default the `vst3` crate's `gain` example built from the registry copy into
  `target/fixtures/platform/vst3/` (skipped when it cannot be built).

## Consequences

- FilmCraft can use the VST3 effects users already own.
- The trust boundary grows: a plug-in can crash the process (above). Changes to `vst3host` get the
  same review as any FFI code (ADR 0001).
- Projects that use plug-ins depend on them being installed; without them they still open, play
  and export (the effect passes audio through and is kept as it was).
