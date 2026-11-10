#!/usr/bin/env python3
"""Copy what the game puts on screen from a fresh conversion into a script that was edited since.

`eve_to_script.py --screen` writes movies (`video` lines), sub CG insets (`subCg`, `subCgAnchor`), the
side of a character in a two-shot (`slot`), fades (`transition`) and `hideCharacters`. An episode that
was cut, adapted, voiced and corrected from an earlier conversion has none of them, and converting again
would lose that work. The ids of scenes and lines are the same in both, so this tool takes the screen
fields from the conversion (the donor) and writes them into the episode (the script):

    python screen_fields.py --script ep02.fcscript.json --donor k=kojirou.fcscript.json \
        --donor m=marina.fcscript.json --siblings "adapted/eve.ep*.fcscript.json" --out ep02.screen.fcscript.json

`k=` is the prefix merge_routes.py put before the ids of that side (`k-s0001-001`); a donor without `=`
has the ids as they are. What is written:

- a scene: `hideCharacters` as in the donor; `transition: "fade"` when the donor fades into it or into a
  scene that was cut before it, when the side changes, and on the first scene of the script
  (`--no-fade-first` leaves that one out);
- a line: `slot` as in the donor. While one side of a two-shot is taken, a pictured speaker without a
  side (the hero, whom the game never draws, or someone the game put in the centre) takes the free side,
  so the two stay on screen together instead of replacing each other;
- `subCg` / `subCgAnchor`: the inset the donor has on screen at that line (an inset that came up on a
  line that was cut still shows on the lines kept after it, and goes when the donor takes it away);
- movies: a `video` line of the donor goes before the first line kept after it, and before the lines
  the adaptation added in front of that line. It is placed only when that line is near (`--max-gap`,
  40 lines of the donor) or when the movie opens a story day (the scenes' `when` up to the `-`) and the
  line is of that day; a movie whose surroundings were cut is left out. A movie with no line kept after
  it goes after the last line kept before it when that one is near, and the one after the donor's last
  line (the ending) to the end of the script. With `--siblings` (the other episodes of the series) a movie
  between two episodes goes to the one that has the next line. A file is placed once in a script.

Lines the adaptation added (ids the donor does not have) keep the screen of the line before them. The
fields already in the script are replaced, so the tool can be run again. Exit code 1, and nothing written,
when no line of the script is in the donors. Standard library only; nothing is read but the scripts.
"""

from __future__ import annotations

import argparse
import glob
import io
import json
import os
import sys

MAX_BYTES = 256 * 1024 * 1024
MAX_GAP = 40
LINE_FIELDS = ("slot", "subCg", "subCgAnchor")


def load(path: str):
    """A script as a dict, or None when it cannot be read."""
    try:
        if os.path.getsize(path) > MAX_BYTES:
            return None
        with io.open(path, encoding="utf-8-sig") as f:
            s = json.load(f)
    except (OSError, ValueError, RecursionError):
        return None
    return s if isinstance(s, dict) else None


def scenes_of(script) -> list:
    sc = script.get("scenes") if isinstance(script, dict) else None
    return [s for s in sc if isinstance(s, dict)] if isinstance(sc, list) else []


def lines_of(scene) -> list:
    ls = scene.get("lines") if isinstance(scene, dict) else None
    return [l for l in ls if isinstance(l, dict)] if isinstance(ls, list) else []


def rebase(path, src_dir: str, dst_dir: str):
    """A relative path of the donor as a path relative to the output script."""
    if not isinstance(path, str) or not path:
        return path
    full = path if os.path.isabs(path) else os.path.normpath(os.path.join(src_dir, path))
    try:
        return os.path.relpath(full, dst_dir).replace("\\", "/")
    except ValueError:  # another drive
        return full.replace("\\", "/")


def day_of(scene) -> str | None:
    """The story day of a scene: its `when` up to the first `-` (`D04-0007` -> `D04`)."""
    w = scene.get("when") if isinstance(scene, dict) else None
    return w.split("-", 1)[0] if isinstance(w, str) and w else None


class Donor:
    """One conversion: the screen at each of its lines, by the id the script uses."""

    def __init__(self, prefix: str, script: dict, src_dir: str):
        self.prefix = prefix
        self.src_dir = src_dir
        self.index: dict[str, int] = {}  # line id -> its place among the lines that are not movies
        self.day: list = []  # the story day of the line at each place
        self.slot: dict[str, int] = {}
        self.inset: dict[str, tuple | None] = {}  # line id -> (path, anchor) on screen at that line
        self.scene_index: dict[str, int] = {}
        self.fade: list[bool] = []
        self.hide: dict[str, bool] = {}
        self.videos: list[dict] = []  # {"before": place of the next line, "id", "path", "day", "opens"}
        days_begun = set()
        n = 0
        for si, sc in enumerate(scenes_of(script)):
            sid = self.name(sc.get("id"))
            self.fade.append(sid is not None and sc.get("transition") == "fade")
            if sid is None:
                continue
            self.scene_index.setdefault(sid, si)
            self.hide[sid] = sc.get("hideCharacters") is True
            day = day_of(sc)
            shown = None
            for ln in lines_of(sc):
                lid = self.name(ln.get("id"))
                if lid is None:
                    continue
                if isinstance(ln.get("video"), str) and ln["video"]:
                    self.videos.append({"before": n, "id": lid, "path": ln["video"], "day": day, "opens": day is not None and day not in days_begun})
                    shown = None
                    continue
                if lid in self.index:  # an id a second time: the first one counts
                    continue
                if "subCg" in ln:
                    p = ln.get("subCg")
                    a = ln.get("subCgAnchor")
                    ok = isinstance(a, list) and len(a) == 2 and all(isinstance(v, (int, float)) and not isinstance(v, bool) and 0 <= v <= 1 for v in a)
                    shown = (p, tuple(a) if ok else None) if isinstance(p, str) and p else None
                self.inset[lid] = shown
                if ln.get("slot") in (1, 2, 3) and not isinstance(ln.get("slot"), bool):
                    self.slot[lid] = ln["slot"]
                self.index[lid] = n
                self.day.append(day)
                days_begun.add(day)
                n += 1
        self.count = n

    def name(self, i):
        if not isinstance(i, str) or not i:
            return None
        return f"{self.prefix}-{i}" if self.prefix else i


def places(script, donor: Donor) -> list[int]:
    """The donor places of the lines of a script that the donor has, in the script's order."""
    out = []
    for sc in scenes_of(script):
        for ln in lines_of(sc):
            i = donor.index.get(ln.get("id")) if isinstance(ln.get("id"), str) else None
            if i is not None:
                out.append(i)
    return out


def owned(video: dict, mine: list[int], others: list[list[int]]) -> bool:
    """Whether a movie of the donor belongs in this script: it has the first line kept after the movie,
    or, when no script has a line after it, the last line kept before it."""
    b = video["before"]
    after = [min((i for i in p if i >= b), default=None) for p in [mine] + others]
    best = min((i for i in after if i is not None), default=None)
    if best is not None:
        return after[0] == best
    before = [max((i for i in p if i < b), default=None) for p in [mine] + others]
    last = max((i for i in before if i is not None), default=None)
    return last is not None and before[0] == last


def apply(script: dict, donors: list[Donor], siblings: list[dict], out_dir: str, fade_first: bool = True, max_gap: int = MAX_GAP) -> dict:
    """Write the screen fields into `script` (changed in place). Returns the report."""
    rep = {"lines": 0, "matched": 0, "scenes": 0, "scenesMatched": 0, "fades": 0, "hidden": 0, "slots": 0, "freeSide": 0,
           "insetsOn": 0, "insetsOff": 0, "videos": [], "videosElsewhere": 0, "videosRepeated": 0, "videosLeftOut": []}
    chars = script.get("characters") if isinstance(script.get("characters"), dict) else {}
    pictured = {c for c, v in chars.items() if isinstance(v, dict) and isinstance(v.get("expressions"), dict) and v["expressions"]}
    movie_ids = {v["id"] for d in donors for v in d.videos}

    def donor_of(i):
        if isinstance(i, str):
            for d in donors:
                if i in d.index or i in d.scene_index:
                    return d
        return None

    # where each movie of a donor that is this script's goes: before a line, after a line, or at the end
    before: dict[tuple[int, int], list[dict]] = {}
    after: dict[tuple[int, int], list[dict]] = {}
    ending: list[tuple[Donor, dict]] = []
    for di, d in enumerate(donors):
        mine = places(script, d)
        if not mine:
            continue
        others = [p for p in (places(s, d) for s in siblings) if p and p != mine]
        for v in d.videos:
            if not owned(v, mine, others):
                rep["videosElsewhere"] += 1
                continue
            b = v["before"]
            nxt = min((i for i in mine if i >= b), default=None)
            if nxt is not None:
                if nxt - b <= max_gap or (v["opens"] and d.day[nxt] == v["day"]):
                    before.setdefault((di, nxt), []).append(v)
                    continue
            else:
                prev = max((i for i in mine if i < b), default=None)
                if prev is not None and b - 1 - prev <= max_gap:
                    if b >= d.count:
                        ending.append((d, v))
                    else:
                        after.setdefault((di, prev), []).append(v)
                    continue
            rep["videosLeftOut"].append({"id": v["id"], "file": os.path.basename(v["path"])})

    placed: set[str] = set()

    def video_line(d: Donor, v: dict):
        key = os.path.normcase(os.path.normpath(os.path.join(d.src_dir, v["path"])))
        if key in placed:
            rep["videosRepeated"] += 1
            return None
        placed.add(key)
        rep["videos"].append({"id": v["id"], "file": os.path.basename(v["path"])})
        return {"id": v["id"], "kind": "direction", "video": rebase(v["path"], d.src_dir, out_dir)}

    prev_donor, prev_si, first = None, None, True
    last_scene = None
    for sc in scenes_of(script):
        rep["scenes"] += 1
        last_scene = sc
        sid = sc.get("id") if isinstance(sc.get("id"), str) else None
        d = donor_of(sid)
        if d is not None:
            rep["scenesMatched"] += 1
            si = d.scene_index.get(sid)
            fade = False
            if si is not None:
                lo = prev_si + 1 if (prev_donor is d and prev_si is not None and prev_si < si) else si
                fade = any(d.fade[lo:si + 1]) or (prev_donor is not None and prev_donor is not d)
                prev_si = si
            prev_donor = d
            if fade or (first and fade_first):
                sc["transition"] = "fade"
                rep["fades"] += 1
            elif sc.get("transition") == "fade":
                del sc["transition"]
            if d.hide.get(sid):
                sc["hideCharacters"] = True
                rep["hidden"] += 1
            else:
                sc.pop("hideCharacters", None)
        elif first and fade_first:
            sc["transition"] = "fade"
            rep["fades"] += 1
        first = False

        # the lines in their final order: movies written by an earlier run are taken out, the donor's put in
        seq: list[tuple] = []  # (line, its donor or None)
        added = 0  # lines at the end of `seq` that the donor does not have
        for ln in lines_of(sc):
            lid = ln.get("id") if isinstance(ln.get("id"), str) else None
            if "video" in ln and lid in movie_ids:
                continue
            rep["lines"] += 1
            ld = d if (d is not None and lid in d.index) else donor_of(lid)
            if ld is None or lid not in ld.index:
                seq.append((ln, None))
                added += 1
                continue
            rep["matched"] += 1
            key = (donors.index(ld), ld.index[lid])
            movies = [m for m in (video_line(ld, v) for v in before.get(key, [])) if m is not None]
            if movies:
                at = len(seq) - added
                seq[at:at] = [(m, None) for m in movies]
            seq.append((ln, ld))
            added = 0
            seq.extend((m, None) for m in (video_line(ld, v) for v in after.get(key, [])) if m is not None)

        shown = None  # the inset the lines written so far leave on screen
        want = None  # the inset the donor shows
        sides: dict[int, str] = {}  # who stands at the left (1) and at the right (3)
        for ln, ld in seq:
            if "video" in ln and ln.get("id") in movie_ids:
                shown, sides = None, {}  # a movie takes the inset and the two-shot off the screen
                continue
            lid = ln.get("id") if isinstance(ln.get("id"), str) else None
            if ld is not None:
                want = ld.inset.get(lid)
                if want is not None:
                    want = (rebase(want[0], ld.src_dir, out_dir), want[1])
            for k in LINE_FIELDS:
                ln.pop(k, None)
            if want != shown:
                ln["subCg"] = want[0] if want else ""
                if want and want[1]:
                    ln["subCgAnchor"] = list(want[1])
                rep["insetsOn" if want else "insetsOff"] += 1
                shown = want
            who = ln.get("speaker")
            slot = ld.slot.get(lid) if ld is not None else None
            if ln.get("kind") != "direction" and isinstance(who, str) and who in pictured:
                taken = {s: w for s, w in sides.items() if w != who}
                if slot in (1, 3):
                    sides = {s: w for s, w in taken.items() if s != slot}
                    sides[slot] = who
                    ln["slot"] = slot
                    rep["slots"] += 1
                elif who in sides.values():
                    pass  # stays at their side
                elif len(taken) == 1:
                    free = 1 if 3 in taken else 3
                    sides = dict(taken)
                    sides[free] = who
                    ln["slot"] = free
                    rep["freeSide"] += 1
                else:
                    if slot == 2:
                        ln["slot"] = 2
                    sides = {}  # the centre ends the two-shot
        sc["lines"] = [ln for ln, _ in seq]

    if last_scene is not None:
        for d, v in ending:
            m = video_line(d, v)
            if m is not None:
                last_scene["lines"].append(m)
    return rep


def parse_donor(spec: str):
    """`k=path` -> ("k", path); `path` -> ("", path). A drive letter is not a prefix."""
    head, sep, tail = spec.partition("=")
    if sep and tail and head and len(head) <= 16 and all(c.isalnum() or c == "_" for c in head):
        return head, tail
    return "", spec


def main(argv=None) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--script", required=True, help="the episode to write the screen fields into")
    ap.add_argument("--donor", action="append", required=True, help="a conversion made with --screen: PREFIX=path or path (may be given more than once)")
    ap.add_argument("--siblings", action="append", default=[], help="the other episodes of the series (a glob; may be given more than once)")
    ap.add_argument("--out", required=True)
    ap.add_argument("--report")
    ap.add_argument("--no-fade-first", action="store_true", help="do not fade the first scene in")
    ap.add_argument("--max-gap", type=int, default=MAX_GAP, help="lines of the donor between a movie and the nearest line kept, at most (40)")
    a = ap.parse_args(argv)

    script = load(a.script)
    if script is None:
        print(f"cannot read {a.script}", file=sys.stderr)
        return 2
    donors = []
    for spec in a.donor:
        prefix, path = parse_donor(spec)
        s = load(path)
        if s is None:
            print(f"cannot read the donor {path}", file=sys.stderr)
            return 2
        donors.append(Donor(prefix, s, os.path.dirname(os.path.abspath(path))))
    siblings = []
    me = os.path.normcase(os.path.abspath(a.script))
    for pattern in a.siblings:
        for p in sorted(glob.glob(pattern))[:500]:
            if os.path.normcase(os.path.abspath(p)) == me:
                continue
            s = load(p)
            if s is not None:
                siblings.append(s)
    out_dir = os.path.dirname(os.path.abspath(a.out)) or "."
    rep = apply(script, donors, siblings, out_dir, not a.no_fade_first, max(0, min(a.max_gap, 1_000_000)))
    rep["script"], rep["out"] = a.script, a.out
    if not rep["matched"]:
        print(f"no line of {a.script} is in the donors (other ids, or another conversion): nothing written", file=sys.stderr)
        return 1
    try:
        os.makedirs(out_dir, exist_ok=True)
        tmp = a.out + ".tmp"
        with io.open(tmp, "w", encoding="utf-8", newline="\n") as f:
            json.dump(script, f, ensure_ascii=False, indent=1)
            f.write("\n")
        os.replace(tmp, a.out)
        if a.report:
            with io.open(a.report, "w", encoding="utf-8", newline="\n") as f:
                json.dump(rep, f, ensure_ascii=False, indent=1)
                f.write("\n")
    except OSError as e:
        print(f"cannot write {a.out}: {e}", file=sys.stderr)
        return 2
    print(json.dumps(rep, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
