"""Put hand-made mouth and blink variants of the sprites into a script made by eve_to_script.py.

    python sprite_variants.py --script story.fcscript.json --root "E:/filmcraft_JPN/PS2_EVE_Bursterror解析データ" \
        [--variants DIR] [--out other.json] [--overwrite] [--report FILE]
    python sprite_variants.py --script story.fcscript.json --root ... --list     # what to draw

The game has no open-mouth or blink pictures (docs/eve-burst-assets.md, "Mouth and blink variants"),
so they are drawn outside FilmCraft and saved next to each other under one naming rule. For a sprite
`kyo01bd.png` (the file name of the expression's `image`) the variants are, in `<variants>/kyo/`:

    kyo01bd_open.png    open mouth                       -> expressions.<name>.openImage
    kyo01bd_a.png ...   one open mouth per vowel a i u e o -> expressions.<name>.vowelImages
    kyo01bd_blink.png   eyes closed (mouth closed)       -> expressions.<name>.blink
    kyo01bd_closed.png  closed mouth, only for a picture whose mouth is drawn open -> expressions.<name>.image

The default `<variants>` is `<root>/../eve_sprites` (outside the dump; the dump is never written).
Only files that exist are written into the script; the rest are listed as missing. Without `_open`,
`_a` is used as the open mouth (an unknown beat then shows the widest mouth). Fields already set are
kept unless `--overwrite`. Paths are written relative to the script file (absolute on another drive).
`--list` writes nothing: one row per file the script needs (character, expression, lines, file,
whether it exists), so the drawing work can be planned.

`anime.build` places the blink picture on V4 over the sprite. A blink already given the older way, as
a separate expression `<name>_blink`, counts as set and is kept. Standard library only; the script and
the folder are treated as hostile (sizes, counts and names are capped, a missing folder is just
"nothing there").
"""

import argparse
import json
import os
import re
import sys

MAX_SCRIPT_BYTES = 64 * 1024 * 1024
MAX_CHARACTERS = 2000
MAX_EXPRESSIONS = 2000  # per character
MAX_LINES = 400_000
MAX_DIR_ENTRIES = 50_000
MAX_NAME = 200

VOWELS = ("a", "i", "u", "e", "o")
# every variant kind, in the order the list shows them; `required` = needed for a full face
KINDS = ("open", "a", "i", "u", "e", "o", "blink", "closed")
REQUIRED = ("a", "i", "u", "e", "o", "blink")  # `open` falls back to `a`, `closed` only when needed
BLINK_SUFFIX = "_blink"
SAFE_STEM = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_\-.]*$")
SPRITE_STEM = re.compile(r"^([a-z]{3})\d\d[a-z]+$")


def default_variants_dir(root: str) -> str:
    return os.path.normpath(os.path.join(os.path.abspath(root), os.pardir, "eve_sprites"))


def base_of(image) -> str | None:
    """`.../kyo01bd.png` -> `kyo01bd` (also from an already applied `kyo01bd_closed.png`)."""
    if not isinstance(image, str) or not image or len(image) > 4096:
        return None
    stem = os.path.splitext(os.path.basename(image.replace("\\", "/")))[0]
    if stem.lower().endswith("_closed"):
        stem = stem[: -len("_closed")]
    if not stem or len(stem) > MAX_NAME or not SAFE_STEM.match(stem) or ".." in stem:
        return None
    return stem


def code_of(base: str, character: str) -> str:
    m = SPRITE_STEM.match(base.lower())
    if m:
        return m.group(1)
    c = character if isinstance(character, str) else ""
    return c if SAFE_STEM.match(c or "-") and len(c) <= MAX_NAME else ""


class Folder:
    """Case-insensitive view of `<variants>/<code>/` then `<variants>/`, listed once each."""

    def __init__(self, root: str):
        self.root = root
        self.cache: dict[str, dict[str, str]] = {}
        self.listed_any = False

    def _listing(self, d: str) -> dict[str, str]:
        if d in self.cache:
            return self.cache[d]
        out: dict[str, str] = {}
        try:
            with os.scandir(d) as it:
                for n, e in enumerate(it):
                    if n >= MAX_DIR_ENTRIES:
                        break
                    try:
                        if e.is_file():
                            out.setdefault(e.name.lower(), e.path)
                    except OSError:
                        continue
            self.listed_any = True
        except (OSError, ValueError):
            pass
        self.cache[d] = out
        return out

    def find(self, code: str, filename: str) -> str | None:
        key = filename.lower()
        dirs = [os.path.join(self.root, code)] if code else []
        dirs.append(self.root)
        for d in dirs:
            p = self._listing(d).get(key)
            if p:
                return p
        return None

    def expected(self, code: str, filename: str) -> str:
        return os.path.join(self.root, code, filename) if code else os.path.join(self.root, filename)


def shown_path(full: str, script_dir: str) -> str:
    try:
        return os.path.relpath(full, script_dir).replace("\\", "/")
    except ValueError:  # another drive
        return os.path.abspath(full).replace("\\", "/")


def characters_of(script) -> dict:
    chars = script.get("characters") if isinstance(script, dict) else None
    if not isinstance(chars, dict):
        return {}
    out = {}
    for cid in sorted(k for k in chars if isinstance(k, str))[:MAX_CHARACTERS]:
        c = chars[cid]
        if isinstance(c, dict):
            out[cid] = c
    return out


def expressions_of(c: dict) -> dict:
    e = c.get("expressions")
    if not isinstance(e, dict):
        return {}
    names = sorted(k for k in e if isinstance(k, str) and not k.endswith(BLINK_SUFFIX))[:MAX_EXPRESSIONS]
    return {k: e[k] for k in names if isinstance(e[k], dict)}


def usage(script) -> dict:
    """{(character, expression): [lines shown, spoken lines]} using anime.build's expression rule."""
    chars = characters_of(script)
    scenes = script.get("scenes") if isinstance(script, dict) else None
    out: dict[tuple, list] = {}
    if not isinstance(scenes, list):
        return out
    seen = 0
    for sc in scenes:
        lines = sc.get("lines") if isinstance(sc, dict) else None
        if not isinstance(lines, list):
            continue
        for ln in lines:
            seen += 1
            if seen > MAX_LINES:
                return out
            if not isinstance(ln, dict) or ln.get("kind") == "direction":
                continue
            who = ln.get("speaker")
            c = chars.get(who) if isinstance(who, str) else None
            if c is None:
                continue
            exps = expressions_of(c)
            name = None
            for cand in (ln.get("expression"), c.get("defaultExpression"), "normal"):
                if isinstance(cand, str) and cand in exps:
                    name = cand
                    break
            if name is None and exps:
                name = next(iter(exps))
            if name is None:
                continue
            u = out.setdefault((who, name), [0, 0])
            u[0] += 1
            if isinstance(ln.get("text"), str) and ln["text"].strip():
                u[1] += 1
    return out


def plan(script, folder: Folder) -> list[dict]:
    """One entry per (character, expression) the script shows, with the files found for it."""
    use = usage(script)
    rows = []
    for cid, c in characters_of(script).items():
        for name, sp in expressions_of(c).items():
            if (cid, name) not in use:
                continue
            base = base_of(sp.get("image"))
            if base is None:
                rows.append({"character": cid, "expression": name, "base": None, "found": {}, "lines": use[(cid, name)]})
                continue
            code = code_of(base, cid)
            found = {}
            for k in KINDS:
                p = folder.find(code, f"{base}_{k}.png")
                if p:
                    found[k] = p
            rows.append({"character": cid, "expression": name, "base": base, "code": code, "found": found, "lines": use[(cid, name)]})
    return rows


def apply(script: dict, folder: Folder, script_dir: str, overwrite: bool = False) -> dict:
    """Writes what exists into `script` (in place) and returns the report."""
    report = {"written": [], "kept": [], "missing": [], "openFromA": [], "unusable": [], "counts": {}}
    chars = characters_of(script)
    rows = plan(script, folder)
    for row in rows:
        cid, name = row["character"], row["expression"]
        if row["base"] is None:
            report["unusable"].append({"character": cid, "expression": name, "why": "the image has no usable file name"})
            continue
        exps = chars[cid]["expressions"]
        sp = exps[name]
        found = row["found"]
        wrote, kept = [], []

        def put(field, value, kind, target=sp):
            if target.get(field) and not overwrite:
                kept.append(kind)
            else:
                target[field] = value
                wrote.append(kind)

        if "closed" in found:
            # replaces the picture itself; base_of() strips `_closed`, so a second run finds the same base
            closed = shown_path(found["closed"], script_dir)
            if sp.get("image") == closed:
                kept.append("closed")
            else:
                sp["image"] = closed
                wrote.append("closed")
        open_p = found.get("open") or found.get("a")
        if open_p:
            put("openImage", shown_path(open_p, script_dir), "open")
            if "open" not in found:
                report["openFromA"].append(f"{cid}/{name}")
        vi = sp.get("vowelImages")
        if not isinstance(vi, dict):
            vi = {}
        for v in VOWELS:
            if v in found:
                if vi.get(v) and not overwrite:
                    kept.append(v)
                else:
                    vi[v] = shown_path(found[v], script_dir)
                    wrote.append(v)
        if vi:
            sp["vowelImages"] = vi
        if "blink" in found:
            old = exps.get(name + BLINK_SUFFIX)  # the older way: an expression of its own
            if not overwrite and (sp.get("blink") or (isinstance(old, dict) and old.get("image"))):
                kept.append("blink")
            else:
                sp["blink"] = shown_path(found["blink"], script_dir)
                wrote.append("blink")
        missing = [k for k in REQUIRED if k not in found]  # `_open` is never required: `_a` stands in
        entry = {"character": cid, "expression": name, "base": row["base"]}
        if wrote:
            report["written"].append({**entry, "kinds": wrote})
        if kept:
            report["kept"].append({**entry, "kinds": kept})
        if missing:
            report["missing"].append({**entry, "kinds": missing, "folder": folder.expected(row["code"], "").replace("\\", "/")})
    report["counts"] = {
        "expressionsShown": len(rows),
        "complete": sum(1 for r in rows if r["base"] and all(k in r["found"] for k in REQUIRED)),
        "filesWritten": sum(len(w["kinds"]) for w in report["written"]),
        "filesMissing": sum(len(m["kinds"]) for m in report["missing"]),
    }
    return report


def listing(script, folder: Folder) -> list[list[str]]:
    """Rows: character, expression, lines shown, spoken lines, file, kind, status."""
    rows = []
    for r in plan(script, folder):
        if r["base"] is None:
            rows.append([r["character"], r["expression"], str(r["lines"][0]), str(r["lines"][1]), "-", "-", "unusable"])
            continue
        for k in KINDS:
            have = k in r["found"]
            if k in ("open", "closed") and not have:
                status = "optional"  # `_a` stands in for `_open`; `_closed` only for a mouth drawn open
            else:
                status = "have" if have else "missing"
            path = r["found"].get(k) or folder.expected(r["code"], f"{r['base']}_{k}.png")
            rows.append([r["character"], r["expression"], str(r["lines"][0]), str(r["lines"][1]), path.replace("\\", "/"), k, status])
    return rows


def read_script(path: str):
    try:
        if os.path.getsize(path) > MAX_SCRIPT_BYTES:
            return None, f"{path} is larger than {MAX_SCRIPT_BYTES} bytes"
        with open(path, encoding="utf-8") as f:
            s = json.load(f)
    except (OSError, ValueError, RecursionError) as e:
        return None, f"cannot read {path}: {e}"
    if not isinstance(s, dict):
        return None, f"{path} is not a script (not a JSON object)"
    return s, None


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--script", required=True)
    ap.add_argument("--root", help="the dump folder; the default --variants is <root>/../eve_sprites")
    ap.add_argument("--variants", help="the folder of drawn variants (<variants>/<code>/<base>_<kind>.png)")
    ap.add_argument("--out", help="write here instead of changing --script")
    ap.add_argument("--overwrite", action="store_true", help="replace openImage / vowelImages / blink already set")
    ap.add_argument("--list", action="store_true", help="print the files the script needs (TSV) and write nothing")
    ap.add_argument("--report", help="also write the JSON report here")
    a = ap.parse_args(argv)
    if not a.variants and not a.root:
        print("give --variants or --root", file=sys.stderr)
        return 2
    vdir = a.variants or default_variants_dir(a.root)
    script, err = read_script(a.script)
    if err:
        print(err, file=sys.stderr)
        return 2
    folder = Folder(vdir)
    if a.list:
        rows = listing(script, folder)
        out = sys.stdout
        out.write("character\texpression\tlines\tspoken\tfile\tkind\tstatus\n")
        for r in rows:
            out.write("\t".join(r) + "\n")
        exprs = {(r[0], r[1]) for r in rows}
        need = sum(1 for r in rows if r[6] == "missing")
        have = sum(1 for r in rows if r[6] == "have")
        print(f"{len(exprs)} expressions shown; {have} files present, {need} required files missing "
              f"({len(REQUIRED)} per expression: {' '.join('_' + k for k in REQUIRED)}; _open and _closed optional)", file=sys.stderr)
        if not folder.listed_any:
            print(f"note: {vdir} does not exist or cannot be read", file=sys.stderr)
        return 0
    dest = a.out or a.script
    rep = apply(script, folder, os.path.dirname(os.path.abspath(dest)), a.overwrite)
    rep["variantsDir"] = vdir.replace("\\", "/")
    if not folder.listed_any:
        print(f"warning: {vdir} does not exist or cannot be read; nothing to add", file=sys.stderr)
    for m in rep["missing"][:200]:
        print(f"missing: {m['character']}/{m['expression']} ({m['base']}): {' '.join(m['kinds'])}", file=sys.stderr)
    if len(rep["missing"]) > 200:
        print(f"... and {len(rep['missing']) - 200} more expressions with missing files", file=sys.stderr)
    try:
        with open(dest, "w", encoding="utf-8", newline="\n") as f:
            json.dump(script, f, ensure_ascii=False, indent=1)
        if a.report:
            with open(a.report, "w", encoding="utf-8", newline="\n") as f:
                json.dump(rep, f, ensure_ascii=False, indent=1)
    except OSError as e:
        print(f"cannot write: {e}", file=sys.stderr)
        return 2
    print(json.dumps(rep["counts"], ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
