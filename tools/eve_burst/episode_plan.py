#!/usr/bin/env python3
"""Cut the merged "EVE burst error PLUS" script into a series of episodes of a set length.

The input is one script with both sides (``merge_routes.py``), in story order. This tool:

1. **trims** what is safe to drop without reading it: passages shown again (three or more lines
   in a row whose speaker and text were all shown before: the same conversation from the other
   side, a corridor walked twice) and unrecorded lines of dots only (「‥‥。」, which become a pause
   on the line before);
2. **estimates** the length of every line the way ``anime.build`` lays it out (a recording or a
   sound effect by its own length, an unrecorded line by its characters at ``--cps``, the pause
   after it, a beat at the start of each scene);
3. **plans** the series: ``--episodes`` episodes of ``--minutes`` minutes each after adaptation,
   the last one ``--final-minutes`` (the meeting of the two sides and the ending). One keep ratio
   for the whole series follows from that; each day of the story gets episodes in proportion to
   its length, and within a day each side's part is cut into whole episodes that alternate
   between the sides in time order (one viewpoint per episode, as the game switches sides);
4. **writes** the draft episodes (``<stem>.ep01.fcscript.json``...), a plan (``<stem>.plan.json``)
   and the brief for whoever adapts them (``<stem>.plan.md``). ``check_adaptation.py`` then checks
   an adapted episode against its draft.

    python episode_plan.py --script out/both.fcscript.json --out-dir out/series --stem eve \\
        --episodes 29 --minutes 27 --final-minutes 60

Nothing is rewritten here: lines are only dropped by the two rules above. Everything read is
treated as hostile (sizes and counts capped, a bad file refused with a message). Standard library
only. See docs/eve-burst-convert.md.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import struct
import sys
import wave

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from eve_to_script import FINALE, day_label, scene_days, scenes_in_route  # noqa: E402
from merge_routes import MergeError, read_script, rebase_all  # noqa: E402

MAX_EPISODES = 99
MAX_SECONDS = 3600.0  # one clip; anime.build ignores longer audio
MAX_MP4_BOXES = 4096
POV_LABELS = {"koj": "小次郎", "mar": "まりな"}
# a line made of these only says nothing (dots, dashes, marks)
SILENT_RE = re.compile(r"^[\s　‥…・、。，．,.！？!?ー～〜―\-「」『』（）()〈〉]*$")
RUBY_RE = re.compile(r"[｜|]?([^《｜|]*)《([^》]*)》")


class PlanError(Exception):
    pass


# -- length of a line, as anime.build lays it out -------------------------------------------------------
def wav_seconds(path: str):
    try:
        with wave.open(path, "rb") as w:
            rate = w.getframerate()
            if rate <= 0:
                return None
            s = w.getnframes() / rate
    except (OSError, EOFError, wave.Error, ValueError, struct.error):
        return None
    return s if 0 < s <= MAX_SECONDS else None


def mp4_seconds(path: str):
    """The movie header's duration (``moov``/``mvhd``), read box by box without loading the file."""
    try:
        size_all = os.path.getsize(path)
        with open(path, "rb") as f:
            def boxes(start, end):
                pos, n = start, 0
                while pos + 8 <= end and n < MAX_MP4_BOXES:
                    n += 1
                    f.seek(pos)
                    head = f.read(8)
                    if len(head) < 8:
                        return
                    size, kind = struct.unpack(">I4s", head)
                    hdr = 8
                    if size == 1:
                        big = f.read(8)
                        if len(big) < 8:
                            return
                        size = struct.unpack(">Q", big)[0]
                        hdr = 16
                    elif size == 0:
                        size = end - pos
                    if size < hdr or pos + size > end:
                        return
                    yield kind, pos + hdr, pos + size
                    pos += size
            for kind, s, e in boxes(0, size_all):
                if kind != b"moov":
                    continue
                for k2, s2, _e2 in boxes(s, e):
                    if k2 != b"mvhd":
                        continue
                    f.seek(s2)
                    ver = f.read(4)
                    if len(ver) < 4:
                        return None
                    if ver[0] == 1:
                        data = f.read(28)
                        if len(data) < 28:
                            return None
                        scale, dur = struct.unpack(">I Q", data[16:28])
                    else:
                        data = f.read(16)
                        if len(data) < 16:
                            return None
                        scale, dur = struct.unpack(">II", data[8:16])
                    if scale <= 0:
                        return None
                    sec = dur / scale
                    return sec if 0 < sec <= MAX_SECONDS else None
    except (OSError, struct.error, ValueError):
        return None
    return None


def speech_text(ln: dict) -> str:
    """What is spoken: ``reading``, else the text with its ruby read out (as script_tools does)."""
    r = ln.get("reading")
    if isinstance(r, str) and r.strip():
        return r
    t = ln.get("text") if isinstance(ln.get("text"), str) else ""
    return RUBY_RE.sub(lambda m: m.group(2), t)


def is_spoken(ln: dict) -> bool:
    return ln.get("kind", "dialogue") != "direction" and isinstance(ln.get("text"), str) and bool(ln["text"].strip())


class Estimator:
    """Seconds of lines and scenes, with anime.build's defaults (``pause`` 0.35, ``sceneGap`` 0.6,
    ``directionSeconds`` 1, ``charsPerSecond`` 7, at least 0.8 s for an estimated line)."""

    def __init__(self, base_dir: str, cps: float = 7.0, pause: float = 0.35, scene_gap: float = 0.6, direction: float = 1.0):
        if not (cps > 0) or pause < 0 or scene_gap < 0 or not (direction > 0):
            raise PlanError("--cps and the direction time must be positive, pauses not negative")
        self.base = base_dir
        self.cps, self.pause, self.scene_gap, self.direction = cps, pause, scene_gap, direction
        self.cache: dict = {}

    def media(self, p, fn):
        if not isinstance(p, str) or not p:
            return None
        full = p if os.path.isabs(p) else os.path.join(self.base, p)
        key = (fn.__name__, full)
        if key not in self.cache:
            self.cache[key] = fn(full)
        return self.cache[key]

    def line(self, ln: dict) -> float:
        pa = ln.get("pauseAfter")
        pa = float(pa) if isinstance(pa, (int, float)) and not isinstance(pa, bool) and 0 <= pa <= 60 else None
        if is_spoken(ln):
            s = self.media(ln.get("voiceFile"), wav_seconds)
            if s is None:
                s = max(0.8, len(speech_text(ln)) / self.cps)
            return s + (self.pause if pa is None else pa)
        s = self.media(ln.get("video"), mp4_seconds) if ln.get("video") else None
        if s is None:
            s = self.media(ln.get("sfx"), wav_seconds) or self.direction
        return s + (pa or 0.0)

    def scene(self, sc: dict) -> float:
        return self.scene_gap + sum(self.line(ln) for ln in sc.get("lines") or [] if isinstance(ln, dict))


# -- trimming -------------------------------------------------------------------------------------------------
def trim(scenes: list, min_run: int = 3, drop_dots: bool = True, dot_pause: float = 0.6):
    """Drop passages shown again and unrecorded dot-only lines. Returns (scenes, stats); the input
    is not changed. A scene left with no lines is dropped, and what required it requires the
    scene before it instead."""
    flat = []  # (scene index, line index, line)
    for i, sc in enumerate(scenes):
        for j, ln in enumerate(sc.get("lines") or []):
            if isinstance(ln, dict):
                flat.append((i, j, ln))
    seen = set()
    dup = [False] * len(flat)
    for k, (_i, _j, ln) in enumerate(flat):
        if not is_spoken(ln):
            continue
        key = (ln.get("speaker"), ln.get("text"))
        dup[k] = key in seen
        seen.add(key)
    drop = set()
    if min_run > 0:
        k = 0
        while k < len(flat):
            if dup[k]:
                m, spoken_run = k, []
                while m < len(flat) and (dup[m] or not is_spoken(flat[m][2])):
                    if dup[m]:
                        spoken_run.append(m)
                    m += 1
                if len(spoken_run) >= min_run:
                    drop.update(spoken_run)
                k = m
            else:
                k += 1
    dropped = {(flat[k][0], flat[k][1]) for k in drop}
    dots = 0
    out_scenes, stats = [], {"replayedLines": len(drop), "dotLines": 0, "scenesDropped": 0}
    kept_ids: list = []
    alias: dict = {}
    prev_line = None
    for i, sc in enumerate(scenes):
        new = dict(sc)
        lines = []
        for j, ln in enumerate(sc.get("lines") or []):
            if not isinstance(ln, dict) or (i, j) in dropped:
                continue
            if drop_dots and is_spoken(ln) and not ln.get("voiceFile") and not ln.get("sfx") and SILENT_RE.match(ln["text"]):
                dots += 1
                target = lines[-1] if lines else prev_line
                if target is not None:
                    base = target.get("pauseAfter")
                    base = base if isinstance(base, (int, float)) and not isinstance(base, bool) else (0.35 if is_spoken(target) else 0.0)
                    target["pauseAfter"] = round(min(60.0, base + dot_pause), 3)
                continue
            ln = dict(ln)
            lines.append(ln)
        if not lines:
            stats["scenesDropped"] += 1
            if kept_ids:
                alias[sc.get("id")] = kept_ids[-1]
            continue
        new["lines"] = lines
        prev_line = lines[-1]
        kept_ids.append(new.get("id"))
        out_scenes.append(new)
    stats["dotLines"] = dots
    for sc in out_scenes:
        req = []
        for r in sc.get("requires") or []:
            r = alias.get(r, r)
            if r != sc.get("id") and r not in req:
                req.append(r)
        if req:
            sc["requires"] = req
        else:
            sc.pop("requires", None)
    return out_scenes, stats


# -- planning -------------------------------------------------------------------------------------------------
def largest_remainder(weights: list, total: int, minimum: int = 0) -> list:
    """Split ``total`` whole parts by ``weights`` (each at least ``minimum``)."""
    n = len(weights)
    if n == 0:
        return []
    if total < minimum * n:
        raise PlanError(f"{total} parts for {n} groups that need at least {minimum} each")
    wsum = sum(w for w in weights if w > 0)
    if wsum <= 0:
        out = [minimum] * n
        for k in range(total - minimum * n):
            out[k % n] += 1
        return out
    shares = [max(0.0, w) / wsum * total for w in weights]
    out = [max(minimum, int(s)) for s in shares]
    # too few: the largest remainders get one more; too many (minimums): the largest give one back
    while sum(out) < total:
        k = max(range(n), key=lambda i: (shares[i] - out[i], -i))
        out[k] += 1
    while sum(out) > total:
        k = max((i for i in range(n) if out[i] > minimum), key=lambda i: (out[i] - shares[i], -i))
        out[k] -= 1
    return out


def cut_even(items: list, secs: list, k: int) -> list:
    """Cut a sequence into ``k`` runs of about equal length, between items (never empty runs)."""
    n = len(items)
    k = max(1, min(k, n))
    total = sum(secs)
    cuts, acc, want = [], 0.0, 1
    for idx in range(n - 1):
        acc += secs[idx]
        if want >= k:
            break
        target = total * want / k
        nxt = acc + secs[idx + 1]
        left_items = n - (idx + 1)
        need = k - want
        # cut here when this boundary is closer to the target than the next one, or items run out
        if (abs(acc - target) <= abs(nxt - target) and acc >= target * 0.5) or left_items <= need:
            cuts.append(idx + 1)
            want += 1
    runs, start = [], 0
    for c in cuts:
        runs.append(items[start:c])
        start = c
    runs.append(items[start:])
    return [r for r in runs if r]


def side_of(sc: dict, finale: set) -> str:
    t = sc.get("title")
    if isinstance(t, str) and t.split(" ", 1)[0] in finale:
        return "finale"
    p = sc.get("pov")
    return p if isinstance(p, str) else "?"


def plan(script: dict, est: Estimator, episodes: int = 29, minutes: float = 27.0, final_minutes: float = 60.0,
         min_run: int = 3, drop_dots: bool = True, finale_names=FINALE) -> dict:
    if not 1 <= episodes <= MAX_EPISODES:
        raise PlanError(f"--episodes must be 1..{MAX_EPISODES}")
    if not (0 < minutes <= 600) or not (0 <= final_minutes <= 600):
        raise PlanError("--minutes must be 0..600 (more than 0), --final-minutes 0..600")
    finale = set(finale_names)
    scenes = scenes_in_route(script)
    if not scenes:
        raise PlanError("the script has no scenes")
    src_secs = sum(est.scene(sc) for sc in scenes)
    if drop_dots or min_run > 0:
        scenes, tstats = trim(scenes, min_run, drop_dots)
    else:
        tstats = {"replayedLines": 0, "dotLines": 0, "scenesDropped": 0}
    if not scenes:
        raise PlanError("nothing is left after trimming")
    days = scene_days(scenes)
    secs = [est.scene(sc) for sc in scenes]
    units = [{"scene": sc, "day": d, "side": side_of(sc, finale), "secs": s} for sc, d, s in zip(scenes, days, secs)]
    total = sum(secs)
    with_final = final_minutes > 0 and episodes > 1
    regular = episodes - 1 if with_final else episodes
    target_total = (regular * minutes + (final_minutes if with_final else 0)) * 60.0
    keep = min(1.0, target_total / total) if total > 0 else 1.0

    # the last episode: the meeting part and the ending, and before it the end of the last day on
    # each side, in proportion to the sides, as far as it fits
    final_units: list = []
    if with_final:
        budget = final_minutes * 60.0 / keep
        k = len(units)
        while k > 0 and units[k - 1]["side"] == "finale":
            k -= 1
        if k == 0:
            raise PlanError("the script is nothing but the meeting part: use --final-minutes 0")
        chosen = set(range(k, len(units)))
        rest = budget - sum(units[i]["secs"] for i in chosen)
        last_day = units[k - 1]["day"]
        day_idx = [i for i in range(k) if units[i]["day"] == last_day]
        by_side: dict = {}
        for i in day_idx:
            by_side.setdefault(units[i]["side"], []).append(i)
        day_total = sum(units[i]["secs"] for i in day_idx) or 1.0
        for _s, idx in by_side.items():
            want = rest * sum(units[i]["secs"] for i in idx) / day_total
            acc = 0.0
            for i in reversed(idx):
                if acc + units[i]["secs"] > want:
                    break
                acc += units[i]["secs"]
                chosen.add(i)
        if len(chosen) >= len(units) or all(i in chosen for i in range(k)):
            raise PlanError("the last episode would take the whole script: lower --final-minutes")
        final_units = [units[i] for i in sorted(chosen)]
        units = [u for i, u in enumerate(units) if i not in chosen]

    # days, in order; a day too short for an episode of its own goes with the next one
    per_ep = minutes * 60.0 / keep
    groups: list = []
    for u in units:
        if not groups or groups[-1]["days"][-1] != u["day"]:
            groups.append({"days": [u["day"]], "units": []})
        groups[-1]["units"].append(u)
    merged: list = []
    for g in groups:
        if merged and sum(u["secs"] for u in merged[-1]["units"]) < per_ep * 0.5:
            merged[-1]["days"] += g["days"]
            merged[-1]["units"] += g["units"]
        else:
            merged.append(g)
    if len(merged) > 1 and sum(u["secs"] for u in merged[-1]["units"]) < per_ep * 0.5:
        last = merged.pop()
        merged[-1]["days"] += last["days"]
        merged[-1]["units"] += last["units"]
    while len(merged) > regular and len(merged) > 1:
        # fewer episodes than days: join the shortest neighbouring pair
        sizes = [sum(u["secs"] for u in g["units"]) for g in merged]
        j = min(range(len(merged) - 1), key=lambda k: sizes[k] + sizes[k + 1])
        merged[j]["days"] += merged[j + 1]["days"]
        merged[j]["units"] += merged.pop(j + 1)["units"]
    if regular < 1 or not merged:
        raise PlanError("no regular episodes are left: raise --episodes")
    counts = largest_remainder([sum(u["secs"] for u in g["units"]) for g in merged], regular, 1)

    # within a day: each side's part in whole episodes, alternating in time order
    eps: list = []
    for g, n in zip(merged, counts):
        sides: dict = {}
        for u in g["units"]:
            sides.setdefault(u["side"], []).append(u)
        names = list(sides)
        ssecs = [sum(u["secs"] for u in sides[s]) for s in names]
        if n >= len(names) and all(x >= per_ep * 0.25 for x in ssecs):
            ks = largest_remainder(ssecs, n, 1)
            chunks = []
            for order, (s, k) in enumerate(zip(names, ks)):
                runs = cut_even(sides[s], [u["secs"] for u in sides[s]], k)
                tot = sum(u["secs"] for u in sides[s]) or 1.0
                acc = 0.0
                for r in runs:
                    rs = sum(u["secs"] for u in r)
                    chunks.append(((acc + rs / 2) / tot, order, r))
                    acc += rs
            chunks.sort(key=lambda c: (c[0], c[1]))
            eps.extend(c[2] for c in chunks)
        else:
            eps.extend(cut_even(g["units"], [u["secs"] for u in g["units"]], n))
    if final_units:
        eps.append(final_units)
    return {"episodes": eps, "keep": keep, "sourceSeconds": src_secs, "trimmedSeconds": total,
            "trim": tstats, "targetMinutes": minutes, "finalMinutes": final_minutes if with_final else None}


# -- writing -------------------------------------------------------------------------------------------------
def episode_title(n: int, units: list) -> str:
    days = sorted({u["day"] for u in units})
    if days and days[0] != days[-1]:
        label = f"{day_label(days[0])}〜{days[-1]}日" if days[0] > 0 else f"{day_label(days[0])}〜{day_label(days[-1])}"
    else:
        label = day_label(days[0]) if days else ""
    povs = {u["scene"].get("pov") for u in units if u["side"] != "finale"}
    who = POV_LABELS.get(next(iter(povs))) if len(povs) == 1 else None
    return f"第 {n} 話 {label}" + (f"（{who}）" if who else "")


def build_episode(script: dict, n: int, total: int, units: list, src_dir: str, out_dir: str) -> dict:
    width = max(2, len(str(total)))
    ids = {u["scene"].get("id") for u in units}
    scenes, seq = [], 0
    for u in units:
        sc = json.loads(json.dumps(u["scene"], ensure_ascii=False))
        seq += 1
        sc["when"] = f"D{u['day']:02d}-{n:0{width}d}-{min(seq, 9999):04d}"
        req = [r for r in sc.get("requires") or [] if r in ids]
        if req:
            sc["requires"] = req
        else:
            sc.pop("requires", None)
        rebase_all(sc, src_dir, out_dir)
        scenes.append(sc)
    used = set()
    for sc in scenes:
        if isinstance(sc.get("pov"), str):
            used.add(sc["pov"])
        for ln in sc.get("lines") or []:
            if isinstance(ln.get("speaker"), str):
                used.add(ln["speaker"])
    chars = {}
    for k, v in sorted((script.get("characters") or {}).items()):
        if k in used and isinstance(v, dict):
            c = json.loads(json.dumps(v, ensure_ascii=False))
            rebase_all(c, src_dir, out_dir)
            chars[k] = c
    return {"version": 1, "title": episode_title(n, units), "language": script.get("language", "Japanese"),
            "characters": chars, "scenes": scenes, "route": [sc["id"] for sc in scenes]}


BRIEF_RULES = """\
## 脚色の決まり

- **録音のある行**（`voiceFile` あり）は**削除だけ**。`text`・`speaker`・`kind`・`voiceFile` を変えない
  （録音と合わなくなる）。読みを直す `reading` は足してよい。
- **録音の無い行**（主人公の台詞の大半、語り）は、**筋に影響しない範囲で書き換え・統合してよい**:
  事実・手がかり・人物の関係・伏線・誰が何を知ったかを変えない。言い回しを詰める、続く数行を 1 行に
  まとめる、調べ物の反応を要約する、のは可。書き換えた行には `"generated": true` を付ける（id はそのまま、
  まとめた行は先頭の行の id を残して残りを消す）。
- **新しい行**（場面のつなぎ）は `"generated": true` と新しい id（`<場面 id>-g01` など）。録音は付けない。
- **順序は入れ替えない**（行も場面も）。場面は消してよいが、新しい場面は作らない。
- 目標の尺に入れる（下の表、推定は anime.build と同じ見積もり）。
- 終わったら `python tools/eve_burst/check_adaptation.py --source <下書き> --edited <脚色後>` と
  `script.validate` がエラー 0。
"""


def write_outputs(script: dict, res: dict, src_path: str, out_dir: str, stem: str, est_out: Estimator) -> dict:
    os.makedirs(out_dir, exist_ok=True)
    src_dir = os.path.dirname(os.path.abspath(src_path))
    eps = res["episodes"]
    width = max(2, len(str(len(eps))))
    rows = []
    for n, units in enumerate(eps, 1):
        ep = build_episode(script, n, len(eps), units, src_dir, out_dir)
        name = f"{stem}.ep{n:0{width}d}.fcscript.json"
        with open(os.path.join(out_dir, name), "w", encoding="utf-8", newline="\n") as f:
            json.dump(ep, f, ensure_ascii=False, indent=1)
            f.write("\n")
        src_s = sum(u["secs"] for u in units)
        is_final = res["finalMinutes"] is not None and n == len(eps)
        target = (res["finalMinutes"] if is_final else res["targetMinutes"])
        scen = []
        for u in units:
            t = u["scene"].get("title")
            w = t.split(" ", 1)[0] if isinstance(t, str) and t else "?"
            if not scen or scen[-1] != w:
                scen.append(w)
        lines = sum(len(u["scene"].get("lines") or []) for u in units)
        voiced = sum(1 for u in units for ln in u["scene"].get("lines") or [] if ln.get("voiceFile"))
        rows.append({
            "episode": n, "file": name, "title": ep["title"], "final": is_final,
            "days": sorted({u["day"] for u in units}), "povs": sorted({str(u["scene"].get("pov")) for u in units}),
            "scenes": len(units), "lines": lines, "voicedLines": voiced,
            "draftMinutes": round(src_s / 60, 1), "targetMinutes": target,
            "keep": round(min(1.0, target * 60 / src_s), 3) if src_s > 0 else 1.0,
            "firstScene": units[0]["scene"].get("id"), "lastScene": units[-1]["scene"].get("id"),
            "scenarios": scen[:60],
        })
    summary = {
        "source": os.path.abspath(src_path),
        "sourceHours": round(res["sourceSeconds"] / 3600, 2),
        "afterTrimHours": round(res["trimmedSeconds"] / 3600, 2),
        "trim": res["trim"],
        "keep": round(res["keep"], 3),
        "episodes": rows,
        "estimate": {"cps": est_out.cps, "pause": est_out.pause, "sceneGap": est_out.scene_gap, "directionSeconds": est_out.direction},
    }
    with open(os.path.join(out_dir, f"{stem}.plan.json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump(summary, f, ensure_ascii=False, indent=1)
        f.write("\n")
    md = [f"# 話数の計画（{stem}）", "",
          f"元: `{os.path.basename(src_path)}` 推定 {summary['sourceHours']} 時間 → 機械的な間引きの後 {summary['afterTrimHours']} 時間"
          f"（再表示 {res['trim']['replayedLines']} 行、点だけの行 {res['trim']['dotLines']} 行を除いた）。"
          f"シリーズ全体で約 {round(res['keep'] * 100)}% を残す。", "",
          "| 話 | ファイル | タイトル | 日 | 場面 | 行（録音） | 下書き | 目標 | 残す割合 | シナリオ |",
          "|---|---|---|---|---|---|---|---|---|---|"]
    for r in rows:
        md.append(f"| {r['episode']} | `{r['file']}` | {r['title']} | {','.join(str(d) for d in r['days'])} | {r['scenes']} | "
                  f"{r['lines']}（{r['voicedLines']}） | {r['draftMinutes']} 分 | {r['targetMinutes']} 分 | {round(r['keep'] * 100)}% | "
                  f"{' '.join(r['scenarios'][:12])}{' …' if len(r['scenarios']) > 12 else ''} |")
    md += ["", BRIEF_RULES]
    with open(os.path.join(out_dir, f"{stem}.plan.md"), "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(md) + "\n")
    return summary


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--script", required=True, help="the merged script (merge_routes.py), in story order")
    ap.add_argument("--out-dir", required=True, help="folder for the draft episodes, the plan and the brief")
    ap.add_argument("--stem", default="eve", help="file name start: <stem>.ep01.fcscript.json (default eve)")
    ap.add_argument("--episodes", type=int, default=29, help="episodes in all, the last one included (default 29)")
    ap.add_argument("--minutes", type=float, default=27.0, help="length of a regular episode after adaptation (default 27)")
    ap.add_argument("--final-minutes", type=float, default=60.0, help="length of the last episode (default 60; 0 = no special last episode)")
    ap.add_argument("--min-run", type=int, default=3, help="drop passages shown again of at least this many lines (default 3; 0 = keep them)")
    ap.add_argument("--keep-dots", action="store_true", help="keep unrecorded lines of dots only (default: they become a pause)")
    ap.add_argument("--cps", type=float, default=7.0, help="characters per second of an unrecorded line (anime.build's charsPerSecond)")
    ap.add_argument("--pause", type=float, default=0.35)
    ap.add_argument("--scene-gap", type=float, default=0.6)
    a = ap.parse_args(argv)
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except Exception:  # noqa: BLE001
            pass
    if not re.fullmatch(r"[A-Za-z0-9_.\-]{1,64}", a.stem):
        print("episode_plan: --stem: use 1-64 letters, digits, _ . -", file=sys.stderr)
        return 2
    if a.min_run < 0:
        print("episode_plan: --min-run must not be negative", file=sys.stderr)
        return 2
    try:
        script = read_script(a.script)
        est = Estimator(os.path.dirname(os.path.abspath(a.script)), a.cps, a.pause, a.scene_gap)
        res = plan(script, est, a.episodes, a.minutes, a.final_minutes, a.min_run, not a.keep_dots)
        summary = write_outputs(script, res, a.script, a.out_dir, a.stem, est)
    except (MergeError, PlanError) as e:
        print(f"episode_plan: {e}", file=sys.stderr)
        return 2
    brief = {k: summary[k] for k in ("sourceHours", "afterTrimHours", "keep", "trim")}
    brief["episodes"] = [f"{r['episode']}: {r['title']} {r['draftMinutes']}→{r['targetMinutes']} 分" for r in summary["episodes"]]
    print(json.dumps(brief, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
