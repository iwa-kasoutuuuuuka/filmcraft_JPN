# Qwen3-TTS helper

Text-to-speech and voice cloning for FilmCraft's `speech.synthesize` command, on
[Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS) (Apache-2.0; Japanese among its ten languages).
FilmCraft itself stays pure Rust: the app runs `filmcraft_tts.py` as an external process and reads
its JSON Lines output, the way ffmpeg is used as a test oracle. Nothing here is linked or shipped
in the binaries; without Python installed the command says so and everything else works.

## Install

```bash
python -m venv .venv
.venv/Scripts/activate            # Windows; `source .venv/bin/activate` elsewhere
pip install torch --index-url https://download.pytorch.org/whl/cu128   # a CUDA build (optional)
pip install -r requirements.txt
python filmcraft_tts.py --probe   # {"ok": true, "cuda": true, "gpu": "...", ...}
```

The model (≈3.4 GB for the 1.7B Base model) downloads from Hugging Face on first use into the
Hugging Face cache. CPU inference works but is slow; a GPU with 8 GB or more is comfortable.

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
speech; without `refText` only the speaker embedding is used (lower quality); without `refAudio`
the model's default voice speaks. Voice prompts are computed once per reference file.

Where the script and Python are found, in order: the command's `python` / `script` params, the
`FILMCRAFT_TTS_PYTHON` / `FILMCRAFT_TTS_SCRIPT` environment variables, `tools/qwen3-tts/` next to
the executable or in the current directory, and the source tree. `speech.synthesizer` reports what
was found and runs `--probe`.

Stand-alone:

```bash
python filmcraft_tts.py --text "こんにちは" --out hello.wav --ref-audio ref.wav --ref-text "参照音声の書き起こし"
python filmcraft_tts.py --jobs jobs.jsonl --device cuda:0
```

## Voices and rights

Cloning a voice reproduces a real person's voice. Use reference audio only with the consent of
the speaker and the rights holder of the recording, and keep what you make within what that
consent covers. The model's licence (Apache-2.0) does not grant any rights in the voices you feed
it.
