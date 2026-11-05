# Turning a game's scenario into a script

`tools/game2script/game2script.py` converts an adventure game's scenario text into a FilmCraft
[script](script.md), which `anime.run` then turns into a video. It is for personal study of
material you already have (see "Rights" below); FilmCraft does not read a game's own archives.

Every game stores its scenario differently, so the converter does not guess. A **profile** (JSON)
says how a game writes things, in regular expressions; the converter applies it and **reports what
it could not understand**. Getting a new game to convert is a loop: run, read the report, adjust
the profile.

## Before you start

You need, as ordinary files this machine can read:

- the **scenario** as text (UTF-8, or Shift-JIS: the encoding is detected), one file or a folder of
  them, read in natural order (`ch2.txt` before `ch10.txt`);
- the **pictures** (backgrounds, character sprites) as PNG, JPG, WebP or BMP, the **voices**, **music** and **sound
  effects** as WAV, OGG, MP3 or FLAC, in folders for each kind.

Unpacking a game's archive into those files is done with the game's own tools and is out of scope
here.

## Running it

```bash
python tools/game2script/game2script.py --input scenario/ --assets game_files/ --out story.fcscript.json
python tools/game2script/game2script.py --profile my_game.json --input chapter1.txt --assets . \
    --out out/story.fcscript.json --report report.json
```

It prints a summary (scenes, lines, characters, unmatched lines, missing files) and writes the
script. Paths inside the script are relative to the script file, so keep the script and the asset
folders where they can find each other, or write the script beside them. Then:

```json
{"command": "anime.run", "params": {"path": "story.fcscript.json", "pickVoices": true}}
```

## The plain profile

`profiles/plain.json` reads a simple text form, which is also a good target when you convert a
scenario by hand or with another tool:

```
■ s01 屋上                      a scene (id, title)
@pov まりな                     whose viewpoint; @when 1998-04-12T10:00 story time; @requires s00
@bg roof                        a background (a file in bg/)
@bgm calm   /  @bgm_stop        music (a file in bgm/)
夜だった。                      narration
@face まりな smile              from now on, this character's expression
[v001]まりな「待って、[漢字/かんじ]が読めない。」    a line: voice file v001, ruby
まりな（どうして…）             a thought
@se door    /  @wait 0.5        a sound effect (a direction line) / a pause after the line before
```

A change of background or music in the middle of a scene starts the next *part* of that scene (the
script has one of each per scene): `s01`, `s01-p2`, and so on. Sprites are found by name in
`chara/`: `marina_smile.png` is the character `marina`'s `smile`, `marina_smile_open.png` its
open-mouth picture and `marina_smile_blink.png` its blink picture.

## Writing a profile for a new game

Start from the plain one and change only what differs:

```json
{
  "extends": "plain",
  "encoding": "cp932",
  "characters": {
    "まりな": {"id": "marina", "name": "まりな", "voice": {"speaker": "Ono_Anna"}},
    "小次郎": {"id": "kojiro"}
  },
  "assets": {"backgrounds": "image/bg", "voice": "sound/voice"},
  "commands": [{"match": "^\\[背景\\s+(?P<file>\\S+)\\]", "set": "background"}]
}
```

(`characters`, `assets` and `scene` merge with the base; any other key replaces it.) The parts:

| Key | What it is |
|---|---|
| `encoding` | `auto` (UTF-8, then Shift-JIS), or a Python codec name |
| `inputs` | file patterns when `--input` is a folder (default `*.txt`) |
| `scene.start` | A regex with groups `id` and `title`; a line that matches opens a scene |
| `ignore` | Regexes for lines to skip (blank lines, comments) |
| `commands` | `{match, set}`: the line is a command. `set` is `background`, `bgm`, `bgm_stop`, `sfx`, `expression`, `pause`, `pov`, `when`, `requires` or `scene`. Groups: `file`; `speaker` and `expr`; `sec`; `speaker`; `when`; `ids`; `id` and `title` |
| `rules` | `{match, kind}` tried in order: the first that matches makes a line of `kind` `dialogue`, `monologue` or `narration`. Groups: `text`, `speaker` (dialogue and thoughts), `voice` (a voice id) |
| `ruby` | `{match, to}` replacements that turn the game's ruby notation into `｜漢字《かんじ》` |
| `strip` | Regexes removed from the text (control codes, line breaks) |
| `narrator` | A character id given to narration, so it can have its own voice |
| `characters` | The scenario's names mapped to ids, with an optional `voice` (`speaker`/`instruct`, or `refAudio`/`refText`) |
| `assets` | Folder names for `backgrounds`, `sprites`, `bgm`, `se`, `voice`; `image_ext`, `audio_ext`; `sprite_pattern` for the sprite file names |

Commands are tried before rules, so a command line never becomes a spoken line.

## Reading the report

`--report report.json` lists, up to 200 of each:

- `unmatched`: lines no rule matched. These are what the profile still has to learn: add a rule or
  a command for each shape you see, or an `ignore` for the ones that do not matter;
- `unknownSpeakers`: names that were not in `characters` (they got generated ids): map them, or fix
  a pattern that took part of a line for a name;
- `missing`: voice, background, music or sound-effect files that a line names but the assets
  folders do not hold (the line is kept, without the file);
- `unknownExpressions`: `speaker:expression` pairs a line asks for that have no sprite (the
  expression is dropped).

Then check the result with `script.validate`, and look at the first minutes of a built sequence
before running everything.

## What it does not do

- It does not unpack a game's archives, decode its image or audio formats, or run its script
  language. Only text rules.
- It does not decide the order of a game with several viewpoints. The script's `route` is the
  scenes in file order; reorder it, and add `requires` and `when`, as described in
  [script.md](script.md#branching-stories).
- Conditional branches, choices and variables in a scenario are lines like any other to the
  converter: unreachable or alternative text ends up in the script unless the profile ignores it.
- Sprites sit where the picture puts them. A game that positions characters left, right or
  centre with commands gets one character at a time, in the middle.

## Rights

This is meant for personal technical study of material you own. The pictures, sounds, voices and
text are other people's work; making a video from them is a derivative work, and a voice made in
the likeness of a real performer needs their consent. Do not publish what you make from material
you have no right to; see [text-to-speech.md](text-to-speech.md).

## Tests

- `tools/game2script/tests/test_game2script.py` (`python -m unittest discover tools/game2script/tests`):
  a Shift-JIS scenario with every command, scene parts, ruby, voices and sound effects, named
  characters and their sprites, the report of unmatched lines and missing files, natural file
  order, profiles that extend each other, and bad input.
- `crates/engine/src/anime_tests.rs`: a converted scenario passes `script.validate` and builds a
  sequence through `anime.run` (skipped when no Python is on the PATH).
