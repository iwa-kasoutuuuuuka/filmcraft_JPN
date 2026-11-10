#!/usr/bin/env python3
"""Voice styles: a reference recording per emotion label for every cloned character (docs/script.md,
"Voice styles"), so that a line labelled 怒り（強） is cloned from an angry recording of its speaker.

    pick    for each character's voice folder, label its recordings (emotion_labels: the expression the
            game shows with the line, else the text) and pick per label the best 4-12 s clip whose
            recording says its text (checked with two recognisers, as tts_readings does); where no
            single clip is long enough, join up to three short ones (1.5-4 s) with 0.3 s of silence.
            Needs the Qwen3-TTS venv and the recogniser models (see tts_readings.py).
    apply   write `voice.styles` into scripts, for characters whose voice is cloned from a recording
            in the dump or that an alias names (narrator=koj). Standard library only.

    python voice_styles.py pick --root DUMP --expressions eve_expressions.json --voice-expr voice_expr.json \\
        --exclude test_clips.txt --out styles.json --wav-dir joined/
    python voice_styles.py apply --styles styles.json --script ep01.fcscript.json --alias narrator=koj --alias koj=koj --out ep01.styled.json

Treats every input as hostile (sizes and counts capped). Nothing but `voice.styles` is written.
"""

from __future__ import annotations

import argparse
import csv
import io
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import emotion_labels as el  # noqa: E402

LABELS = ("怒り", "驚き", "問い", "静か", "呆れ", "喜び", "甘え", "動揺")
STRONG = ("怒り", "驚き", "動揺")
VOICE_DIR = "03_音声/ボイス"
TABLE = "00_目録/ボイスと台詞の対応表.tsv"
MAX_CANDIDATES = 50_000
NOT_PEOPLE = {"eve", "sfx", "bgm"}
MAX_CHECKS_PER_LABEL = 12
FOLDER_RE = re.compile(r"03_音声/ボイス/([^/\\]+)[/\\]")


def folder_of(ref_audio) -> str | None:
    """The dump's voice folder a reference lives in (`.../03_音声/ボイス/aka_茜/aka0140.wav` → aka_茜)."""
    if not isinstance(ref_audio, str):
        return None
    m = FOLDER_RE.search(ref_audio.replace("\\", "/"))
    return m.group(1) if m else None


def prefix_of(folder: str) -> str:
    return folder.split("_", 1)[0]


def rank_key(c: dict, label: str, ideal: float = 7.0) -> tuple:
    """Sort key of a candidate clip for a label: the strength that suits the label, a label from the
    picture over one from the text, a length near `ideal`, then a small recogniser mismatch."""
    strong_wanted = label in STRONG
    strength_off = (c.get("strength") != "強") if strong_wanted else (c.get("strength") == "強")
    return (strength_off, c.get("src") != "expression", abs(float(c.get("dur", 0)) - ideal), float(c.get("per", 1.0)))


def plan_join(shorts: list[dict], need: float = 4.0, most: int = 3) -> list[dict]:
    """Up to `most` different short clips (already ranked) whose total length reaches `need` seconds, else []."""
    out, total, seen = [], 0.0, set()
    for c in shorts:
        if len(out) >= most:
            break
        w = c.get("wav")
        if w is not None and w in seen:
            continue
        seen.add(w)
        out.append(c)
        total += float(c.get("dur", 0)) + 0.3
        if total >= need:
            return out
    return []


def apply_styles(script: dict, styles: dict, aliases: dict[str, str]) -> list[tuple[str, str, int]]:
    """Set `voice.styles` of every cloned character that has styles; returns (character, prefix, count)."""
    done = []
    chars = script.get("characters") if isinstance(script.get("characters"), dict) else {}
    cloned = {}
    for cid, c in list(chars.items())[:500]:
        v = c.get("voice") if isinstance(c, dict) else None
        if isinstance(v, dict) and isinstance(v.get("refAudio"), str) and v["refAudio"].strip():
            cloned[cid] = v  # a preset or designed voice without a recording of the character is left out

    def own_prefix(cid: str, v: dict) -> str | None:
        if cid in aliases:
            return aliases[cid]
        f = folder_of(v["refAudio"])
        return prefix_of(f) if f else None

    # a character cloned from the same recording as another (the narrator reads with the point of
    # view's voice) takes that one's voice folder
    by_ref = {}
    for cid, v in cloned.items():
        p = own_prefix(cid, v)
        if p:
            by_ref.setdefault(os.path.basename(v["refAudio"].replace("\\", "/")), p)
    for cid, v in cloned.items():
        pre = own_prefix(cid, v) or by_ref.get(os.path.basename(v["refAudio"].replace("\\", "/")))
        st = styles.get(pre) if pre else None
        if not isinstance(st, dict) or not st:
            continue
        out = {}
        for lab, r in list(st.items())[:64]:
            if lab in LABELS and isinstance(r, dict) and isinstance(r.get("refAudio"), str) and isinstance(r.get("refText"), str):
                out[lab] = {"refAudio": r["refAudio"], "refText": r["refText"]}
        if out:
            v["styles"] = out
            done.append((cid, pre, len(out)))
    return done


# ---------------------------------------------------------------- pick (needs the venv)

def candidates(root: str, expressions: dict, voice_expr: dict, exclude: set[str], lo: float = 1.5, hi: float = 12.0) -> list[dict]:
    import soundfile as sf  # noqa: WPS433

    out = []
    folders = {}
    vd = os.path.join(root, VOICE_DIR)
    for f in os.listdir(vd):
        # people only: a folder `<code>_<name>` whose code is three letters (not se_効果音, j_擬音, ...)
        if os.path.isdir(os.path.join(vd, f)) and re.fullmatch(r"[a-z]{3}_.+", f) and prefix_of(f) not in NOT_PEOPLE:
            folders[prefix_of(f)] = f
    with io.open(os.path.join(root, TABLE), encoding="utf-8-sig") as f:
        for r in csv.reader(f, delimiter="\t"):
            if len(out) >= MAX_CANDIDATES:
                break
            if len(r) < 6 or not r[5] or r[5].startswith("（効果音"):
                continue
            wav = r[0].replace(".ahx", ".wav").replace(".adx", ".wav")
            pre = re.match(r"[a-z]+", wav)
            if not pre or pre.group(0) not in folders or wav in exclude:
                continue
            p = os.path.join(vd, folders[pre.group(0)], wav)
            if not os.path.isfile(p):
                continue
            try:
                dur = sf.info(p).duration
            except Exception:  # noqa: BLE001
                continue
            if not lo <= dur <= hi:
                continue
            expr = voice_expr.get(wav)
            line = {"speaker": pre.group(0), "text": r[5], "expression": expr[1] if isinstance(expr, list) and len(expr) > 1 else None}
            lab, st, src = el.label_line(line, expressions)
            if lab in LABELS:
                out.append({"prefix": pre.group(0), "wav": wav, "path": p, "text": r[5], "dur": round(dur, 2), "label": lab, "strength": st, "src": src})
    return out


def join_wavs(paths: list[str], out: str) -> None:
    import numpy as np  # noqa: WPS433
    import soundfile as sf  # noqa: WPS433

    parts, sr = [], 48000
    for p in paths:
        a, sr = sf.read(p, always_2d=True)
        parts += [a.mean(axis=1), np.zeros(int(sr * 0.3))]
    sf.write(out, np.concatenate(parts[:-1]), sr, subtype="PCM_16")


def cmd_pick(a) -> int:
    sys.path.insert(0, HERE)
    import tts_readings as tr  # noqa: WPS433

    expressions = el.load_json(a.expressions) if a.expressions else {}
    voice_expr = el.load_json(a.voice_expr) if a.voice_expr else {}
    exclude = set()
    if a.exclude:
        with io.open(a.exclude, encoding="utf-8") as f:
            exclude = {x.strip() for x in f if x.strip()}
    names = el.load_json(a.names) if a.names else {}
    cands = candidates(a.root, expressions, voice_expr, exclude)
    print(json.dumps({"candidates": len(cands)}), file=sys.stderr, flush=True)
    reader = tr.Reader({k: v for k, v in names.items() if isinstance(v, str)}, {})
    ears = tr.Ears(a.whisper, a.ctc, a.device)

    def per_of(c):
        if "per" not in c:
            exp, _ = tr.norm_tokens(reader.phonemes(c["text"]))
            heard = ears.text(c["path"])
            pw = tr.align_ok(exp, tr.norm_tokens(reader.phonemes(heard))[0])[0] / max(1, len(exp))
            pc = tr.align_ok(exp, tr.norm_tokens(ears.phonemes(c["path"]))[0])[0] / max(1, len(exp))
            c["per"] = round(min(pw, pc), 3)
        return c["per"]

    os.makedirs(a.wav_dir, exist_ok=True)
    styles, report = {}, []
    for pre in sorted({c["prefix"] for c in cands}):
        for lab in LABELS:
            pool = [c for c in cands if c["prefix"] == pre and c["label"] == lab]
            longs = sorted((c for c in pool if c["dur"] >= 4.0), key=lambda c: rank_key(c, lab))
            pick = None
            for c in longs[:MAX_CHECKS_PER_LABEL]:
                if per_of(c) <= a.max_error:
                    pick = {"refAudio": c["path"], "refText": c["text"], "files": [c["wav"]], "seconds": c["dur"], "per": c["per"]}
                    break
            if pick is None:
                shorts = [c for c in sorted((c for c in pool if c["dur"] < 4.0), key=lambda c: rank_key(c, lab, 3.0))[:MAX_CHECKS_PER_LABEL] if per_of(c) <= a.max_error]
                j = plan_join(shorts)
                if j:
                    out = os.path.join(a.wav_dir, f"{pre}_{lab}.wav")
                    join_wavs([c["path"] for c in j], out)
                    pick = {"refAudio": out.replace("\\", "/"), "refText": "".join(c["text"] for c in j), "files": [c["wav"] for c in j],
                            "seconds": round(sum(c["dur"] for c in j) + 0.3 * (len(j) - 1), 2), "per": max(c["per"] for c in j)}
            if pick:
                styles.setdefault(pre, {})[lab] = pick
            report.append({"prefix": pre, "label": lab, "pool": len(pool), "picked": pick["files"] if pick else None})
            print(json.dumps(report[-1], ensure_ascii=False), file=sys.stderr, flush=True)
    with io.open(a.out, "w", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps(styles, ensure_ascii=False, indent=1))
    print(json.dumps({"characters": len(styles), "styles": sum(len(v) for v in styles.values()), "out": a.out}, ensure_ascii=False))
    return 0


def cmd_apply(a) -> int:
    try:
        styles = el.load_json(a.styles)
        script = el.load_json(a.script)
    except (OSError, ValueError) as e:
        print(f"cannot read input: {e}", file=sys.stderr)
        return 2
    if not isinstance(styles, dict) or not isinstance(script, dict):
        print("bad input: the styles and the script are JSON objects", file=sys.stderr)
        return 2
    aliases = {}
    for x in a.alias or []:
        if "=" in x:
            k, v = x.split("=", 1)
            aliases[k.strip()] = v.strip()
    done = apply_styles(script, styles, aliases)
    with io.open(a.out, "w", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps(script, ensure_ascii=False, indent=1))
    print(json.dumps({"styled": [{"character": c, "voices": p, "styles": n} for c, p, n in done], "out": a.out}, ensure_ascii=False))
    return 0


def main(argv=None) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except Exception:  # noqa: BLE001
            pass
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("pick")
    p.add_argument("--root", required=True)
    p.add_argument("--expressions")
    p.add_argument("--voice-expr", help="JSON {wav: [speaker, expression, text]} from the scripts")
    p.add_argument("--exclude", help="file names (one per line) not to use, such as test lines")
    p.add_argument("--names", help="JSON {name: kana}")
    p.add_argument("--out", required=True)
    p.add_argument("--wav-dir", required=True)
    p.add_argument("--max-error", type=float, default=0.12)
    p.add_argument("--whisper", default="kotoba-tech/kotoba-whisper-v2.0-faster")
    p.add_argument("--ctc", default="prj-beatrice/japanese-hubert-base-phoneme-ctc-v4")
    p.add_argument("--device", default="cuda")
    q = sub.add_parser("apply")
    q.add_argument("--styles", required=True)
    q.add_argument("--script", required=True)
    q.add_argument("--alias", action="append", help="character=voice prefix (narrator=koj)")
    q.add_argument("--out", required=True)
    a = ap.parse_args(argv)
    return cmd_pick(a) if a.cmd == "pick" else cmd_apply(a)


if __name__ == "__main__":
    sys.exit(main())
