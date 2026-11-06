# Qwen3-TTS helper

Text-to-speech and voice cloning for FilmCraft's `speech.synthesize` command, on
[Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS) (Apache-2.0; Japanese among its ten languages).
FilmCraft itself stays pure Rust: the app runs `filmcraft_tts.py` as an external process and reads
its JSON Lines output, the way ffmpeg is used as a test oracle. Nothing here is linked or shipped
in the binaries; without Python installed the command says so and everything else works.

## Install

```bash
python -m venv .venv              # add --system-site-packages to reuse a torch you already have
.venv/Scripts/activate            # Windows; `source .venv/bin/activate` elsewhere
pip install torch --index-url https://download.pytorch.org/whl/cu128   # a CUDA build (optional)
pip install -r requirements.txt
python filmcraft_tts.py --probe   # {"ok": true, "cuda": true, "gpu": "...", ...}
```

Each model (≈3.4 GB for the 1.7B Base and CustomVoice models) downloads from Hugging Face on
first use into the Hugging Face cache; a batch that mixes cloned and preset voices uses both.
CPU inference works but is slow; a GPU with 8 GB or more is comfortable (on an RTX 5080 a model
loads in 10–20 s and a line of a few seconds takes about a second). The `SoX could not be found`
and `flash-attn is not installed` messages at start-up are harmless: neither is needed. Progress
bars are off, so a host that reads the script's output sees only its JSON lines.

## Use

From FilmCraft (CLI, control channel, MCP):

```json
{"command": "speech.synthesize", "params": {
  "lines": [
    {"id": "l12", "text": "そんなはずはない。", "refAudio": "C:/game/voice/marina_0007.wav", "refText": "待って、まだ話は終わってない。"},
    {"id": "l13", "text": "……わかった。", "refAudio": "C:/game/voice/kojiro_0031.wav", "refText": "俺に任せろ。"}
  ],
  "language": "Japanese", "out": "C:/anime/tts"
}}
```

Every line becomes a WAV file in `out` (default `<data dir>/tts`) and a media item in the
project. `refAudio` + `refText` (its transcript) clone that voice from a few seconds of clean
speech; without `refText` only the speaker embedding is used (lower quality). Without `refAudio`,
`speaker` picks a preset voice of the CustomVoice model (`Ono_Anna` is the Japanese one;
`--list-speakers` prints them all) and `instruct` directs it ("怒った声で"); with neither, the
preset for the line's language speaks. `design` (a job field, or `--design` on the command line)
describes a voice that does not exist and the VoiceDesign model makes it up; there is no Japanese
male preset, so this is how a male extra gets one. Make it once and clone it: speak one sentence
with `--design`, then use that WAV and its text as the `refAudio` / `refText` of the character
(a designed voice drifts from line to line; a cloned one does not). Voice prompts are computed
once per reference file, and each model loads once, on first use.

Where the script and Python are found, in order: the command's `python` / `script` params, the
`FILMCRAFT_TTS_PYTHON` / `FILMCRAFT_TTS_SCRIPT` environment variables, `tools/qwen3-tts/` next to
the executable or in the current directory, and the source tree. `speech.synthesizer` reports what
was found and runs `--probe`.

Stand-alone:

```bash
python filmcraft_tts.py --text "こんにちは" --out hello.wav --ref-audio ref.wav --ref-text "参照音声の書き起こし"
python filmcraft_tts.py --text "こんにちは" --out hello.wav --speaker Ono_Anna --instruct "眠そうに"
python filmcraft_tts.py --text "こちらは警備室です。" --out guard_ref.wav --design "40代の男性警備員、低く落ち着いた声"
python filmcraft_tts.py --jobs jobs.jsonl --device cuda:0
python filmcraft_tts.py --list-speakers
```

## Voices and rights

Cloning a voice reproduces a real person's voice. Use reference audio only with the consent of
the speaker and the rights holder of the recording, and keep what you make within what that
consent covers. The model's licence (Apache-2.0) does not grant any rights in the voices you feed
it.
