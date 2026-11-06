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
| `characters.<id>` | `name` (display), `voice` (`refAudio` + `refText`: a reference recording and its transcript, for cloning; or `speaker`: a preset voice such as `Ono_Anna`, with an optional standing `instruct`), `expressions` (pictures by expression name; `openImage` is the open-mouth picture for lip sync, `vowelImages` one per vowel, `blink` the same picture with the eyes closed), `defaultExpression`. |
| `scenes[]` | `id` (unique), `title`, `pov` (a character id), `when` (story time as text that sorts in time order), `requires` (scenes that must be shown first), `background`, `bgm`, `transition` (`"cut"`, the default, or `"fade"`: in from black), `hideCharacters` (`true`: no character pictures in this scene, as over an event picture or a flashback), `lines`. |
| `lines[]` | `id` (unique across the whole script: it names the audio file), `kind` (`dialogue` default, `narration`, `monologue`, `direction`), `speaker`, `text`, `reading`, `expression`, `voiceFile`, `instruct`, `sfx`, `pauseAfter`, `generated`, `spoken` (`false`: subtitled, not voiced), `video` (a movie, on a direction line), `subCg` (an inset picture; `""` takes it away), `subCgAnchor` (`[x, y]`, fractions of the frame), `slot` (1 left, 2 centre, 3 right). |
| `route` | Scene ids in the order they are shown; empty means file order. Scenes missing from it are not shown. |

Notes:

- `text` is what is spoken **and** subtitled. Ruby in Aozora notation (`｜漢字《かんじ》`,
  `東京《とうきょう》`) is spoken as its reading and drawn as ruby.
- `reading` replaces `text` when speaking (names and odd readings). Put kana there.
- `instruct` directs a preset speaker for one line (`"ささやき声で"`, `"怒って"`) and wins over the
  character's standing one; a cloned voice follows its recording instead.
- `voiceFile` is an existing recording of the line (a game's own voice file). Lines that have
  one are never synthesized; lines that do not are spoken in their character's voice.
- `spoken: false` marks a **sound word** (`ガタッ。`, `プルルルル‥‥`, `ぐぅ～。`): a noise the source
  writes as a line. It is subtitled like any line, but nothing is synthesized, no voice is laid
  on A1 (a `voiceFile` is not played; `script.validate` warns) and no mouth moves. Its sound is
  its `sfx`, on A3 under the voices. It lasts as long as that sound, and at least as long as
  reading it takes (`charsPerSecond`, 0.8 s at the least), then the usual `pause`. Without an
  `sfx` it is a silent beat with its subtitle. A game's sound words are found and marked by
  `tools/eve_burst/onomatopoeia.py`. A sound with no text to show is a direction instead.
- `generated: true` marks a line a tool wrote to bridge scenes (not in the source story), so a
  later run may rewrite it freely.
- A **direction** has no text and no speaker: sound effects, a cut, a pause, a movie (`video`).
- `video`, `subCg`, `slot` and a scene's `transition` are laid out as described under "Movies,
  sub CGs, two-shots and fades" below. `script.validate` reports a `slot` other than 1, 2 or 3, a
  `subCgAnchor` outside the frame, an empty `video` and an unknown `transition` as errors, and a
  `video` on a line that is not a direction as a warning (it is not shown there).
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
| `script.synthesize {path, out?, ids?, force?, all?, language?, model?, device?, python?, script?}` | Speak every spoken line that has no recording (`all` includes recorded ones), in route order, into `<out>/<id>.wav` (default `<data dir>/tts/<script name>-<8 hex of its folder>`, so same-named scripts in different folders do not share audio; ids that differ only in case are refused, since Windows and macOS would write one file). A `manifest.json` there remembers a hash of each line's spoken text, language, model, synthesizer, direction and voice (reference file name and size, or preset speaker), so **a second run speaks nothing**, and after an edit only the changed lines are spoken again. `force` redoes all, `ids` limits the run. Refuses a script with errors. Lines are spoken in chunks of 400 and the manifest is saved after each, so a run that dies loses at most one chunk and the next goes on from there. A voice reference is identified by its file name, size and modified time, not its path, so the same script passed by a relative or an absolute path shares one cache (manifests written before this, which held the reference's path as written, still count and are rewritten in the new form on a hit). Returns `synthesized`, `cached`, `recorded`, `failed` and every line's path and seconds. |
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
| V1 Background | One picture per scene, fitted inside the frame (a picture of another shape leaves bars at the sides or top; `backgroundFit: "fill"` covers the frame instead, cutting the edges; `fitBackground: false` keeps its size), pushing in slowly (`push`, 5 % by default; `kenBurns: false` turns it off). A line's `video` takes its place for the movie's length (fitted or filled the same way); the picture is cut around it and goes on pushing in where it left off. A scene with `transition: "fade"` fades in from black here |
| V2 Character | The speaker's picture for the expression of the line, in the centre (slot 2, as imported). Consecutive lines in the same picture (and the same scene) are one clip. Directions, and narration without a `speaker`, show no character |
| V3 Mouth | The open-mouth picture (`openImage`) over V2 while the voice is loud (the shapes of `script.lipsync`); the picture for the vowel being spoken where the sprite has `vowelImages` |
| V4 Blink | The sprite's `blink` picture (else the expression `<expression>_blink`, else one called `blink`) over V2 for about 0.12 s (at least three frames), first 1.2 to 2.2 s into a line and then every 2.7 to 4.6 s, at times fixed by the line's position, so a rebuild blinks the same way. The blink picture is a whole sprite with the mouth closed, so a blink that would fall while the mouth is open waits until it closes (and is left out when the line ends first) |
| V5–V7 Left, Left Mouth, Left Blink | Only when a line has `slot: 1`: the same three for the character at the left of a two-shot |
| V8–V10 Right, Right Mouth, Right Blink | Only when a line has `slot: 3` (they follow V4 directly when no line is at the left) |
| Top: Sub CG | Only when a line has a `subCg`: the inset picture over everything else, mouths and blinks included |
| A1 Voice | Recordings and synthesized lines, brought to one loudness (`levelVoices`, on by default: each clip's gain is set so it matches the median voice, or `voiceLufs`, within 12 dB; a cloned and a preset voice differ by 10 LU or more), as long as their audio (cut at the line's end if a `pauseAfter` of 0 rounds the end down by up to half a frame) |
| A2 Music | A scene's `bgm`, repeated to fill the scene (each repeat is a whole number of frames, so repeats abut and never overlap); scenes that share it share the clips; `bgmGainDb` (-18); at most 20 000 repeats per stretch, with a warning. It goes on under a movie |
| A3 Effects | A line's `sfx`, and a movie's own sound with it; a long one ends where the next begins, or where the sequence ends. The effects are levelled **under the voices** (`levelEffects`, on by default): each one is set to `effectsBelowVoiceLu` (10) under the voices' loudness (`voiceLufs`, else the median voice), cut by as much as it needs and raised by at most 6 dB (a click shorter than one 400 ms loudness block is measured as that block, padded with silence); one that cannot be measured (silent, unreadable, or with `levelEffects: false`) gets `sfxGainDb` (-10). A game's effects are each as loud as they can be, so at full gain a door is as loud as a shout. A movie's sound gets `videoGainDb` (0) |
| Subtitles | The plain text (ruby removed), broken at sentence ends, then commas, into pieces of `captionChars` (36) characters, timed in proportion to their length |

`anime.build {path, out?, name?, width=1920, height=1080, fps=24, pause=0.35, sceneGap=0.6,
directionSeconds=1, charsPerSecond=7, push=5, spriteScale?, spriteAlign="center", backgroundFit="fit",
bgmGainDb=-18, sfxGainDb=-10, videoGainDb=0, levelEffects=true, effectsBelowVoiceLu=10, captionChars=36, blink=true, transitionSeconds=0.5, leftX=0.25, rightX=0.75,
subCgScale=100}`: `spriteAlign: "bottom"` stands every character picture (and its mouth and blink)
on the bottom edge of the frame, as on a game screen, instead of centring it; with game pictures
whose sprites are as tall as the background, `spriteScale` = frame height / picture height × 100
(1080 / 448 → 241) keeps them in proportion to a fitted background (with `backgroundFit: "fill"`,
the larger ratio, as the background: 1920 / 640 → 300).
`pause` follows a spoken line unless its `pauseAfter` says
otherwise; `sceneGap` is a beat of background at the start of each scene; a line without audio
yet gets a length estimated at `charsPerSecond` and is listed in `withoutAudio`. The result
counts the clips by kind (`backgrounds`, `videos`, `characters`, `mouths`, `blinks`, `subCgs`,
`voices`, `music`, `effects`, `videoSounds`, `fades`, `subtitles`) and names the video tracks in
order (`tracks`). Pictures that are
missing (backgrounds, sprites, and the open-mouth and blink pictures alike) are skipped with a warning; a script with errors, or with no lines, builds nothing.
Building again makes another sequence, named apart (`name (2)`). The files are imported in one
step into a bin named after the script, so the whole build is one undo step however long the
script is, and numbered pictures (`bg_01.png`, `bg_02.png`) are not mistaken for an image
sequence. A scene lasts at least one frame, even without lines. A script, a size or a frame rate
that cannot be built is refused before anything is imported, and a build that fails part way
puts the project back as it was.

A character's expression is the line's `expression`, else `defaultExpression`, else `normal`,
else the first by name. The open-mouth and blink pictures are optional: without them the
character simply does not move its mouth or blink. A scene with `hideCharacters: true` places no
character picture, mouth or blink at all (its voices, subtitles and effects are unchanged), and
ends a two-shot that the scene before it left on screen.

### Movies, sub CGs, two-shots and fades

- **Movies.** A direction line with `video` (a game's event or date movie) puts the file on V1
  from the line's start for its own length, as the media reports it when opened, and its sound,
  if any, on A3 (`videoGainDb`). The line lasts that long (plus its `pauseAfter`; the end is on the
  frame grid, so a movie whose length is not a whole number of frames loses the last part of a
  frame). A file with no length of its own (a still picture) is shown for `directionSeconds`. A
  missing or unreadable movie, or one longer than an hour, is skipped with a warning, and the line
  lasts `directionSeconds` (or its `sfx`) as a direction without one would. When the movie has
  sound, the line's `sfx` is not played (one sound at a time on A3), with a warning. A movie ends
  the sub CG and the two-shot on screen; characters are never shown over it (a direction shows
  none), and the music goes on under it.
- **Sub CGs.** A line's `subCg` (an inset, such as a 272x247 picture over the scene) is placed on
  the top track at its own pixel size (`scaleToFrame` off, Scale 100 %, or `subCgScale` for all of
  them), centred in the frame or at `subCgAnchor` (`[0.25, 0.25]`: a quarter across, a quarter
  down). It stays from that line until the next `subCg`, a `video` or the end of the scene;
  `"subCg": ""` takes it away. It is above the mouth and blink tracks because those pictures are
  whole sprites (the closed one with another mouth or eyes): under them, a speaking or blinking
  character would cover the inset. A missing picture is a warning; the one before it still ends.
- **Two-shots.** `slot: 1` puts the speaker at the left, `3` at the right (their centres at
  `leftX` and `rightX` of the frame width, a quarter and three quarters by default, half way
  down; Scale is as for the centre, `spriteScale`), `2` in the centre alone. Someone at the left
  or right stays there, in the picture of their last line, while others speak and through
  narration and directions, until someone else takes the place, they move to the other side, a
  line puts a character in the centre, a movie plays, or the scene ends. A line without `slot`
  stays where its speaker stands, else goes to the centre (and so ends a two-shot); a line whose
  speaker has no picture (a narrator, a hero who is never shown) changes nothing. So giving the
  one pictured character of a dialogue a side keeps them on screen through the other's lines. The open
  mouth and the blinks go only with the one speaking, on that side's own tracks and at its place;
  the listener keeps still.
- **Fades.** A scene with `transition: "fade"` fades in from black over `transitionSeconds` (0.5),
  from the scene's start: a Cross Dissolve from nothing onto the first clip of V1 (shortened to
  that clip when it is shorter). The fade is part of `sceneGap`'s beat of background, so with the
  default gap (0.6 s) no character appears before it ends; with a shorter gap the first character
  shows over the fade. A scene with nothing on V1 at its start (no background, no movie) is
  warned about and cuts. Scenes without `transition`, or with `"cut"`, cut as before.

`anime.check {sequence?, path?, out?, maxSilenceSeconds=5, maxCharsPerSecond=20, captionChars=36,
loudnessToleranceLu=6, effectMarginLu=6}` reports `ok`, `errors`, `warnings`, `stats` and `problems` (each with a
`kind` and the time):

| Kind | Severity | Meaning |
|---|---|---|
| `offline` | error | A clip's media is missing |
| `gap` | error | The background track has a hole, or ends before the rest |
| `overlap` | error | Two voices at once |
| `silence` | warning | More than `maxSilenceSeconds` without a voice |
| `loudness` | warning | A voice is more than `loudnessToleranceLu` from the median voice (integrated LUFS, BS.1770) |
| `effect-loud` | warning | A sound effect (A3, not a movie's sound; a click shorter than 0.4 s measured as one 400 ms block) is less than `effectMarginLu` under the median voice. `stats` gives `effects` (how many were measured), `medianEffectLufs` and `loudestEffectLufs` beside `medianVoiceLufs` |
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
- **Three places, two at once.** A character stands at the left, the centre or the right; the
  centre shows one person alone, so three on screen at once is not possible. Who stands where is
  only what the lines' `slot` says: a character who does not speak cannot be put on screen, and
  a listener cannot change expression until they speak again.
- **Mouth shapes follow the reading, not the sound.** The vowels are spread evenly over the
  open mouth (see above); nothing listens to which vowel is actually spoken.
- **Sprites are static.** A sprite is placed as it is imported, centred or standing on the bottom
  (`spriteScale` sets one scale for all). Only the background moves, so the mouth and blink pictures always line up with the
  character.
- **Limits.** A single audio file longer than an hour is not used, a sequence stops at 100 hours,
  and a script at 400 000 lines; very long scripts also make the subtitle track slow to build
  (each caption re-sorts the track).
- **One transition.** A scene cuts or fades in from black; there is no fade out, no cross
  dissolve between two scenes and no transition between lines (add them on V1 by hand or with
  the transition commands). The fade is on V1 only: what is above it (a character, a sub CG) is
  not faded.
- **Sprites and sub CGs do not move.** A sub CG appears and disappears with a cut; nothing slides
  in.

## Tests

- `crates/project/src/script.rs`: parsing and defaults, ids and speakers, size caps, the route
  against prerequisites and story time, expression fallback, the checks of `video`, `subCg`,
  `slot` and `transition`, `spoken: false` (still subtitled, warned with a recording or on a direction).
- `crates/audio-dsp/src/lipsync.rs`: silence and nonsense input, bursts at the right times, a
  click shorter than the minimum, three shapes, a wobbling level that must not flutter.
- `crates/engine/src/anime_tests.rs`: the plan with files faked (timing on the frame grid, runs, blinks, mouth, music, subtitles, missing files; a movie's length and sound, a sub CG's time on screen, who stands where in a two-shot and when they blink, a blink waiting for the mouth to close and a sprite's own blink picture), building with real files (tracks, clip counts, one undo step, nothing built from a bad script; a movie cutting the background, its sound, the sub CG's place and size and that it is drawn on top, the two-shot's tracks and mouth positions, a fade that is rendered from black, sprites standing on the bottom with their mouths and blinks and a background filling the frame, checked in the rendered frame), sound words (their length from their sound or their reading, subtitled, nothing synthesized or laid on A1) and effects levelled under the voices, the checks (gaps, overlaps, a quiet voice, a loud effect), the one-call run and its cache, and an export that is opened again and measured.
- `crates/engine/src/script_tools_tests.rs`: validation and refusal, voice picking and writing,
  synthesizing only what is missing and only once (cache, edit, deleted file, force, language,
  corrupt manifest, a changed reference, the script's path written relative, absolute or with
  `..`, a manifest from older builds), lip sync for one file and for a whole script.
