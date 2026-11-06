#!/usr/bin/env python3
"""Read a draft episode compactly, and apply an adaptation written as a short list of edits.

An episode draft (``episode_plan.py``) is a large JSON file. Adapting it by rewriting the JSON is
slow and easy to get wrong, so the adapter reads it as text and writes only what changes:

    python adapt.py view --script out/series/eve.ep01.fcscript.json > ep01.txt
    python adapt.py apply --script out/series/eve.ep01.fcscript.json --edits ep01.edits.json \\
        --out out/adapted/eve.ep01.fcscript.json --min 24 --max 30

``view`` prints one line per script line, ``<id> <speaker> <R or -> <text>`` (R: it has a
recording), under a header per scene, and the estimated minutes. ``apply`` reads the edits:

    {
      "deleteScenes": ["m-s0003"],
      "delete": ["m-s0001-004", "m-s0001-010..m-s0002-007"],
      "rewrite": {"m-s0001-001": "もっと短い一行。"},
      "merge": [{"ids": ["m-s0001-005", "m-s0001-006", "m-s0001-007"], "text": "三行を一行に。"}],
      "insert": [{"after": "m-s0001-002", "speaker": "mar", "text": "つなぎの台詞。", "kind": "monologue"}],
      "reading": {"m-s0001-002": "かな"},
      "pause": {"m-s0001-003": 1.2}
    }

- ``delete`` takes line ids and ranges ``a..b`` (inclusive, in the draft's order, across scenes);
  a scene left without lines goes. ``deleteScenes`` deletes whole scenes.
- ``rewrite`` and ``merge`` are for **unrecorded** lines only (a recorded line can only be
  deleted): the line gets the new text and ``"generated": true``; a merge keeps the first id and
  deletes the others, which must be lines of the same speaker in the same scene.
- ``insert`` adds a new line (``before`` or ``after`` a kept line) with ``"generated": true`` and
  an id ``<scene>-gNN``; its speaker must be a character of the episode (``narrator`` for the
  narrator) and it has no recording. ``kind`` defaults to ``narration`` for the narrator and
  ``dialogue`` otherwise.
- ``reading`` sets how a line is spoken (kana); ``pause`` sets its ``pauseAfter`` (0..60 s).
- ``voiceOnly`` (line ids) plays a recorded line without its text: it becomes a direction whose
  ``sfx`` is the recording (no subtitle, nothing synthesized). For a recording whose text is not
  the line (the ending shows only the speaker's name over the voice). It wins over a delete; in a
  deleted scene only these lines are kept.
- ``speaker`` (``{id: character}``) names who really speaks an **unrecorded** line given to a
  numbered minor character (``n003`` who is in fact 二階堂); the character must be in the episode.

The result is checked with ``check_adaptation.py`` (the rules, and the length with ``--min`` /
``--max``); it is written even when the check fails, so the edits can be fixed and applied again.
Exit codes: 0 applied and checked, 1 the check failed, 2 the edits or a file are not usable.
Standard library only.
"""

from __future__ import annotations

import argparse
import copy
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from check_adaptation import check  # noqa: E402
from episode_plan import Estimator, PlanError  # noqa: E402
from eve_to_script import scenes_in_route  # noqa: E402
from merge_routes import MergeError, read_script, rebase_all  # noqa: E402

MAX_EDIT_BYTES = 8 * 1024 * 1024
MAX_TEXT = 3_900
MAX_ITEMS = 50_000
GENERIC_RE = re.compile(r"^n\d+$")


class EditError(Exception):
    pass


def view(script: dict, est: Estimator) -> str:
    out = [f"# {script.get('title', '')}"]
    chars = script.get("characters") or {}
    out.append("characters: " + ", ".join(f"{k}={v.get('name')}" for k, v in chars.items() if isinstance(v, dict)))
    total = 0.0
    for sc in scenes_in_route(script):
        secs = est.scene(sc)
        total += secs
        out.append(f"\n## {sc.get('id')}  {sc.get('when', '')}  {sc.get('title', '')}  pov={sc.get('pov', '')}  {secs / 60:.1f} min")
        for ln in sc.get("lines") or []:
            if not isinstance(ln, dict):
                continue
            kind = ln.get("kind", "dialogue")
            rec = "R" if ln.get("voiceFile") else "-"
            if kind == "direction":
                text = "（direction" + (" sfx" if ln.get("sfx") else "") + (" video" if ln.get("video") else "") + "）"
            else:
                text = str(ln.get("text", "")).replace("\n", " ")
                if kind == "monologue":
                    text = "[心] " + text
            out.append(f"{ln.get('id')} {ln.get('speaker') or '-'} {rec} {text}")
    out.append(f"\nestimated {total / 60:.1f} min")
    return "\n".join(out) + "\n"


def _str_list(v, what):
    if v is None:
        return []
    if not isinstance(v, list) or not all(isinstance(x, str) for x in v):
        raise EditError(f"{what} must be a list of ids")
    if len(v) > MAX_ITEMS:
        raise EditError(f"{what}: more than {MAX_ITEMS} items")
    return v


def _text(v, what):
    if not isinstance(v, str) or not v.strip():
        raise EditError(f"{what}: the text must be a non-empty string")
    if len(v) > MAX_TEXT:
        raise EditError(f"{what}: longer than {MAX_TEXT} characters")
    return v


def apply_edits(draft: dict, edits: dict) -> dict:
    """The adapted episode (a new dict). Raises EditError for an edit that cannot be applied."""
    if not isinstance(edits, dict):
        raise EditError("the edits must be a JSON object")
    known = {"deleteScenes", "delete", "rewrite", "merge", "insert", "reading", "pause", "voiceOnly", "speaker", "note", "notes"}
    unknown = set(edits) - known
    if unknown:
        raise EditError(f"unknown edit kinds: {sorted(unknown)}")
    script = copy.deepcopy(draft)
    scenes = scenes_in_route(script)
    order: list = []  # line ids in draft order
    where: dict = {}  # id -> (scene, line)
    for sc in scenes:
        for ln in sc.get("lines") or []:
            if isinstance(ln, dict) and isinstance(ln.get("id"), str) and ln["id"] not in where:
                where[ln["id"]] = (sc, ln)
                order.append(ln["id"])
    pos = {lid: k for k, lid in enumerate(order)}
    scene_ids = {sc.get("id") for sc in scenes}

    def need(lid, what):
        if lid not in where:
            raise EditError(f"{what}: {lid} is not a line of the draft")
        return where[lid]

    gone: set = set()
    gone_scenes = set(_str_list(edits.get("deleteScenes"), "deleteScenes"))
    for sid in gone_scenes:
        if sid not in scene_ids:
            raise EditError(f"deleteScenes: {sid} is not a scene of the draft")
    for item in _str_list(edits.get("delete"), "delete"):
        if ".." in item:
            a, _, b = item.partition("..")
            a, b = a.strip(), b.strip()
            need(a, "delete")
            need(b, "delete")
            if pos[a] > pos[b]:
                raise EditError(f"delete: range {item} runs backwards")
            gone.update(order[pos[a]: pos[b] + 1])
        else:
            need(item, "delete")
            gone.add(item)

    # a recording played without its text (the text is not a line: a name over a voice): it becomes
    # a direction whose sound is the recording; it wins over a delete, and a deleted scene that holds
    # one keeps only those
    voice_only = set(_str_list(edits.get("voiceOnly"), "voiceOnly"))
    for lid in sorted(voice_only, key=lambda x: pos.get(x, -1)):
        sc, ln = need(lid, "voiceOnly")
        if not ln.get("voiceFile"):
            raise EditError(f"voiceOnly: {lid} has no recording")
        if ln.get("kind") == "direction" or ln.get("sfx"):
            raise EditError(f"voiceOnly: {lid} is a direction or already has a sound effect")
        sid = sc.get("id")
        if sid in gone_scenes:
            gone_scenes.discard(sid)
            gone.update(x.get("id") for x in sc.get("lines") or [] if isinstance(x, dict) and x.get("id") not in voice_only)
        gone.discard(lid)
        new = {"id": lid, "kind": "direction", "sfx": ln["voiceFile"]}
        if "pauseAfter" in ln:
            new["pauseAfter"] = ln["pauseAfter"]
        ln.clear()
        ln.update(new)

    # a numbered minor character (n003) who is really someone the episode knows: for unrecorded lines
    speakers = edits.get("speaker") or {}
    if not isinstance(speakers, dict):
        raise EditError("speaker must be an object {id: character}")
    chars_all = script.get("characters") or {}
    for lid, who in speakers.items():
        _sc, ln = need(lid, "speaker")
        if ln.get("voiceFile"):
            raise EditError(f"speaker: {lid} has a recording; its speaker stays")
        if not (isinstance(ln.get("speaker"), str) and GENERIC_RE.match(ln["speaker"])):
            raise EditError(f"speaker: {lid} is spoken by {ln.get('speaker')!r}; only a numbered minor character (n001...) can be named")
        if not isinstance(who, str) or who not in chars_all:
            raise EditError(f"speaker: {who!r} is not a character of the episode")
        ln["speaker"] = who

    rewrite = edits.get("rewrite") or {}
    if not isinstance(rewrite, dict):
        raise EditError("rewrite must be an object {id: text}")
    for lid, text in rewrite.items():
        _sc, ln = need(lid, "rewrite")
        if ln.get("voiceFile"):
            raise EditError(f"rewrite: {lid} has a recording; it can only be deleted")
        if ln.get("kind") == "direction":
            raise EditError(f"rewrite: {lid} is a direction")
        if lid in gone:
            raise EditError(f"rewrite: {lid} is also deleted")
        ln["text"] = _text(text, f"rewrite {lid}")
        ln["generated"] = True

    merges = edits.get("merge") or []
    if not isinstance(merges, list):
        raise EditError("merge must be a list of {ids, text}")
    for m in merges:
        if not isinstance(m, dict):
            raise EditError("merge: each item is {ids, text}")
        ids = _str_list(m.get("ids"), "merge ids")
        if len(ids) < 2:
            raise EditError("merge: give at least two ids")
        first_sc, first = need(ids[0], "merge")
        for lid in ids:
            sc, ln = need(lid, "merge")
            if sc is not first_sc:
                raise EditError(f"merge: {lid} is in another scene than {ids[0]}")
            if ln.get("voiceFile"):
                raise EditError(f"merge: {lid} has a recording; it can only be deleted")
            if ln.get("speaker") != first.get("speaker") or ln.get("kind") == "direction":
                raise EditError(f"merge: {lid} is not a line of the same speaker as {ids[0]}")
            if lid in gone:
                raise EditError(f"merge: {lid} is also deleted")
        if sorted(ids, key=pos.get) != ids:
            raise EditError(f"merge: the ids must be in the draft's order ({ids[0]} first)")
        first["text"] = _text(m.get("text"), f"merge {ids[0]}")
        first["generated"] = True
        gone.update(ids[1:])

    readings = edits.get("reading") or {}
    if not isinstance(readings, dict):
        raise EditError("reading must be an object {id: kana}")
    for lid, r in readings.items():
        _sc, ln = need(lid, "reading")
        ln["reading"] = _text(r, f"reading {lid}")
    pauses = edits.get("pause") or {}
    if not isinstance(pauses, dict):
        raise EditError("pause must be an object {id: seconds}")
    for lid, p in pauses.items():
        _sc, ln = need(lid, "pause")
        if isinstance(p, bool) or not isinstance(p, (int, float)) or not 0 <= p <= 60:
            raise EditError(f"pause {lid}: seconds must be 0..60")
        ln["pauseAfter"] = round(float(p), 3)

    chars = script.get("characters") or {}
    inserts = edits.get("insert") or []
    if not isinstance(inserts, list) or len(inserts) > MAX_ITEMS:
        raise EditError("insert must be a list of new lines")
    counters: dict = {}
    taken = set(where)
    placed: dict = {}  # (anchor id, "before"/"after") -> [new lines]
    for k, ins in enumerate(inserts):
        if not isinstance(ins, dict):
            raise EditError(f"insert {k}: not an object")
        anchor = ins.get("after") if ins.get("after") is not None else ins.get("before")
        side = "after" if ins.get("after") is not None else "before"
        if not isinstance(anchor, str):
            raise EditError(f"insert {k}: give \"after\" or \"before\" a line id")
        sc, _ln = need(anchor, f"insert {k}")
        if anchor in gone or sc.get("id") in gone_scenes:
            raise EditError(f"insert {k}: {anchor} is deleted; anchor it to a kept line")
        speaker = ins.get("speaker")
        if not isinstance(speaker, str) or speaker not in chars:
            raise EditError(f"insert {k}: speaker {speaker!r} is not a character of the episode")
        kind = ins.get("kind") or ("narration" if speaker == "narrator" else "dialogue")
        if kind not in ("dialogue", "narration", "monologue"):
            raise EditError(f"insert {k}: kind must be dialogue, narration or monologue")
        sid = sc.get("id")
        n = counters.get(sid, 0)
        while True:
            n += 1
            nid = f"{sid}-g{n:02d}"
            if nid not in taken:
                break
        counters[sid] = n
        taken.add(nid)
        new = {"id": nid, "kind": kind, "speaker": speaker, "text": _text(ins.get("text"), f"insert {k}"), "generated": True}
        if ins.get("pauseAfter") is not None:
            p = ins["pauseAfter"]
            if isinstance(p, bool) or not isinstance(p, (int, float)) or not 0 <= p <= 60:
                raise EditError(f"insert {k}: pauseAfter must be 0..60")
            new["pauseAfter"] = round(float(p), 3)
        placed.setdefault((anchor, side), []).append(new)

    new_scenes = []
    for sc in script.get("scenes") or []:
        if not isinstance(sc, dict) or sc.get("id") in gone_scenes:
            continue
        lines = []
        for ln in sc.get("lines") or []:
            if not isinstance(ln, dict):
                continue
            lid = ln.get("id")
            lines.extend(placed.get((lid, "before"), []))
            if lid not in gone:
                lines.append(ln)
            lines.extend(placed.get((lid, "after"), []))
        if lines:
            sc["lines"] = lines
            new_scenes.append(sc)
    kept = {sc["id"] for sc in new_scenes}
    for sc in new_scenes:
        req = [r for r in sc.get("requires") or [] if r in kept]
        if req:
            sc["requires"] = req
        else:
            sc.pop("requires", None)
    script["scenes"] = new_scenes
    script["route"] = [sc["id"] for sc in new_scenes]
    return script


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    v = sub.add_parser("view", help="print the draft compactly")
    v.add_argument("--script", required=True)
    v.add_argument("--out", help="write the text here instead of printing it")
    p = sub.add_parser("apply", help="apply an edit list and check the result")
    p.add_argument("--script", required=True, help="the draft episode")
    p.add_argument("--edits", required=True, help="the edit list (JSON)")
    p.add_argument("--out", required=True, help="the adapted episode to write")
    p.add_argument("--min", type=float, default=None)
    p.add_argument("--max", type=float, default=None)
    a = ap.parse_args(argv)
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except Exception:  # noqa: BLE001
            pass
    try:
        draft = read_script(a.script)
        src_dir = os.path.dirname(os.path.abspath(a.script))
        if a.cmd == "view":
            text = view(draft, Estimator(src_dir))
            if a.out:
                with open(a.out, "w", encoding="utf-8", newline="\n") as f:
                    f.write(text)
            else:
                sys.stdout.write(text)
            return 0
        if os.path.getsize(a.edits) > MAX_EDIT_BYTES:
            raise EditError(f"{a.edits}: larger than {MAX_EDIT_BYTES // (1024 * 1024)} MB")
        with open(a.edits, "rb") as f:
            edits = json.loads(f.read().decode("utf-8-sig"))
        result = apply_edits(draft, edits)
        out_dir = os.path.dirname(os.path.abspath(a.out)) or "."
        os.makedirs(out_dir, exist_ok=True)
        if os.path.normcase(out_dir) != os.path.normcase(src_dir):
            rebase_all(result, src_dir, out_dir)
        with open(a.out, "w", encoding="utf-8", newline="\n") as f:
            json.dump(result, f, ensure_ascii=False, indent=1)
            f.write("\n")
        res = check(draft, result, src_dir, out_dir, a.min, a.max, Estimator(out_dir))
    except (MergeError, PlanError, EditError) as e:
        print(f"adapt: {e}", file=sys.stderr)
        return 2
    except (OSError, ValueError, UnicodeDecodeError) as e:
        print(f"adapt: {e}", file=sys.stderr)
        return 2
    print(json.dumps(res, ensure_ascii=False, indent=1))
    return 0 if res["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
