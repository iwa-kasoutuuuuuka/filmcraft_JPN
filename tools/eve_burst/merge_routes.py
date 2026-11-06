#!/usr/bin/env python3
"""Interleave the Kojirou and Marina scripts of "EVE burst error PLUS" into one viewing order.

The game is told from two sides that run on the same days. ``eve_to_script.py`` writes one script
per side, each scene with its story day in ``when`` (``D03-0012``: 3 December, 12th scene of that
day). This tool puts both into one script: day by day, the stretch of one side, then the stretch
of the other, then the part where the two sides meet (``ab01`` / ``c02``) once. That part is the
same text on both sides, so by default only side A's copy is kept. Its first scene ``requires``
the last scene before it on both sides, and the order is made to satisfy every ``requires``.

    python merge_routes.py --a out/kojirou.fcscript.json --b out/marina.fcscript.json \\
        --out out/both.fcscript.json

Either side may be given as several files (the episodes of ``eve_to_script.py --episodes``, in
order; a pattern such as ``out/kojirou.ep*.fcscript.json`` is expanded here). Check the result with
``script.validate`` (docs/script.md). Ids are prefixed by side (``k-s0001``, ``k-s0001-001``) so the
two sides never collide; minor characters (``n001``...) that differ between the sides are renamed.
Paths inside the scripts are rewritten so they stay right from the folder of ``--out``.

Everything read is treated as hostile: sizes and counts are capped and a bad file is refused with
a message. Standard library only. See docs/eve-burst-convert.md.
"""

from __future__ import annotations

import argparse
import glob
import heapq
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from eve_to_script import FINALE, natural_key, scene_days, scenes_in_route  # noqa: E402

MAX_FILE_BYTES = 64 * 1024 * 1024
MAX_FILES = 2_000
MAX_SCENES = 10_000  # what the script format allows
MAX_LINES = 200_000
MAX_CHARACTERS = 500
MAX_DEPTH = 8
REPORT_LIMIT = 200
GENERIC_RE = re.compile(r"^n\d+$")
# fields that hold a file path (relative to the script's folder)
PATH_KEYS = {"background", "bgm", "voiceFile", "sfx", "video", "subCg", "image", "openImage", "blink", "refAudio"}


class MergeError(Exception):
    pass


# -- reading ------------------------------------------------------------------------------------------
def expand(paths) -> list:
    out = []
    for p in paths:
        if any(c in p for c in "*?["):
            hits = sorted(glob.glob(p), key=natural_key)
            if not hits:
                raise MergeError(f"{p}: no file matches")
            out.extend(hits)
        else:
            out.append(p)
    if len(out) > MAX_FILES:
        raise MergeError(f"{len(out)} files: at most {MAX_FILES}")
    return out


def read_script(path: str) -> dict:
    try:
        if os.path.getsize(path) > MAX_FILE_BYTES:
            raise MergeError(f"{path}: larger than {MAX_FILE_BYTES // (1024 * 1024)} MB")
        with open(path, "rb") as f:
            data = json.loads(f.read().decode("utf-8-sig"))
    except OSError as e:
        raise MergeError(f"{path}: {e}") from e
    except (ValueError, UnicodeDecodeError) as e:
        raise MergeError(f"{path}: not a JSON script ({e})") from e
    if not isinstance(data, dict) or not isinstance(data.get("scenes"), list):
        raise MergeError(f"{path}: not a script (no scene list)")
    if data.get("version", 1) != 1:
        raise MergeError(f"{path}: script version {data.get('version')!r}, expected 1")
    return data


def rebase(p, src_dir: str, out_dir: str):
    """A path relative to ``src_dir`` made relative to ``out_dir`` (absolute across drives)."""
    if not isinstance(p, str) or not p or "://" in p or os.path.isabs(p) or p.startswith(("/", "\\")) or src_dir == out_dir:
        return p
    full = os.path.normpath(os.path.join(src_dir, p))
    try:
        rel = os.path.relpath(full, out_dir)
    except ValueError:  # another drive
        rel = full
    return rel.replace("\\", "/")


def rebase_all(node, src_dir: str, out_dir: str, depth: int = 0):
    if depth > MAX_DEPTH:
        return
    if isinstance(node, dict):
        for k, v in list(node.items()):
            if k in PATH_KEYS and isinstance(v, str):
                node[k] = rebase(v, src_dir, out_dir)
            elif k == "vowelImages" and isinstance(v, dict):
                for vk, vv in list(v.items()):
                    v[vk] = rebase(vv, src_dir, out_dir)
            elif isinstance(v, (dict, list)):
                rebase_all(v, src_dir, out_dir, depth + 1)
    elif isinstance(node, list):
        for v in node:
            rebase_all(v, src_dir, out_dir, depth + 1)


def merge_character(into: dict, other: dict) -> None:
    """Add what ``other`` has and ``into`` lacks: expressions, a voice."""
    exps = other.get("expressions")
    if isinstance(exps, dict):
        mine = into.setdefault("expressions", {})
        if isinstance(mine, dict):
            for k, v in exps.items():
                mine.setdefault(k, v)
    for k, v in other.items():
        if k not in into:
            into[k] = v


def load_side(files, out_dir: str, notes: list) -> dict:
    """One side: its characters and its scenes in order (file by file, each in route order)."""
    chars: dict = {}
    scenes: list = []
    seen: set = set()
    title = language = None
    for path in files:
        data = read_script(path)
        src_dir = os.path.dirname(os.path.abspath(path))
        title = title or data.get("title")
        language = language or data.get("language")
        cs = data.get("characters", {})
        if cs is None:
            cs = {}
        if not isinstance(cs, dict):
            raise MergeError(f"{path}: characters is not an object")
        for cid, c in cs.items():
            if not isinstance(c, dict):
                continue
            rebase_all(c, src_dir, out_dir)
            if cid in chars:
                merge_character(chars[cid], c)
            else:
                chars[cid] = c
        for sc in scenes_in_route(data):
            sid = sc.get("id")
            if not isinstance(sid, str) or not sid:
                raise MergeError(f"{path}: a scene has no id")
            if sid in seen:
                if len(notes) < REPORT_LIMIT:
                    notes.append(f"{path}: scene {sid} is already in an earlier file of the same side; the later copy is left out")
                continue
            seen.add(sid)
            if not isinstance(sc.get("lines"), list):
                raise MergeError(f"{path}: scene {sid} has no line list")
            rebase_all(sc, src_dir, out_dir)
            scenes.append(sc)
            if len(scenes) > MAX_SCENES:
                raise MergeError(f"more than {MAX_SCENES} scenes on one side")
    return {"chars": chars, "scenes": scenes, "title": title, "language": language}


# -- merging ------------------------------------------------------------------------------------------------
def is_finale(sc: dict, finale: set) -> bool:
    t = sc.get("title")
    return isinstance(t, str) and t.split(" ", 1)[0] in finale


def remap_characters(a_chars: dict, b_chars: dict, prefix_b: str, notes: list) -> dict:
    """Merge B's characters into A's; returns B's id -> merged id. The game's own codes (koj, mar,
    kat...) are one person on both sides; numbered minor characters (n001) are the same person
    only when the name is the same."""
    by_name = {}
    for cid, c in a_chars.items():
        if GENERIC_RE.match(cid) and isinstance(c.get("name"), str):
            by_name.setdefault(c["name"], cid)
    idmap = {}
    for cid, c in b_chars.items():
        name = c.get("name")
        if GENERIC_RE.match(cid):
            if isinstance(name, str) and name in by_name:
                new = by_name[name]
            elif cid not in a_chars:
                new = cid
            else:
                new = f"{prefix_b}-{cid}"
                if len(notes) < REPORT_LIMIT:
                    notes.append(f"character {cid} ({name}) of side B is someone else than {cid} ({a_chars[cid].get('name')}) of side A: renamed {new}")
        else:
            new = cid
        idmap[cid] = new
        if new in a_chars:
            merge_character(a_chars[new], c)
        else:
            a_chars[new] = c
    return idmap


def prefix_scene(sc: dict, prefix: str, speaker_map: dict | None = None) -> dict:
    sc = dict(sc)
    sc["id"] = f"{prefix}-{sc['id']}"
    req = sc.get("requires")
    if isinstance(req, list):
        sc["requires"] = [f"{prefix}-{r}" for r in req if isinstance(r, str)]
    if speaker_map and isinstance(sc.get("pov"), str):
        sc["pov"] = speaker_map.get(sc["pov"], sc["pov"])
    lines = []
    for ln in sc.get("lines") or []:
        if not isinstance(ln, dict):
            continue
        ln = dict(ln)
        if isinstance(ln.get("id"), str):
            ln["id"] = f"{prefix}-{ln['id']}"
        if speaker_map and isinstance(ln.get("speaker"), str):
            ln["speaker"] = speaker_map.get(ln["speaker"], ln["speaker"])
        lines.append(ln)
    sc["lines"] = lines
    return sc


def stable_order(scenes: list, notes: list) -> list:
    """The scenes in the given order, except that a scene waits until everything it requires has
    been shown (Kahn's algorithm, earliest first). A cycle is left in the given order (and
    script.validate reports it)."""
    pos = {sc["id"]: i for i, sc in enumerate(scenes)}
    waiting = [0] * len(scenes)
    users: dict = {}
    for i, sc in enumerate(scenes):
        for r in sc.get("requires") or []:
            j = pos.get(r)
            if j is not None and j != i:
                waiting[i] += 1
                users.setdefault(j, []).append(i)
    heap = [i for i, w in enumerate(waiting) if w == 0]
    heapq.heapify(heap)
    out, done = [], [False] * len(scenes)
    while heap:
        i = heapq.heappop(heap)
        done[i] = True
        out.append(scenes[i])
        for u in users.get(i, ()):
            waiting[u] -= 1
            if waiting[u] == 0:
                heapq.heappush(heap, u)
    left = [scenes[i] for i in range(len(scenes)) if not done[i]]
    if left:
        notes.append(f"{len(left)} scenes require each other in a circle; they are left at the end in their order")
        out.extend(left)
    moved = sum(1 for k, sc in enumerate(out) if pos[sc["id"]] != k)
    if moved and not left:
        notes.append(f"{moved} scenes moved so that what they require comes first")
    return out


def merge(a_files, b_files, out_path: str, prefix_a: str = "k", prefix_b: str = "m", finale_from: str = "a",
          finale_names=FINALE, order: str = "ab", title: str | None = None):
    """Returns (script, report)."""
    if not prefix_a or prefix_a == prefix_b:
        raise MergeError("the two sides need different, non-empty id prefixes")
    for p in (prefix_a, prefix_b):
        if not re.fullmatch(r"[A-Za-z0-9_]{1,16}", p):
            raise MergeError(f"prefix {p!r}: use 1-16 letters, digits or _")
    if finale_from not in ("a", "b", "both"):
        raise MergeError("--finale is a, b or both")
    if order not in ("ab", "ba"):
        raise MergeError("--order is ab or ba")
    out_dir = os.path.dirname(os.path.abspath(out_path)) or "."
    notes: list = []
    a = load_side(expand(a_files), out_dir, notes)
    b = load_side(expand(b_files), out_dir, notes)
    chars = a["chars"]
    idmap = remap_characters(chars, b["chars"], prefix_b, notes)
    finale = set(finale_names)

    sides = {}
    for key, side, prefix, smap in (("a", a, prefix_a, None), ("b", b, prefix_b, idmap)):
        scenes = [prefix_scene(sc, prefix, smap) for sc in side["scenes"]]
        days = scene_days(side["scenes"])
        pre = [(d, sc) for d, sc in zip(days, scenes) if not is_finale(sc, finale)]
        fin = [(d, sc) for d, sc in zip(days, scenes) if is_finale(sc, finale)]
        sides[key] = {"pre": pre, "fin": fin}

    # day by day: one side's stretch, then the other's; then where the sides meet
    first, second = ("a", "b") if order == "ab" else ("b", "a")
    all_days = sorted({d for s in sides.values() for d, _ in s["pre"]})
    planned: list = []  # (day, side tag, scene)
    for d in all_days:
        for key in (first, second):
            planned.extend((d, key, sc) for dd, sc in sides[key]["pre"] if dd == d)
    keep_fin = (first, second) if finale_from == "both" else (finale_from,)
    dropped = [sc["id"] for key in ("a", "b") if key not in keep_fin for _d, sc in sides[key]["fin"]]
    for d in sorted({d for key in keep_fin for d, _ in sides[key]["fin"]}):
        for key in keep_fin:
            planned.extend((d, "finale", sc) for dd, sc in sides[key]["fin"] if dd == d)

    # the first scene where the sides meet requires the last scene before it on both sides
    first_fin = next((sc for _d, tag, sc in planned if tag == "finale"), None)
    joins = []
    if first_fin is not None:
        for key in ("a", "b"):
            if sides[key]["pre"]:
                joins.append(sides[key]["pre"][-1][1]["id"])
        req = [r for r in first_fin.get("requires") or [] if r not in dropped]
        first_fin["requires"] = req + [j for j in joins if j not in req]

    ids = {sc["id"] for _d, _t, sc in planned}
    gone = 0
    for _d, _t, sc in planned:
        req = [r for r in sc.get("requires") or [] if r in ids and r != sc["id"]]
        gone += len(sc.get("requires") or []) - len(req)
        if req:
            sc["requires"] = req
        else:
            sc.pop("requires", None)
    if gone:
        notes.append(f"{gone} prerequisites named scenes that are not in the merged script (the left-out copy of the meeting part); dropped")

    tag_of = {sc["id"]: (d, t) for d, t, sc in planned}
    ordered = stable_order([sc for _d, _t, sc in planned], notes)

    # story time: D07-02-0013 = 7 December, the 2nd stretch of that day, its 13th scene
    blocks: list = []
    day_blocks: dict = {}
    for sc in ordered:
        d, t = tag_of[sc["id"]]
        if not blocks or blocks[-1]["day"] != d or blocks[-1]["side"] != t:
            day_blocks[d] = day_blocks.get(d, 0) + 1
            blocks.append({"day": d, "side": t, "block": day_blocks[d], "first": sc["id"], "scenes": 0, "lines": 0})
        blk = blocks[-1]
        blk["scenes"] += 1
        blk["lines"] += len(sc["lines"])
        blk["last"] = sc["id"]
        sc["when"] = f"D{d:02d}-{min(blk['block'], 99):02d}-{min(blk['scenes'], 9999):04d}"

    n_lines = sum(len(sc["lines"]) for sc in ordered)
    if len(ordered) > MAX_SCENES:
        raise MergeError(f"the merged script would have {len(ordered)} scenes; the format allows {MAX_SCENES}")
    if n_lines > MAX_LINES:
        raise MergeError(f"the merged script would have {n_lines} lines; the format allows {MAX_LINES}")
    used = set()
    for sc in ordered:
        if isinstance(sc.get("pov"), str):
            used.add(sc["pov"])
        for ln in sc["lines"]:
            if isinstance(ln.get("speaker"), str):
                used.add(ln["speaker"])
    out_chars = {k: v for k, v in sorted(chars.items()) if k in used}
    if len(out_chars) > MAX_CHARACTERS:
        raise MergeError(f"{len(out_chars)} characters; the format allows {MAX_CHARACTERS}")
    script = {
        "version": 1,
        "title": title or "EVE burst error PLUS 多視点",
        "language": a["language"] or b["language"] or "Japanese",
        "characters": out_chars,
        "scenes": ordered,
        "route": [sc["id"] for sc in ordered],
    }
    side_name = {"a": "A", "b": "B", "finale": "finale"}
    report = {
        "scenes": len(ordered),
        "lines": n_lines,
        "sideA": {"scenes": len(a["scenes"]), "prefix": prefix_a},
        "sideB": {"scenes": len(b["scenes"]), "prefix": prefix_b},
        "finaleFrom": finale_from,
        "finaleScenesLeftOut": len(dropped),
        "finaleRequires": first_fin.get("requires") if first_fin else None,
        "blocks": [dict(blk, side=side_name[blk["side"]]) for blk in blocks][:REPORT_LIMIT],
        "renamedCharacters": {k: v for k, v in idmap.items() if k != v},
        "notes": notes[:REPORT_LIMIT],
    }
    return script, report


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--a", "--kojirou", dest="a", nargs="+", required=True, metavar="FILE", help="side A: one script, or its episodes in order (patterns are expanded)")
    ap.add_argument("--b", "--marina", dest="b", nargs="+", required=True, metavar="FILE", help="side B, the same way")
    ap.add_argument("--out", required=True, help="the merged script (*.fcscript.json)")
    ap.add_argument("--report", help="also write what was merged (stretches by day, renamed characters, notes) here")
    ap.add_argument("--prefix-a", default="k", help="id prefix of side A (default k: k-s0001)")
    ap.add_argument("--prefix-b", default="m", help="id prefix of side B (default m)")
    ap.add_argument("--finale", choices=("a", "b", "both"), default="a", help="whose copy of the part where the sides meet (ab01, c02) to keep (default a; it is the same text)")
    ap.add_argument("--finale-scenarios", default=",".join(FINALE), help="scenarios of that part, by the first word of a scene title (default ab01,c02)")
    ap.add_argument("--order", choices=("ab", "ba"), default="ab", help="within a day, side A's stretch first (ab, default) or side B's")
    ap.add_argument("--title", default=None)
    a = ap.parse_args(argv)
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except Exception:  # noqa: BLE001
            pass
    names = tuple(n.strip() for n in a.finale_scenarios.split(",") if n.strip())
    try:
        script, report = merge(a.a, a.b, a.out, a.prefix_a, a.prefix_b, a.finale, names, a.order, a.title)
    except MergeError as e:
        print(f"merge_routes: {e}", file=sys.stderr)
        return 2
    out_dir = os.path.dirname(os.path.abspath(a.out)) or "."
    os.makedirs(out_dir, exist_ok=True)
    with open(a.out, "w", encoding="utf-8", newline="\n") as f:
        json.dump(script, f, ensure_ascii=False, indent=1)
        f.write("\n")
    if a.report:
        with open(a.report, "w", encoding="utf-8", newline="\n") as f:
            json.dump(report, f, ensure_ascii=False, indent=1)
            f.write("\n")
    brief = {k: report[k] for k in ("scenes", "lines", "finaleScenesLeftOut", "finaleRequires", "renamedCharacters")}
    brief["out"] = os.path.abspath(a.out)
    brief["stretches"] = [f"D{b['day']:02d} {b['side']} {b['lines']}" for b in report["blocks"]]
    print(json.dumps(brief, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
