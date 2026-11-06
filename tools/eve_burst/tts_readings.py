#!/usr/bin/env python3
"""Fix the readings Qwen3-TTS gets wrong: listen back, find the misread words, write them in kana.

Qwen3-TTS reads kanji by itself (no reading dictionary), so rarer words (絵画 → えいが, 紛失 → ふんしち,
物騒 → むっそう) and names come out wrong now and then. Writing the whole line in kana fixes the reading
but risks the intonation, so this tool keeps the kanji and writes only the misread words as ruby in the
line's `reading` (`｜絵画《かいが》`; FilmCraft speaks the ruby and the subtitle keeps the text,
docs/script.md). See docs/eve-burst-tts-quality.md for the measurements behind it.

    check   transcribe each synthesized line twice, with a phoneme recogniser (sounds, so 上手 read as
            うわて shows) and Whisper (words), compare both with the expected reading (pyopenjtalk-plus,
            the names dictionary and the overrides), and report the kanji words both disagree on;
    apply   write `reading` for the lines of a report (names and game-style 茜（あかね） as well);
    refs    check that every reference clip of voice_refs.json says exactly its refText (a refText
            with words the clip does not say makes every cloned line worse).

    python tts_readings.py check --script ep01.fcscript.json --tts-dir tts --names eve_names.json --report r.json
    python tts_readings.py apply --script ep01.fcscript.json --report r.json --names eve_names.json --out ep01.read.json
    python tts_readings.py refs --voice-refs voice_refs.json --root DUMP --report refs.json

`check` and `refs` need the Qwen3-TTS venv with pyopenjtalk-plus (BSD), faster-whisper (MIT),
transformers and the models kotoba-tech/kotoba-whisper-v2.0-faster (MIT) and
prj-beatrice/japanese-hubert-base-phoneme-ctc-v4 (Apache-2.0) (`--whisper` / `--ctc` take local
folders). `apply` is standard library only. Input is treated as hostile: sizes and counts are capped,
and nothing but `reading` is written.
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
MAX_ALIGN = 4_000_000  # cells of the alignment table
MAX_REPORT = 50_000_000  # bytes

KANJI = re.compile(r"[一-鿿々]")
KANA_ONLY = re.compile(r"^[ぁ-んァ-ヶー]+$")
RUBY = re.compile(r"｜([^《｜]+)《([^》]+)》|([一-鿿々]+)《([^》]+)》")
PAREN_READING = re.compile(r"([一-鿿々]+)（([ぁ-んァ-ヶー]+)）")
PUNCT = re.compile(r"[。、，．！？!?…‥・「」『』（）()［］\[\]〈〉《》｜\s　〜～―‐\-~☆♪“”\"'’]+")
SILENT = {"pau", "sil", "", "unk", "pad", "sos", "eos"}
NUMERAL = re.compile(r"^[〇一二三四五六七八九十百千万億兆]+$")
DIGITS = re.compile(r"[0-9０-９][0-9０-９,，]*")


def kata2hira(s: str) -> str:
    return "".join(chr(ord(c) - 0x60) if "ァ" <= c <= "ヶ" else c for c in s)


def hira2kata(s: str) -> str:
    return "".join(chr(ord(c) + 0x60) if "ぁ" <= c <= "ゖ" else c for c in s)


def spoken_of(text: str) -> str:
    """What FilmCraft gives the synthesizer: ruby read out (script_tools::spoken_text)."""
    return RUBY.sub(lambda m: m.group(2) or m.group(4), text)


def split_ruby(text: str) -> list[tuple[bool, str]]:
    """Pieces of the text: (True, ruby markup) or (False, plain text)."""
    out, pos = [], 0
    for m in RUBY.finditer(text):
        if m.start() > pos:
            out.append((False, text[pos:m.start()]))
        out.append((True, m.group(0)))
        pos = m.end()
    if pos < len(text):
        out.append((False, text[pos:]))
    return out


def ruby_words(text: str, words: dict[str, str]) -> str:
    """Every occurrence of a word (outside existing ruby) written as ｜word《kana》, longest first."""
    keys = sorted((k for k in words if k and words[k]), key=len, reverse=True)
    if not keys:
        return text
    pat = re.compile("|".join(re.escape(k) for k in keys))
    return "".join(seg if is_ruby else pat.sub(lambda m: f"｜{m.group(0)}《{words[m.group(0)]}》", seg)
                   for is_ruby, seg in split_ruby(text))


def paren_to_ruby(text: str) -> str:
    """The game's own readings, 茜（あかね）, which a synthesizer would read twice, as ruby."""
    return PAREN_READING.sub(lambda m: f"｜{m.group(1)}《{m.group(2)}》", text)


def number_words(spoken: str, flagged: dict[str, str], kana_of) -> dict[str, str]:
    """The analyser turns "５０" into the words 五 + 十, which are not in the text. When such numeral
    words are flagged, write the digit runs of the line in kana instead (kana_of(run) -> hiragana)."""
    out = {k: v for k, v in flagged.items() if not NUMERAL.match(k)}
    if len(out) == len(flagged):
        return out
    for m in list(DIGITS.finditer(spoken_of(spoken)))[:50]:
        run = m.group(0)
        k = kana_of(run)
        if k and len(run) <= 40 and KANA_ONLY.match(k):
            out[run] = k
    return out


def norm_tokens(tokens: list[str], owners: list[int] | None = None) -> tuple[list[str], list[int]]:
    """Phonemes lower-cased, silences dropped, long vowels spelt one way (ou → oo, ei → ee) across the
    whole sequence, so a sequence and the same sounds heard without word boundaries normalize alike."""
    owners = owners if owners is not None else [0] * len(tokens)
    out: list[str] = []
    own: list[int] = []
    for t, o in zip(tokens, owners):
        t = t.lower()
        if t in SILENT:
            continue
        if out and ((out[-1] == "o" and t == "u") or (out[-1] == "e" and t == "i")):
            t = out[-1]
        out.append(t)
        own.append(o)
    return out, own


def align_ok(exp: list[str], heard: list[str]) -> tuple[int, list[bool]]:
    """Edit distance and, per expected phoneme, whether it was heard (an insertion marks its neighbours).
    Too long to align (hostile input): everything counts as not heard."""
    n, m = len(exp), len(heard)
    if (n + 1) * (m + 1) > MAX_ALIGN:
        return max(n, m), [False] * n
    d = [[0] * (m + 1) for _ in range(n + 1)]
    for i in range(n + 1):
        d[i][0] = i
    for j in range(m + 1):
        d[0][j] = j
    for i in range(1, n + 1):
        di, dp = d[i], d[i - 1]
        for j in range(1, m + 1):
            di[j] = min(dp[j] + 1, di[j - 1] + 1, dp[j - 1] + (exp[i - 1] != heard[j - 1]))
    ok = [False] * n
    ins = [0] * (n + 1)
    i, j = n, m
    while i > 0 or j > 0:
        if i > 0 and j > 0 and d[i][j] == d[i - 1][j - 1] + (exp[i - 1] != heard[j - 1]):
            ok[i - 1] = exp[i - 1] == heard[j - 1]
            i, j = i - 1, j - 1
        elif i > 0 and d[i][j] == d[i - 1][j] + 1:
            i -= 1
        else:
            ins[i] += 1
            j -= 1
    for k, c in enumerate(ins):
        if c:
            if k < n:
                ok[k] = False
            if k > 0:
                ok[k - 1] = False
    return d[n][m], ok


def misheard_words(words: list[tuple[str, str, list[str]]], heard: list[str]) -> tuple[float, set[int]]:
    """Phoneme error rate and the indices of the words not fully heard.
    `words`: (surface, kana, phonemes) per word of the expected reading."""
    toks, owners = [], []
    for k, (_, _, ph) in enumerate(words):
        toks += ph
        owners += [k] * len(ph)
    exp, own = norm_tokens(toks, owners)
    got, _ = norm_tokens(heard)
    dist, ok = align_ok(exp, got)
    bad = {own[i] for i, good in enumerate(ok) if not good}
    return dist / max(1, len(exp)), bad


# ---------------------------------------------------------------- analysis (needs the venv)

class Reader:
    """Expected readings with pyopenjtalk-plus: names and overrides first, as katakana single words."""

    def __init__(self, names: dict[str, str], overrides: dict[str, str]):
        import pyopenjtalk  # noqa: WPS433

        self.jt = pyopenjtalk
        fixed = {**names, **overrides}
        self.fixed = dict(sorted(((k, kata2hira(v)) for k, v in fixed.items()), key=lambda kv: -len(kv[0])))

    def words(self, text: str) -> list[tuple[str, str, list[str]]]:
        """(surface, hiragana reading, phonemes) per word of the spoken form of `text`."""
        t = spoken_of(paren_to_ruby(text))
        marks = {}
        for k, v in self.fixed.items():
            if k in t:
                t = t.replace(k, hira2kata(v))
                marks[hira2kata(v)] = k
        out = []
        for seg in PUNCT.split(t):
            if not seg:
                continue
            for w in self.jt.run_frontend(seg):
                s = w.get("string") or ""
                pron = (w.get("pron") or "").replace("’", "")
                ph = self.jt.g2p(pron).split() if pron and pron != "、" else []
                surf = marks.get(s, s)
                out.append((surf, kata2hira(w.get("read") or s), ph))
        return out

    def kana(self, text: str) -> str:
        """Hiragana reading of a short text (a number, for instance)."""
        return kata2hira(PUNCT.sub("", self.jt.g2p(text, kana=True))).replace("ー", "う") if text else ""

    def phonemes(self, text: str) -> list[str]:
        t = PUNCT.sub(" ", spoken_of(text)).strip()
        for k, v in self.fixed.items():
            t = t.replace(k, hira2kata(v))
        return self.jt.g2p(t).split() if t else []


class Ears:
    def __init__(self, whisper: str, ctc: str, device: str):
        import torch  # noqa: WPS433
        from faster_whisper import WhisperModel  # noqa: WPS433
        from transformers import AutoModelForCTC, AutoProcessor  # noqa: WPS433

        self.torch, self.device = torch, device
        self.wm = WhisperModel(whisper, device="cuda" if device.startswith("cuda") else "cpu",
                               compute_type="float16" if device.startswith("cuda") else "int8")
        self.p = AutoProcessor.from_pretrained(ctc)
        self.m = AutoModelForCTC.from_pretrained(ctc).to(device).eval()
        self.inv = {v: k for k, v in self.p.tokenizer.get_vocab().items()}
        self.pad = self.p.tokenizer.pad_token_id

    def phonemes(self, wav: str) -> list[str]:
        import librosa  # noqa: WPS433
        import numpy as np  # noqa: WPS433
        import soundfile as sf  # noqa: WPS433

        w, sr = sf.read(wav, always_2d=True)
        w = w.mean(axis=1)[: int(sr * 60)]
        if sr != 16000:
            w = librosa.resample(w, orig_sr=sr, target_sr=16000)
        pad = np.zeros(6400)  # without silence around it the recogniser misses a clip's first sounds
        w = np.concatenate([pad, w, pad])
        x = self.p(w, sampling_rate=16000, return_tensors="pt").input_values.to(self.device)
        with self.torch.no_grad():
            ids = self.m(x).logits.argmax(-1)[0].tolist()
        out, prev = [], None
        for i in ids:
            if i != prev and i != self.pad:
                out.append(self.inv.get(i, ""))
            prev = i
        return out

    def text(self, wav: str) -> str:
        segs, _ = self.wm.transcribe(wav, language="ja", beam_size=5, condition_on_previous_text=False, without_timestamps=True)
        return "".join(s.text for s in segs)[:MAX_TEXT]


def judge(reader: Reader, ears: Ears, spoken: str, wav: str) -> dict:
    words = reader.words(spoken)
    per_c, bad_c = misheard_words(words, ears.phonemes(wav))
    heard = ears.text(wav)
    per_w, bad_w = misheard_words(words, reader.phonemes(heard))
    both = sorted(bad_c & bad_w)
    flagged = {}
    for k in both:
        surf, kana, _ = words[k]
        if KANJI.search(surf) and surf not in reader.fixed and kana and not KANJI.search(kana):
            flagged[surf] = kana
    flagged = number_words(spoken, flagged, reader.kana)
    return {"perCtc": round(per_c, 4), "perWhisper": round(per_w, 4), "whisper": heard, "words": flagged}


# ---------------------------------------------------------------- script handling (stdlib)

def spoken_lines(script: dict):
    """(line, text the synthesizer reads) for every synthesized line (no recording, not a direction)."""
    n = 0
    for sc in (script.get("scenes") or [])[:MAX_LINES] if isinstance(script.get("scenes"), list) else []:
        lines = sc.get("lines") if isinstance(sc, dict) else None
        for l in (lines or [])[:MAX_LINES] if isinstance(lines, list) else []:
            n += 1
            if n > MAX_LINES:
                return
            if not isinstance(l, dict) or l.get("kind") == "direction" or l.get("voiceFile"):
                continue
            text, rd = l.get("text"), l.get("reading")
            if not isinstance(l.get("id"), str) or not isinstance(text, str) or len(text) > MAX_TEXT:
                continue
            base = rd if isinstance(rd, str) and rd.strip() and len(rd) <= MAX_TEXT else text
            yield l, base


def apply_report(script: dict, report: dict, names: dict[str, str], form: str = "hira") -> dict:
    """Write `reading` for every line that needs one; returns counts. Lines are changed in place."""
    conv = hira2kata if form == "kata" else kata2hira
    names_k = {k: conv(v) for k, v in names.items()}
    lines = report.get("lines") if isinstance(report.get("lines"), dict) else {}
    n_lines = n_names = n_fixed = 0
    for l, base in spoken_lines(script):
        n_lines += 1
        new = ruby_words(paren_to_ruby(base), names_k)
        if new != base:
            n_names += 1
        r = lines.get(l["id"])
        words = r.get("words") if isinstance(r, dict) and isinstance(r.get("words"), dict) else {}
        fix = {k: conv(v) for k, v in words.items()
               if isinstance(k, str) and isinstance(v, str) and 0 < len(k) <= 40 and 0 < len(v) <= 80 and KANA_ONLY.match(v)}
        if fix:
            new = ruby_words(new, fix)
            n_fixed += 1
        if new != base and len(new) <= MAX_TEXT:
            l["reading"] = new
    return {"lines": n_lines, "withNames": n_names, "fixed": n_fixed}


def load_json(path: str):
    if os.path.getsize(path) > MAX_REPORT:
        raise ValueError(f"{path}: too large")
    with io.open(path, encoding="utf-8") as f:
        return json.load(f)


def str_dict(d) -> dict[str, str]:
    return {k: v for k, v in d.items() if isinstance(k, str) and isinstance(v, str) and k and v} if isinstance(d, dict) else {}


def cmd_apply(a) -> int:
    try:
        script = load_json(a.script)
        report = load_json(a.report) if a.report else {}
        names = str_dict(load_json(a.names)) if a.names else {}
    except (OSError, ValueError) as e:
        print(f"cannot read input: {e}", file=sys.stderr)
        return 2
    if not isinstance(script, dict) or not isinstance(report, dict):
        print("bad input: a script and a report are JSON objects", file=sys.stderr)
        return 2
    counts = apply_report(script, report, names, a.form)
    with io.open(a.out, "w", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps(script, ensure_ascii=False, indent=1))
    print(json.dumps({**counts, "out": a.out}, ensure_ascii=False))
    return 0


def cmd_check(a) -> int:
    try:
        script = load_json(a.script)
        names = str_dict(load_json(a.names)) if a.names else {}
        overrides = str_dict(load_json(a.overrides)) if a.overrides else {}
    except (OSError, ValueError) as e:
        print(f"cannot read input: {e}", file=sys.stderr)
        return 2
    if not isinstance(script, dict):
        print("bad input: the script is a JSON object", file=sys.stderr)
        return 2
    reader = Reader(names, overrides)
    ears = Ears(a.whisper, a.ctc, a.device)
    out, n = {}, 0
    for l, base in spoken_lines(script):
        wav = os.path.join(a.tts_dir, re.sub(r"[^0-9A-Za-z_\-぀-ヿ一-鿿]", "", l["id"]) + ".wav")
        if not os.path.isfile(wav):
            continue
        try:
            out[l["id"]] = {"spoken": base, **judge(reader, ears, base, wav)}
        except Exception as e:  # noqa: BLE001  a bad file must not end the run
            out[l["id"]] = {"spoken": base, "error": f"{type(e).__name__}: {e}"}
        n += 1
        if n % 25 == 0:
            print(json.dumps({"progress": n}), file=sys.stderr, flush=True)
    flagged = sum(1 for r in out.values() if r.get("words"))
    with io.open(a.report, "w", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps({"version": 1, "lines": out}, ensure_ascii=False, indent=1))
    print(json.dumps({"checked": n, "flagged": flagged, "report": a.report}, ensure_ascii=False))
    return 0


def cmd_refs(a) -> int:
    try:
        refs = load_json(a.voice_refs)
        names = str_dict(load_json(a.names)) if a.names else {}
    except (OSError, ValueError) as e:
        print(f"cannot read input: {e}", file=sys.stderr)
        return 2
    if not isinstance(refs, dict):
        print("bad input", file=sys.stderr)
        return 2
    reader = Reader(names, {})
    ears = Ears(a.whisper, a.ctc, a.device)
    rows = []
    for ch, r in list(refs.items())[:10_000]:
        if not isinstance(r, dict):
            continue
        for rank, c in enumerate(([r] + list(r.get("alternatives") or []))[:20]):
            if not isinstance(c, dict) or not isinstance(c.get("refAudio"), str) or not isinstance(c.get("refText"), str):
                continue
            p = os.path.join(a.root, c["refAudio"])
            if not os.path.isfile(p):
                continue
            heard = ears.text(p)
            exp, _ = norm_tokens(reader.phonemes(c["refText"]))
            got_w, _ = norm_tokens(reader.phonemes(heard))
            got_c, _ = norm_tokens(ears.phonemes(p))
            per = min(align_ok(exp, got_w)[0], align_ok(exp, got_c)[0]) / max(1, len(exp))
            rows.append({"char": ch, "rank": rank, "refAudio": c["refAudio"], "refText": c["refText"], "heard": heard, "per": round(per, 3)})
    rows.sort(key=lambda x: -x["per"])
    with io.open(a.report, "w", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps(rows, ensure_ascii=False, indent=1))
    bad = [x for x in rows if x["per"] > a.max_error]
    print(json.dumps({"checked": len(rows), "mismatched": len(bad), "report": a.report}, ensure_ascii=False))
    return 0


def main(argv=None) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except Exception:  # noqa: BLE001
            pass
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    models = argparse.ArgumentParser(add_help=False)
    models.add_argument("--whisper", default="kotoba-tech/kotoba-whisper-v2.0-faster")
    models.add_argument("--ctc", default="prj-beatrice/japanese-hubert-base-phoneme-ctc-v4")
    models.add_argument("--device", default="cuda")
    models.add_argument("--names", help="JSON {name: kana}")
    c = sub.add_parser("check", parents=[models], help="find misread words in synthesized lines")
    c.add_argument("--script", required=True)
    c.add_argument("--tts-dir", required=True)
    c.add_argument("--overrides", help="JSON {word: kana}: readings pyopenjtalk gets wrong in this work (町中: まちなか)")
    c.add_argument("--report", required=True)
    p = sub.add_parser("apply", help="write `reading` from a report")
    p.add_argument("--script", required=True)
    p.add_argument("--report")
    p.add_argument("--names", help="JSON {name: kana}")
    p.add_argument("--form", choices=["hira", "kata"], default="hira")
    p.add_argument("--out", required=True)
    r = sub.add_parser("refs", parents=[models], help="check reference clips against their refText")
    r.add_argument("--voice-refs", required=True)
    r.add_argument("--root", required=True)
    r.add_argument("--report", required=True)
    r.add_argument("--max-error", type=float, default=0.15)
    a = ap.parse_args(argv)
    return {"check": cmd_check, "apply": cmd_apply, "refs": cmd_refs}[a.cmd](a)


if __name__ == "__main__":
    sys.exit(main())
