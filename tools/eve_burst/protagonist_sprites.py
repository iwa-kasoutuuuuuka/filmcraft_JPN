"""Give the protagonist of a script the sprites of the game, so their lines are shown (personal study).

In their own route the game never shows the player character, so a script made by eve_to_script.py
can have a 小次郎 (koj) or まりな (mar) without `expressions`, and such a speaker is not drawn. This
tool finds the protagonists (the `pov` of the script's scenes that are in --who), and for one
without expressions adds every sprite of theirs in the data (`02_画像/立ち絵/<code>_*/<code>NNxy.png`,
resolved by assets.py) under the converter's expression names (`koj00ad` -> `00ad`) and sets
`defaultExpression` (`00ad` when there is one, else the first of outfit `a`, else the first).
A character that already has expressions is left alone (`--fill-default` only adds a missing
`defaultExpression` to it). Lines keep their own `expression`; the others show the default.

    python protagonist_sprites.py --scripts "eve_out/voiced/*.fcscript.json" --root DATA --in-place
    python protagonist_sprites.py --scripts S --root DATA --out-dir DIR [--tag T] [--who koj,mar]

Image paths are written relative to the output script; the script's other relative paths are
rebased when the output folder differs. Standard library only; the data folder is only read.
"""

import argparse
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import assets as eve_assets  # noqa: E402
from extras_voices import (  # noqa: E402
    chars_of, expand_scripts, lines_of, load_all, out_path_for, rebase_paths, rel_to, scenes_of, write_json,
)

DEFAULT_WHO = ("koj", "mar")
MAX_SPRITES = 200


def pick_default(names) -> str | None:
    names = sorted(n for n in names if isinstance(n, str))
    if not names:
        return None
    if "00ad" in names:
        return "00ad"
    return next((n for n in names if len(n) >= 3 and n[2] == "a"), names[0])


def protagonists(script, who) -> list[str]:
    """The scene povs of the script that are in `who`, in `who` order."""
    povs = {sc.get("pov") for sc in scenes_of(script) if isinstance(sc.get("pov"), str)}
    return [w for w in who if w in povs]


def sprites_of(assets, code: str, out_dir: str) -> dict:
    """{expression name: {"image": path relative to out_dir}} of every sprite of `code`."""
    out = {}
    for stem in assets.sprite_names(code)[:MAX_SPRITES]:
        p = eve_assets.parse_sprite_name(stem)
        if not p or p["code"] != code:
            continue
        rel = assets.sprite(stem)
        if not rel:
            continue
        out[stem[len(code):]] = {"image": rel_to(os.path.join(assets.root, *rel.split("/")), out_dir)}
    return out


def apply(script, assets, out_dir: str, who=DEFAULT_WHO, fill_default: bool = False) -> dict:
    """Changes `script` in place; returns {added: {id: n}, kept, defaulted, noSprites, missing, shown: {id: lines}}."""
    rep = {"added": {}, "kept": [], "defaulted": [], "noSprites": [], "missing": [], "shown": {}}
    chars = chars_of(script)
    for pid in protagonists(script, who):
        c = chars.get(pid)
        if not isinstance(c, dict):
            rep["missing"].append(pid)
            continue
        exps = c.get("expressions")
        if isinstance(exps, dict) and exps:
            rep["kept"].append(pid)
            if fill_default and not (isinstance(c.get("defaultExpression"), str) and c["defaultExpression"] in exps):
                d = pick_default(exps.keys())
                if d:
                    c["defaultExpression"] = d
                    rep["defaulted"].append(pid)
        else:
            found = sprites_of(assets, pid, out_dir)
            if not found:
                rep["noSprites"].append(pid)
                continue
            c["expressions"] = found
            c["defaultExpression"] = pick_default(found.keys())
            rep["added"][pid] = len(found)
        n = sum(1 for sc in scenes_of(script) if not sc.get("hideCharacters")
                for ln in lines_of(sc) if ln.get("speaker") == pid and ln.get("kind") != "direction")
        rep["shown"][pid] = n
    return rep


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--scripts", action="append", default=[], help="script file or glob (repeatable)")
    ap.add_argument("--root", required=True, help="the extracted game folder (02_画像/立ち絵 inside)")
    ap.add_argument("--who", default=",".join(DEFAULT_WHO), help="protagonist codes (comma separated)")
    ap.add_argument("--out", help="with one script: write here")
    ap.add_argument("--out-dir", help="write the scripts into this folder")
    ap.add_argument("--tag", help="insert .<tag> before .fcscript.json in the output name")
    ap.add_argument("--in-place", action="store_true", help="overwrite the inputs")
    ap.add_argument("--fill-default", action="store_true", help="also give a missing defaultExpression to a protagonist that has expressions")
    a = ap.parse_args(argv)
    if not (a.out or a.out_dir or a.in_place):
        ap.error("pass --out, --out-dir or --in-place")
    scripts, bad = load_all(expand_scripts(a.scripts))
    for b in bad:
        print("skipped " + b, file=sys.stderr)
    if not scripts:
        print("no scripts", file=sys.stderr)
        return 2
    if a.out and len(scripts) != 1:
        print("--out needs exactly one script", file=sys.stderr)
        return 2
    who = [w.strip() for w in a.who.split(",") if w.strip()][:20]
    assets = eve_assets.Assets(a.root)
    rc = 0
    for src, s in scripts:
        dst = a.out or (src if a.in_place and not a.out_dir else out_path_for(src, a.out_dir, a.tag))
        if os.path.normcase(os.path.abspath(dst)) == os.path.normcase(os.path.abspath(src)) and not a.in_place:
            print(f"{src}: refusing to overwrite the input without --in-place", file=sys.stderr)
            rc = 2
            continue
        out_dir = os.path.dirname(os.path.abspath(dst))
        rebase_paths(s, os.path.dirname(os.path.abspath(src)), out_dir)
        rep = apply(s, assets, out_dir, who, a.fill_default)
        try:
            write_json(dst, s)
        except OSError as e:
            print(f"cannot write {dst}: {e}", file=sys.stderr)
            rc = 2
            continue
        parts = [f"{k} +{v} sprites" for k, v in rep["added"].items()] + [f"{k} kept" for k in rep["kept"]]
        parts += [f"{k} default set" for k in rep["defaulted"]] + [f"{k} NO SPRITES" for k in rep["noSprites"]]
        parts += [f"{k} missing" for k in rep["missing"]]
        shown = ", ".join(f"{k} {v} lines" for k, v in rep["shown"].items())
        print(f"{os.path.basename(dst)}: {'; '.join(parts) or 'no protagonist'}" + (f" (shown: {shown})" if shown else ""))
    return rc


if __name__ == "__main__":
    sys.exit(main())
