# Preferences (Settings dialog)

The Settings dialog (app menu ▸ Settings ▸ *category*; Edit ▸ Preferences; Cmd/Ctrl+,) is drawn
from the engine's field schema (`crates/engine/src/settings.rs`, `categories()`); the values live
in `filmcraft_engine::autosave::Preferences` and are saved as `preferences.json` in the per-user
data directory. Every value is a dotted camelCase key (`timeline.stillImageDuration`) that
`prefs.get` / `prefs.set` / `prefs.reset` reach from the CLI, the control channel and MCP; the
dialog's control for it has the automation id `settings.<key>`.

Each field says whether FilmCraft acts on it (`Field::wired`). A setting FilmCraft does **not** act
on is drawn **disabled**, with the reason as its tooltip (in Japanese too) and an automation element
`settings.<key>.unavailable`; `prefs.schema` returns `"unavailable": "notApplicable" | "notYet" | null`
per field (`settings::unavailable(key)`):

- **notApplicable** — refers to an Adobe service or application (Audition, After Effects Dynamic
  Link, Mercury Transmit, Content Credentials, the Media Cache database, .cfa/.pek files), hardware
  FilmCraft does not drive (HDR/EDR display output, ProRes hardware encoding), or a feature
  FilmCraft does not have (importing other projects, writing XMP into media files, plugins).
Graphics ▸ default font is **Automatic** when empty (the default since preferences v4; a v3 file's
stored `Inter`, the old default, migrates to Automatic): new text layers, caption tracks and charts
use the bundled Japanese face (BIZ UDPGothic) when the interface is Japanese, so Latin and kana come
from one face, and Inter otherwise (`Session::default_text_font`). Existing text keeps its font.

- **notYet** — could be done, not implemented yet. The value is kept (and round-trips) but nothing
  reads it.

`every_inactive_setting_says_why` (engine) checks that every unwired field has a reason and every
wired one has none.

## Settings wired in this pass

| Setting | Was | Now | How | Test |
|---|---|---|---|---|
| Timeline ▸ Set focus on the Timeline when performing Insert/Overwrite edits | marked wired, but nothing read it | wired | `menus::invoke`: after a successful `source.insert` / `source.overwrite` the Timeline panel gets focus | `settings_ui::insert_and_overwrite_focus_the_timeline_when_set` |
| Timeline ▸ Match frame sets in point | unwired | wired | `sequence.matchFrame` also runs `project.setMarks {in}` at the matched source frame | `settings_tests::match_frame_sets_in_point_when_asked` |
| Timeline ▸ Restore open sequences when opening projects | unwired | wired | off: `install_project` ignores the saved Timeline tabs and opens only the first sequence | `settings_tests::restore_open_sequences_off_opens_only_the_first` |
| Timeline ▸ Timeline Mouse Scrolling | unwired (wheel always scrolled time; Shift+wheel arrived as a sideways delta and also scrolled time) | wired | `settings::timeline_wheel`: Horizontal = wheel scrolls time, Shift+wheel the tracks; Vertical = the reverse; a sideways swipe always scrolls time. Default changed to `horizontal` (what the app did); prefs v3 migrates v2 files' `vertical` (the old, ineffective default) to `horizontal` | `settings_tests::timeline_mouse_scrolling_and_its_migration` |
| General ▸ Show "Project:" and "Bin:" in Project panel tab | unwired | wired | Project panel tab strip and floating bin windows drop the prefixes when off | `project_panel_ui::tab_titles_follow_the_project_bin_prefix_setting` |
| Media ▸ Timecode (Use Media Source / Generate Timecode) | unwired | wired | Generate: `import_source` drops the file's start timecode (clips start at 00:00:00:00) | `settings_tests::generate_timecode_ignores_the_files_timecode` (Broadcast WAV with `bext`) |
| Media ▸ Enable hardware accelerated decoding | unwired | wired | off forces `HwMode::Off` in `Preferences::hardware_mode` (as Playback ▸ Hardware decoding: Off) | `settings_tests::media_hardware_decoding_off_turns_decoding_off` |
| Graphics ▸ Hindi digits | unwired | wired | `graphics.newText` turns ASCII digits into Arabic-Indic digits (`settings::hindi_digits`), like Smart quotes | `settings_tests::hindi_digits_in_new_text` |
| Audio ▸ Sum multichannel outputs to mono in Source Monitor | unwired | wired | Source Monitor playback of a clip with more than two channels plays the average of all channels on both sides (`settings::source_monitor_pair`); off: the first two | `settings_tests::source_monitor_sums_multichannel_to_mono` |
| Playback ▸ Pause Media Encoder queue during playback | unwired | wired (partly) | the UI sets `Session::playback_active` every frame; `pump_queue` starts no new queued export while a monitor plays. An export already encoding finishes (it is not suspended) | `settings_tests::export_queue_waits_while_playing`, `settings_ui::playback_state_reaches_the_export_queue` |
| General ▸ Bins ▸ Double-click / + Cmd / + Opt | read by the Project panel, but marked unwired | marked wired | (already worked: `panels::project::open_bin`) | `project_panel_ui::bins_open_in_place_in_a_tab_or_in_a_window` |
| Media ▸ Import numbered stills as image sequences | read by `file.import`, marked unwired | marked wired | (already worked) | existing image-sequence tests |
| Audio Hardware ▸ Default Input | read by voice-over recording, marked unwired | marked wired | (already worked) | existing voice-over tests |

## Settings that remain inactive

**Not applicable** (disabled, "Not available in FilmCraft"): General ▸ Projects double-click (both),
Show system compatibility issues; Audio ▸ Render Edit in Audition files to; Color ▸ Display Color
Management, Extended Dynamic Range Monitoring, HDR Graphics White; Labels ▸ Label Defaults ▸ Dynamic
Link, Bin, Captions (FilmCraft bins and captions carry no label); Media ▸ Write XMP ID, Validate
Content Credentials, Write clip markers to XMP, Enable clip and XMP metadata linking, Allow duplicate
media during project import, Create folder for imported projects, ProRes hardware encoding; Media
Cache ▸ Save .cfa/.pek next to media, Media Cache Database location; Playback ▸ Enable Mercury
Transmit; Plugins ▸ Enable developer mode.

**Not yet** (disabled, "Not available yet"), with what it would take:

| Setting | What is missing |
|---|---|
| General ▸ When Opening a Project | FilmCraft has no Home screen distinct from the demo project |
| General ▸ Show Event Indicator, Show Rich Tool Tips, Show Workspace Reset Warning, Show Project Load Error Dialog, Show Mask Tracker Preview | the matching UI (event badge, rich tooltips, reset warning dialog, load-error dialog, tracker preview) does not exist yet |
| Audio ▸ Play audio while scrubbing (also `audio.toggleScrubbing`, Shift+S) | scrubbing has no audio path yet |
| Audio ▸ Maintain pitch while shuttling | needs a time-stretch on shuttle playback |
| Audio ▸ Mute input during timeline recording | voice-over does not monitor the input |
| Audio ▸ Generate waveforms automatically during import, multithreaded waveforms | waveforms are computed lazily when first drawn, on a background thread |
| Audio ▸ Render audio when rendering video | render previews are video-only |
| Audio ▸ Auto-tag audio types, Always override audio tags | no automatic audio-type classifier |
| Graphics ▸ paragraph direction, stroke line join / miter limit / line cap (text and shapes), background fill, missing-font replacement, emoji font | the text/shape layers have no such parameters yet; font fallback is fixed |
| Graphics ▸ Closed Captions font, Monospace only, CEA-708 wide | caption rendering uses its own font choice |
| Media ▸ Frame Count | frame-count display is always 0-based |
| Media ▸ Automatically Hide Dependent Clips, Include captions on import | no dependent clips / no embedded-caption import |
| Media ▸ Growing files (refresh, resume, interval) | media is not re-probed while it grows |
| Media Analysis ▸ Cache analysis results, Analyze all imported media | no visual/audio search analysis |
| Memory ▸ RAM reserved for other applications | the frame cache has its own MB budget (Memory ▸ Frame cache, wired); total RAM is not queried (pure Rust, no OS API binding yet) |
| Playback ▸ Disable video output when in the background | the monitors do not check window focus |
| Timeline ▸ Default Audio Tracks (Mono / Stereo / 5.1 / Multichannel Mono media) | track channel layout on edit follows the clip |
| Timeline ▸ Display out of sync indicators for unlinked clips, Show Clip Mismatch Warning dialog, Add tracks automatically | no out-of-sync badges for unlinked clips, no mismatch dialog, tracks are not added on edit |
| Trim ▸ Large Trim Offset (audio units), Allow current tool to change trim type, Shift overlapping clips, Ripple trim adds edits, Update timeline dynamically during ripple edits | the trim engine does not have these modes |
