# Text-to-speech and voice cloning

`speech.synthesize` turns lines of text into WAV files and media items, in a cloned voice when
given a reference recording. The built-in route is [Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS)
(Apache-2.0, ten languages including Japanese), run as an **external Python process** by the
helper in [`tools/qwen3-tts/`](../tools/qwen3-tts/README.md). FilmCraft stays pure Rust: nothing
of it is linked or shipped, and without Python the command fails with the path it looked at while
everything else keeps working. The companion of [transcripts](transcripts.md) (speech-to-text): a
spoken line transcribed with Whisper gets word times for captions and lip sync.

The use it was built for is the novel / game-script-to-anime pipeline in
[novel-to-anime.md](novel-to-anime.md): lines that have no recording are spoken in the voice of
the lines that do.

## Commands

| Command | What it does |
|---|---|
| `speech.synthesize` | Speak `lines` (`[{id?, text, refAudio?, refText?, language?}]`) or one `text`. Each line becomes `<out>/<id>.wav` (`out` defaults to `<data dir>/tts`; ids are reduced to file-name-safe characters) and, unless `import: false`, a media item (in `bin`). `refAudio` / `refText` / `language` on the call are the defaults for every line. `model`, `device` (`cuda:0`, `cpu`), `python`, `script` pick the model and the interpreter. Returns `lines` (`id`, `path`, `seconds`, `sampleRate`, `item`) and `failed` (`id`, `error`); one undo step. The call blocks until the script is done, like `transcript.generate`. |
| `speech.synthesizer` | What would speak: `installed` (a host-provided synthesizer), else the `python` and `script` found, `scriptFound`, `out`, and the script's `--probe` report (`torch`, `cuda`, `gpu`, `qwenTts`, `error`), summarised as `ok`. |

Example (CLI, control channel or MCP):

```json
{"command": "speech.synthesize", "params": {
  "lines": [
    {"id": "s03-012", "text": "そんなはずはない。", "refAudio": "C:/game/voice/marina_0007.wav", "refText": "待って、まだ話は終わってない。"},
    {"id": "s03-013", "text": "……わかった。", "refAudio": "C:/game/voice/kojiro_0031.wav", "refText": "俺に任せろ。"}
  ],
  "language": "Japanese", "out": "C:/anime/tts"
}}
```

## Voices

A voice is a reference recording and its transcript:

- `refAudio`: a few seconds of clean speech by the speaker (no music, no other voices; a game's
  voice files are ideal). Voice prompts are computed once per file, so a long batch in one voice
  costs one prompt.
- `refText`: what is said in the recording. Without it only the speaker embedding is used
  (`x_vector_only_mode`), which clones less well.
- Neither: the model's default voice.

Emotion follows the reference: a calm reference gives calm lines. For a scene with shouting or
crying, pick a reference line in that register.

Cloning reproduces a real person's voice. Use reference audio only with the consent of the
speaker and the rights holder of the recording, and keep what you make within what that consent
covers; the model's licence grants no rights in the voices you feed it.

## Finding the script

In order: the command's `python` / `script` params; the `FILMCRAFT_TTS_PYTHON` /
`FILMCRAFT_TTS_SCRIPT` environment variables; `tools/qwen3-tts/filmcraft_tts.py` next to the
executable (and up to three directories above it, so a `target/<profile>/` build finds the source
tree), in the current directory, and in the source tree the engine was built from. The interpreter
defaults to `python` on the PATH; a virtual environment's `python.exe` is the usual value. The
script runs with `-I` (isolated: no site customisation or current-directory imports) and UTF-8
I/O.

## The protocol

The script reads a JSON Lines job file (`--jobs`) and writes one JSON object per line on stdout:
`{"progress", "status"}`, `{"id", "out", "sampleRate", "seconds", "ok": true}`, `{"id", "error"}`
and a final `{"done": true, "ok", "failed"}`. The Rust side (`filmcraft_speech::synth`) treats every
value as hostile: line counts and text lengths are capped (2 000 lines, 4 000 characters), sample
rates and durations are range-checked, results for unknown ids are dropped, stderr is drained on
its own thread, and a script that exits without its final line is an error carrying the tail of
its stderr. Cancelling from the progress callback kills the process. Any program that speaks this
protocol can stand in for the Qwen3-TTS script (`tools/qwen3-tts/tests/fake_tts.py` is the one
the tests use), so another model is a new script, not a new build.

## Tests

- `crates/speech/src/synth.rs`: event parsing (including implausible sample rates and durations),
  the fixed synthesizer, input checks and cancellation, and the external runner against the fake
  script (skipped when no `python` is on the PATH): results, failures, progress, probe, a
  simulated crash, a missing script and a missing interpreter.
- `crates/engine/src/speech_synth_tests.rs`: lines to WAV files and media items in one undo
  step, `import: false`, odd ids (`../evil/名前 ?` becomes `..evil名前.wav` inside `out`),
  duplicate ids, empty input, every line failing, and the errors when the script or Python is
  missing.
