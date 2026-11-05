# Scripts: the upstream document of the novel / game to anime pipeline

A **script** is a standalone JSON file (`*.fcscript.json`) that describes a story as characters,
scenes and lines. It is not part of the `.fcproj`: outside tools write it (an LLM turning a novel
into lines, a converter for a game's scenario files), and FilmCraft reads it. Everything after it
is automatic and repeatable: check it, pick voices, speak the missing lines, time the mouths,
assemble. The plan and the reasons are in [novel-to-anime.md](novel-to-anime.md); speech is in
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
Shapes per vowel need the text's reading aligned to the audio and are a later step.

The open picture of a character is `expressions.<name>.openImage` (made once per character, by
any image tool; FilmCraft generates no pictures).

## What comes next

The assembly step (script → sequence: background, sprites, voices, subtitles and music placed
by the lines' durations, rebuilt for changed lines only) builds on these files: the manifest gives
each line's audio and length, `lipsync.json` the mouth, the script the rest.
See [novel-to-anime.md](novel-to-anime.md) §1.4.

## Tests

- `crates/project/src/script.rs`: parsing and defaults, ids and speakers, size caps, the route
  against prerequisites and story time, expression fallback.
- `crates/audio-dsp/src/lipsync.rs`: silence and nonsense input, bursts at the right times, a
  click shorter than the minimum, three shapes, a wobbling level that must not flutter.
- `crates/engine/src/script_tools_tests.rs`: validation and refusal, voice picking and writing,
  synthesizing only what is missing and only once (cache, edit, deleted file, force, language,
  corrupt manifest, a changed reference), lip sync for one file and for a whole script.
