# Scripts: the upstream document of the novel / game to anime pipeline

A **script** is a standalone JSON file (`*.fcscript.json`) that describes a story as characters,
scenes and lines. It is not part of the `.fcproj`: outside tools write it (an LLM turning a novel
into lines, a converter for a game's scenario files), and FilmCraft reads it. Everything after it
is automatic and repeatable: check it, pick voices, speak the missing lines, time the mouths,
assemble. A game's scenario text converts into one with [game-import.md](game-import.md). The plan and the reasons are in [novel-to-anime.md](novel-to-anime.md); speech is in
[text-to-speech.md](text-to-speech.md).

The commands only read the script and write files beside their other outputs (`<out>/…`), so
they change nothing in the project and can run again at any time. Relative paths in a script
(recordings, voice references, pictures) are relative to the script's folder.

## Format (version 1)

```json
{
  "version": 1,
  "title": "Episode 1",
  "language": "Japanese",
  "characters": {
    "marina": {
      "name": "まりな",
      "voice": {"refAudio": "voice/marina_0007.wav", "refText": "待って、まだ話は終わってない。"},
      "expressions": {
        "normal": {"image": "sprites/marina_normal.png", "openImage": "sprites/marina_normal_open.png"},
        "angry":  {"image": "sprites/marina_angry.png"}
      },
      "defaultExpression": "normal"
    }
  },
  "scenes": [
    {
      "id": "s01", "title": "Rooftop", "pov": "marina", "when": "1998-04-12T10:00",
      "requires": [], "background": "bg/roof.png", "bgm": "bgm/calm.ogg",
      "lines": [
        {"id": "s01-001", "kind": "narration", "text": "夜だった。"},
        {"id": "s01-002", "speaker": "marina", "text": "｜漢字《かんじ》が読めない。", "expression": "angry",
         "voiceFile": "voice/marina_0012.wav", "pauseAfter": 0.6},
        {"id": "s01-003", "kind": "direction", "sfx": "se/door.wav"}
      ]
    }
  ],
  "route": ["s01"]
}
```

| Field | Meaning |
|---|---|
| `characters.<id>` | `name` (display), `voice` (`refAudio` + `refText`: a reference recording and its transcript, for cloning; or `speaker`: a preset voice such as `Ono_Anna`, with an optional standing `instruct`), `expressions` (pictures by expression name; `openImage` is the open-mouth picture for lip sync), `defaultExpression`. |
| `scenes[]` | `id` (unique), `title`, `pov` (a character id), `when` (story time as text that sorts in time order), `requires` (scenes that must be shown first), `background`, `bgm`, `lines`. |
| `lines[]` | `id` (unique across the whole script: it names the audio file), `kind` (`dialogue` default, `narration`, `monologue`, `direction`), `speaker`, `text`, `reading`, `expression`, `voiceFile`, `instruct`, `sfx`, `pauseAfter`, `generated`. |
| `route` | Scene ids in the order they are shown; empty means file order. Scenes missing from it are not shown. |

Notes:

- `text` is what is spoken **and** subtitled. Ruby in Aozora notation (`｜漢字《かんじ》`,
  `東京《とうきょう》`) is spoken as its reading and drawn as ruby.
- `reading` replaces `text` when speaking (names and odd readings). Put kana there.
- `instruct` directs a preset speaker for one line (`"ささやき声で"`, `"怒って"`) and wins over the
  character's standing one; a cloned voice follows its recording instead.
- `voiceFile` is an existing recording of the line (a game's own voice file). Lines that have
  one are never synthesized; lines that do not are spoken in their character's voice.
- `generated: true` marks a line a tool wrote to bridge scenes (not in the source story), so a
  later run may rewrite it freely.
- A **direction** has no text and no speaker: sound effects, a cut, a pause.
- Counts and lengths are capped (500 characters, 10 000 scenes, 200 000 lines, 4 000 characters
  per text, 64 MB per file); everything in the file is treated as hostile.

## Branching stories

A game with several viewpoints becomes one story by choosing the order of its scenes:

- give each scene its `pov`, its story time `when`, and `requires` (the scenes of the other
  viewpoint that must have been shown for it to make sense);
- write the order in `route`. `script.validate` then checks that every prerequisite is shown
  earlier (an **error**), that no scene or id appears twice (error), and that story time does not
  run backwards (a **warning**: fine for a flashback, otherwise reorder).

Cutting between viewpoints at a peak instead of where the game's save points were is a creative
choice. An LLM can draft a route and the bridging lines (mark them `generated`); the checks above
keep any draft consistent.

## Commands

| Command | What it does |
|---|---|
| `script.validate {path}` | `ok`, `errors`, `warnings`, `problems` (severity, scene, line, message) and `stats` (characters, scenes, lines, spoken, recorded, recordingMissing, toSynthesize). |
| `script.pickVoices {path, write?, overwrite?}` | For each character without a voice, the best of their lines that have a recording of their own: 2.5–12 s, nearest 6 s, a calm line (few `！？…、`), a plausible speaking rate (2–12 characters per second), and the text known. Returns `picked` (character, line, refAudio, refText, seconds, candidates), `skipped` with reasons, and `unreadable` files. `write: true` stores the picks in the script (the file is re-formatted; fields this build does not know are kept); without it nothing is written. `overwrite` replaces voices already set. |
| `script.synthesize {path, out?, ids?, force?, all?, language?, model?, device?, python?, script?}` | Speak every spoken line that has no recording (`all` includes recorded ones), in route order, into `<out>/<id>.wav` (default `<data dir>/tts/<script name>-<8 hex of its folder>`, so same-named scripts in different folders do not share audio; ids that differ only in case are refused, since Windows and macOS would write one file). A `manifest.json` there remembers a hash of each line's spoken text, language, model, synthesizer, direction and voice (reference file name and size, or preset speaker), so **a second run speaks nothing**, and after an edit only the changed lines are spoken again. `force` redoes all, `ids` limits the run. Refuses a script with errors. Lines are spoken in chunks of 400 and the manifest is saved after each, so a run that dies loses at most one chunk and the next goes on from there. A voice reference is identified by its name, size and modified time. Returns `synthesized`, `cached`, `recorded`, `failed` and every line's path and seconds. |
| `script.lipsync {path, out?, three?, openDb?, halfDb?, minSeconds?}` | Mouth shapes for every spoken line that has audio (its own recording, else the synthesized file), written to `<out>/lipsync.json`. |
| `lipsync.analyze {path, three?, openDb?, halfDb?, minSeconds?}` | The same for one audio file: `segments` of `{start, end, level, startTick, endTick}`. |

## Lip sync

`level` 0 is the closed mouth, 1 open (with `three: true`, 1 is half open and 2 fully open). The
shape follows the loudness of the voice measured against the speaker's own loud passages, with
hysteresis (a shape ends only 3 dB below the level that started it) and a minimum time per shape
(50 ms), under an absolute floor (−50 dBFS) so a quiet room never opens the mouth. It is
deterministic and uses no model, which is what a hand-timed two- or three-picture mouth does.
A picture for each vowel is described under "Mouths for each vowel" below.

The open picture of a character is `expressions.<name>.openImage` (made once per character, by
any image tool; FilmCraft generates no pictures).

### Mouths for each vowel

A sprite can give an open-mouth picture for each of the five Japanese vowels:

```json
"normal": {"image": "m.png", "openImage": "mo.png",
           "vowelImages": {"a": "m_a.png", "i": "m_i.png", "u": "m_u.png", "e": "m_e.png", "o": "m_o.png"}}
```

The reading of the line (`reading` if given, else the text with its ruby read out) is turned
into beats: one for each kana, none for punctuation, a closed beat for ん and っ, the vowel
again for ー, the vowel of ゃゅょ for きゃ, two unknown beats for each kanji left without a
reading. The beats are spread evenly over the stretches where the voice is open (see
`script.lipsync`), so the pictures follow the voice's own pauses. A vowel without a picture, and
an unknown beat, use `openImage`; a closed beat shows the closed mouth. A sprite with no
`vowelImages` is unchanged.

It is a fit, not an alignment: every beat gets the same time, so a drawn-out syllable is cut
short, and a line full of kanji without ruby or `reading` falls back to the plain open mouth for
most of its length. Give such lines a `reading` for a better result. Letters and digits count as
one unknown beat each. A sprite with `vowelImages` and no `openImage` shows nothing for an unknown
beat or a vowel without a picture, so give every such sprite an `openImage`.

## Building the sequence

`anime.build` lays the script out as a new sequence (one undo step: the sequence, the bin of
imported files and the subtitle track together). Times are exact: a line lasts as long as its
audio plus a pause, every boundary is on the frame grid, and neighbouring clips abut.

| Track | Holds |
|---|---|
| V1 Background | One picture per scene, scaled to fill the frame (`fitBackground: false` keeps its size), pushing in slowly (`push`, 5 % by default; `kenBurns: false` turns it off) |
| V2 Character | The speaker's picture for the expression of the line. Consecutive lines in the same picture (and the same scene) are one clip. Directions, and narration without a `speaker`, show no character |
| V3 Mouth | The open-mouth picture (`openImage`) over V2 while the voice is loud (the shapes of `script.lipsync`); the picture for the vowel being spoken where the sprite has `vowelImages` |
| V4 Blink | The `blink` picture (or `<expression>_blink`) over V2 for about 0.12 s (at least three frames), first 1.2 to 2.2 s into a clip and then every 2.7 to 4.6 s, at times fixed by the clip's position, so a rebuild blinks the same way |
| A1 Voice | Recordings and synthesized lines, brought to one loudness (`levelVoices`, on by default: each clip's gain is set so it matches the median voice, or `voiceLufs`, within 12 dB; a cloned and a preset voice differ by 10 LU or more), as long as their audio (cut at the line's end if a `pauseAfter` of 0 rounds the end down by up to half a frame) |
| A2 Music | A scene's `bgm`, repeated to fill the scene (each repeat is a whole number of frames, so repeats abut and never overlap); scenes that share it share the clips; `bgmGainDb` (-18); at most 20 000 repeats per stretch, with a warning |
| A3 Effects | A line's `sfx`; a long one ends where the next begins, or where the sequence ends |
| Subtitles | The plain text (ruby removed), broken at sentence ends, then commas, into pieces of `captionChars` (36) characters, timed in proportion to their length |

`anime.build {path, out?, name?, width=1920, height=1080, fps=24, pause=0.35, sceneGap=0.6,
directionSeconds=1, charsPerSecond=7, push=5, spriteScale?, bgmGainDb=-18, sfxGainDb=0,
captionChars=36, blink=true}`: `pause` follows a spoken line unless its `pauseAfter` says
otherwise; `sceneGap` is a beat of background at the start of each scene; a line without audio
yet gets a length estimated at `charsPerSecond` and is listed in `withoutAudio`. Pictures that are
missing (backgrounds, sprites, and the open-mouth and blink pictures alike) are skipped with a warning; a script with errors, or with no lines, builds nothing.
Building again makes another sequence, named apart (`name (2)`). The files are imported in one
step into a bin named after the script, so the whole build is one undo step however long the
script is, and numbered pictures (`bg_01.png`, `bg_02.png`) are not mistaken for an image
sequence. A scene lasts at least one frame, even without lines. A script, a size or a frame rate
that cannot be built is refused before anything is imported, and a build that fails part way
puts the project back as it was.

A character's expression is the line's `expression`, else `defaultExpression`, else `normal`,
else the first by name. The open-mouth and blink pictures are optional: without them the
character simply does not move its mouth or blink.

`anime.check {sequence?, path?, out?, maxSilenceSeconds=5, maxCharsPerSecond=20, captionChars=36,
loudnessToleranceLu=6}` reports `ok`, `errors`, `warnings`, `stats` and `problems` (each with a
`kind` and the time):

| Kind | Severity | Meaning |
|---|---|---|
| `offline` | error | A clip's media is missing |
| `gap` | error | The background track has a hole, or ends before the rest |
| `overlap` | error | Two voices at once |
| `silence` | warning | More than `maxSilenceSeconds` without a voice |
| `loudness` | warning | A voice is more than `loudnessToleranceLu` from the median voice (integrated LUFS, BS.1770) |
| `caption-long` / `caption-fast` / `caption-short` | warning | A subtitle is over twice `captionChars`, runs over `maxCharsPerSecond`, or shows under 0.4 s |
| `no-audio` | warning | With `path`: a spoken line has neither a recording nor a synthesized file |
| `empty`, `no-voice` | warning | An empty background or voice track |

`anime.run {path, pickVoices=false, synthesize=true, check=true, export?, preset?, ...}` runs the
steps in order and returns each one's summary: `validate`, `pickVoices` (only when asked: it
rewrites the script), `synthesize`, `build`, `check` and, with `export: "out.mp4"`, the export
(through `export.queue.add`, waiting for it; an encode that ends as anything but `done` is a
failed step, with the encoder's error). It stops at the first step that fails and names it
(`failedStep`). Every step can be repeated: spoken lines are cached, so a second run only
speaks what changed, and builds a fresh sequence beside the first.

## What is not here

- **Pictures are not generated.** Backgrounds, sprites and their open-mouth and blink variants
  come from you; FilmCraft only places them.
- **One character at a time.** The sprite track shows the speaker; a scene with two people on
  screen needs a second sprite track by hand.
- **Mouth shapes follow the reading, not the sound.** The vowels are spread evenly over the
  open mouth (see above); nothing listens to which vowel is actually spoken.
- **Sprites are static.** A sprite is placed as it is imported (`spriteScale` sets one scale for
  all). Only the background moves, so the mouth and blink pictures always line up with the
  character.
- **Limits.** A single audio file longer than an hour is not used, a sequence stops at 100 hours,
  and a script at 400 000 lines; very long scripts also make the subtitle track slow to build
  (each caption re-sorts the track).
- **No transitions or fades yet.** Scenes cut; add transitions on V1 by hand or with the
  effects commands.

## Tests

- `crates/project/src/script.rs`: parsing and defaults, ids and speakers, size caps, the route
  against prerequisites and story time, expression fallback.
- `crates/audio-dsp/src/lipsync.rs`: silence and nonsense input, bursts at the right times, a
  click shorter than the minimum, three shapes, a wobbling level that must not flutter.
- `crates/engine/src/anime_tests.rs`: the plan with files faked (timing on the frame grid, runs, blinks, mouth, music, subtitles, missing files), building with real files (tracks, clip counts, one undo step, nothing built from a bad script), the checks (gaps, overlaps, a quiet voice), the one-call run and its cache, and an export that is opened again and measured.
- `crates/engine/src/script_tools_tests.rs`: validation and refusal, voice picking and writing,
  synthesizing only what is missing and only once (cache, edit, deleted file, force, language,
  corrupt manifest, a changed reference), lip sync for one file and for a whole script.
