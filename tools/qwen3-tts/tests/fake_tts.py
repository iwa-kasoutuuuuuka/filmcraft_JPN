#!/usr/bin/env python3
"""A stand-in for filmcraft_tts.py used by FilmCraft's tests: same command line and output
protocol, no model. Each job gets a 0.3 s silent WAV; a text containing FAIL fails; a text
containing CRASH makes the script die without its final line (an error on the Rust side)."""

import argparse
import json
import os
import sys
import time
import wave


def emit(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--probe", action="store_true")
    ap.add_argument("--jobs")
    ap.add_argument("--model")
    ap.add_argument("--device")
    ap.add_argument("--language", default="Japanese")
    a = ap.parse_args()
    if a.probe:
        emit({"ok": True, "fake": True, "torch": "0", "cuda": False, "qwenTts": "fake"})
        return 0
    if not a.jobs:
        ap.error("--jobs or --probe")
    with open(a.jobs, encoding="utf-8") as f:
        jobs = [json.loads(line) for line in f if line.strip()]
    emit({"progress": 0.0, "status": "Loading model (fake)"})
    ok = failed = 0
    for i, j in enumerate(jobs):
        emit({"progress": 0.05 + 0.95 * i / max(len(jobs), 1), "status": f"Synthesising {i + 1}/{len(jobs)}"})
        if "NOISE" in j["text"]:
            # what a real run does on Windows: a console-code-page (cp932) message on stderr and a
            # lot of progress-bar output, and a stdout line that is not UTF-8
            sys.stderr.buffer.write(("'sox' は、内部コマンドまたは外部コマンドとして認識されていません。\n").encode("cp932") * 3)
            sys.stderr.buffer.write(b"x" * 300000 + b"\n")
            sys.stderr.buffer.flush()
            sys.stdout.buffer.write(b"\xff\xfe not json \x80\n")
            sys.stdout.buffer.flush()
        if "HANG" in j["text"]:
            time.sleep(120)
        if "SKIP" in j["text"]:
            continue  # says nothing about this job
        if "CRASH" in j["text"]:
            sys.stderr.write("simulated crash\n")
            sys.stderr.flush()
            os._exit(3)
        if "FAIL" in j["text"]:
            emit({"id": j["id"], "error": "the fake synthesizer fails texts containing FAIL"})
            failed += 1
            continue
        os.makedirs(os.path.dirname(os.path.abspath(j["out"])) or ".", exist_ok=True)
        rate = 24000
        with wave.open(j["out"], "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(rate)
            w.writeframes(b"\x00\x00" * int(rate * 0.3))
        emit({"id": j["id"], "out": os.path.abspath(j["out"]), "sampleRate": rate, "seconds": 0.3, "ok": True})
        ok += 1
    emit({"done": True, "ok": ok, "failed": failed})
    return 0


if __name__ == "__main__":
    sys.exit(main())
