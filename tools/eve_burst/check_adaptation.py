#!/usr/bin/env python3
"""Check an adapted episode against its draft (the files of ``episode_plan.py``).

The rules of the adaptation (docs/eve-burst-convert.md, "Adapting an episode"):

- a **recorded** line (``voiceFile``) may only be deleted: its text, speaker, kind and recording
  stay as they were (a ``reading`` may be added);
- an **unrecorded** line may be rewritten or several merged into one, when the story does not
  change; a rewritten line keeps its id and says ``"generated": true``;
- a **new** line has a new id, ``"generated": true`` and no recording;
- nothing is moved: the lines and scenes that are kept stay in the draft's order, and no new
  scene appears;
- the length, estimated as ``anime.build`` lays it out, is within ``--min`` .. ``--max`` minutes.

    python check_adaptation.py --source out/series/eve.ep01.fcscript.json \\
        --edited out/adapted/eve.ep01.fcscript.json --min 24 --max 30

Prints a summary as JSON (kept, deleted, rewritten and new lines, recorded lines kept, minutes,
the problems) and exits with 1 when a rule is broken, 2 when a file cannot be read. Run
``script.validate`` on the edited file as well: it checks the format. Standard library only.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from eve_to_script import scenes_in_route  # noqa: E402
from episode_plan import Estimator, PlanError, is_spoken  # noqa: E402
from merge_routes import MergeError, read_script  # noqa: E402

REPORT_LIMIT = 200
GENERIC_RE = re.compile(r"^n\d+$")  # a numbered minor character, who may be named on unrecorded lines
FIXED = ("speaker", "kind")  # never change on a kept line
RECORDED_FIXED = ("text", "speaker", "kind", "voiceFile")


def norm_path(p, base: str):
    if not isinstance(p, str) or not p:
        return None
    full = p if os.path.isabs(p) else os.path.join(base, p)
    return os.path.normcase(os.path.normpath(full))


def check(source: dict, edited: dict, src_dir: str, ed_dir: str, min_minutes=None, max_minutes=None,
          est: Estimator | None = None) -> dict:
    errors: list = []
    warnings: list = []

    def err(msg):
        if len(errors) < REPORT_LIMIT:
            errors.append(msg)

    src_scenes = scenes_in_route(source)
    ed_scenes = scenes_in_route(edited)
    scene_pos = {sc.get("id"): k for k, sc in enumerate(src_scenes)}
    src_lines: dict = {}
    order = 0
    for sc in src_scenes:
        for ln in sc.get("lines") or []:
            if isinstance(ln, dict) and isinstance(ln.get("id"), str):
                src_lines.setdefault(ln["id"], (order, sc.get("id"), ln))
                order += 1
    last_scene = -1
    last_line = -1
    seen_ids: set = set()
    kept = rewritten = new = recorded_kept = voice_only = renamed = 0
    for sc in ed_scenes:
        sid = sc.get("id")
        if sid not in scene_pos:
            err(f"scene {sid}: not in the draft (no new scenes)")
            continue
        if scene_pos[sid] < last_scene:
            err(f"scene {sid}: moved before a scene that comes earlier in the draft")
        last_scene = max(last_scene, scene_pos[sid])
        for ln in sc.get("lines") or []:
            if not isinstance(ln, dict):
                err(f"scene {sid}: a line that is not an object")
                continue
            lid = ln.get("id")
            if lid in seen_ids:
                err(f"line {lid}: appears twice")
            seen_ids.add(lid)
            if lid not in src_lines:
                new += 1
                if ln.get("generated") is not True:
                    err(f"line {lid}: new, but not marked \"generated\": true")
                if ln.get("voiceFile"):
                    err(f"line {lid}: new, but has a recording (a recording belongs to its own line)")
                continue
            pos, src_sid, src = src_lines[lid]
            if src_sid != sid:
                err(f"line {lid}: moved from scene {src_sid} to {sid}")
            if pos < last_line:
                err(f"line {lid}: moved before a line that comes earlier in the draft")
            last_line = max(last_line, pos)
            if src.get("voiceFile"):
                recorded_kept += 1
                if (ln.get("kind") == "direction" and not ln.get("text") and ln.get("sfx")
                        and norm_path(ln.get("sfx"), ed_dir) == norm_path(src.get("voiceFile"), src_dir)):
                    voice_only += 1  # the recording played without its text (adapt.py voiceOnly)
                    kept += 1
                    continue
                for k in RECORDED_FIXED:
                    a, b = src.get(k), ln.get(k)
                    if k == "voiceFile":
                        a, b = norm_path(a, src_dir), norm_path(b, ed_dir)
                    if a != b:
                        err(f"line {lid}: recorded, so its {k} must stay as it was")
                kept += 1
                continue
            for k in FIXED:
                if src.get(k) != ln.get(k):
                    named = (k == "speaker" and isinstance(src.get(k), str) and GENERIC_RE.match(src[k])
                             and ln.get(k) in (edited.get("characters") or {}))
                    if named:
                        renamed += 1  # a numbered minor character named (adapt.py speaker)
                    else:
                        err(f"line {lid}: its {k} changed ({src.get(k)!r} -> {ln.get(k)!r})")
            if ln.get("voiceFile"):
                err(f"line {lid}: had no recording and now has one")
            if src.get("text") != ln.get("text"):
                rewritten += 1
                if ln.get("generated") is not True:
                    err(f"line {lid}: rewritten, but not marked \"generated\": true")
            else:
                kept += 1
    deleted = len(src_lines) - kept - rewritten
    est = est or Estimator(ed_dir)
    src_est = Estimator(src_dir, est.cps, est.pause, est.scene_gap, est.direction)
    minutes = sum(est.scene(sc) for sc in ed_scenes) / 60.0
    draft_minutes = sum(src_est.scene(sc) for sc in src_scenes) / 60.0
    if min_minutes is not None and minutes < min_minutes:
        err(f"about {minutes:.1f} minutes: shorter than {min_minutes}")
    if max_minutes is not None and minutes > max_minutes:
        err(f"about {minutes:.1f} minutes: longer than {max_minutes}")
    spoken_src = sum(1 for _p, _s, ln in src_lines.values() if is_spoken(ln))
    if spoken_src and not any(is_spoken(ln) for sc in ed_scenes for ln in sc.get("lines") or [] if isinstance(ln, dict)):
        warnings.append("nothing is spoken any more")
    recorded_src = sum(1 for _p, _s, ln in src_lines.values() if ln.get("voiceFile"))
    return {
        "ok": not errors,
        "errors": errors,
        "warnings": warnings,
        "draftLines": len(src_lines),
        "kept": kept,
        "rewritten": rewritten,
        "voiceOnly": voice_only,
        "speakersNamed": renamed,
        "new": new,
        "deleted": deleted,
        "recordedKept": recorded_kept,
        "recordedDeleted": recorded_src - recorded_kept,
        "scenes": len(ed_scenes),
        "scenesDeleted": len(src_scenes) - len({sc.get("id") for sc in ed_scenes} & set(scene_pos)),
        "draftMinutes": round(draft_minutes, 1),
        "minutes": round(minutes, 1),
    }


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--source", required=True, help="the draft episode (episode_plan.py)")
    ap.add_argument("--edited", required=True, help="the adapted episode")
    ap.add_argument("--min", type=float, default=None, help="shortest allowed, minutes")
    ap.add_argument("--max", type=float, default=None, help="longest allowed, minutes")
    ap.add_argument("--cps", type=float, default=7.0)
    ap.add_argument("--pause", type=float, default=0.35)
    ap.add_argument("--scene-gap", type=float, default=0.6)
    a = ap.parse_args(argv)
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except Exception:  # noqa: BLE001
            pass
    try:
        src = read_script(a.source)
        ed = read_script(a.edited)
        ed_dir = os.path.dirname(os.path.abspath(a.edited))
        est = Estimator(ed_dir, a.cps, a.pause, a.scene_gap)
        res = check(src, ed, os.path.dirname(os.path.abspath(a.source)), ed_dir, a.min, a.max, est)
    except (MergeError, PlanError) as e:
        print(f"check_adaptation: {e}", file=sys.stderr)
        return 2
    print(json.dumps(res, ensure_ascii=False, indent=1))
    return 0 if res["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
