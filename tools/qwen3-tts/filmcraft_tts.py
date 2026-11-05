#!/usr/bin/env python3
"""Text-to-speech helper for FilmCraft, on Qwen3-TTS (Apache-2.0, QwenLM/Qwen3-TTS).

FilmCraft is pure Rust and never links Python; it runs this script as an external process
(`filmcraft_speech::ExternalSynthesizer`) the way ffmpeg is used as a test oracle. The script
reads jobs, synthesises each one into a WAV file and reports on stdout, one JSON object per line:

    {"progress": 0.25, "status": "Loading model"}            progress (fraction, text)
    {"id": "l12", "out": "...wav", "sampleRate": 24000,
     "seconds": 1.83, "ok": true}                            one job done
    {"id": "l13", "error": "..."}                            one job failed (the rest go on)
    {"done": true, "ok": 7, "failed": 1}                     the last line

Jobs come from a JSON Lines file (`--jobs`), one object per line:

    {"id": "l12", "text": "……", "out": "C:/tts/l12.wav",
     "refAudio": "C:/game/voice/marina_0007.wav", "refText": "その台詞の書き起こし",
     "language": "Japanese"}

`refAudio` + `refText` clone that voice (a few seconds of clean speech) with the Base model;
`refText` left out uses the speaker embedding alone (`x_vector_only_mode`, lower quality). Without
`refAudio`, `speaker` names one of the CustomVoice model's preset voices (Ono_Anna is the Japanese
one; `--list-speakers` prints them), and `instruct` is a free-text direction for it ("angry, low");
neither: the preset for the line's language. `language` defaults to `--language`; "Auto" lets the
model detect it. Models load once each, on first use; voice prompts are computed once per
reference and reused across the batch.

    python filmcraft_tts.py --probe                               check the install (no model load)
    python filmcraft_tts.py --list-speakers                      the preset speakers (loads CustomVoice)
    python filmcraft_tts.py --jobs jobs.jsonl [--model …] [--device cuda:0|cpu]
    python filmcraft_tts.py --text "こんにちは" --out hello.wav --ref-audio ref.wav --ref-text "…"

Install (CUDA build of torch first if you have a GPU): `pip install -r requirements.txt`.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time

# Quiet the libraries before they are imported: no download / generation progress bars (tqdm
# writes them to stderr, and under a host that pipes stderr that write can fail with EINVAL on
# Windows) and only errors from transformers. The host reads this script's stdout lines only.
os.environ.setdefault("HF_HUB_DISABLE_PROGRESS_BARS", "1")
os.environ.setdefault("TQDM_DISABLE", "1")
os.environ.setdefault("TRANSFORMERS_VERBOSITY", "error")
os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")

MODEL_DEFAULT = "Qwen/Qwen3-TTS-12Hz-1.7B-Base"
CUSTOM_MODEL_DEFAULT = "Qwen/Qwen3-TTS-12Hz-1.7B-CustomVoice"
LANGUAGE_DEFAULT = "Japanese"
# The preset speaker for a language when a line names neither a reference nor a speaker.
DEFAULT_SPEAKERS = {"japanese": "Ono_Anna", "english": "Ryan", "chinese": "Vivian", "korean": "Sohee"}
MAX_TEXT_CHARS = 4000
MAX_JOBS = 2000


def where(e: BaseException) -> str:
    """The innermost frame of an exception, `file:line in function`, for error messages."""
    import traceback  # noqa: WPS433

    frames = traceback.extract_tb(e.__traceback__)
    if not frames:
        return "?"
    f = frames[-1]
    return f"{os.path.basename(f.filename)}:{f.lineno} in {f.name}"


def emit(obj: dict) -> None:
    """One JSON object per stdout line, flushed so the host sees progress as it happens."""
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def probe() -> int:
    """Report what is installed without loading a model (fast; the host calls this first)."""
    out: dict = {"script": os.path.abspath(__file__), "python": sys.version.split()[0], "model": MODEL_DEFAULT, "customModel": CUSTOM_MODEL_DEFAULT}
    try:
        import torch  # noqa: WPS433

        out["torch"] = torch.__version__
        out["cuda"] = bool(torch.cuda.is_available())
        if out["cuda"]:
            out["gpu"] = torch.cuda.get_device_name(0)
    except Exception as e:  # noqa: BLE001
        out["torch"] = None
        out["error"] = f"torch is not installed: {e}"
    try:
        import qwen_tts  # noqa: F401, WPS433

        out["qwenTts"] = getattr(qwen_tts, "__version__", "installed")
    except Exception as e:  # noqa: BLE001
        out["qwenTts"] = None
        out.setdefault("error", f"qwen-tts is not installed: {e}")
    out["ok"] = "error" not in out
    emit(out)
    return 0 if out["ok"] else 2


def read_jobs(path: str) -> list[dict]:
    jobs: list[dict] = []
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue
            try:
                j = json.loads(line)
            except json.JSONDecodeError as e:
                raise SystemExit(f"{path}:{n}: not JSON: {e}") from e
            if not isinstance(j, dict):
                raise SystemExit(f"{path}:{n}: each line must be an object")
            jobs.append(j)
            if len(jobs) > MAX_JOBS:
                raise SystemExit(f"{path}: more than {MAX_JOBS} jobs")
    return jobs


def check_job(j: dict, i: int, default_language: str) -> dict | None:
    """Normalise one job; returns an error object instead when it cannot run."""
    jid = str(j.get("id", i))
    text = j.get("text")
    out = j.get("out")
    if not isinstance(text, str) or not text.strip():
        return {"id": jid, "error": "`text` is required"}
    if len(text) > MAX_TEXT_CHARS:
        return {"id": jid, "error": f"`text` is longer than {MAX_TEXT_CHARS} characters"}
    if not isinstance(out, str) or not out:
        return {"id": jid, "error": "`out` (the WAV path to write) is required"}
    ref = j.get("refAudio")
    if ref is not None and (not isinstance(ref, str) or not os.path.isfile(ref)):
        return {"id": jid, "error": f"`refAudio` is not a file: {ref}"}
    ref_text = j.get("refText")
    if ref_text is not None and not isinstance(ref_text, str):
        return {"id": jid, "error": "`refText` must be a string"}
    for k in ("speaker", "instruct"):
        if j.get(k) is not None and not isinstance(j[k], str):
            return {"id": jid, "error": f"`{k}` must be a string"}
    lang = j.get("language") or default_language
    speaker = j.get("speaker")
    if ref is None and not speaker:
        speaker = DEFAULT_SPEAKERS.get(str(lang).lower(), "Ono_Anna")
    j.update({"id": jid, "text": text, "out": out, "refAudio": ref, "refText": ref_text, "language": lang, "speaker": speaker, "instruct": j.get("instruct") or None})
    return None


def load_model(model_id: str, device: str | None):
    import torch  # noqa: WPS433
    from qwen_tts import Qwen3TTSModel  # noqa: WPS433

    if device is None:
        device = "cuda:0" if torch.cuda.is_available() else "cpu"
    dtype = torch.bfloat16 if device.startswith("cuda") else torch.float32
    kwargs = {"device_map": device, "dtype": dtype}
    # Flash attention only where it is installed; the model runs without it.
    try:
        import flash_attn  # noqa: F401, WPS433

        kwargs["attn_implementation"] = "flash_attention_2"
    except Exception:  # noqa: BLE001
        pass
    return Qwen3TTSModel.from_pretrained(model_id, **kwargs), device


class Models:
    """The Base (clone) and CustomVoice (preset) models, each loaded on first use."""

    def __init__(self, base_id: str, custom_id: str, device: str | None):
        self.ids = {"base": base_id, "custom": custom_id}
        self.device = device
        self.loaded: dict[str, object] = {}
        # a model that failed to load (after its retry) is not tried again for every remaining job
        self.failed: dict[str, BaseException] = {}

    def get(self, kind: str):
        if kind in self.failed:
            raise self.failed[kind]
        if kind not in self.loaded:
            # The first load also downloads the weights. On Windows the Hugging Face cache can
            # fail once while linking the freshly downloaded blobs (WinError 1314, no symlink
            # privilege) and succeed on the next attempt, so one retry is cheap insurance.
            for attempt in (1, 2):
                emit({"progress": 0.0, "status": f"Loading {self.ids[kind]}" + (" (retry)" if attempt == 2 else "")})
                t0 = time.time()
                try:
                    model, device = load_model(self.ids[kind], self.device)
                    break
                except Exception as e:  # noqa: BLE001
                    if attempt == 2:
                        self.failed[kind] = e
                        raise
                    emit({"progress": 0.0, "status": f"Loading {self.ids[kind]} failed ({type(e).__name__}); retrying once"})
            self.loaded[kind] = model
            emit({"progress": 0.05, "status": f"{self.ids[kind]} loaded on {device} in {time.time() - t0:.1f} s"})
        return self.loaded[kind]


def list_speakers(custom_id: str, device: str | None) -> int:
    model = Models(MODEL_DEFAULT, custom_id, device).get("custom")
    emit({"model": custom_id, "speakers": list(model.get_supported_speakers()), "languages": list(model.get_supported_languages())})
    return 0


def run_jobs(jobs: list[dict], model_id: str, custom_id: str, device: str | None, default_language: str) -> int:
    import soundfile as sf  # noqa: WPS433

    models = Models(model_id, custom_id, device)
    prompts: dict[tuple, object] = {}
    ok = failed = 0
    total = max(len(jobs), 1)
    for i, j in enumerate(jobs):
        err = check_job(j, i, default_language)
        if err:
            emit(err)
            failed += 1
            continue
        emit({"progress": 0.05 + 0.95 * i / total, "status": f"Synthesising {i + 1}/{len(jobs)}"})
        try:
            key = (j["refAudio"], j["refText"])
            if j["refAudio"] is None:
                kwargs = {"text": j["text"], "language": j["language"], "speaker": j["speaker"]}
                if j["instruct"]:
                    kwargs["instruct"] = j["instruct"]
                wavs, sr = models.get("custom").generate_custom_voice(**kwargs)
            else:
                model = models.get("base")
                if key not in prompts:
                    prompts[key] = model.create_voice_clone_prompt(
                        ref_audio=j["refAudio"], ref_text=j["refText"] or "", x_vector_only_mode=j["refText"] is None
                    )
                wavs, sr = model.generate_voice_clone(text=j["text"], language=j["language"], voice_clone_prompt=prompts[key])
            wav = wavs[0]
            os.makedirs(os.path.dirname(os.path.abspath(j["out"])) or ".", exist_ok=True)
            sf.write(j["out"], wav, sr, subtype="PCM_16")
            emit({"id": j["id"], "out": os.path.abspath(j["out"]), "sampleRate": int(sr), "seconds": len(wav) / float(sr), "ok": True})
            ok += 1
        except Exception as e:  # noqa: BLE001
            emit({"id": j["id"], "error": f"{type(e).__name__}: {e} (at {where(e)})"})
            failed += 1
    emit({"done": True, "ok": ok, "failed": failed})
    return 0 if failed == 0 else 1


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--probe", action="store_true", help="report the install as JSON and exit")
    ap.add_argument("--jobs", help="JSON Lines file of jobs")
    ap.add_argument("--text", help="one line to synthesise (with --out)")
    ap.add_argument("--out", help="WAV path for --text")
    ap.add_argument("--ref-audio", help="reference speech to clone (with --text)")
    ap.add_argument("--ref-text", help="transcript of --ref-audio")
    ap.add_argument("--language", default=LANGUAGE_DEFAULT, help=f"default language (default {LANGUAGE_DEFAULT}; Auto = detect)")
    ap.add_argument("--model", default=MODEL_DEFAULT, help=f"Base (voice clone) model: Hugging Face id or local directory (default {MODEL_DEFAULT})")
    ap.add_argument("--custom-model", default=CUSTOM_MODEL_DEFAULT, help=f"CustomVoice (preset speakers) model (default {CUSTOM_MODEL_DEFAULT})")
    ap.add_argument("--list-speakers", action="store_true", help="print the CustomVoice model's preset speakers and languages as JSON")
    ap.add_argument("--speaker", help="preset speaker for --text without --ref-audio")
    ap.add_argument("--instruct", help="free-text direction for a preset speaker (with --text)")
    ap.add_argument("--device", help="cuda:0 / cpu (default: CUDA if available)")
    a = ap.parse_args(argv)
    # the host starts Python with -X utf8; this covers a script started by hand on a code-page console
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except Exception:  # noqa: BLE001
            pass

    if a.probe:
        return probe()
    if a.list_speakers:
        return list_speakers(a.custom_model, a.device)
    if a.jobs:
        jobs = read_jobs(a.jobs)
    elif a.text:
        if not a.out:
            ap.error("--text needs --out")
        jobs = [{"id": "0", "text": a.text, "out": a.out, "refAudio": a.ref_audio, "refText": a.ref_text, "speaker": a.speaker, "instruct": a.instruct}]
    else:
        ap.error("pass --probe, --jobs or --text")
        return 2
    if not jobs:
        emit({"done": True, "ok": 0, "failed": 0})
        return 0
    return run_jobs(jobs, a.model, a.custom_model, a.device, a.language)


if __name__ == "__main__":
    sys.exit(main())
