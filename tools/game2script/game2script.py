#!/usr/bin/env python3
"""Convert an adventure game's scenario text into a FilmCraft script (docs/script.md).

Every game stores its scenario differently, so the format is described by a *profile* (JSON, see
profiles/plain.json and docs/game-import.md): regular expressions that say which lines are
dialogue, narration or thoughts, which are commands (background, music, sound effect, expression,
wait, scene breaks) and where voice files and pictures live. This script applies the profile; it
never guesses a format. Lines no rule matches are listed in the report so the profile can be
completed.

    python game2script.py --input scenario/ --assets game_files/ --out story.fcscript.json
    python game2script.py --profile my_game.json --input chapter1.txt --assets . --out out.fcscript.json

The scenario and the assets must already be in files this machine can read (text, PNG/JPG, WAV/
OGG/MP3); unpacking a game's own archives is out of scope. Standard library only.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
MAX_FILE_BYTES = 64 * 1024 * 1024
MAX_LINES = 2_000_000
REPORT_LIMIT = 200

DEFAULT_ASSETS = {
    "backgrounds": "bg",
    "sprites": "chara",
    "bgm": "bgm",
    "se": "se",
    "voice": "voice",
    "image_ext": [".png", ".webp", ".jpg", ".jpeg", ".bmp"],
    "audio_ext": [".wav", ".ogg", ".mp3", ".flac", ".m4a"],
    "sprite_pattern": r"^(?P<char>[^_.]+)_(?P<expr>[^_.]+?)(?P<kind>_open|_blink)?\.(png|webp|jpg|jpeg|bmp)$",
}


def natural_key(s: str):
    return [int(t) if t.isdigit() else t.lower() for t in re.split(r"(\d+)", s)]


def read_text(path: str, encoding: str) -> str:
    if os.path.getsize(path) > MAX_FILE_BYTES:
        raise SystemExit(f"{path}: larger than {MAX_FILE_BYTES // 1024 // 1024} MB")
    with open(path, "rb") as f:
        raw = f.read()
    if encoding != "auto":
        return raw.decode(encoding)
    for enc in ("utf-8-sig", "cp932"):
        try:
            return raw.decode(enc)
        except UnicodeDecodeError:
            continue
    return raw.decode("utf-8", "replace")


def load_profile(name_or_path: str, depth: int = 0, relative_to: str | None = None) -> dict:
    path = name_or_path
    if not os.path.isfile(path) and relative_to:
        path = os.path.join(relative_to, name_or_path)
    if not os.path.isfile(path):
        path = os.path.join(HERE, "profiles", name_or_path + ".json")
    if not os.path.isfile(path):
        raise SystemExit(f"no profile {name_or_path!r} (a file, or one of {sorted(os.listdir(os.path.join(HERE, 'profiles')))})")
    with open(path, encoding="utf-8") as f:
        p = json.load(f)
    if "extends" in p:
        # a profile can start from another and change only what differs (`characters` and `assets` merge)
        if depth >= 5:
            raise SystemExit(f"{path}: profiles extend each other too deeply")
        base = load_profile(p["extends"], depth + 1, os.path.dirname(os.path.abspath(path)))
        merged = dict(base)
        for k, v in p.items():
            if k == "extends":
                continue
            merged[k] = {**base.get(k, {}), **v} if k in ("characters", "assets", "scene") and isinstance(v, dict) else v
        p = merged
    p.setdefault("encoding", "auto")
    p.setdefault("language", "Japanese")
    p.setdefault("scene", {})
    p.setdefault("ignore", [])
    p.setdefault("commands", [])
    p.setdefault("rules", [])
    p.setdefault("ruby", [])
    p.setdefault("strip", [])
    p.setdefault("characters", {})
    p["assets"] = {**DEFAULT_ASSETS, **p.get("assets", {})}
    return p


def rx(pattern: str, where: str) -> re.Pattern:
    try:
        return re.compile(pattern)
    except re.error as e:
        raise SystemExit(f"profile: bad regular expression in {where}: {pattern!r}: {e}") from e


def slug(name: str, taken: set[str]) -> str:
    base = re.sub(r"[^A-Za-z0-9_]+", "", name).lower()
    if not base:
        n = 1
        while f"c{n}" in taken:
            n += 1
        return f"c{n}"
    cand, n = base, 2
    while cand in taken:
        cand, n = f"{base}{n}", n + 1
    return cand


class Converter:
    def __init__(self, profile: dict, assets: str, out_dir: str):
        self.p = profile
        self.assets = assets
        self.out_dir = out_dir
        a = profile["assets"]
        self.scene_start = rx(profile["scene"]["start"], "scene.start") if profile["scene"].get("start") else None
        self.ignore = [rx(x, "ignore") for x in profile["ignore"]]
        self.commands = [(rx(c["match"], "commands"), c["set"]) for c in profile["commands"]]
        self.rules = [(rx(r["match"], "rules"), r.get("kind", "dialogue")) for r in profile["rules"]]
        self.ruby = [(rx(r["match"], "ruby"), r["to"]) for r in profile["ruby"]]
        self.strip = [rx(x, "strip") for x in profile["strip"]]
        self.sprite_re = rx(a["sprite_pattern"], "assets.sprite_pattern")
        self.chars: dict[str, dict] = {}  # id -> character
        self.alias: dict[str, str] = {}  # name in the scenario -> id
        for name, c in profile["characters"].items():
            cid = c.get("id") or slug(name, set(self.chars))
            self.alias[name] = cid
            self.chars[cid] = {"name": c.get("name", name)}
            if "voice" in c:
                self.chars[cid]["voice"] = c["voice"]
        self.narrator = None
        if profile.get("narrator"):
            self.narrator = profile["narrator"]
            self.chars.setdefault(self.narrator, {"name": profile.get("narrator_name", "語り")})
            if profile.get("narrator_voice"):
                self.chars[self.narrator]["voice"] = profile["narrator_voice"]
        self.scenes: list[dict] = []
        self.cur: dict | None = None
        self.n_in_scene = 0
        self.expr: dict[str, str] = {}
        self.bgm: str | None = None
        self.bg: str | None = None
        self.part_of: str | None = None  # the scene a part continues
        self.pov: str | None = None
        self.when: str | None = None
        self.requires: list[str] = []
        self.report = {"unmatched": [], "unknownSpeakers": [], "missing": {}, "unknownExpressions": [], "files": 0, "lines": 0}

    # -- helpers -------------------------------------------------------------------------------
    def note(self, key: str, value: str):
        lst = self.report["missing"].setdefault(key, [])
        if value not in lst and len(lst) < REPORT_LIMIT:
            lst.append(value)

    def rel(self, path: str) -> str:
        return os.path.relpath(path, self.out_dir).replace("\\", "/")

    def find(self, kind: str, name: str, exts: list[str]) -> str | None:
        """A file named `name` in the assets folder for `kind` (`backgrounds`, `bgm`...)."""
        folder = os.path.join(self.assets, self.p["assets"][kind])
        cands = [name] + [name + e for e in exts]
        for c in cands:
            path = os.path.join(folder, c)
            if os.path.isfile(path):
                return self.rel(path)
        # case-insensitive, for file systems that care and scenarios that do not
        if os.path.isdir(folder):
            low = {f.lower(): f for f in os.listdir(folder)}
            for c in cands:
                if c.lower() in low:
                    return self.rel(os.path.join(folder, low[c.lower()]))
        self.note(kind, name)
        return None

    def speaker_id(self, name: str) -> str:
        name = name.strip().strip("　 ")
        if name in self.alias:
            return self.alias[name]
        if name in self.chars:  # a command may name a character by its id
            return name
        for cid, c in self.chars.items():
            if c.get("name") == name:
                self.alias[name] = cid
                return cid
        cid = slug(name, set(self.chars))
        self.alias[name] = cid
        self.chars[cid] = {"name": name}
        if len(self.report["unknownSpeakers"]) < REPORT_LIMIT:
            self.report["unknownSpeakers"].append(name)
        return cid

    def new_scene(self, sid: str | None = None, title: str = "", part: bool = False):
        """Start a scene. A `part` continues the scene before it (same title, point of view and
        story time) after a change of background or music: a script scene has one of each."""
        n = len(self.scenes) + 1
        if part and self.cur is not None:
            root = self.part_of or self.cur["id"]
            sid, k = None, 2
            while sid is None or sid in {x["id"] for x in self.scenes}:
                sid, k = f"{root}-p{k}", k + 1
            self.part_of = root
        else:
            self.part_of = None
            sid = sid or f"s{n:03d}"
            used = {x["id"] for x in self.scenes}
            base, k = sid, 2
            while sid in used:
                sid, k = f"{base}-{k}", k + 1
        sc: dict = {"id": sid, "title": title, "lines": []}
        if self.pov:
            sc["pov"] = self.pov
        if self.when:
            sc["when"] = self.when
        if self.requires and not part:
            sc["requires"] = list(self.requires)
        elif part and self.part_of:
            sc["requires"] = [self.part_of] if self.part_of != sid else []
        if self.bgm:
            sc["bgm"] = self.bgm
        if self.bg:
            sc["background"] = self.bg
        self.scenes.append(sc)
        self.cur = sc
        self.n_in_scene = 0
        return sc

    def scene(self) -> dict:
        return self.cur or self.new_scene()

    def add_line(self, line: dict):
        sc = self.scene()
        self.n_in_scene += 1
        line["id"] = f"{sc['id']}-{self.n_in_scene:03d}"
        sc["lines"].append(line)

    def clean(self, text: str) -> str:
        for r in self.strip:
            text = r.sub("", text)
        for r, to in self.ruby:
            text = r.sub(to, text)
        return text.strip()

    # -- one line ------------------------------------------------------------------------------
    def command(self, m: re.Match, kind: str):
        g = m.groupdict()
        a = self.p["assets"]
        if kind == "background":
            path = self.find("backgrounds", g.get("file", ""), a["image_ext"])
            if path:
                self.bg = path
                if self.cur is not None and self.cur["lines"] and self.cur.get("background") != path:
                    self.new_scene(title=self.cur.get("title", ""), part=True)
                elif self.cur is not None:
                    self.cur["background"] = path
        elif kind == "bgm":
            self.bgm = self.find("bgm", g.get("file", ""), a["audio_ext"])
            if self.cur is not None:
                if not self.cur["lines"]:
                    if self.bgm:
                        self.cur["bgm"] = self.bgm
                elif self.bgm and self.cur.get("bgm") != self.bgm:
                    self.new_scene(title=self.cur.get("title", ""), part=True)
        elif kind == "bgm_stop":
            self.bgm = None
        elif kind == "sfx":
            path = self.find("se", g.get("file", ""), a["audio_ext"])
            if path:
                self.add_line({"kind": "direction", "sfx": path})
        elif kind == "expression":
            sp = g.get("speaker")
            if sp:
                self.expr[self.speaker_id(sp)] = g.get("expr", "")
        elif kind == "pause":
            sc = self.cur
            if sc and sc["lines"]:
                try:
                    sc["lines"][-1]["pauseAfter"] = max(0.0, min(60.0, float(g.get("sec", "0"))))
                except ValueError:
                    pass
        elif kind in ("pov", "when", "requires"):
            # for the scenes that follow, and for the one just opened if it has no lines yet (a
            # header usually comes first and its settings after it)
            if kind == "pov":
                self.pov = self.speaker_id(g["speaker"]) if g.get("speaker") else None
                value = self.pov
            elif kind == "when":
                self.when = g.get("when")
                value = self.when
            else:
                self.requires = [x for x in re.split(r"[\s,、]+", g.get("ids", "")) if x]
                value = self.requires
            if self.cur is not None and not self.cur["lines"]:
                if value:
                    self.cur[kind] = value
                else:
                    self.cur.pop(kind, None)
        elif kind == "scene":
            self.new_scene(g.get("id"), g.get("title", ""))

    def text_line(self, raw: str):
        for r, kind in self.rules:
            m = r.match(raw)
            if not m:
                continue
            g = m.groupdict()
            text = self.clean(g.get("text") or "")
            if not text:
                return
            line: dict = {"kind": kind, "text": text}
            if kind in ("dialogue", "monologue"):
                if not g.get("speaker"):
                    continue
                cid = self.speaker_id(g["speaker"])
                line["speaker"] = cid
                if self.expr.get(cid):
                    line["expression"] = self.expr[cid]
            elif self.narrator:
                line["speaker"] = self.narrator
            if g.get("voice"):
                v = self.find("voice", g["voice"], self.p["assets"]["audio_ext"])
                if v:
                    line["voiceFile"] = v
            self.add_line(line)
            return
        if len(self.report["unmatched"]) < REPORT_LIMIT:
            self.report["unmatched"].append(raw)

    def feed(self, text: str):
        for raw in text.splitlines():
            self.report["lines"] += 1
            if self.report["lines"] > MAX_LINES:
                raise SystemExit("too many lines")
            line = raw.strip()
            if any(r.search(line) for r in self.ignore):
                continue
            if self.scene_start:
                m = self.scene_start.match(line)
                if m:
                    g = m.groupdict()
                    self.new_scene(g.get("id"), g.get("title", "").strip())
                    continue
            for r, kind in self.commands:
                m = r.match(line)
                if m:
                    self.command(m, kind)
                    break
            else:
                self.text_line(line)

    # -- the whole script ----------------------------------------------------------------------
    def sprites(self):
        folder = os.path.join(self.assets, self.p["assets"]["sprites"])
        if not os.path.isdir(folder):
            return
        by_alias = {**{c: c for c in self.chars}, **{c["name"]: i for i, c in self.chars.items()}, **self.alias}
        for f in sorted(os.listdir(folder), key=natural_key):
            m = self.sprite_re.match(f)
            if not m:
                continue
            cid = by_alias.get(m.group("char"))
            if cid is None or cid not in self.chars:
                continue
            expr, kind = m.group("expr"), (m.group("kind") or "")
            exps = self.chars[cid].setdefault("expressions", {})
            path = self.rel(os.path.join(folder, f))
            if kind == "_blink":
                exps[f"{expr}_blink"] = {"image": path}
            elif kind == "_open":
                exps.setdefault(expr, {})["openImage"] = path
            else:
                exps.setdefault(expr, {})["image"] = path
        for cid, c in self.chars.items():
            exps = c.get("expressions", {})
            for k in [k for k, v in exps.items() if "image" not in v]:
                del exps[k]  # an open mouth with no closed one is no picture
            if "normal" in exps:
                c["defaultExpression"] = "normal"
        # an expression a line asks for that has no picture
        for sc in self.scenes:
            for ln in sc["lines"]:
                e, sp = ln.get("expression"), ln.get("speaker")
                if e and sp and e not in self.chars.get(sp, {}).get("expressions", {}):
                    key = f"{sp}:{e}"
                    if key not in self.report["unknownExpressions"] and len(self.report["unknownExpressions"]) < REPORT_LIMIT:
                        self.report["unknownExpressions"].append(key)
                    del ln["expression"]

    def script(self, title: str) -> dict:
        self.sprites()
        scenes = [s for s in self.scenes if s["lines"] or s.get("background")]
        for s in scenes:
            if s.get("pov") and s["pov"] not in self.chars:
                del s["pov"]
        out = {
            "version": 1,
            "title": title or self.p.get("title", ""),
            "language": self.p["language"],
            "characters": self.chars,
            "scenes": scenes,
            "route": [s["id"] for s in scenes],
        }
        self.report["scenes"] = len(scenes)
        self.report["lineCount"] = sum(len(s["lines"]) for s in scenes)
        self.report["characters"] = sorted(self.chars)
        return out


def collect(inputs: str, patterns: list[str]) -> list[str]:
    if os.path.isfile(inputs):
        return [inputs]
    files: list[str] = []
    for pat in patterns or ["*.txt"]:
        files += glob.glob(os.path.join(inputs, pat), recursive=True)
    return sorted({f for f in files if os.path.isfile(f)}, key=natural_key)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--input", required=True, help="a scenario file, or a folder of them (read in natural order)")
    ap.add_argument("--assets", help="the game's files: folders bg, chara, bgm, se, voice (names set by the profile); default: the input folder")
    ap.add_argument("--out", required=True, help="the script to write (*.fcscript.json); paths in it are relative to this file")
    ap.add_argument("--profile", default="plain", help="a profile file, or the name of one in profiles/ (default plain)")
    ap.add_argument("--title", default="")
    ap.add_argument("--report", help="also write the report (unmatched lines, missing files...) here")
    a = ap.parse_args(argv)
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except Exception:  # noqa: BLE001
            pass

    profile = load_profile(a.profile)
    files = collect(a.input, profile.get("inputs", []))
    if not files:
        raise SystemExit(f"no scenario files in {a.input}")
    assets = a.assets or (a.input if os.path.isdir(a.input) else os.path.dirname(os.path.abspath(a.input)))
    out_dir = os.path.dirname(os.path.abspath(a.out)) or "."
    os.makedirs(out_dir, exist_ok=True)
    conv = Converter(profile, assets, out_dir)
    for f in files:
        conv.report["files"] += 1
        conv.feed(read_text(f, profile["encoding"]))
    script = conv.script(a.title)
    with open(a.out, "w", encoding="utf-8", newline="\n") as f:
        json.dump(script, f, ensure_ascii=False, indent=2)
        f.write("\n")
    rep = conv.report
    if a.report:
        with open(a.report, "w", encoding="utf-8") as f:
            json.dump(rep, f, ensure_ascii=False, indent=2)
    print(json.dumps({"out": os.path.abspath(a.out), **{k: rep[k] for k in ("files", "scenes", "lineCount", "characters")}, "unmatched": len(rep["unmatched"]), "unknownSpeakers": rep["unknownSpeakers"], "missing": {k: len(v) for k, v in rep["missing"].items()}}, ensure_ascii=False))
    if rep["unmatched"]:
        print(f"{len(rep['unmatched'])} lines matched no rule, first: {rep['unmatched'][0]!r} (see --report)", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
