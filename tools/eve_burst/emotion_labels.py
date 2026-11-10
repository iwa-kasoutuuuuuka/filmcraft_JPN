#!/usr/bin/env python3
"""Emotion labels for script lines, written to `instruct` as "<label>（<strength>）" (for example 怒り（強）).

A synthesizer that knows the label can pick a matching reference clip (Qwen3-TTS cloning) or style
(Style-Bert-VITS2) per line; see docs/eve-burst-tts-quality.md §10. Labels come from, in order:

  1. a label already in `instruct` (kept unless --overwrite);
  2. the character's expression: a map from the expression name's number (aka06ad → "06") to a label,
     given per character in --expressions (茜: 06 怒り, 07 動揺, ...);
  3. the text: （…） inner voice → 静か, ！！ or a shout → 怒り, ！？ → 驚き, ？ → 問い, ‥‥ → 静か, 笑い → 喜び.

The strength is 強 for doubled marks (！！, ！？) or a long ー/ぉ, 弱 for a trailing ‥‥ in a quiet line, else 中.

    python emotion_labels.py --script ep01.fcscript.json --expressions eve_expressions.json --out ep01.emo.json
    python emotion_labels.py --script ep01.fcscript.json --report labels.tsv --out ep01.emo.json

Standard library only. Nothing but `instruct` is written; lines with a recording are skipped unless --all.
Input is treated as hostile (sizes and counts are capped).
"""

from __future__ import annotations

import argparse
import io
import json
import os
import re
import sys

MAX_LINES = 200_000
MAX_TEXT = 4_000
LABELS = ("普通", "怒り", "驚き", "問い", "呆れ", "喜び", "甘え", "動揺", "静か")
STRENGTHS = ("弱", "中", "強")
INSTRUCT = re.compile(r"^(" + "|".join(LABELS) + r")（(" + "|".join(STRENGTHS) + r")）")
EXPR_NUM = re.compile(r"(\d\d)")
LAUGH = re.compile(r"(えへ|エヘ|うふ|ウフ|ふふ|フフ|はは|ハハ|へへ|ヘヘ)")
SHOUT = re.compile(r"(野郎|バカ|ばか|うるさい|失せ|ふざけ|黙れ|コラ|こら|くそ|クソ)")


def parse_instruct(instruct) -> tuple[str, str] | None:
    """(label, strength) of an instruct that holds a label, else None."""
    if not isinstance(instruct, str):
        return None
    m = INSTRUCT.match(instruct.strip())
    return (m.group(1), m.group(2)) if m else None


def format_label(label: str, strength: str) -> str:
    return f"{label}（{strength}）"


def strength_of(text: str, label: str) -> str:
    if re.search(r"[！!]{2,}|！？|？！|[!?]{2,}|[ーぉぅぃ～]{2,}", text):
        return "強"
    if label in ("静か", "普通", "問い") and re.search(r"[‥…]{2,}。?）?$", text):
        return "弱"
    return "中"


def label_from_text(text: str, kind: str | None) -> str:
    t = text.strip()
    if kind == "monologue" or (t.startswith("（") and t.endswith("）")):
        return "静か"
    excl = len(re.findall(r"[！!]", t))
    ques = len(re.findall(r"[？?]", t))
    if LAUGH.search(t) and excl == 0:
        return "喜び"
    if excl and ques and re.search(r"！？|？！", t):
        return "驚き"
    if excl >= 2 or (excl and SHOUT.search(t)):
        return "怒り"
    if excl:
        return "驚き" if len(re.sub(r"[^぀-ヿ一-鿿]", "", t)) <= 6 else "怒り"
    if ques:
        return "問い"
    if re.search(r"[‥…]{2,}", t):
        return "静か"
    return "普通"


def label_line(line: dict, expressions: dict[str, dict[str, str]]) -> tuple[str, str, str]:
    """(label, strength, source) for a line."""
    text = line.get("text") if isinstance(line.get("text"), str) else ""
    have = parse_instruct(line.get("instruct"))
    if have:
        return have[0], have[1], "instruct"
    emap = expressions.get(line.get("speaker")) if isinstance(line.get("speaker"), str) else None
    expr = line.get("expression")
    if emap and isinstance(expr, str):
        m = EXPR_NUM.search(expr)
        lab = emap.get(m.group(1)) if m else None
        if lab in LABELS:
            if lab == "普通":  # a neutral face still shouts or asks
                tl = label_from_text(text, line.get("kind"))
                if tl in ("問い", "静か"):
                    lab = tl
            return lab, strength_of(text, lab), "expression"
    lab = label_from_text(text, line.get("kind"))
    return lab, strength_of(text, lab), "text"


def label_script(script: dict, expressions: dict, overwrite: bool = False, recorded: bool = False, skip_neutral: bool = False) -> list[tuple]:
    """Write labels into `instruct`; returns rows (id, speaker, label, strength, source, text). With
    `skip_neutral`, a 普通 line gets no `instruct` (it is spoken from the plain voice anyway, and its
    synthesis cache stays valid)."""
    rows = []
    n = 0
    scenes = script.get("scenes") if isinstance(script.get("scenes"), list) else []
    for sc in scenes[:MAX_LINES]:
        lines = sc.get("lines") if isinstance(sc, dict) and isinstance(sc.get("lines"), list) else []
        for l in lines[:MAX_LINES]:
            n += 1
            if n > MAX_LINES:
                return rows
            if not isinstance(l, dict) or l.get("kind") == "direction" or l.get("spoken") is False:
                continue
            if l.get("voiceFile") and not recorded:
                continue
            text = l.get("text")
            if not isinstance(text, str) or not text.strip() or len(text) > MAX_TEXT:
                continue
            if overwrite and parse_instruct(l.get("instruct")):
                l.pop("instruct", None)
            if l.get("instruct") and not parse_instruct(l.get("instruct")):
                continue  # a hand-written direction is kept as it is
            lab, st, src = label_line(l, expressions)
            if skip_neutral and lab == "普通" and src != "instruct":
                rows.append((l.get("id"), l.get("speaker"), lab, st, src, text))
                continue
            l["instruct"] = format_label(lab, st)
            rows.append((l.get("id"), l.get("speaker"), lab, st, src, text))
    return rows


def load_json(path: str):
    if os.path.getsize(path) > 50_000_000:
        raise ValueError(f"{path}: too large")
    with io.open(path, encoding="utf-8") as f:
        return json.load(f)


def main(argv=None) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except Exception:  # noqa: BLE001
            pass
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--script", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--expressions", help='JSON {character: {"06": "怒り", ...}}')
    ap.add_argument("--report", help="TSV: id, speaker, label, strength, source, text")
    ap.add_argument("--overwrite", action="store_true", help="replace labels already in instruct")
    ap.add_argument("--all", action="store_true", help="label recorded lines too")
    ap.add_argument("--skip-neutral", action="store_true", help="write no instruct for 普通 lines (keeps their cache)")
    a = ap.parse_args(argv)
    try:
        script = load_json(a.script)
        expr = load_json(a.expressions) if a.expressions else {}
    except (OSError, ValueError) as e:
        print(f"cannot read input: {e}", file=sys.stderr)
        return 2
    if not isinstance(script, dict) or not isinstance(expr, dict):
        print("bad input: the script and the expressions are JSON objects", file=sys.stderr)
        return 2
    expr = {k: {str(n): v for n, v in m.items() if isinstance(v, str)} for k, m in expr.items() if isinstance(k, str) and isinstance(m, dict)}
    rows = label_script(script, expr, a.overwrite, a.all, a.skip_neutral)
    with io.open(a.out, "w", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps(script, ensure_ascii=False, indent=1))
    if a.report:
        with io.open(a.report, "w", encoding="utf-8", newline="\n") as f:
            for r in rows:
                f.write("\t".join(str(x) for x in r) + "\n")
    counts = {}
    for r in rows:
        counts[r[2]] = counts.get(r[2], 0) + 1
    print(json.dumps({"labelled": len(rows), "labels": counts, "out": a.out}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
