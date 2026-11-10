#!/usr/bin/env python3
"""Check that no line of the game's own text is in the repository (run before pushing).

The repository is public and the game's text is other people's work: tools, tests and documents
must not quote it. This reads every line the game shows (the catalog `00_目録/ボイスと台詞の対応表.tsv`
and the `text` / `raw_text` of the scenario JSON files of the dump) and looks for each of them,
whole, in the tracked files (and the untracked ones git would add), or in the tree of a commit.

    python check_no_game_text.py --root "E:/.../PS2_EVE_Bursterror解析データ"            # the working tree
    python check_no_game_text.py --root DUMP --rev HEAD [--paths a.md b.py]             # one commit
    python check_no_game_text.py --root DUMP --range origin/main..HEAD                  # every unpushed commit

Exit code 0: nothing found; 1: something found (each hit is printed as file:line and the first
characters of the line); 2: the dump is not there or cannot be read (nothing was checked). A line
counts when it is at least `--min` characters (12) of mostly kana and kanji, so everyday phrases
that the game also happens to say are not reported; whitespace, and the difference between `‥` and
`…` and between `《》` and `〈〉`, are ignored. Standard library only; the dump is only read; sizes are
capped (4 MB a file, 400 000 lines) and an unreadable file is skipped.
"""

from __future__ import annotations

import argparse
import csv
import glob
import io
import json
import os
import subprocess
import sys

CATALOG = os.path.join("00_目録", "ボイスと台詞の対応表.tsv")
SCENARIOS = os.path.join("01_シナリオ", "*", "*.json")
KEY = 8  # characters of a line used as the search key
MAX_FILE = 4 * 1024 * 1024
MAX_LINES = 400_000
MAX_DEPTH = 40
BINARY = (".png", ".jpg", ".jpeg", ".gif", ".ico", ".icns", ".ttf", ".otf", ".woff", ".woff2", ".wav", ".mp4", ".mov", ".bin", ".lock", ".pdf", ".zip")


def norm(s: str) -> str:
    return s.replace("‥", "…").replace("\u3000", "").replace(" ", "").replace("\t", "").replace("《", "〈").replace("》", "〉")


def japanese(c: str) -> bool:
    return "\u3040" <= c <= "\u30ff" or "\u4e00" <= c <= "\u9fff"


def game_lines(root: str, minimum: int) -> set[str] | None:
    """Every line the game shows, normalised; None when the dump cannot be read."""
    out: set[str] = set()

    def add(s: object) -> None:
        if not isinstance(s, str) or len(out) >= MAX_LINES:
            return
        for part in s.replace("\r", "\n").split("\n"):
            n = norm(part)
            if len(n) >= minimum and sum(japanese(c) for c in n) >= minimum - 2:
                out.add(n)

    def walk(o: object, depth: int = 0) -> None:
        if depth > MAX_DEPTH:
            return
        if isinstance(o, dict):
            for k, v in o.items():
                if k in ("text", "raw_text"):
                    add(v)
                else:
                    walk(v, depth + 1)
        elif isinstance(o, list):
            for v in o:
                walk(v, depth + 1)

    found = False
    try:
        with io.open(os.path.join(root, CATALOG), encoding="utf-8-sig", newline="") as f:
            for i, row in enumerate(csv.reader(f, delimiter="\t")):
                # a row of an effect command carries a note by the author of the catalog, not a line of the game
                if i and len(row) > 6 and row[6].startswith("台詞"):
                    add(row[5])
        found = True
    except (OSError, ValueError, csv.Error):
        pass
    for p in glob.glob(os.path.join(root, SCENARIOS)):
        try:
            if os.path.getsize(p) > 64 * 1024 * 1024:
                continue
            with io.open(p, encoding="utf-8") as f:
                walk(json.load(f))
            found = True
        except (OSError, ValueError, RecursionError):
            continue
    return out if found else None


def git(*args: str) -> bytes:
    try:
        return subprocess.run(["git", *args], capture_output=True, check=False).stdout
    except OSError:
        return b""


def names_of(rev: str | None) -> list[str]:
    if rev:
        raw = git("ls-tree", "-r", "--name-only", "-z", rev)
    else:
        raw = git("ls-files", "-z") + git("ls-files", "-o", "--exclude-standard", "-z")
    return [n for n in raw.decode("utf-8", "replace").split("\0") if n]


def text_of(name: str, rev: str | None) -> str | None:
    if name.lower().endswith(BINARY):
        return None
    try:
        if rev:
            data = git("show", f"{rev}:{name}")
        else:
            if os.path.getsize(name) > MAX_FILE:
                return None
            with open(name, "rb") as f:
                data = f.read()
        if len(data) > MAX_FILE:
            return None
        return data.decode("utf-8")
    except (OSError, UnicodeDecodeError):
        return None


def find(text: str, index: dict[str, list[str]]) -> list[tuple[int, str]]:
    hits = []
    for number, raw in enumerate(text.split("\n"), 1):
        s = norm(raw)
        seen = set()
        for i in range(len(s) - KEY + 1):
            for cand in index.get(s[i : i + KEY], ()):
                if cand not in seen and s.startswith(cand, i):
                    seen.add(cand)
                    hits.append((number, cand))
    return hits


def scan(lines: set[str], rev: str | None, paths: list[str] | None) -> dict[str, list[tuple[int, str]]]:
    index: dict[str, list[str]] = {}
    for line in lines:
        index.setdefault(line[:KEY], []).append(line)
    out = {}
    for name in paths if paths is not None else names_of(rev):
        text = text_of(name, rev)
        if text is None or not any(japanese(c) for c in text):
            continue
        hits = find(text, index)
        if hits:
            out[name] = hits
    return out


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--root", default=os.environ.get("EVE_BURST_DATA", ""), help="the dump (or EVE_BURST_DATA)")
    ap.add_argument("--rev", help="check the tree of this commit instead of the working tree")
    ap.add_argument("--range", dest="rng", help="check every commit of A..B (only the files each one changes)")
    ap.add_argument("--paths", nargs="*", help="only these files")
    ap.add_argument("--min", type=int, default=12, help="shortest line that counts (12)")
    a = ap.parse_args(argv)
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    lines = game_lines(a.root, max(a.min, KEY)) if a.root else None
    if lines is None:
        print(f"the dump was not found or cannot be read: {a.root or '(no --root)'}; nothing checked", file=sys.stderr)
        return 2
    total = 0
    if a.rng:
        revs = [r for r in git("rev-list", "--reverse", a.rng).decode().split() if r]
        for rev in revs:
            raw = git("diff-tree", "--no-commit-id", "--name-only", "-r", "-z", "--root", rev)
            changed = [n for n in raw.decode("utf-8", "replace").split("\0") if n]
            present = set(names_of(rev))
            hits = scan(lines, rev, [n for n in changed if n in present])
            for name in sorted(hits):
                for number, line in hits[name][:20]:
                    print(f"{rev[:9]} {name}:{number}: {line[:24]}…")
                total += len(hits[name])
        print(f"{len(revs)} commits of {a.rng}: {total} game lines found")
    else:
        hits = scan(lines, a.rev, a.paths)
        for name in sorted(hits):
            for number, line in hits[name][:20]:
                print(f"{name}:{number}: {line[:24]}…")
            total += len(hits[name])
        print(f"{a.rev or 'working tree'}: {total} game lines found in {len(hits)} files ({len(lines)} lines of the game known)")
    return 1 if total else 0


if __name__ == "__main__":
    sys.exit(main())
