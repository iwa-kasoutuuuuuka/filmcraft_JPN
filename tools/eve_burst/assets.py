"""Asset resolver for the converted PS2 "EVE burst error PLUS" data (personal study only).

Maps the names used inside the scenario JSON files (01_シナリオ/**/*.json) to the real converted
files under the data root.  Every lookup returns a path RELATIVE to the root (forward slashes) or
None, is case-insensitive, and never raises for a missing file or folder.

Public interface (kept stable, other tools call it):
    Assets(root)
    .background(name)        -> str | None   02_画像/背景 (then イベントCG, ハッキング画面, 日付画面, システム画面)
    .event_cg(name)          -> str | None   02_画像/イベントCG (then 背景)
    .sub_cg(name)            -> str | None   02_画像/サブCG
    .sprite(code, pose=None, outfit="a", suffix="d") -> str | None   02_画像/立ち絵/<code>_*/
    .bgm(name)               -> str | None   03_音声/BGM/eveNNN_<title>.wav
    .se(name)                -> str | None   03_音声/効果音 (then ボイス j/m/se/eve_ folders)
    .voice(name)             -> str | None   03_音声/ボイス (then その他, 効果音)
    .movie(name)             -> str | None   04_動画 (イベントムービー, 日付表示, オープニング)
    .coverage()              -> dict         scan all scenario JSONs, report unresolved assets
Extra helpers: .resolve(asset_type, name), .sprite_names(code), .sprite_transition(a, b),
    parse_sprite_name(name), Assets.format_coverage(report).
Nothing touches the disk at import time; directory listings are cached lazily per Assets object.
"""
import json
import os
import re

SCENARIO_DIR = "01_シナリオ"
BG_DIR = "02_画像/背景"
EVENT_CG_DIR = "02_画像/イベントCG"
SUB_CG_DIR = "02_画像/サブCG"
SPRITE_DIR = "02_画像/立ち絵"
HACK_DIR = "02_画像/ハッキング画面"
DATE_DIR = "02_画像/日付画面"
SYSTEM_SCREEN_DIR = "02_画像/システム画面"
BGM_DIR = "03_音声/BGM"
SE_DIR = "03_音声/効果音"
VOICE_DIR = "03_音声/ボイス"
VOICE_OTHER_DIR = "03_音声/その他/ADX形式のボイス"
MOVIE_DIRS = ("04_動画/イベントムービー", "04_動画/日付表示", "04_動画/オープニング")
FACE_ANIM_DIR = "04_動画/表情アニメ"

# Scenario asset "type" strings -> resolver method name.
TYPE_METHODS = {
    "背景": "background",
    "背景（端末画面）": "background",
    "立ち絵": "sprite",
    "サブCG": "sub_cg",
    "BGM": "bgm",
    "効果音": "se",
    "音声": "se",
    "ムービー": "movie",
    "演出ムービー": "movie",
    "ボイス": "voice",
}

_SPRITE_RE = re.compile(r"^([a-z]{3})(\d{2})([a-z])([a-z]?)$")
_EXTS = (".png", ".wav", ".mp4", ".bmp", ".adx", ".ahx", ".pbd", ".sfd")


def _norm(name):
    """Lower-case a scenario name and drop a trailing known extension."""
    s = str(name or "").strip().replace("\\", "/").rsplit("/", 1)[-1].lower()
    for e in _EXTS:
        if s.endswith(e):
            return s[: -len(e)]
    return s


def parse_sprite_name(name):
    """'kyo02ad' -> {'code':'kyo','pose':2,'outfit':'a','suffix':'d'}; None if not a sprite name."""
    m = _SPRITE_RE.match(_norm(name))
    if not m:
        return None
    return {"code": m.group(1), "pose": int(m.group(2)), "outfit": m.group(3), "suffix": m.group(4)}


class Assets:
    def __init__(self, root):
        self.root = str(root)
        self._dir_cache = {}
        self._catalog = None

    # ---- filesystem helpers -------------------------------------------------
    def _scan(self, rel, want_dirs):
        key = rel + ("\0d" if want_dirs else "\0f")
        got = self._dir_cache.get(key)
        if got is None:
            got = {}
            try:
                with os.scandir(os.path.join(self.root, *rel.split("/"))) as it:
                    for e in it:
                        if e.is_dir() == want_dirs:
                            got[e.name.lower()] = e.name
            except OSError:
                pass
            self._dir_cache[key] = got
        return got

    def _list(self, rel):
        """{lower-case name: real name} of the files in a folder (cached; {} if absent)."""
        return self._scan(rel, False)

    def _subdirs(self, rel):
        return self._scan(rel, True)

    def _find(self, rel, stem, ext):
        real = self._list(rel).get(stem + ext)
        return rel + "/" + real if real else None

    def _find_prefixed(self, rel, stem, ext):
        """File named '<stem>.ext' or '<stem>_<anything>.ext' (BGM carries the title in its name)."""
        files = self._list(rel)
        real = files.get(stem + ext)
        if real:
            return rel + "/" + real
        for low in sorted(files):
            if low.startswith(stem + "_") and low.endswith(ext):
                return rel + "/" + files[low]
        return None

    def _code_folder(self, base, code):
        """Sub-folder of `base` named '<code>_<something>' (e.g. kyo -> kyo_氷室恭子)."""
        for low, real in sorted(self._subdirs(base).items()):
            if low.split("_", 1)[0] == code:
                return base + "/" + real
        return None

    # ---- images -------------------------------------------------------------
    def background(self, name):
        s = _norm(name)
        for d in (BG_DIR, EVENT_CG_DIR, HACK_DIR, DATE_DIR, SYSTEM_SCREEN_DIR):
            r = self._find(d, s, ".png")
            if r:
                return r
        return None

    def event_cg(self, name):
        s = _norm(name)
        return self._find(EVENT_CG_DIR, s, ".png") or self._find(BG_DIR, s, ".png")

    def sub_cg(self, name):
        return self._find(SUB_CG_DIR, _norm(name), ".png")

    def sprite(self, code, pose=None, outfit="a", suffix="d"):
        """sprite('kyo00ad') or sprite('kyo', 2, 'b') -> '02_画像/立ち絵/kyo_氷室恭子/kyo02bd.png'."""
        s = _norm(code)
        if pose is not None:
            try:
                s = "%s%02d%s%s" % (s[:3], int(pose), _norm(outfit or "a"), _norm(suffix or ""))
            except (TypeError, ValueError):
                return None
        folder = self._code_folder(SPRITE_DIR, s[:3])
        return self._find(folder, s, ".png") if folder else None

    def sprite_names(self, code):
        """All sprite stems available for a 3-letter code, sorted (['kyo00ad','kyo00bd',...])."""
        folder = self._code_folder(SPRITE_DIR, _norm(code)[:3])
        if not folder:
            return []
        return sorted(n[:-4] for n in self._list(folder) if n.endswith(".png"))

    def sprite_transition(self, frm, to):
        """Face/pose change clip between two poses of one outfit: ('kyo00ad', 'kyo01ad') ->
        04_動画/表情アニメ/kyo_氷室恭子/kyo00ad01.mp4 (the clip name is <from><to pose number>)."""
        a, b = parse_sprite_name(frm), parse_sprite_name(to)
        if not a or not b:
            return None
        folder = self._code_folder(FACE_ANIM_DIR, a["code"])
        if not folder:
            return None
        stem = "%s%02d%s%s%02d" % (a["code"], a["pose"], a["outfit"], a["suffix"], b["pose"])
        return self._find(folder, stem, ".mp4")

    # ---- audio --------------------------------------------------------------
    def bgm(self, name):
        return self._find_prefixed(BGM_DIR, _norm(name), ".wav")

    def _voice_only(self, stem):
        m = re.match(r"^([a-z]+)", stem)
        folder = self._code_folder(VOICE_DIR, m.group(1)) if m else None
        r = self._find(folder, stem, ".wav") if folder else None
        return r or self._find(VOICE_OTHER_DIR, stem, ".wav")

    def _se_only(self, stem):
        for low, real in sorted(self._subdirs(SE_DIR).items()):
            r = self._find(SE_DIR + "/" + real, stem, ".wav")
            if r:
                return r
        return None

    def voice(self, name):
        """Spoken voice or onomatopoeia played from a dialogue line (.ahx originals)."""
        s = _norm(name)
        return self._voice_only(s) or self._se_only(s)

    def se(self, name):
        """Sound effect / voice played by an effect command (.adx originals)."""
        s = _norm(name)
        return self._se_only(s) or self._voice_only(s)

    # ---- movies -------------------------------------------------------------
    def movie(self, name):
        s = _norm(name)
        for d in MOVIE_DIRS:
            r = self._find(d, s, ".mp4")
            if r:
                return r
        return None

    # ---- generic ------------------------------------------------------------
    def resolve(self, asset_type, name):
        meth = TYPE_METHODS.get(asset_type)
        return getattr(self, meth)(name) if meth else None

    def scenario_files(self):
        out = []
        for dp, _dn, fn in os.walk(os.path.join(self.root, SCENARIO_DIR)):
            for f in fn:
                if f.lower().endswith(".json"):
                    out.append(os.path.join(dp, f))
        return sorted(out)

    def _catalog_names(self):
        """Lower-case base names listed in 00_目録/全ファイル一覧.tsv (what SHOULD exist)."""
        if self._catalog is None:
            names = set()
            p = os.path.join(self.root, "00_目録", "全ファイル一覧.tsv")
            try:
                with open(p, encoding="utf-8-sig") as f:
                    next(f, None)
                    for line in f:
                        rel = line.split("\t", 1)[0].replace("\\", "/")
                        base = rel.rsplit("/", 1)[-1].lower()
                        names.add(base.rsplit(".", 1)[0] if "." in base else base)
            except OSError:
                pass
            self._catalog = names
        return self._catalog

    def coverage(self):
        """Scan every scenario JSON; report assets that do not resolve.

        Returns {'scenarios': n, 'total': {type: refs}, 'distinct': {type: names},
                 'unresolved': {type: {name: {'count', 'scenarios' (<=5), 'in_catalog'}}},
                 'unresolved_refs': {type: refs}}
        'in_catalog' True means 00_目録 lists the file (so it is probably not copied yet).
        Voice names (instruction 'voices' lists) are reported under the type 'ボイス'."""
        total, seen, bad, cache = {}, {}, {}, {}
        files = self.scenario_files()

        def note(typ, name, scen):
            total[typ] = total.get(typ, 0) + 1
            seen.setdefault(typ, set()).add(name.lower())
            key = (typ, name.lower())
            if key not in cache:
                cache[key] = self.resolve(typ, name)
            if cache[key] is None:
                e = bad.setdefault(typ, {}).setdefault(name, {"count": 0, "scenarios": []})
                e["count"] += 1
                if scen not in e["scenarios"] and len(e["scenarios"]) < 5:
                    e["scenarios"].append(scen)

        for path in files:
            scen = os.path.splitext(os.path.basename(path))[0]
            try:
                with open(path, encoding="utf-8-sig") as f:
                    data = json.load(f)
            except (OSError, ValueError):
                continue
            for ins in data.get("instructions", []) if isinstance(data, dict) else []:
                if not isinstance(ins, dict):
                    continue
                a = ins.get("asset")
                if isinstance(a, dict) and a.get("name"):
                    note(str(a.get("type", "")), str(a["name"]), scen)
                for v in ins.get("voices") or []:
                    if isinstance(v, dict) and v.get("name"):
                        note("ボイス", str(v["name"]), scen)
        cat = self._catalog_names()
        for names in bad.values():
            for n, e in names.items():
                e["in_catalog"] = _norm(n) in cat
        return {
            "scenarios": len(files),
            "total": total,
            "distinct": {t: len(s) for t, s in seen.items()},
            "unresolved": bad,
            "unresolved_refs": {t: sum(e["count"] for e in n.values()) for t, n in bad.items()},
        }

    @staticmethod
    def format_coverage(rep, limit=40):
        lines = ["scenarios: %d" % rep["scenarios"]]
        for t in sorted(rep["total"]):
            miss = rep["unresolved"].get(t, {})
            lines.append("%s: %d refs, %d distinct names, %d names unresolved (%d refs)" % (
                t, rep["total"][t], rep["distinct"].get(t, 0), len(miss), rep["unresolved_refs"].get(t, 0)))
            for n, e in sorted(miss.items(), key=lambda kv: -kv[1]["count"])[:limit]:
                lines.append("    %-14s x%-4d %s  %s" % (
                    n, e["count"], "listed-in-catalog" if e["in_catalog"] else "NOT-IN-CATALOG",
                    ",".join(e["scenarios"])))
        return "\n".join(lines)


if __name__ == "__main__":
    import io
    import sys
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")
    print(Assets.format_coverage(Assets(sys.argv[1]).coverage()))
