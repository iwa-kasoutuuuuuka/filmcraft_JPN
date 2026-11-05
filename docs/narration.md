# Narration (text-to-speech)

FilmCraft makes natural Japanese narration with two free text-to-speech apps that run on your own
computer:

| Engine | Address (default) | Good for |
|---|---|---|
| **AivisSpeech** | `http://127.0.0.1:10101` | very natural, expressive Japanese; voice models from AivisHub |
| **VOICEVOX** | `http://127.0.0.1:50021` | many well-known characters (ずんだもん, 四国めたん, …) |

FilmCraft only talks to the app over HTTP on this computer. It never bundles, links or starts the
engines: they are separate programs with their own licences, and you install and start them
yourself. Nothing is sent to the internet.

Agents (Claude Code, Antigravity, …): use `tts.synthesize` for narration instead of the
operating system's speech (Windows SAPI sounds robotic). See [For agents](#for-agents).

## Install and start an engine

1. Download and install the app:
   - AivisSpeech: <https://aivis-project.com/> (Windows / macOS). Voice models are added from
     AivisHub inside the app.
   - VOICEVOX: <https://voicevox.hiroshiba.jp/> (Windows / macOS / Linux).
2. Start the app and leave it running (its window can stay in the background). The engine starts
   with it.
3. In FilmCraft, open **Graphics and Titles ▸ Generate Narration…**: the engine shows
   "● running". If it says "not running", start the app and press **Refresh**.

If you changed the engine's port, set it in **Settings ▸ Narration** (`narration.aivisspeechUrl`,
`narration.voicevoxUrl`). Only plain `http://` addresses are accepted.

## The dialog

**Graphics and Titles ▸ Generate Narration…** (ナレーションを生成…):

- **Engine**: AivisSpeech or VOICEVOX, with whether it is running.
- **Lines**: each line has a voice (from the engine's list), the text and the speed. Add, remove
  and reorder lines; **Preview** synthesizes one line to a temporary WAV and opens it in the
  system's audio player; **Terms** shows that voice's terms.
- **Paste a script**: one line of dialogue per line, `話者: セリフ` (or `話者「セリフ」`). Speakers
  are matched to voice names (`ずんだもん`, `四国めたん/ノーマル`, …); lines without a speaker keep
  the previous line's voice.
- **Make captions** and **Pause after each line**.
- **Auto-level** and **Duck other audio under the narration** (both on by default; see
  [Sitting in the mix](#sitting-in-the-mix)).
- **Generate** synthesizes in the background (the window stays usable), then imports and places
  every line on an audio track at the playhead, back to back with the pauses, and adds a caption
  track whose captions are exactly the lines, timed to their audio. The whole thing is one
  **Undo** ("Synthesize Narration").
- **Credit roll at the end** (Settings ▸ Narration ▸ "Add a credit roll of the voices at the
  end", off by default): after generating, add or update a rolling credit of the voices at the
  end of the sequence.
- Afterwards the **credits** of every voice the sequence uses are shown with **Copy Credits**,
  **Add to Credit Roll** (エンドロールに追加) and **Export Description Text** (説明欄用テキストを書き出す).
  See [Credits in the video and the description](#credits-in-the-video-and-the-description).

## Sitting in the mix

Narration placed over a project that already has music would otherwise play at whatever level the
engine produced, on top of the music. `tts.synthesize` therefore mixes it in with the Essential
Sound tools, in the same undo step as the placement:

1. **Auto-level** (Settings ▸ Narration ▸ Auto-level narration, `autoLevel`): the narration clips
   get the **Dialogue** audio type and their integrated loudness (ITU-R BS.1770) is matched to
   the dialogue target, the `audio.dialogueTargetLufs` preference (−23 LUFS by default;
   `dialogueLufs` overrides it for one call). This is `essentialSound.autoMatch`; the match gain
   shows in Essential Sound ▸ Loudness.
2. **Ducking** (Settings ▸ Narration ▸ Duck other audio under the narration, `duck`): the music
   under the narration gets Volume keyframes that lower it by **Duck other audio by** (−15 dB by
   default, −40…0; `duckReduceDb`) while the narration speaks, with fades. This is
   `essentialSound.generateDucking`; the level between the ducks is the clip's highest earlier
   level. Like Premiere, only music is ducked. A clip is ducked when it is enabled, on another
   audio track, overlaps a spoken line (a clip that only sits in a pause between lines is left
   alone), and is
   - typed **Music** or **Ambience**, or
   - has no audio type yet and is *plain audio*: not linked to a video clip (camera audio is
     usually on-camera speech), not a nested sequence, and not narration FilmCraft made (items
     with the `Narration Credit` / `Narration Voice` metadata, or a WAV in the narration
     folder). These become Music.

   Never touched, and listed in `mix.skipped` with the reason:
   - clips on **locked** or **muted** tracks. Linked selection is ignored here, so a linked
     partner on a locked track is never changed either;
   - untyped camera audio, nested sequences and earlier narration;
   - music whose Volume already has keyframes the ducking did not make, such as fades you drew
     ("has volume keyframes; run essentialSound.generateDucking to replace them"). Ducking
     replaces a clip's Volume keyframes, so run it yourself on that clip if you want that.
     Keyframes from an earlier ducking (Ducking enabled in Essential Sound) are regenerated.

   A Music clip whose ducking you already enabled keeps its own **Reduce by** depth unless you
   pass `duckReduceDb` (`mix.keptReduceDb` lists those clips). Dialogue and SFX clips are always
   left alone. When nothing is under the narration, nothing is ducked. To duck camera audio or a
   nested sequence, give it the Music type first.

The narration clips get the Dialogue type whenever either option is on (the ducking listens to
Dialogue). The whole mix is part of the one **Synthesize Narration** undo step, even when the undo
history is full. The result's `mix` says what was done:

```json
"mix": {
  "autoLevel": true, "duck": true,
  "dialogue": [1201, 1202],
  "loudness": {"targetLufs": -23.0, "clips": [{"clip": 1201, "measuredLufs": -31.2, "targetLufs": -23.0, "gainDb": 8.2}, …]},
  "typedMusic": [12, 18],
  "reduceDb": -15.0,
  "ducked": [{"clip": 18, "keyframes": 4, "regions": [[4.0, 6.4]]}, …],
  "skipped": [{"clip": 7, "name": "Interview", "reason": "untyped and linked to video (likely on-camera speech); set its audio type to Music to duck it"}],
  "errors": []
}
```

A step that fails (for example a clip that is silent or shorter than the 0.4 s a loudness
measurement needs) is listed in `errors`; the narration is still placed. To deliver at a
platform's loudness, turn on **Export ▸ Effects ▸ Loudness Normalization** and pick a **Delivery Target** (or pass `loudness: "streaming"` to
`file.exportMedia` / `export.queue.add`; `export.loudnessTargets` lists the targets):

| target | integrated | true peak |
|---|---|---|
| `streaming` (YouTube, Spotify, most streaming services) | −14 LUFS | −1 dBTP |
| `podcast` | −16 LUFS | −1 dBTP |
| `ebuR128` (EBU R 128, European broadcast) | −23 LUFS | −1 dBTP |
| `atscA85` (ATSC A/85, US broadcast) | −24 LKFS | −2 dBTP |
| `aribTrB32` (ARIB TR-B32, Japanese broadcast) | −24 LKFS | −1 dBTP |

The export measures the whole range, applies one gain to reach the target and a true-peak
limiter at the ceiling, so the delivered file no longer peaks at 0 dBFS. `loudnessLufs` and
`truePeakDbtp` set custom numbers.

## Where the audio goes

Each line is a WAV file named `<name>_<n>_<speaker>-<style>.wav` (for example
`Narration_001_ずんだもん-ノーマル.wav`) in:

1. `dir` (command parameter), else
2. Settings ▸ Narration ▸ Folder for narration audio, else
3. `Narration` next to the saved project file, else (unsaved project)
4. `Narration` in FilmCraft's data directory.

Existing files are never overwritten (a free name such as `…_2.wav` is chosen), and Undo does not
delete the files.

## Multiple voices (dialogue)

Every line can have its own voice, so a conversation is one call. Captions of a multi-voice
narration carry the speaker's name (WebVTT `<v Name>`; Captions ▸ Color by Speaker colours them).

```json
{"command": "tts.synthesize", "params": {
  "engine": "voicevox",
  "lines": [
    {"text": "こんにちは、ずんだもんなのだ。", "voice": "ずんだもん"},
    {"text": "四国めたんよ。よろしくね。", "voice": "四国めたん/ノーマル", "pauseAfter": 0.6},
    {"text": "今日は動画の作り方を紹介するのだ。", "voice": "ずんだもん/あまあま", "speed": 1.1}
  ]
}}
```

Or as a script with a cast (script names → voices):

```json
{"command": "tts.synthesize", "params": {
  "engine": "aivisspeech",
  "script": "A: 今日はいい天気ですね。\nB: 散歩に行きましょう。",
  "cast": {"A": "まい", "B": "Anneli/ノーマル"}
}}
```

## Credits and terms

**Every voice has terms. Read them before publishing.**

- **VOICEVOX**: each character requires a credit such as `VOICEVOX:ずんだもん` wherever the
  audio is published (in the video or its description), and each character has its own terms
  (some forbid certain uses). The general terms are at <https://voicevox.hiroshiba.jp/term/>.
- **AivisSpeech**: the software needs no credit, but each **voice model has its own licence**
  (for example ACML, ACML-NC or CC0; some require a credit, some forbid commercial use). Check
  the model's page on AivisHub.

`tts.synthesize` returns `credits` (one per distinct voice) and `creditText` (one per line, ready
to paste):

```json
"credits": [
  {"engine": "voicevox", "speaker": "ずんだもん", "credit": "VOICEVOX:ずんだもん", "required": true,
   "termsUrl": "https://voicevox.hiroshiba.jp/term/", "note": "…",
   "termsCommand": {"command": "tts.voiceTerms", "params": {"engine": "voicevox", "speaker_uuid": "…"}}},
  {"engine": "aivisspeech", "speaker": "まい", "credit": "AivisSpeech:まい", "checkLicense": true,
   "model": "まい", "license": "ACML 1.0", "modelUrl": "https://hub.aivis-project.com/aivm-models/…", "note": "…"}
],
"creditText": "VOICEVOX:ずんだもん\nAivisSpeech:まい"
```

For AivisSpeech the licence comes from the engine's model list (`/aivm_models`) when it reports
one; otherwise the credit says to check the model's licence. `tts.voiceTerms` returns the full
terms text the engine ships for a voice (`policy`). See also [voice-terms.md](voice-terms.md).

## Credits in the video and the description

FilmCraft remembers which voice made every narration item, so the credits can't be forgotten:

- **Where it is stored.** `tts.synthesize` writes two metadata fields on each WAV item it imports,
  saved with the project (no format change):
  - `Narration Credit`: the credit line, for example `VOICEVOX:ずんだもん`. Edit it in the
    Metadata panel when a voice's terms want a longer form (for example
    `VOICEVOX:もち子(cv 明日葉よもぎ)`); the roll and the description text use your text.
  - `Narration Voice`: the whole credit as JSON (engine, speaker, uuid, model, licence, URLs).
- **What is credited.** The narration items **still used in the sequence** (clips on its tracks,
  nested sequences included), one line per voice in timeline order. Narration you deleted from
  the timeline is not credited. `scope: "project"` credits every narration item in the project.
  Credits accumulate over any number of Generate Narration runs.
- **In the video: Graphics and Titles ▸ Add Narration Credit Roll** (`tts.addCreditRoll`) adds a
  graphic clip named **Narration Credits** at the end of the sequence: a heading ("Voices", or
  「音声」 in the Japanese interface), then one credit per line, centred, rolling up from below the
  frame until it has left the top (Roll, start and end off screen). It uses the default text font,
  or the bundled Japanese face when the font is Automatic and the text has Japanese in it. It is
  an ordinary graphic: change its font, size or speed in Essential Graphics. Running the command
  again (or generating more narration) **updates that clip** instead of adding another one: the
  text follows the voices, and it moves to the new end when narration was added after it. Rename
  the clip to stop that. Each add / update is one Undo.
- **Generating with the roll.** `tts.synthesize {creditRoll: true}` (or the dialog's checkbox, or
  the preference) adds or updates the roll after placing the lines, in the same undo step. When a
  roll already exists it is kept up to date after every generation unless `creditRoll: false`.
- **In the description: Graphics and Titles ▸ Export Narration Credits** (`tts.exportCredits`)
  writes a UTF-8 text file with a heading and the credit lines. For AivisSpeech voices whose model
  the engine reported, the model name, licence and AivisHub page are indented under the line:

  ```text
  音声:
  VOICEVOX:ずんだもん
  AivisSpeech:まお
    モデル: まお / ライセンス: ACML 1.0 / https://hub.aivis-project.com/aivm-models/…
  ```

  The default file is `<project name>_credits.txt` next to the saved project, else `credits.txt`
  in the narration folder; an existing file of that name is replaced. `path` picks another file.

## Commands

| command | params | returns |
|---|---|---|
| `tts.engines` | — | `engines: [{id, name, url, running, version, error, default, homeUrl, termsUrl}]` |
| `tts.voices` | `engine?`, `url?`, `refresh?` | `voices: [{engine, speaker, style, id, uuid, voice: "Speaker/Style"}]`, `errors` (engines that did not answer). Without `engine`, every engine that answers. Cached for 30 s. |
| `tts.voiceTerms` | `engine?`, `speaker_uuid` or `voice`, `url?` | `{speaker, uuid, policy, credit, license, model, modelUrl, termsUrl, note}` |
| `tts.preview` | `engine?`, `text`, `voice?`, prosody, `url?` | `{path, seconds, voice, credit}`: a temporary WAV; the project is not changed |
| `tts.synthesize` | see below | `{clips, items, files, dir, track, trackId, captionTrack, captions, start, end, lines, credits, creditText, mix}`, plus `creditRoll` (the `tts.addCreditRoll` result) or `creditRollError` when the roll was added / updated |
| `tts.credits` | `sequence?`, `scope?` (`sequence` \| `project`) | `{credits, creditText}`: the credits of the narration used (same objects as `tts.synthesize`'s `credits`) |
| `tts.addCreditRoll` | `seconds?` (default 4 + 0.5 per line, 5–30), `title?` (default "Voices" / 「音声」; `""` = none), `extra?` (lines added after the credits), `at?` (`"end"` (default), ticks, or `{seconds\|frame\|timecode}`) | `{clip, updated, moved, start, duration, text, credits}`; adds or updates the **Narration Credits** roll; one undo step |
| `tts.exportCredits` | `path?`, `sequence?`, `scope?` | `{path, text, creditText, credits}`: writes the description text |

`tts.synthesize` parameters:

| param | meaning |
|---|---|
| `engine` | `aivisspeech` \| `voicevox` (default: Settings ▸ Narration ▸ Default engine) |
| `lines` | `[{text, voice?, speed?, pitch?, intonation?, volume?, tempoDynamics?, pauseAfter?}]` (strings are accepted too); at most 500 lines, 2000 characters each |
| `script`, `cast` | instead of `lines`: `話者: セリフ` lines, and a map of script names to voices |
| `text` | instead of `lines`: one line |
| `voice` | default voice for lines without one: `"Speaker/Style"`, `"Speaker"` (its first style), a style id, or `{speaker, style}` / `{id}` |
| `speed`, `pitch`, `intonation`, `volume`, `tempoDynamics` | defaults for every line (`speedScale` 0.5–2, `pitchScale` −0.15–0.15, `intonationScale` 0–2, `volumeScale` 0–2, `tempoDynamicsScale` 0–2, AivisSpeech only) |
| `pause` | seconds of silence after each line (default 0.3) |
| `track` | audio track (`"A2"` or id; a name past the last track adds tracks). Default: the first unlocked audio track that is empty over the narration, else a new one |
| `time` / `seconds` / `timecode` | where the first line starts (default: the playhead) |
| `captions` | make captions (default true) |
| `captionTrack` | append to this caption track (`"C1"` or id) instead of a new "Narration" track |
| `captionMaxChars` | split captions longer than this at punctuation (default 40; 0 = never). Break times follow the engine's pauses when it reports mora timings (VOICEVOX), else the characters' share |
| `name` | file name prefix (default `Narration`) |
| `dir` | folder for the WAVs |
| `url` | engine address for this call |
| `autoLevel` | Dialogue type + loudness match (default: Settings ▸ Narration ▸ Auto-level narration, on) |
| `dialogueLufs` | loudness target for this call (default: the `audio.dialogueTargetLufs` preference, −23) |
| `duck` | duck the Music / Ambience / untyped clips on other tracks under the narration (default: Settings ▸ Narration ▸ Duck other audio, on) |
| `duckReduceDb` | how far to duck, −40…0 dB (default: Settings ▸ Narration ▸ Duck other audio by, −15) |
| `creditRoll` | add or update the credit roll after placing (default: Settings ▸ Narration ▸ credit roll, off). An existing roll is updated anyway unless this is `false` |

Notes on the engines' parameters: AivisSpeech uses `intonation` as the strength of the style's
emotion and ignores it for the Normal style; changing `pitch` can lower its quality.

### Errors

- Engine not running: `AivisSpeech is not running at http://127.0.0.1:10101 — start the AivisSpeech app`
  (the dialog shows it in Japanese). Nothing changes in the project.
- Unknown voice: `VOICEVOX has no voice named `X` (tts.voices lists them)`.
- A failed line (engine error, an answer that is not a WAV, a too-large answer) fails the whole
  command and changes nothing. Files already written stay on disk.

## For agents

1. `tts.engines` to see which engine runs (ask the user to start the app if none does).
2. `tts.voices {engine}` to pick voices.
3. `tts.synthesize {engine, lines}` — one call places all the narration with captions.
4. Put the credits in: `tts.addCreditRoll` (in the video) and/or `tts.exportCredits` (text for
   the description). `tts.credits` lists them at any time.
5. Show the user `creditText` and remind them to check each voice's terms
   (`tts.voiceTerms`) before publishing.

## Limits

- The web version cannot reach a local engine (`tts.*` returns "not available in this build").
- Preview opens the WAV in the system's player instead of playing it inside FilmCraft.
- `tts.synthesize` called directly (CLI / MCP / control channel) waits until every line is
  synthesized; the dialog renders in the background.
- Singing styles are not listed (they cannot read text).
