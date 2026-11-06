#!/usr/bin/env python3
"""Voice inventory and reference-clip picker for the extracted EVE burst error PLUS data.

The data folder (never part of this repository) holds, per character code, a folder of voice WAVs
(03_音声/ボイス/<code>_<name>/<voice>.wav), a catalog that says which line each clip speaks
(00_目録/ボイスと台詞の対応表.tsv, 00_目録/キャラクターコード表.tsv) and the scenarios as JSON
(01_シナリオ/*/*.json). This tool

  1. counts, per character code and per speaker name, the dialogue lines of the whole game against
     the lines that have a recorded voice (the gap that script.synthesize must fill), and
  2. picks for every character the best 1-3 reference clips for voice cloning (docs/eve-burst-voices.md
     explains the scoring) and writes them as JSON that can be pasted into a script character's
     "voice" field: {"koj": {"refAudio": "...", "refText": "...", "seconds": 6.4, "why": "..."}}.

    python voice_refs.py --root "E:/filmcraft_JPN/PS2_EVE_Bursterror解析データ"
    python voice_refs.py --root DATA --inventory-md inventory.md --inventory-json inventory.json
    python voice_refs.py --root DATA --abs      # refAudio as absolute paths (else relative to --root)

Standard library only. numpy is used when installed (faster); soundfile only for WAVs the wave
module cannot read (not 8/16-bit PCM). Read-only on the data folder. Deterministic: the same tree
gives the same output.
"""

from __future__ import annotations

import argparse
import collections
import csv
import glob
import json
import math
import os
import re
import sys
import wave
from array import array

try:  # optional
    import numpy as np  # type: ignore
except Exception:  # pragma: no cover - depends on the machine
    np = None
try:  # optional
    import soundfile as sf  # type: ignore
except Exception:  # pragma: no cover
    sf = None

HERE = os.path.dirname(os.path.abspath(__file__))
VOICE_DIR = os.path.join("03_音声", "ボイス")
CODES_TSV = os.path.join("00_目録", "キャラクターコード表.tsv")
LINES_TSV = os.path.join("00_目録", "ボイスと台詞の対応表.tsv")
SCENARIO_GLOB = os.path.join("01_シナリオ", "*", "*.json")
MESSAGE_OP = "0x8035"
# Codes that are not people: sound effects and jingles played with a text line.
NON_PERSON = {"j", "m", "se", "eve_"}
NARRATION = "（地の文）"

# Picker limits (seconds, dB, ratios). RELAXED is used only for a character with no clip that
# passes the strict ones.
STRICT = {"min_s": 4.0, "max_s": 12.0, "max_gap": 0.8, "max_sil": 0.35, "min_snr": 12.0,
          "min_level": -35.0, "max_clip": 0.001}
RELAXED = {"min_s": 2.5, "max_s": 14.0, "max_gap": 1.4, "max_sil": 0.5, "min_snr": 8.0,
           "min_level": -40.0, "max_clip": 0.003}
FRAME_S = 0.02
SILENCE_BELOW_PEAK_DB = 35.0  # a frame this far under the speech level is silence
MAX_BYTES = 64 * 1024 * 1024


# ---------------------------------------------------------------- catalog / inventory

def read_tsv(path):
    """Rows of a UTF-8(-BOM) TSV without its header; a missing file gives []."""
    try:
        with open(path, encoding="utf-8-sig", newline="") as f:
            rows = list(csv.reader(f, delimiter="\t", quoting=csv.QUOTE_NONE))
    except OSError:
        return []
    return rows[1:]


def load_codes(root):
    """{code: {"name", "folder", "confidence"}} from the character code table."""
    out = {}
    for r in read_tsv(os.path.join(root, CODES_TSV)):
        if len(r) >= 7 and r[0]:
            out[r[0]] = {"name": r[2], "folder": r[6], "confidence": r[5]}
    return out


def code_of(voice_name, codes):
    """The character code of a voice name ("kyo0005" -> "kyo", "eve_12" -> "eve_"), or None."""
    m = re.match(r"[a-z_]+", voice_name or "")
    if not m:
        return None
    p = m.group(0)
    return p if (p in codes or p in NON_PERSON) else None


def load_clip_lines(root):
    """{voice stem: [(speaker, text), ...]} from the voice/line correspondence table."""
    out = collections.defaultdict(list)
    for r in read_tsv(os.path.join(root, LINES_TSV)):
        if len(r) < 7 or not r[0].lower().endswith((".ahx", ".adx", ".wav")):
            continue
        if not r[4] or r[5].startswith("（効果音命令"):
            continue
        out[os.path.splitext(r[0])[0]].append((r[4], r[5]))
    return out


def wav_info(path):
    """(rate, channels, seconds) from the WAV header only, or None when unreadable."""
    try:
        with wave.open(path, "rb") as w:
            fr = w.getframerate()
            return fr, w.getnchannels(), (w.getnframes() / fr if fr else 0.0)
    except Exception:
        return None


def scan_scenarios(root):
    """Walk every scenario JSON; returns (messages, number of files, unreadable files).

    A message is (speaker, tuple of voice names). Unreadable files are reported, never fatal."""
    msgs, files, bad = [], 0, []
    for path in sorted(glob.glob(os.path.join(root, SCENARIO_GLOB))):
        try:
            if os.path.getsize(path) > MAX_BYTES:
                bad.append(path)
                continue
            with open(path, encoding="utf-8-sig") as f:
                data = json.load(f)
            ins = data.get("instructions", []) if isinstance(data, dict) else []
        except (OSError, ValueError):
            bad.append(path)
            continue
        files += 1
        for i in ins:
            if isinstance(i, dict) and i.get("op") == MESSAGE_OP:
                voices = tuple(v.get("name", "") for v in i.get("voices") or [] if isinstance(v, dict))
                msgs.append(((i.get("speaker") or "").strip(), voices))
    return msgs, files, bad


def median(xs):
    xs = sorted(xs)
    if not xs:
        return 0.0
    h = len(xs) // 2
    return xs[h] if len(xs) % 2 else (xs[h - 1] + xs[h]) / 2


def voice_files(root, codes):
    """{voice stem: path} of every WAV present in the voice folders (folder from the code table)."""
    out = {}
    for code, c in sorted(codes.items()):
        d = os.path.join(root, VOICE_DIR, c["folder"])
        try:
            names = sorted(os.listdir(d))
        except OSError:
            continue
        for n in names:
            if n.lower().endswith(".wav"):
                out.setdefault(os.path.splitext(n)[0], os.path.join(d, n))
    return out


def inventory(root, with_audio=True):
    """Lines versus recorded voices per character code and per speaker name."""
    codes = load_codes(root)
    msgs, nfiles, bad = scan_scenarios(root)
    present = voice_files(root, codes)
    persons = {c for c in codes if c not in NON_PERSON}

    # speaker -> code by the voices that speak the speaker's lines
    votes = collections.defaultdict(collections.Counter)
    for spk, voices in msgs:
        if not spk:
            continue
        for v in voices:
            c = code_of(v, codes)
            if c in persons:
                votes[spk][c] += 1
    # A speaker belongs to a code when most of its voiced lines use that code and either most of its
    # lines are voiced or it is one of the code's main speakers (>= 10 % of the code's voiced lines).
    # Generic names ("男", "声") that borrow several actors' voices stay unattributed.
    lines_of = collections.Counter(spk for spk, _ in msgs if spk)
    voiced_of = collections.Counter(spk for spk, v in msgs if spk and v)
    code_total = collections.Counter()
    for cnt in votes.values():
        code_total.update(cnt)
    spk_code = {}
    for spk, cnt in votes.items():
        c, n = cnt.most_common(1)[0]
        if n < 3 or n / sum(cnt.values()) < 0.6:
            continue
        if voiced_of[spk] / lines_of[spk] >= 0.5 or n >= 0.1 * code_total[c]:
            spk_code[spk] = c

    speakers = collections.defaultdict(lambda: {"lines": 0, "voiced": 0})
    percode = {c: {"lines": 0, "voiced": 0, "unvoiced": 0} for c in codes}
    sfx_lines = 0
    unattributed = collections.Counter()
    referenced = collections.defaultdict(set)
    for spk, voices in msgs:
        s = speakers[spk or NARRATION]
        s["lines"] += 1
        vcodes = {code_of(v, codes) for v in voices} - {None}
        for v in voices:
            c = code_of(v, codes)
            if c:
                referenced[c].add(v)
        person_voices = vcodes & persons
        if voices:
            s["voiced"] += 1
        if person_voices:
            for c in person_voices:
                percode[c]["lines"] += 1
                percode[c]["voiced"] += 1
        elif voices:
            sfx_lines += 1
        else:
            c = spk_code.get(spk)
            if c:
                percode[c]["lines"] += 1
                percode[c]["unvoiced"] += 1
            else:
                unattributed[spk or NARRATION] += 1
    # clip statistics from the files (headers only)
    rates = collections.Counter()
    by_code = collections.defaultdict(list)
    for stem, path in present.items():
        by_code[code_of(stem, codes)].append((stem, path))
    for code, c in sorted(codes.items()):
        secs, n = [], 0
        for stem, path in by_code.get(code, ()):
            if os.path.basename(os.path.dirname(path)) != c["folder"]:
                continue
            n += 1
            if with_audio:
                info = wav_info(path)
                if info:
                    rates[(info[0], info[1])] += 1
                    secs.append(info[2])
        percode[code].update({
            "name": c["name"], "folder": c["folder"], "clips_present": n,
            "clips_referenced": len(referenced.get(code, ())),
            "seconds_total": round(sum(secs), 1), "seconds_median": round(median(secs), 2),
            "usable_4_12s": sum(1 for x in secs if 4.0 <= x <= 12.0),
        })
    for spk, s in speakers.items():
        s["code"] = spk_code.get(spk, "")
    return {
        "scenarios": nfiles, "unreadable_scenarios": [os.path.basename(b) for b in bad],
        "lines": len(msgs), "voiced_lines": sum(1 for _, v in msgs if v),
        "sfx_only_lines": sfx_lines,
        "clips_present": len(present), "formats": {f"{r}Hz/{c}ch": n for (r, c), n in sorted(rates.items())},
        "codes": percode,
        "speakers": {k: v for k, v in sorted(speakers.items(), key=lambda kv: (-kv[1]["lines"], kv[0]))},
        "unattributed_unvoiced": dict(unattributed.most_common()),
    }


def inventory_markdown(inv):
    out = ["| code | character | lines | voiced | to synthesize | voiced % | clips on disk | referenced | 4-12 s clips | median s |",
           "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for code, c in sorted(inv["codes"].items(), key=lambda kv: (-kv[1]["lines"], kv[0])):
        if code in NON_PERSON or not c.get("name"):
            continue
        pct = 100.0 * c["voiced"] / c["lines"] if c["lines"] else 0.0
        out.append(f"| {code} | {c['name']} | {c['lines']} | {c['voiced']} | {c['unvoiced']} | {pct:.1f} | "
                   f"{c['clips_present']} | {c['clips_referenced']} | {c['usable_4_12s']} | {c['seconds_median']} |")
    return "\n".join(out)


# ---------------------------------------------------------------- text scoring

_PUNCT = set("　 、。，．・？！?!‥…―ー～〜「」『』（）()《》[]［］-")
_LAUGH_HARD = re.compile(r"(ははは|あはは|うふふ|ふふふ|くくく|ククク|ひひひ|へへへ|ハハハ|フフフ|ふふっ|くくっ|ぎゃ|ギャ|きゃ|キャ"
                         r"|うわあ|うわっ|ぐあ|ぬお|うお|あああ|いやあ|ひいい|ぁぁ|あぁ)")
_INTERJ = re.compile(r"(あっ|えっ|うっ|くっ|ひっ|はっ|んっ|ちっ|ふっ|はあ|はぁ|ふう|ふぅ|くそ|ちくしょう|うう|ふふ|はは|ひゃ|えぇ|ああ)")
_SMALL = set("ゃゅょぁぃぅぇぉャュョァィゥェォ")
_ELLIPSIS = set("‥…")


def is_kana(c):
    return "\u3041" <= c <= "\u30ff"


def core_chars(text):
    return [c for c in text if c not in _PUNCT]


def moras(text):
    """Rough count of morae: kana (small ones fused; っ and ー count), anything else as 2."""
    n = 0
    for c in text:
        if c in _PUNCT or c in _SMALL:
            continue
        n += 1 if is_kana(c) else 2
    return n


def clean_ref_text(text):
    """The transcript to hand to the cloner: each ‥ becomes …, spaces go."""
    t = re.sub(r"‥+", lambda m: "…" * len(m.group(0)), text)
    return re.sub(r"[\s\u3000]+", "", t).strip()


def text_check(text, relaxed=False):
    """(score 0-40, hard rejects, notes) for a transcript used as a cloning reference.

    Hard rejects: no words, fewer than 6 letters, inner-voice/aside marks, laughs and screams,
    Latin letters or digits (their reading is ambiguous), more than a quarter ellipsis. Soft
    penalties: exclamation marks, interjections, ellipsis, an ellipsis opener, a missing sentence end."""
    rejects, notes = [], []
    t = text.strip()
    core = core_chars(t)
    if not core:
        return 0.0, ["no words"], notes
    if len(core) < 6:
        rejects.append("too short")
    if any(c in t for c in "《》（）()"):
        rejects.append("aside or inner voice")
    if _LAUGH_HARD.search(t):
        rejects.append("laugh or scream")
    if re.search(r"[0-9A-Za-z０-９Ａ-Ｚａ-ｚ]", t):
        rejects.append("digits or letters")
    ell = sum(1 for c in t if c in _ELLIPSIS)
    if ell / max(1, len(t)) > 0.25:
        rejects.append("mostly ellipsis")
    score = 40.0
    ex = t.count("！")
    if ex:
        score -= min(16.0, 8.0 * ex)
        notes.append("exclamation")
    score -= min(12.0, 4.0 * len(_INTERJ.findall(t)))
    score -= min(12.0, 30.0 * ell / max(1, len(t)))
    if t[:1] in _ELLIPSIS:
        score -= 8.0
        notes.append("starts with ellipsis")
    if t.count("～") + t.count("―"):
        score -= 6.0
    if t[-1:] not in ("。", "？") and not t.endswith(("。」", "？」")):
        score -= 3.0
    if re.search(r"[‥…]+。?$", t):
        score -= 4.0
    return max(0.0, score), rejects, notes


# ---------------------------------------------------------------- audio analysis

def read_samples(path):
    """(rate, first-channel samples in int16 range) or raises ValueError."""
    try:
        with wave.open(path, "rb") as w:
            ch, sw, rate, n = w.getnchannels(), w.getsampwidth(), w.getframerate(), w.getnframes()
            if rate <= 0 or ch <= 0 or n <= 0 or n * ch * sw > MAX_BYTES:
                raise ValueError("bad WAV header")
            raw = w.readframes(n)
        if sw == 2:
            if np is not None:
                a = np.frombuffer(raw[: len(raw) // 2 * 2], dtype="<i2")
                return rate, (a[::ch] if ch > 1 else a)
            a = array("h")
            a.frombytes(raw[: len(raw) // 2 * 2])
            if sys.byteorder == "big":
                a.byteswap()
            return rate, (a[::ch] if ch > 1 else a)
        if sw == 1:
            a = [(b - 128) * 256 for b in raw[::ch]]
            return rate, (np.array(a, dtype=np.int32) if np is not None else a)
    except (wave.Error, EOFError, OSError):
        pass
    if sf is not None:
        try:
            data, rate = sf.read(path, dtype="int16", always_2d=True)
            col = data[:, 0]
            return rate, (col if np is not None else list(map(int, col)))
        except Exception as e:
            raise ValueError(f"unreadable: {e}")
    raise ValueError("unsupported WAV (not 8/16-bit PCM; install soundfile)")


def _db(ms):
    return 10.0 * math.log10(ms / (32768.0 * 32768.0) + 1e-12)


def _pct(sorted_vals, p):
    return sorted_vals[min(len(sorted_vals) - 1, max(0, int(round(p * (len(sorted_vals) - 1)))))]


def analyse(rate, samples):
    """Measurements of one clip: seconds, peak/RMS dBFS, clipping, SNR, silence, longest gap."""
    n = len(samples)
    if n == 0 or rate <= 0:
        raise ValueError("empty clip")
    fl = max(1, int(rate * FRAME_S))
    nfr = n // fl
    if nfr < 5:
        raise ValueError("clip too short to analyse")
    if np is not None:
        x = np.asarray(samples, dtype=np.float64)
        peak = float(np.max(np.abs(x)))
        clip = float(np.mean(np.abs(x) >= 32700.0))
        ms = (x[: nfr * fl].reshape(nfr, fl) ** 2).mean(axis=1).tolist()
    else:
        peak = float(max(max(samples), -min(samples)))
        clip = sum(1 for v in samples if v >= 32700 or v <= -32700) / n
        ms = [sum(v * v for v in samples[i * fl:(i + 1) * fl]) / fl for i in range(nfr)]
    dbs = [_db(m) for m in ms]
    srt = sorted(dbs)
    speech, noise = _pct(srt, 0.90), _pct(srt, 0.10)
    thr = speech - SILENCE_BELOW_PEAK_DB
    sil = [d < thr for d in dbs]
    active = [m for m, s in zip(ms, sil) if not s]
    rms = _db(sum(active) / len(active)) if active else -120.0
    lead = 0
    while lead < nfr and sil[lead]:
        lead += 1
    trail = 0
    while trail < nfr - lead and sil[nfr - 1 - trail]:
        trail += 1
    gap = cur = 0
    for s in sil[lead:nfr - trail]:
        cur = cur + 1 if s else 0
        gap = max(gap, cur)
    peak_db = 20.0 * math.log10(max(peak, 1.0) / 32768.0)
    return {
        "seconds": n / rate, "rate": rate, "peak_db": peak_db, "clip_frac": clip, "rms_db": rms,
        "speech_db": speech, "noise_db": noise, "snr_db": min(60.0, speech - noise),
        "silence_ratio": sum(sil) / nfr, "lead_s": lead * FRAME_S, "trail_s": trail * FRAME_S,
        "max_gap_s": gap * FRAME_S,
    }


def audio_check(m, text, relaxed=False):
    """(score 0-60, hard rejects, morae per second) for the measurements of a clip and its transcript."""
    lim = RELAXED if relaxed else STRICT
    rej = []
    s = m["seconds"]
    if not lim["min_s"] <= s <= lim["max_s"]:
        rej.append(f"{s:.1f}s outside {lim['min_s']}-{lim['max_s']}s")
    if m["clip_frac"] > lim["max_clip"]:
        rej.append("clipping")
    if m["speech_db"] < lim["min_level"]:
        rej.append("too quiet")
    if m["max_gap_s"] > lim["max_gap"]:
        rej.append(f"{m['max_gap_s']:.1f}s silence inside")
    if m["silence_ratio"] > lim["max_sil"]:
        rej.append("too much silence")
    if m["snr_db"] < lim["min_snr"]:
        rej.append("noisy or backed by music")
    # duration: full marks 5.5-9.5 s, falling to 3/12 at the limits
    d = 12.0 if 5.5 <= s <= 9.5 else 12.0 * max(0.25, 1.0 - (5.5 - s if s < 5.5 else s - 9.5) / 6.0)
    # peak level: -12..-1 dBFS is healthy
    p = m["peak_db"]
    pk = 8.0 if -12.0 <= p <= -1.0 else max(0.0, 8.0 - (abs(p + 12.0) * 0.6 if p < -12.0 else abs(p + 1.0) * 3.0))
    snr = max(0.0, min(16.0, (m["snr_db"] - 12.0) / 2.5))
    sil = 12.0 * max(0.0, 1.0 - m["silence_ratio"] / lim["max_sil"])
    speaking = max(0.5, s - m["lead_s"] - m["trail_s"])
    rate = moras(text) / speaking
    r = 8.0 if 5.5 <= rate <= 10.0 else max(0.0, 8.0 - (5.5 - rate if rate < 5.5 else rate - 10.0) * 2.0)
    if not 3.0 <= rate <= 14.0:
        rej.append(f"speaking rate {rate:.1f} mora/s does not fit the text")
    lv = 4.0 if -30.0 <= m["speech_db"] <= -12.0 else 1.5
    gp = 4.0 * max(0.0, 1.0 - m["max_gap_s"] / lim["max_gap"])
    return d + pk + snr + sil + r + lv + gp, rej, rate


def candidate_clips(code, codes, lines, present, relaxed):
    """Clips of one character that pass the text rules (no audio measurements yet)."""
    stems = sorted(st for st in present if code_of(st, codes) == code)
    cnt = collections.Counter()
    for stem in stems:
        for spk, _ in lines.get(stem, ()):
            cnt[spk] += 1
    total = sum(cnt.values())
    # speakers that are the character itself, not a crowd, an unnamed voice or a duet
    ok_spk = {s for s, n in cnt.items()
              if n / total >= 0.1 and not re.search(r"声|目の前|＆|&|[？?]|^[男女]$", s)} if total else set()
    out = []
    for stem in stems:
        rows = lines.get(stem, ())
        texts = {t for _, t in rows}
        spk = {s for s, _ in rows}
        if len(texts) != 1 or not spk or not spk <= ok_spk:
            continue
        text = clean_ref_text(next(iter(texts)))
        score, rej, notes = text_check(text, relaxed)
        if [r for r in rej if not (relaxed and r == "digits or letters")]:
            continue
        info = wav_info(present[stem])
        if not info:
            continue
        out.append({"stem": stem, "path": present[stem], "text": text, "text_score": score,
                    "speaker": sorted(spk)[0], "seconds": info[2], "notes": notes})
    return out


def pick_for_code(code, codes, lines, present, per_char=3, pool=60):
    """(picks best first, candidate count, relaxed?) for one character; picks may be empty."""
    for rel in (False, True):
        lim = RELAXED if rel else STRICT
        cands = [c for c in candidate_clips(code, codes, lines, present, rel)
                 if lim["min_s"] <= c["seconds"] <= lim["max_s"]]
        # cheap pre-rank by text, then closeness to 7 s; only the top `pool` are measured
        cands.sort(key=lambda c: (-(c["text_score"] - abs(c["seconds"] - 7.0)), c["stem"]))
        scored = []
        for c in cands[:pool]:
            try:
                rate, smp = read_samples(c["path"])
                m = analyse(rate, smp)
            except ValueError:
                continue
            a, rej, mora_rate = audio_check(m, c["text"], rel)
            if not rej:
                scored.append((c["text_score"] + a, c, m, mora_rate))
        if not scored:
            continue
        scored.sort(key=lambda t: (-t[0], t[1]["stem"]))
        picks, seen = [], set()
        best = scored[0][0]
        for sc, c, m, mr in scored:
            if c["text"] in seen or (picks and sc < best - 15.0):
                continue
            seen.add(c["text"])
            why = (f"{m['seconds']:.1f} s, {c['speaker']}, SNR {m['snr_db']:.0f} dB, peak {m['peak_db']:.1f} dBFS, "
                   f"silence {100 * m['silence_ratio']:.0f} %, {mr:.1f} mora/s, score {sc:.1f}/100"
                   + (", RELAXED limits" if rel else "") + (f", note: {', '.join(c['notes'])}" if c["notes"] else ""))
            picks.append({"stem": c["stem"], "path": c["path"], "refText": c["text"],
                          "seconds": round(m["seconds"], 2), "why": why, "score": round(sc, 1),
                          "measure": {k: round(v, 3) for k, v in m.items()}})
            if len(picks) >= per_char:
                break
        return picks, len(cands), rel
    return [], 0, False


def build_refs(root, per_char=3, pool=60, absolute=False):
    """(refs map, report) for every person code that has clips on disk."""
    codes = load_codes(root)
    lines = load_clip_lines(root)
    present = voice_files(root, codes)
    refs, report = {}, {}

    def rp(p):
        return os.path.abspath(p["path"]).replace("\\", "/") if absolute else \
            os.path.relpath(p["path"], root).replace("\\", "/")

    for code in sorted(codes):
        if code in NON_PERSON:
            continue
        picks, ncand, rel = pick_for_code(code, codes, lines, present, per_char, pool)
        report[code] = {"candidates": ncand, "picked": len(picks), "relaxed": rel}
        if not picks:
            continue
        top = picks[0]
        refs[code] = {"refAudio": rp(top), "refText": top["refText"], "seconds": top["seconds"], "why": top["why"]}
        if len(picks) > 1:
            refs[code]["alternatives"] = [
                {"refAudio": rp(p), "refText": p["refText"], "seconds": p["seconds"], "why": p["why"]} for p in picks[1:]]
    return refs, report


def main(argv=None):
    global np
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--root", required=True, help="the extracted data folder (read-only)")
    ap.add_argument("--out", default=os.path.join(HERE, "voice_refs.json"), help="where to write the reference map")
    ap.add_argument("--per-char", type=int, default=3, help="clips per character (1-3, default 3)")
    ap.add_argument("--pool", type=int, default=60, help="clips measured per character (default 60)")
    ap.add_argument("--abs", action="store_true", help="absolute refAudio paths instead of relative to --root")
    ap.add_argument("--no-numpy", action="store_true", help="use the stdlib path even when numpy is installed")
    ap.add_argument("--inventory-md", help="also write the per-character inventory table (Markdown)")
    ap.add_argument("--inventory-json", help="also write the full inventory (JSON)")
    a = ap.parse_args(argv)
    if a.no_numpy:
        np = None
    if not os.path.isdir(a.root):
        print(f"error: {a.root} is not a folder", file=sys.stderr)
        return 2
    print(f"numpy: {'yes' if np is not None else 'no (stdlib fallback)'}; "
          f"soundfile: {'yes' if sf is not None else 'no (not needed for PCM WAV)'}", file=sys.stderr)
    inv = inventory(a.root)
    if a.inventory_md:
        with open(a.inventory_md, "w", encoding="utf-8", newline="\n") as f:
            f.write(inventory_markdown(inv) + "\n")
    if a.inventory_json:
        with open(a.inventory_json, "w", encoding="utf-8", newline="\n") as f:
            json.dump(inv, f, ensure_ascii=False, indent=1)
    refs, report = build_refs(a.root, max(1, min(3, a.per_char)), max(1, a.pool), a.abs)
    with open(a.out, "w", encoding="utf-8", newline="\n") as f:
        json.dump(refs, f, ensure_ascii=False, indent=2)
        f.write("\n")
    print(f"{len(inv['codes'])} codes, {inv['lines']} lines, {inv['voiced_lines']} voiced, "
          f"{inv['clips_present']} clips on disk; references for {len(refs)} characters -> {a.out}")
    for code, r in sorted(report.items()):
        if not r["picked"]:
            print(f"  no usable reference: {code} ({r['candidates']} candidates)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
