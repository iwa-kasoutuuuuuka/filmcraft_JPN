#!/usr/bin/env python3
"""Turn the decoded scenarios of the PS2 game "EVE burst error PLUS" into FilmCraft scripts.

The game is a branching adventure (free roaming between places, command menus, flags), so a
linear video needs a *playthrough*. This tool contains a small interpreter of the scenario
instruction set (flags, conditional branches, jumps, command-select menus, scenario moves, the
ending) that follows ONE path and writes what that path shows and says as a FilmCraft script
(docs/script.md): characters, scenes, lines with their voice files, sprite expressions, sound
effects, music and pauses. Everything it could not interpret is listed in the report.

    python eve_to_script.py --root "E:/filmcraft_JPN/PS2_EVE_Bursterror解析データ" --route kojirou \\
        --out out/kojirou.fcscript.json --report out/kojirou.report.json

The input is the decoded dump (01_シナリオ/*/*.json, see its このフォルダについて.txt); nothing in it is
ever written to. Pictures and sounds are found through ``assets.py`` (class ``Assets``) beside this
file; without it every file name resolves to nothing and the script has no pictures or audio.
The path policy, the options and what is lost are described in docs/eve-burst-convert.md.
Standard library only.
"""

from __future__ import annotations

import argparse
import collections
import copy
import importlib.util
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))

# -- limits (every number in the input is hostile; nothing here may hang) ------------------------
DEFAULT_MAX_STEPS = 3_000_000  # instructions executed in the real run
DEFAULT_MAX_LINES = 60_000  # text lines emitted
DEFAULT_MAX_MOVES = 4_000  # scenario moves
DEFAULT_LOOP_VISITS = 4_000  # times one instruction may run before the path is cut
MAX_LOOK_DEPTH = 4  # submenus deep a look-ahead goes
DRY_STEPS = 6_000  # instructions a look-ahead may run to classify one menu entry
MAX_FILE_BYTES = 64 * 1024 * 1024
REPORT_LIMIT = 200
MAX_TEXT = 3_900  # the script format allows 4000 characters per line

OPS_NO_EFFECT = {
    # empty or "reads its arguments and does nothing" instructions (per the dump's own names)
    0x8009, 0x800A, 0x801E, 0x8028, 0x802D, 0x802E, 0x8034,
    0x801F, 0x8022, 0x8025, 0x8029, 0x802B, 0x8031, 0x8032, 0x8033, 0x803A, 0x8045,
}
OPS_COSMETIC = {
    # known, but nothing in a script can express them
    0x8013, 0x8018,  # commit the pending picture
    0x8016, 0x8017,  # sprite fading off / on
    0x800E, 0x800F, 0x8010,  # fade in / out to black / white
    0x804C,  # position on the map screen
    0x804D,  # CHANGE blinking flag
    0x800C,  # stop voice
    0x8021,  # clear the message window
}
FINALE = ('ab01', 'c02')  # the scenarios after the two sides join
# what counts as something new on the path: not the control flow that finds it
NOVEL_OPS = {0x8035, 0x8043, 0x8007, 0x8008, 0x8046, 0x8020, 0x800B, 0x8039, 0x8047, 0x8036, 0x800D}
OPS_ASSETLESS = {0x8008, 0x8046}  # movies: no place in a script (listed in the report)
DATE_OPS = {0x8014, 0x801A} | OPS_ASSETLESS  # where a date screen or a day movie can be
ROAD_NOVEL_OPS = {0x8035, 0x8043, 0x8036, 0x8008, 0x8046, 0x800D}  # what a walk is for (and flag sets)
ROAD_EDGES = 6_000  # menu choices one road search may try
ROAD_LENGTH = 40  # menu choices one road may take

# -- story time ------------------------------------------------------------------------------------
# The game shows the day as a date screen (a background ``1203md`` from 02_画像/日付画面) or a day
# movie (``1203`` from 04_動画/日付表示). Both are December; the two digits are the day.
DATE_SCREEN_RE = re.compile(r"^12(\d{2})md", re.I)
DATE_MOVIE_RE = re.compile(r"^12(\d{2})$")
WHEN_RE = re.compile(r"^D(\d{1,4})-")
MAX_SEQ = 9999  # scenes of one day that get their own number; later ones share the last (still in order)
ROUTE_LABELS = {"kojirou": "小次郎編", "marina": "まりな編"}
MAX_EPISODES = 10_000

# -- characters ------------------------------------------------------------------------------------
# Ids are the game's own 3-letter codes (キャラクターコード表.tsv). pul (voice) and pur (sprite) are
# the same person.
CODE_ALIASES = {"pul": "pur"}
CHAR_NAMES = {
    "koj": "小次郎", "mar": "まりな", "kyo": "恭子", "yay": "弥生", "may": "真弥子", "pur": "プリン",
    "aka": "茜", "nik": "二階堂", "hon": "本部長", "mid": "御堂", "mat": "松乃", "kat": "鈴木",
    "kag": "香川", "sir": "シリア", "aqu": "アクア", "gul": "グレン", "dep": "ディーブ", "kou": "孔",
    "uwe": "ウエイトレス",
}
# speaker names the table pins to one person (their voice prefix agrees: see the code table)
NAME_MAP = {
    "小次郎": "koj", "まりな": "mar", "恭子": "kyo", "制服の女の子": "kyo", "弥生": "yay", "弥生の声": "yay",
    "真弥子": "may", "真弥子の声": "may", "プリン": "pur", "プリシア": "pur", "プリシアの声": "pur",
    "茜": "aka", "二階堂": "nik", "本部長": "hon", "本部長の声": "hon", "御堂": "mid", "御堂の声": "mid",
    "松乃": "mat", "鈴木": "kat", "桂木": "kat", "香川": "kag", "シリア": "sir", "シリアの声": "sir",
    "アクア": "aqu", "グレン": "gul", "ディーブ": "dep", "偽者の孔": "dep", "ウエイトレス": "uwe",
}
PLAIN_NAMES = {n for n in NAME_MAP if not n.endswith("の声") and n != "制服の女の子"}
VOICE_CODE_RE = re.compile(r"^([a-z]{3})\d{3,}")
SFX_VOICE_RE = re.compile(r"^(?:j\d|m\d|se\d|eve_)")
SIDE_CHAR = {1: "koj", 2: "mar"}

# menu entries that are the console's own (save / load / clearing the system flags): never chosen
SYSTEM_ITEM_RE = re.compile(r"^(?:セーブ|ロード|Ｓｙｓｔｅｍ|システム|オールクリア)")

ROUTES = {
    # name -> (scenario, label or None for "the first instruction")
    "kojirou": ("a01", 0),  # a01's label 0 is "start as Kojirou": clears the flags, then the intro
    "marina": ("b01", None),
}


def natural_key(s: str):
    return [int(t) if t.isdigit() else t.lower() for t in re.split(r"(\d+)", s)]


def normalize_scenario(name: str) -> str:
    """The game reads "a020-2" as a020_2."""
    return name.strip().replace("-", "_")


# -- assets ----------------------------------------------------------------------------------------
class StubAssets:
    """What the converter uses when assets.py is not there: every file resolves to nothing."""

    available = False

    def background(self, name):
        return None

    def sub_cg(self, name):
        return None

    def sprite(self, *a, **k):
        return None

    def bgm(self, name):
        return None

    def se(self, name):
        return None

    def voice(self, name):
        return None

    def movie(self, name):
        return None


def load_assets(root: str):
    """``Assets(root)`` from assets.py beside this file, imported lazily; the stub if it is not
    there or does not load."""
    path = os.path.join(HERE, "assets.py")
    if os.path.isfile(path):
        try:
            spec = importlib.util.spec_from_file_location("eve_burst_assets", path)
            mod = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(mod)
            a = mod.Assets(root)
            a.available = True
            return a
        except Exception as e:  # noqa: BLE001 - any failure falls back to the stub
            print(f"warning: assets.py did not load ({e}); no pictures or audio will be resolved", file=sys.stderr)
    return StubAssets()


# -- scenarios -------------------------------------------------------------------------------------
class Scenario:
    """One decoded scenario, indexed for the interpreter."""

    def __init__(self, name: str, data: dict):
        self.name = name
        raw = data.get("instructions")
        if not isinstance(raw, list):
            raise ValueError(f"{name}: no instruction list")
        self.raw = raw
        n = len(raw)
        self.n = n
        self.ops = [0] * n
        self.by_off: dict[int, int] = {}
        self.labels: dict[int, int] = {}
        self.targets: list[int | None] = [None] * n  # 8003 / 8005 jump target (instruction index)
        self.menus: dict[int, list[dict]] = {}
        for k, d in enumerate(raw):
            try:
                self.ops[k] = int(d["op"], 16)
                self.by_off[int(d["off"], 16)] = k
            except (KeyError, ValueError, TypeError) as e:
                raise ValueError(f"{name}: instruction {k} has no usable op/off ({e})") from e
        for k, d in enumerate(raw):
            op = self.ops[k]
            if op == 0x8001:
                lab = d.get("label")
                if isinstance(lab, int) and lab not in self.labels:
                    self.labels[lab] = k
            elif op in (0x8003, 0x8005):
                self.targets[k] = self._off(d.get("target"))
            elif op == 0x8002:
                items = []
                for it in d.get("items") or []:
                    conds = []
                    for c in it.get("conditions") or []:
                        if isinstance(c, (list, tuple)) and len(c) == 2:
                            conds.append((int(c[0]), int(c[1])))
                    items.append({"text": str(it.get("text", "")), "conds": conds, "tgt": self._off(it.get("target"))})
                self.menus[k] = items

    def _off(self, v):
        try:
            return self.by_off.get(int(v, 16)) if isinstance(v, str) else self.by_off.get(int(v))
        except (ValueError, TypeError):
            return None


class ScenarioStore:
    def __init__(self, folder: str):
        self.folder = folder
        self.files: dict[str, str] = {}
        if os.path.isdir(folder):
            for dp, _dn, fn in os.walk(folder):
                for f in fn:
                    if f.lower().endswith(".json"):
                        self.files.setdefault(f[:-5], os.path.join(dp, f))
        self.cache: dict[str, Scenario | None] = {}
        self.errors: list[str] = []

    def get(self, name: str) -> Scenario | None:
        name = normalize_scenario(name)
        if name in self.cache:
            return self.cache[name]
        path = self.files.get(name)
        sc = None
        if path:
            try:
                if os.path.getsize(path) > MAX_FILE_BYTES:
                    raise ValueError("file too large")
                with open(path, "rb") as f:
                    data = json.loads(f.read().decode("utf-8-sig"))
                sc = Scenario(name, data)
            except (OSError, ValueError, json.JSONDecodeError, UnicodeDecodeError) as e:
                self.errors.append(f"{name}: {e}")
        self.cache[name] = sc
        return sc


# -- the interpreter ---------------------------------------------------------------------------------
class Frame:
    """An open command-select menu."""

    __slots__ = ("scn", "pc", "cur_key", "child")

    def __init__(self, scn: Scenario, pc: int):
        self.scn = scn
        self.pc = pc
        self.cur_key = None  # key of the entry chosen last
        self.child = False  # a submenu was opened from that entry

    def copy(self):
        f = Frame(self.scn, self.pc)
        f.cur_key = self.cur_key
        f.child = self.child
        return f


def sooner(a: int, b: int) -> int:
    """The sooner of two later days, 0 meaning none."""
    return b if not a else (a if not b else min(a, b))


def marker_day(op: int, d: dict) -> int | None:
    """The story day a date screen (a background ``1204md``) or a day movie (``1204``) shows, or
    None for any other instruction."""
    if op in (0x8014, 0x801A):
        rx = DATE_SCREEN_RE
    elif op in OPS_ASSETLESS:
        rx = DATE_MOVIE_RE
    else:
        return None
    a = d.get("asset")
    name = a.get("name") if isinstance(a, dict) else None
    m = rx.match(name) if isinstance(name, str) else None
    if not m:
        return None
    day = int(m.group(1))
    return day if 1 <= day <= 31 else None


POLICIES = ("days", "explore", "order")


class Pause(Exception):
    """The side played alongside (the "days" policy) has come to the end of the day the main
    side is on, or is stuck until the main side changes a shared flag."""


class Options:
    def __init__(self, **kw):
        self.policy = "days"  # or "explore", "order"
        self.choices: list = []
        self.choices_only = False
        self.max_steps = DEFAULT_MAX_STEPS
        self.max_lines = DEFAULT_MAX_LINES
        self.max_moves = DEFAULT_MAX_MOVES
        self.loop_visits = DEFAULT_LOOP_VISITS
        self.max_repeat = 40  # times one menu entry may be chosen
        self.on_stuck = "teleport"  # or "stop"
        self.max_teleports = 300
        self.zapping = True  # days: play the other side alongside (they share flags F0-F255)
        for k, v in kw.items():
            if not hasattr(self, k):
                raise TypeError(f"unknown option {k}")
            setattr(self, k, v)


class Interp:
    """Follows one path through the game's scenarios and records events (see `Builder`).

    Menus are where the paths fork. The policy ("explore"): at each menu, look ahead at every
    entry that can be chosen and keep those that would show something not shown yet on this path
    (an instruction that has not run); take them in menu order, those that come back to the
    menu before those that move on; when no entry has anything new, back out of the menu (the
    cancel button) and, from the outermost menu, end the path (or teleport).

    The "days" policy (the default) keeps the story days in order on top of that: an entry that
    shows a later day comes last and one that shows a day again comes after everything, from a
    stuck outermost menu the path walks to the nearest new thing (`find_road`) before it
    teleports, and the other side is played alongside with the same flags (`sync_partner`).
    """

    def __init__(self, store: ScenarioStore, opts: Options):
        self.store = store
        self.o = opts
        self.flags: dict = {}
        self.flag_ver = 0
        self.side = 1
        self.scn: Scenario | None = None
        self.pc = 0
        self.stack: list[Frame] = []
        self.resume = False
        self.takes: collections.Counter = collections.Counter()
        self.events: list | None = []
        self.steps = 0
        self.lines = 0
        self.moves = 0
        self.visits: collections.Counter = collections.Counter()  # (scenario, instruction) -> times run
        self.choice_pos = 0
        self.teleports: list = []
        self.tele_fail: dict = {}
        # look-ahead state (a fork running with dry_base set)
        self.dry_base: int | None = None
        self.newcov: set = set()  # instructions the look-ahead ran that the real path has not
        self.left = False  # the look-ahead has left the menu it started from
        self.newfresh = False  # ...and arrived at a menu that has entries not chosen yet
        self.dry_picked: set = set()
        self.depth = 0  # how many submenus deep the look-ahead is
        self.nested_new = 0  # what the entries of submenus inside the entry would show
        self.nested_returns = False
        # story time (the "days" policy): the latest day shown on the path, and the later day a
        # look-ahead ran into itself (day_jump) or found inside a submenu or at the place it
        # moved to (nested_day, the soonest), 0 for none
        self.day = 0
        self.day_jump = 0
        self.day_back = False
        self.nested_day = 0
        # walking (the "days" policy): a road search step stops at the next menu (probe); the
        # road found is a list of (scenario, menu, entry or "cancel") the real path follows
        self.probe = False
        self.road: collections.deque = collections.deque()
        self.road_later = 0  # the day a road to a later day goes to, 0 for this day
        self.road_fail: dict = {}
        self.walks: list = []
        # zapping (the "days" policy): the other side, played alongside without writing anything
        # (partner), and for that side the main one (main)
        self.partner: Interp | None = None
        self.main: Interp | None = None
        self.done = False
        self.paused = 0
        self.cancelled = False
        # report
        self.unknown_ops: collections.Counter = collections.Counter()
        self.unknown_examples: dict = {}
        self.cosmetic: collections.Counter = collections.Counter()
        self.trace: list = []  # scenario visits
        self.menu_log: list = []
        self.notes: list[str] = []
        self.end_reason = ""
        self.hit_unreadable: set = set()
        self.start_name = ""

    # -- state helpers ---------------------------------------------------------------------------
    def fkey(self, n: int):
        return n if n < 256 else (self.side, n)

    def get_flag(self, n: int) -> int:
        return self.flags.get(self.fkey(n), 0)

    def set_flag(self, n: int, v: int):
        k = self.fkey(n)
        if self.flags.get(k, 0) != v:
            self.flag_ver += 1
        if v == 0:
            self.flags.pop(k, None)
        else:
            self.flags[k] = v

    def clear_side_flags(self):
        sides = (1, 2) if self.side == 3 else (self.side,)
        drop = [k for k in self.flags if isinstance(k, tuple) and k[0] in sides]
        if drop:
            self.flag_ver += 1
        for k in drop:
            del self.flags[k]

    def conds_ok(self, conds) -> bool:
        return all(self.get_flag(f) == v for f, v in conds)

    def emit(self, ev):
        if self.events is not None:
            self.events.append(ev)

    def fork(self, light: bool = False) -> "Interp":
        """A copy to look ahead with. `light` shares the entry counts instead of copying them
        (a road search step never reads or changes them)."""
        c = Interp.__new__(Interp)
        c.store, c.o = self.store, self.o
        c.flags = dict(self.flags)
        c.flag_ver, c.side = self.flag_ver, self.side
        c.scn, c.pc = self.scn, self.pc
        c.stack = [f.copy() for f in self.stack]
        c.resume = self.resume
        c.takes = self.takes if light else collections.Counter(self.takes)
        c.events = None
        c.steps = 0
        c.lines = self.lines
        c.moves = self.moves
        c.visits = self.visits  # shared, read only in a look-ahead
        c.choice_pos = len(self.o.choices)  # a look-ahead never consumes the explicit choices
        c.dry_base = None
        c.newcov, c.left, c.newfresh, c.dry_picked = set(), False, False, set()
        c.depth, c.nested_new, c.nested_returns = 0, 0, False
        c.day, c.day_jump, c.day_back, c.nested_day = self.day, 0, False, 0
        c.probe, c.road, c.road_later, c.road_fail, c.walks = False, collections.deque(), 0, {}, []
        c.partner, c.main, c.done, c.paused, c.cancelled = None, None, False, 0, False
        c.unknown_ops, c.unknown_examples, c.cosmetic = collections.Counter(), {}, collections.Counter()
        c.trace, c.menu_log, c.notes = [], [], []
        c.end_reason = ""
        c.hit_unreadable = set()
        c.start_name = self.start_name
        c.teleports, c.tele_fail = [], {}
        return c

    # -- entering scenarios --------------------------------------------------------------------------
    def enter(self, name: str, label: int | None = None) -> bool:
        sc = self.store.get(name)
        if sc is None:
            self.end_reason = f"scenario {name!r} is not available"
            if name not in self.hit_unreadable:
                self.hit_unreadable.add(name)
                self.notes.append(self.end_reason)
            return False
        pc = 0
        if label is not None:
            if label not in sc.labels:
                self.end_reason = f"{sc.name} has no label {label}"
                return False
            pc = sc.labels[label]
        self.scn, self.pc = sc, pc
        self.stack = []
        self.resume = False
        if self.events is not None:
            self.trace.append(sc.name)
            self.emit(("scn", sc.name))
        return True

    # -- running -------------------------------------------------------------------------------------
    def run(self, scenario: str, label: int | None = None) -> str:
        """The real run. Returns why it ended."""
        self.start_name = scenario
        if scenario[:1].lower() == "b":
            self.side = 2
        if not self.enter(scenario, label):
            return self.end_reason
        try:
            r = self.loop()
        except (ValueError, TypeError, KeyError, IndexError, AttributeError, RecursionError) as e:
            # a scenario file with a malformed instruction ends the path, it does not end the program
            r = f"malformed scenario data at {self.scn.name if self.scn else '?'} instruction {self.pc}: {type(e).__name__}: {e}"
        self.end_reason = r
        return r

    def loop(self) -> str:
        """Execute until the path ends (returns a reason) or, in a look-ahead (`dry_base` set),
        until the entry has come back to its menu ("returns") or has gone elsewhere and reached a
        menu there, or ended ("leaves")."""
        o = self.o
        dry = self.dry_base is not None
        max_steps = DRY_STEPS if dry else o.max_steps
        while True:
            sc = self.scn
            pc = self.pc
            if pc < 0 or pc >= sc.n:
                return "leaves" if dry else f"ran off the end of {sc.name}"
            self.steps += 1
            if self.steps > max_steps:
                return "leaves" if dry else f"step cap ({max_steps}) reached in {sc.name}"
            vk = (sc.name, pc)
            if dry:
                if vk not in self.visits and sc.ops[pc] in NOVEL_OPS:
                    self.newcov.add(vk)
            else:
                v = self.visits[vk] + 1
                self.visits[vk] = v
                if v > o.loop_visits:
                    return f"loop: {sc.name} instruction {sc.raw[pc].get('off')} ran {v} times"
            op = sc.ops[pc]
            d = sc.raw[pc]
            nxt = pc + 1
            if op in DATE_OPS:
                day = marker_day(op, d)
                if day is not None and day > self.day:
                    if dry and o.policy == "days":
                        # a later day: what comes after it is not this day's, so the look-ahead
                        # ends here and the entry is taken only when the day has nothing left
                        self.day_jump = day
                        return "leaves"
                    self.day = day
                elif day is not None and dry and o.policy == "days" and self.day and sc.name not in FINALE:
                    # this day's or an earlier day's movie again: the way back in time (the
                    # ending's look back at every day is not)
                    self.day_back = True
                    return "leaves"

            if op == 0x8035:  # message + voice
                text = d.get("text")
                if isinstance(text, str):
                    if not dry:
                        self.lines += 1
                        if self.lines > o.max_lines:
                            return f"line cap ({o.max_lines}) reached"
                        voices = []
                        for v in d.get("voices") or []:
                            if isinstance(v, dict) and isinstance(v.get("name"), str):
                                voices.append(v["name"])
                        self.emit(("line", str(d.get("speaker") or ""), text, voices, str(d.get("control_suffix") or "")))
            elif op == 0x8007:  # set flag
                try:
                    self.set_flag(int(d["flag"]), int(d["value"]))
                except (KeyError, TypeError, ValueError):
                    self.note_unknown(op, d, "bad flag instruction")
            elif op == 0x8003:  # conditional branch
                conds = []
                for c in d.get("conditions") or []:
                    if isinstance(c, (list, tuple)) and len(c) == 2:
                        conds.append((int(c[0]), int(c[1])))
                if not self.conds_ok(conds):
                    t = sc.targets[pc]
                    if t is None:
                        return "leaves" if dry else f"{sc.name}: branch at {d.get('off')} has no target"
                    nxt = t
            elif op == 0x8004:  # end of a branch body: skip the chain of branches that follows
                j, guard = pc + 1, 0
                while j < sc.n and sc.ops[j] == 0x8003 and guard < 1000:
                    t = sc.targets[j]
                    if t is None:
                        break
                    j, guard = t, guard + 1
                nxt = j
            elif op == 0x8005:  # jump, dropping the return address
                t = sc.targets[pc]
                if t is None:
                    return "leaves" if dry else f"{sc.name}: jump at {d.get('off')} has no target"
                if dry:
                    self.left = True
                self.leave()
                nxt = t
            elif op == 0x8002:  # command-select menu
                r = self.do_menu(pc)
                if r is not None:
                    return r
                continue
            elif op == 0x8006:  # return to the calling menu
                if not self.stack:
                    return "leaves" if dry else f"{sc.name}: return with no menu open at {d.get('off')}"
                self.resume = True
                top = self.stack[-1]
                self.scn, nxt = top.scn, top.pc
                top.child = False
            elif op == 0x8014 or op == 0x801A:  # background
                a = d.get("asset")
                if isinstance(a, dict) and a.get("name"):
                    self.emit(("bg", str(a["name"])))
                else:
                    self.note_unknown(op, d, "background without a name")
            elif op == 0x8026:  # BGM
                a = d.get("asset")
                if isinstance(a, dict) and a.get("name"):
                    self.emit(("bgm", str(a["name"])))
                else:
                    self.note_unknown(op, d, "BGM without a name")
            elif op in (0x8023, 0x8024, 0x8027):  # BGM stop
                self.emit(("bgm", None))
            elif op == 0x8048:  # sprite shown
                a = d.get("asset")
                if isinstance(a, dict) and a.get("name"):
                    self.emit(("sprite_show", str(a["name"]), d.get("slot")))
            elif op == 0x804B:  # sprite swapped (expression change)
                a = d.get("asset")
                if isinstance(a, dict) and a.get("name"):
                    self.emit(("sprite_swap", str(a["name"])))
            elif op == 0x8049:  # all sprites gone
                self.emit(("sprite_clear",))
            elif op == 0x8012:  # clear background, sprites and sub CG
                self.emit(("clear",))
            elif op in (0x800B, 0x8039, 0x8047):  # sound effect / voice
                a = d.get("asset")
                if isinstance(a, dict) and a.get("name"):
                    self.emit(("sfx", str(a["name"]), str(a.get("type") or "")))
                else:
                    self.note_unknown(op, d, "sound without a name")
            elif op == 0x802F:  # wait, frames
                fr = d.get("frames")
                if isinstance(fr, int) and fr > 0:
                    self.emit(("wait", fr))
            elif op == 0x8036:  # sub CG (small window)
                a = d.get("asset")
                if isinstance(a, dict) and a.get("name"):
                    self.emit(("subcg", str(a["name"])))
            elif op in OPS_ASSETLESS:  # movies
                a = d.get("asset")
                self.emit(("movie", str(a.get("name", "")) if isinstance(a, dict) else "", op))
            elif op == 0x8041:  # terminal screen
                a = d.get("asset")
                self.emit(("term_bg", str(a["name"]) if isinstance(a, dict) and a.get("name") else "hack_k"))
            elif op == 0x8042:
                self.emit(("term_clear",))
            elif op == 0x8043:  # terminal text
                t = d.get("text")
                if isinstance(t, str):
                    self.emit(("term", t))
            elif op == 0x8020:  # scenario move
                target = d.get("scenario")
                if not isinstance(target, str) or not target:
                    if dry:
                        return "leaves"
                    self.note_unknown(op, d, "scenario move without a target")
                    return "scenario move without a target"
                if dry:
                    self.left = True
                    if self.store.get(target) is None:
                        return "leaves"
                    self.enter(target)
                    continue
                self.moves += 1
                if self.moves > o.max_moves:
                    return f"scenario-move cap ({o.max_moves}) reached"
                self.leave()
                if not self.enter(target):
                    return self.end_reason
                continue
            elif op == 0x802A:  # which protagonist
                a = d.get("args") or [1]
                if a and a[0] in (1, 2, 3):
                    self.side = a[0]
                    self.emit(("side", self.side))
            elif op == 0x8011:  # clear the protagonist's flags
                self.clear_side_flags()
            elif op == 0x8001:  # label
                pass
            elif op == 0x8000:  # end of a code block: the scenario has nothing more to say
                if dry:
                    return "leaves"
                why = f"{sc.name}: reached the end of the code at {d.get('off')}"
                if o.on_stuck == "teleport" and self.teleport(sc.name):
                    continue
                if self.stuck():
                    continue  # the other side changed a shared flag: try again
                return why
            elif op == 0x800D:  # game clear
                return "leaves" if dry else "game clear (the ending was reached)"
            elif op in OPS_NO_EFFECT:
                pass
            elif op in OPS_COSMETIC:
                if not dry:
                    self.cosmetic[op] += 1
            else:
                self.note_unknown(op, d, "unknown instruction")
            self.pc = nxt

    def note_unknown(self, op, d, why):
        if self.events is None:
            return
        key = f"0x{op:04x} {why}"
        self.unknown_ops[key] += 1
        if key not in self.unknown_examples:
            self.unknown_examples[key] = {"scenario": self.scn.name if self.scn else None, "off": d.get("off"), "name": d.get("name")}

    def leave(self):
        """The path jumps or moves on: the open menus are forgotten."""
        self.stack = []
        self.resume = False

    # -- menus ---------------------------------------------------------------------------------------
    def do_menu(self, pc: int):
        sc = self.scn
        dry = self.dry_base is not None
        items = sc.menus.get(pc) or []
        if self.resume and self.stack and self.stack[-1].scn is sc and self.stack[-1].pc == pc:
            frame = self.stack[-1]
            self.resume = False
            if dry and len(self.stack) == self.dry_base:
                return "returns"
        else:
            self.resume = False
            frame = Frame(sc, pc)
            if self.stack:
                self.stack[-1].child = True
            self.stack.append(frame)
        if self.probe:
            return "menu"  # a step of the road search: arrived at the next menu
        avail = [(i, it) for i, it in enumerate(items) if it["tgt"] is not None and self.conds_ok(it["conds"])]
        if dry:
            # a submenu inside the entry being looked at, or a menu somewhere else the entry
            # arrived at: look at each of its entries on its own
            if self.depth >= MAX_LOOK_DEPTH:
                return "leaves" if self.left else "returns"
            total, any_ret = 0, False
            for i, it in avail:
                if SYSTEM_ITEM_RE.match(it["text"]) or self.takes[(sc.name, pc, i)] >= self.o.max_repeat:
                    continue
                cls, new, _direct, jump, later, back = self.look(frame, i, it, self.depth + 1)
                if jump:
                    self.nested_day = sooner(self.nested_day, jump)  # a later day: not counted as this day's
                    continue
                if back:
                    continue  # back in time: not what this entry is for
                total += new
                self.nested_day = sooner(self.nested_day, later)
                any_ret = any_ret or cls == "returns"
            self.nested_new += total
            if self.left:
                return "leaves"
            self.nested_returns = self.nested_returns or any_ret
            return "returns"
        self.cancelled = False
        pick = self.choose(frame, avail, dry)
        if isinstance(pick, str):
            return pick  # the path ends
        if (pick is None and self.o.policy == "days" and len(self.stack) == 1 and not self.cancelled
                and self.choice_pos >= len(self.o.choices) and not self.road_failed()):
            # nothing new here: walk to the nearest place that has something (menu choices a
            # player could make), instead of teleporting
            pick = self.walk(frame, avail)
        if pick is None:
            # nothing left to try here: back out (the cancel button), to the menu above
            self.stack.pop()
            if not self.stack:
                if dry:
                    return "leaves"
                why = f"menu exhausted in {sc.name} at {sc.raw[pc].get('off')}: nothing new to choose (supply --choices to steer)"
                if self.o.on_stuck == "teleport" and self.teleport(sc.name):
                    return None
                if self.stuck():
                    self.scn, self.pc = sc, pc  # the other side changed a shared flag: this menu again
                    return None
                return why
            parent = self.stack[-1]
            parent.child = False
            self.scn, self.pc = parent.scn, parent.pc
            self.resume = True
            return None
        i, it = pick
        key = (sc.name, pc, i)
        frame.cur_key = key
        frame.child = False
        self.takes[key] += 1
        self.scn, self.pc = sc, it["tgt"]
        return None

    def teleport(self, here: str) -> bool:
        """The path is stuck: nothing at this place has anything new. Move to the first scenario
        (in number order, on the same side of the game) where looking ahead shows something new,
        as if the map had a road there. The scenario starts as it always does, deciding what to
        show from the flags."""
        if len(self.teleports) >= self.o.max_teleports:
            self.notes.append(f"teleport cap ({self.o.max_teleports}) reached")
            return False
        first = self.start_name[:1].lower()
        names = sorted((n for n in self.store.files if n[:1].lower() == first and n not in FINALE), key=natural_key)
        names += [n for n in FINALE if n in self.store.files]  # where both sides meet, last
        days = self.o.policy == "days"
        for _ in range(4):
            r = self.teleport_once(here, names, days)
            if r is not True and r is not False:
                # only a later day has something new: the other side plays up to the end of
                # this day first, which may open something on this day
                if self.before_later_day(False, r[2]):
                    continue
                self.go_teleport(here, r[0], r[1], True)
                return True
            return r
        return False

    def teleport_once(self, here: str, names, days: bool):
        """One search of `teleport`: True when it moved, False when no place has anything new,
        or (name, new) for the first place whose only new things are on a later day."""
        later_day = None  # days: the first place whose only new things are on a later day
        back_day = None  # days: the first place that goes back in time
        for name in names:
            if self.tele_fail.get(name) == self.flag_ver:
                continue
            sc = self.store.get(name)
            if sc is None:
                continue
            c = self.fork()
            c.stack, c.resume = [], False
            c.dry_base, c.left = 0, True
            if not c.enter(name):
                continue
            c.loop()
            new = len(c.newcov) + c.nested_new
            if days and c.day_back:
                # a place that shows a day's movie again: only when nothing else has anything
                if back_day is None and new > 0:
                    back_day = (name, new)
                continue
            if days and (c.day_jump or (new <= 0 and c.nested_day)):
                # a place that moves the story to a later day: only when no place has anything
                # new on this day
                if later_day is None:
                    later_day = (name, new, c.day_jump or c.nested_day)
                continue
            if new <= 0:
                self.tele_fail[name] = self.flag_ver
                continue
            self.go_teleport(here, name, new)
            return True
        if later_day is not None:
            return later_day
        if back_day is not None:
            self.go_teleport(here, back_day[0], back_day[1])
            return True
        return False

    def go_teleport(self, here: str, name: str, new: int, later_day: bool = False) -> None:
        t = {"from": here, "to": name, "new": new, "afterScenarios": len(self.trace)}
        if later_day:
            t["laterDay"] = True
        self.teleports.append(t)
        self.moves += 1
        self.leave()
        self.enter(name)

    # -- walking (the "days" policy) ----------------------------------------------------------------
    def state_key(self):
        # where the path is, not its flags: the flags only come from the shortest way there (with
        # them every look at a place would be a new state, and the search would not end)
        return (self.scn.name, self.pc, self.resume, self.side, tuple((f.scn.name, f.pc) for f in self.stack))

    def road_failed(self) -> bool:
        return self.road_fail.get((self.scn.name, self.pc)) == self.flag_ver

    def found_new(self) -> bool:
        """Did this road search step run something that has not run on the path: a line, a
        picture, a movie, the ending, a flag set (not cleared)? Moves, sounds and the control
        flow that finds them do not count."""
        for name, pc in self.newcov:
            sc = self.store.get(name)
            if sc is None or not 0 <= pc < sc.n:
                continue
            op = sc.ops[pc]
            if op in ROAD_NOVEL_OPS:
                return True
            if op == 0x8007 and sc.raw[pc].get("value") not in (0, None):
                return True
        return False

    def find_road(self):
        """The path is stuck at its outermost menu. Search the menu choices a player could make
        from here (breadth first: the fewest choices first, backing out of submenus included)
        for the nearest one that shows something new, and return the choices that lead there.
        A road that shows a later day is used only when the whole reachable part of the game
        has been searched and nothing on this day is left. None when there is no road (or the
        search is over its bounds); the place is then remembered until a flag changes."""
        start = self.fork(light=True)
        start.probe, start.dry_base = True, -1
        queue = collections.deque([(start, [])])
        seen = {start.state_key()}
        later, complete, edges = None, True, 0
        while queue and complete:
            node, path = queue.popleft()
            top = node.stack[-1]
            sc = top.scn
            steps = [
                (i, it) for i, it in enumerate(sc.menus.get(top.pc) or [])
                if it["tgt"] is not None and node.conds_ok(it["conds"]) and not SYSTEM_ITEM_RE.match(it["text"])
            ]
            if len(node.stack) > 1:
                steps.append(("cancel", None))
            for i, it in steps:
                if edges >= ROAD_EDGES or len(path) >= ROAD_LENGTH:
                    complete = False
                    break
                edges += 1
                c = node.fork(light=True)
                c.probe, c.dry_base = True, -1
                if i == "cancel":
                    c.stack.pop()
                    parent = c.stack[-1]
                    parent.child = False
                    c.scn, c.pc, c.resume = parent.scn, parent.pc, True
                else:
                    f = c.stack[-1]
                    f.cur_key, f.child = (sc.name, top.pc, i), False
                    c.scn, c.pc, c.resume = sc, it["tgt"], False
                r = c.loop()
                p = path + [(sc.name, top.pc, i)]
                if c.day_jump:
                    if later is None:
                        later = (p, c.day_jump)
                    continue
                if c.day_back:
                    continue  # a road back in time is no road
                if c.found_new():
                    self.road_later = 0
                    return p
                if r != "menu":
                    continue
                k = c.state_key()
                if k not in seen:
                    seen.add(k)
                    queue.append((c, p))
        if later is not None and complete:
            self.road_later = later[1]
            return later[0]
        self.road_fail[(self.scn.name, self.pc)] = self.flag_ver
        return None

    def follow_road(self, frame: Frame, avail, shown):
        """The next step of the road being walked: (index, item), None (back out of the submenu)
        or False when the road does not fit the menu that came up (it is dropped)."""
        step = self.road.popleft()
        if step[0] == frame.scn.name and step[1] == frame.pc:
            if step[2] == "cancel":
                if len(self.stack) > 1:
                    self.log_menu(frame, shown, None, "walk")
                    return None
            else:
                for i, it in avail:
                    if i == step[2]:
                        self.log_menu(frame, shown, (i, it), "walk")
                        return (i, it)
        self.road.clear()
        return False

    def walk(self, frame: Frame, avail):
        """Nothing new at the outermost menu (the "days" policy): find a road and take its first
        step, or None. A road to a later day waits until the other side has played this day."""
        road = None
        for _ in range(4):
            road = None if self.road_failed() else self.find_road()
            if road and not self.road_later:
                break
            if road and self.before_later_day(True, self.road_later):
                continue  # shared flags changed: look again
            if not road and self.main is None and self.sync_partner():
                continue
            break
        if not road:
            return None
        self.road = collections.deque(road)
        # the other side may have changed what the menu shows
        items = frame.scn.menus.get(frame.pc) or []
        avail = [(i, it) for i, it in enumerate(items) if it["tgt"] is not None and self.conds_ok(it["conds"])]
        if len(self.walks) < 100_000:
            self.walks.append({"from": frame.scn.name, "steps": len(road), "toDay": self.road_later or self.day, "afterScenarios": len(self.trace)})
        return self.follow_road(frame, avail, [it for _, it in avail]) or None

    # -- zapping (the "days" policy) ---------------------------------------------------------------
    # The two sides share the flags F0-F255, and each needs some the other sets (Kojirou's school
    # on 4 December waits for F14 from Marina's b014): the game is played by switching sides.
    # The main side (the one written) plays the other side alongside, silently: before it moves
    # on to a later day, and when it is stuck, the other side plays until it would go past the
    # main side's day or is stuck itself.
    def before_later_day(self, at_menu: bool, day: int = 0) -> bool:
        """The path is about to move on to a later day (`day`, 0 when not known: the next).
        On the main side: play the other side up to the end of this day first; True when that
        changed a shared flag (look again). On the side played alongside: stop here unless the
        main side is already on that day."""
        if self.main is not None:
            if (day or self.day + 1) > self.main.day:
                self.pause(at_menu)
            return False
        return self.sync_partner()

    def stuck(self) -> bool:
        """The path would end here, stuck. True when it should look again: the main side has
        played the other side and a shared flag changed. The side played alongside stops here
        instead (the main side may change a flag it waits for)."""
        if self.main is not None:
            self.pause(False)
        return self.sync_partner()

    def pause(self, at_menu: bool):
        # at a menu the open frame is kept, so the next run of this instruction comes back to it
        self.resume = at_menu and bool(self.stack)
        self.paused += 1
        raise Pause()

    def sync_partner(self) -> bool:
        """Play the other side until it is at the end of the main side's day (or stuck, or at
        its end). True when it changed a flag."""
        o = self.o
        if o.policy != "days" or not o.zapping or self.main is not None or self.dry_base is not None:
            return False
        p = self.partner
        if p is None:
            o2 = copy.copy(o)
            o2.choices, o2.choices_only = [], False
            p = Interp(self.store, o2)
            p.events, p.main, p.flags = None, self, self.flags  # the same flags: F0-F255 are shared
            name, label = ROUTES["kojirou" if self.start_name[:1].lower() == "b" else "marina"]
            p.start_name = name
            if name[:1].lower() == "b":
                p.side = 2
            self.partner = p
            if not p.enter(name, label):
                p.done = True
                return False
        if p.done or p.day > self.day:
            return False
        before = p.flag_ver = p.flag_ver + 1  # the main side may have changed shared flags
        try:
            p.end_reason = p.loop()
            p.done = True
        except Pause:
            pass
        except (ValueError, TypeError, KeyError, IndexError, AttributeError, RecursionError) as e:
            p.end_reason = f"malformed scenario data at {p.scn.name if p.scn else '?'} instruction {p.pc}: {type(e).__name__}: {e}"
            p.done = True
        if p.flag_ver == before:
            return False
        self.flag_ver += 1
        return True

    def partner_report(self):
        p = self.partner
        if p is None:
            return None
        return {
            "start": p.start_name, "day": p.day, "lines": p.lines, "steps": p.steps, "pauses": p.paused,
            "teleports": len(p.teleports), "walks": len(p.walks), "endedBecause": p.end_reason if p.done else None,
        }

    def item_class(self, frame: Frame, i: int, it: dict, depth: int = 0):
        """Look ahead: choosing this entry, does the path come back to the menu ("returns") or go
        elsewhere ("leaves"), and how much of what it runs has not run on this path before?"""
        cls, new, _direct, _jump, _later, _back = self.look(frame, i, it, depth)
        return cls, new

    def look(self, frame: Frame, i: int, it: dict, depth: int = 0):
        """`item_class` and more: (class, new, direct, jump, later, back). `direct` is what is
        new on the way to the next menu (not inside submenus or at menus the entry arrives at),
        `jump` whether the entry itself shows a later day (the "days" policy stops looking
        there), `later` whether a later day is somewhere inside it, `back` whether the entry
        itself shows this day's or an earlier day's movie again (it stops there too)."""
        c = self.fork()
        c.dry_base = len(c.stack)
        key = (frame.scn.name, frame.pc, i)
        c.stack[-1].cur_key = key
        c.stack[-1].child = False
        c.takes[key] += 1
        c.dry_picked.add(key)
        c.scn, c.pc = frame.scn, it["tgt"]
        c.resume = False
        c.depth = depth
        r = c.loop()
        cls = "returns" if (r == "returns" or c.nested_returns) else "leaves"
        return cls, len(c.newcov) + c.nested_new, len(c.newcov), c.day_jump, c.nested_day, c.day_back

    def choose(self, frame: Frame, avail, dry: bool):
        """Pick an entry of the open menu: (index, item), None for "nothing left, back out", or a
        string (the path ends)."""
        o = self.o
        sc = frame.scn
        shown = [it for _, it in avail]
        if not dry and self.road:
            pick = self.follow_road(frame, avail, shown)
            if pick is not False:
                return pick
        if not dry and self.choice_pos < len(o.choices):
            want = o.choices[self.choice_pos]
            self.choice_pos += 1
            pick = self.explicit(want, avail)
            self.log_menu(frame, shown, pick, "explicit")
            if pick == "auto":
                pass
            elif pick == "cancel":
                self.cancelled = True  # the user's own way out: no walking on
                return None
            elif pick is not None:
                return pick
            else:
                self.notes.append(f"choice {want!r} matches nothing in {sc.name} {sc.raw[frame.pc].get('off')}: {[t['text'] for t in shown]}; using the policy")
        elif not dry and o.choices_only and o.choices:
            return "choices exhausted (--choices-only)"
        usable = [
            (i, it) for i, it in avail
            if not SYSTEM_ITEM_RE.match(it["text"]) and self.takes[(sc.name, frame.pc, i)] < o.max_repeat
        ]
        if dry:
            # inside a look-ahead every entry is tried once, in menu order
            for i, it in usable:
                k = (sc.name, frame.pc, i)
                if k not in self.dry_picked:
                    self.dry_picked.add(k)
                    return (i, it)
            return None
        if not usable:
            self.log_menu(frame, shown, None, "exhausted")
            return None
        if o.policy == "order":
            fresh = [p for p in usable if self.takes[(sc.name, frame.pc, p[0])] == 0]
            if not fresh:
                self.log_menu(frame, shown, None, "exhausted")
                return None
            self.log_menu(frame, shown, fresh[0], "order")
            return fresh[0]
        if o.policy == "days":
            return self.choose_days(frame, shown, usable)
        cands = []
        for i, it in usable:
            cls, new = self.item_class(frame, i, it)
            if new > 0:
                lv = cls == "leaves"
                # first the entries that come back (new ones before repeats), in menu order; then
                # those that move on, the one with the most that is new first
                cands.append((lv, self.takes[(sc.name, frame.pc, i)] > 0, -new if lv else 0, i, it))
        if not cands:
            self.log_menu(frame, shown, None, "exhausted")
            return None
        cands.sort(key=lambda c: c[:4])
        _, again, _, i, it = cands[0]
        self.log_menu(frame, shown, (i, it), "again" if again else "new")
        return (i, it)

    def choose_days(self, frame: Frame, shown, usable):
        """The "days" policy: like "explore", but the story days come in order. An entry that
        shows a later day (a day movie or date screen), itself or inside a submenu or at the
        place it moves to, is taken only when nothing else has anything new; the look-ahead does
        not count what comes after that day change. Of the entries that move on, the ones with
        something new on the way come first (the one with the most), so two places that each
        lead to the other are not run back and forth for what lies beyond both."""
        for _ in range(4):
            pick = self.rank_days(frame, usable)
            if pick is not None and pick[3] and self.before_later_day(True, pick[3]):
                continue  # the other side changed a shared flag: look again
            break
        if pick is None:
            self.log_menu(frame, shown, None, "exhausted")
            return None
        i, it, again, later_day = pick
        self.log_menu(frame, shown, (i, it), "later day" if later_day else ("again" if again else "new"))
        return (i, it)

    def rank_days(self, frame: Frame, usable):
        sc = frame.scn
        cands = []
        for i, it in usable:
            cls, new, direct, jump, later, back = self.look(frame, i, it)
            again = self.takes[(sc.name, frame.pc, i)] > 0
            lv = cls == "leaves"
            if back:
                if new <= 0:
                    continue
                # back in time (a day's movie again): only when nothing else is left
                key = (2, lv, again, 0, 0, i)
            elif new > 0 and not jump:
                # this day: first the entries that come back (new ones before repeats), in menu
                # order; then those that move on, something new on the way first
                key = (0, lv, again, -direct if lv else 0, -new if lv else 0, i)
            elif jump or later:
                # a later day: the entry that shows it itself first, then menu order
                key = (1, not jump, lv, again, 0, i)
            else:
                continue
            # the later day it goes to (0: this day, or back in time)
            cands.append((key, i, it, again, (jump or later) if key[0] == 1 else 0))
        if not cands:
            return None
        cands.sort(key=lambda c: c[0])
        return cands[0][1:]

    def explicit(self, want, avail):
        if want is None or want == "auto":
            return "auto"
        if want == "cancel":
            return "cancel"
        if isinstance(want, bool):
            return None
        if isinstance(want, int):
            return avail[want - 1] if 1 <= want <= len(avail) else None
        if isinstance(want, str):
            for i, it in avail:
                if it["text"] == want:
                    return (i, it)
            for i, it in avail:
                if want in it["text"]:
                    return (i, it)
        return None

    def log_menu(self, frame, shown, pick, how):
        if len(self.menu_log) < 5000:
            self.menu_log.append({
                "scenario": frame.scn.name,
                "off": frame.scn.raw[frame.pc].get("off"),
                "shown": [t["text"] for t in shown],
                "picked": None if (pick is None or isinstance(pick, str)) else pick[1]["text"],
                "how": how,
            })


# -- from events to a script --------------------------------------------------------------------------
def clean_text(t: str) -> str:
    t = t.replace("《", "〈").replace("》", "〉").replace("｜", "|")  # not Aozora ruby
    t = re.sub(r"[\x00-\x08\x0b\x0c\x0e-\x1f]", "", t)
    t = t.replace("\r", "").replace("\n", " ").strip()
    if len(t) > MAX_TEXT:
        t = t[:MAX_TEXT]
    return t


def parse_terminal(t: str):
    """Terminal text: ``\\n`` is a line break, ``\\q`` clears the screen, ``\\c1`` / ``\\w1`` are
    colour and wait codes. Returns a list of ("clear",) / ("text", str)."""
    out = []
    parts = re.split(r"(\\q)", t)
    for p in parts:
        if p == "\\q":
            out.append(("clear",))
            continue
        for ln in p.split("\\n"):
            ln = re.sub(r"\\[cw]\d+", "", ln).replace("\\", "").strip()
            if ln:
                out.append(("text", ln))
    return out


class Builder:
    def __init__(self, assets, root: str, out_dir: str, fps: float, terminal: str, subcg: str, title: str, movies: str = "skip"):
        self.assets = assets
        self.root = root
        self.out_dir = out_dir
        self.fps = fps
        self.terminal = terminal
        self.subcg = subcg
        self.movies_mode = movies
        self.title = title
        self.chars: dict[str, dict] = {}
        self.name_votes: dict[str, collections.Counter] = collections.defaultdict(collections.Counter)
        self.generic: dict[str, str] = {}
        self.scenes: list[dict] = []
        self.cur: dict | None = None
        self.n_in_scene = 0
        self.bg_name = None
        self.bg_kind = "bg"
        self.bgm_name = None
        self.sprites: dict = {}
        self.side = 1
        self.scenario = ""
        self.pending_sfx: list[str] = []
        self.last_line: dict | None = None
        self.path_cache: dict = {}
        self.missing: dict[str, list] = {}
        self.counts = collections.Counter()
        self.movies: list = []
        self.sprite_unmatched = 0
        self.needs_pov = set()
        self.last_bg = None
        # story time: the day (0 until the first date is shown) and the scenes of that day so far
        self.day = 0
        self.day_seq = 0
        self.markers: list = []
        self.n_markers = 0
        # where the two sides join (ab01 / c02): its first scene requires the last scene before it
        self.last_before_finale: str | None = None
        self.finale_first: str | None = None

    # -- asset paths ---------------------------------------------------------------------------------
    def note_missing(self, kind: str, name: str):
        lst = self.missing.setdefault(kind, [])
        if name not in lst and len(lst) < REPORT_LIMIT:
            lst.append(name)

    def script_path(self, p):
        if not p:
            return None
        p = str(p)
        full = p if os.path.isabs(p) else os.path.join(self.root, p)
        try:
            rel = os.path.relpath(full, self.out_dir)
        except ValueError:  # another drive: an absolute path is all there is
            rel = full
        return rel.replace("\\", "/")

    def resolve(self, kind: str, name: str, *fns):
        ck = (kind, name)
        if ck in self.path_cache:
            return self.path_cache[ck]
        p = None
        for fn in fns:
            f = getattr(self.assets, fn, None)
            if f is None:
                continue
            try:
                p = f(name)
            except Exception:  # noqa: BLE001 - a broken lookup is a missing file
                p = None
            if p:
                break
        out = self.script_path(p)
        if not out:
            self.note_missing(kind, name)
        self.path_cache[ck] = out
        return out

    def sprite_path(self, name: str):
        ck = ("sprite", name)
        if ck in self.path_cache:
            return self.path_cache[ck]
        p = None
        f = getattr(self.assets, "sprite", None)
        if f is not None:
            try:
                p = f(name)
            except Exception:  # noqa: BLE001
                p = None
        out = self.script_path(p)
        if not out:
            self.note_missing("sprite", name)
        self.path_cache[ck] = out
        return out

    # -- characters ----------------------------------------------------------------------------------
    def ensure_char(self, cid: str, name: str | None = None):
        if cid not in self.chars:
            self.chars[cid] = {"name": name or CHAR_NAMES.get(cid, cid)}
        return self.chars[cid]

    def voice_char(self, voices):
        """The person a voice file belongs to, and whether it is a sound effect rather than a voice."""
        if not voices:
            return None, False
        v = voices[0]
        m = VOICE_CODE_RE.match(v)
        if m:
            code = CODE_ALIASES.get(m.group(1), m.group(1))
            if code in CHAR_NAMES:
                return code, False
        if SFX_VOICE_RE.match(v):
            return None, True
        return None, False

    def resolve_speaker(self, name: str, voices):
        """-> (char id or None for narration, how)"""
        vchar, _ = self.voice_char(voices)
        if name in NAME_MAP:
            return NAME_MAP[name]
        if not name:
            return vchar
        parts = [p for p in re.split(r"[・＆&]", name) if p]
        if len(parts) > 1:
            return vchar or NAME_MAP.get(parts[0]) or ("kou" if parts[0] == "孔" else self.generic_id(name))
        if vchar:
            return vchar
        if name == "孔":
            return "kou"
        return self.generic_id(name)

    def generic_id(self, name: str) -> str:
        if name not in self.generic:
            self.generic[name] = f"n{len(self.generic) + 1:03d}"
        return self.generic[name]

    # -- scenes ----------------------------------------------------------------------------------------
    def bg_path(self):
        if self.bg_name is None:
            return None
        if self.bg_kind == "sub":
            return self.resolve("subcg", self.bg_name, "sub_cg", "background")
        return self.resolve("background", self.bg_name, "background", "sub_cg")

    def mark_day(self, name: str, kind: str, m) -> None:
        """A date screen or day movie: the story moves on to that day. A date earlier than the
        current one (the ending looks back at every day) does not move time back."""
        try:
            day = int(m.group(1))
        except (ValueError, TypeError, IndexError):
            return
        if not 1 <= day <= 31:
            return
        applied = day > self.day
        self.n_markers += 1
        if len(self.markers) < REPORT_LIMIT:
            self.markers.append({
                "name": name, "kind": kind, "day": day, "scenario": self.scenario,
                "applied": applied, "afterLines": self.counts["lines"],
            })
        if applied:
            self.flush_sfx()  # sounds waiting for a line belong to the day before
            self.day = day
            self.day_seq = 0

    def ensure_scene(self):
        finale = self.scenario in FINALE
        want = (self.bg_name, self.bg_kind, self.bgm_name, self.day, finale)
        if self.cur is not None and self.cur["_key"] == want:
            return self.cur
        sid = f"s{len(self.scenes) + 1:04d}"
        sc: dict = {"id": sid, "title": f"{self.scenario} {self.bg_name or ''}".strip(), "lines": [], "_key": want}
        pov = SIDE_CHAR.get(self.side)
        if pov:
            sc["pov"] = pov
            self.needs_pov.add(pov)
        self.day_seq += 1
        sc["when"] = f"D{self.day:02d}-{min(self.day_seq, MAX_SEQ):04d}"
        if finale:
            if self.finale_first is None:
                self.finale_first = sid
                if self.last_before_finale:
                    sc["requires"] = [self.last_before_finale]
        else:
            self.last_before_finale = sid
        bg = self.bg_path() or self.last_bg  # a picture that is missing keeps the one before it on screen
        if bg:
            sc["background"] = bg
            self.last_bg = bg
        if self.bgm_name:
            bgm = self.resolve("bgm", self.bgm_name, "bgm")
            if bgm:
                sc["bgm"] = bgm
        self.scenes.append(sc)
        self.cur = sc
        self.n_in_scene = 0
        return sc

    def add_line(self, line: dict):
        sc = self.ensure_scene()
        self.n_in_scene += 1
        line["id"] = f"{sc['id']}-{self.n_in_scene:03d}"
        sc["lines"].append(line)
        self.last_line = line
        self.counts["lines"] += 1
        return line

    def flush_sfx(self):
        """Sound effects not followed by a line become directions."""
        sfx, self.pending_sfx = self.pending_sfx, []
        for s in sfx:
            self.add_line({"kind": "direction", "sfx": s})
            self.counts["sfxDirections"] += 1

    # -- events ----------------------------------------------------------------------------------------
    def feed(self, events):
        for ev in events:
            getattr(self, "on_" + ev[0])(*ev[1:])
        self.flush_sfx()

    def on_scn(self, name):
        self.scenario = name

    def on_side(self, side):
        self.side = side

    def on_bg(self, name):
        self.flush_sfx()
        m = DATE_SCREEN_RE.match(name)
        if m:
            self.mark_day(name, "screen", m)
        self.bg_name, self.bg_kind = name, "bg"

    def on_term_bg(self, name):
        self.flush_sfx()
        self.bg_name, self.bg_kind = name, "bg"

    def on_term_clear(self):
        pass

    def on_bgm(self, name):
        self.flush_sfx()
        self.bgm_name = name

    def on_clear(self):
        self.sprites = {}

    def on_sprite_clear(self):
        self.sprites = {}

    def on_sprite_show(self, name, slot):
        self.sprites[slot if slot is not None else name] = name
        self.counts["spriteShows"] += 1

    def on_sprite_swap(self, name):
        code = name[:3]
        for k in reversed(list(self.sprites)):
            if self.sprites[k][:3] == code:
                self.sprites[k] = name
                self.counts["spriteSwaps"] += 1
                return
        self.sprites[len(self.sprites) + 100] = name
        self.sprite_unmatched += 1

    def on_subcg(self, name):
        self.counts["subCg"] += 1
        if self.subcg == "background":
            self.flush_sfx()
            self.bg_name, self.bg_kind = name, "sub"

    def on_movie(self, name, op):
        kind = "event movie" if op == 0x8008 else "effect movie"
        m = DATE_MOVIE_RE.match(name or "")
        if m:
            self.mark_day(name, "movie", m)
        self.counts["movies"] += 1
        if len(self.movies) < REPORT_LIMIT:
            self.movies.append({"scenario": self.scenario, "kind": kind, "name": name, "scene": self.cur["id"] if self.cur else None})
        if self.movies_mode == "direction":
            self.flush_sfx()
            self.add_line({"kind": "direction"})

    def on_sfx(self, name, atype):
        fns = ("voice", "se") if atype == "音声" else ("se", "voice")
        p = self.resolve("sfx", name, *fns)
        if p:
            self.pending_sfx.append(p)

    def on_wait(self, frames):
        secs = max(0.0, min(60.0, frames / self.fps))
        if secs <= 0:
            return
        self.counts["waits"] += 1
        self.flush_sfx()
        if self.cur is None:
            return  # nothing shown yet
        ln = self.cur["lines"][-1] if self.cur["lines"] else None
        if ln is None:
            ln = self.add_line({"kind": "direction"})
        ln["pauseAfter"] = round(min(60.0, ln.get("pauseAfter", 0.0) + secs), 3)

    def on_term(self, text):
        if self.terminal == "skip":
            self.counts["terminalSkipped"] += 1
            return
        for kind, *rest in parse_terminal(text):
            if kind == "clear":
                continue
            self.add_text_line("", rest[0], [], "", terminal=True)

    def on_line(self, speaker, text, voices, suffix):
        self.add_text_line(speaker, text, voices, suffix)

    def add_text_line(self, speaker, text, voices, suffix, terminal=False):
        text = clean_text(text)
        if not text:
            self.counts["emptyTextSkipped"] += 1
            return
        vchar, is_sfx_voice = self.voice_char(voices)
        cid = None if terminal else self.resolve_speaker(speaker, voices)
        voice_path = None
        sfx_path = None
        if voices:
            if is_sfx_voice:
                sfx_path = self.resolve("voice(sfx)", voices[0], "voice", "se")
            else:
                self.counts["linesWithVoiceInGame"] += 1
                voice_path = self.resolve("voice", voices[0], "voice")
            if len(voices) > 1:
                self.counts["extraVoicesDropped"] += len(voices) - 1
        line: dict = {"text": text}
        if speaker == "" or terminal:
            if cid and not terminal:  # unspoken text with a character's voice: their thoughts
                kind, who = "monologue", cid
            elif voice_path is None and sfx_path is not None and not terminal:
                kind, who = "narration", "sfx"
                line["voiceFile"], sfx_path = sfx_path, None  # the sound effect is the "voice" of the onomatopoeia
            else:
                kind, who = "narration", "narrator"
        else:
            who = cid
            kind = "dialogue"
            if re.fullmatch(r"[（(].*[）)]", text, re.S):
                kind = "monologue"
        line["kind"] = kind
        line["speaker"] = who
        self.ensure_char(who, {"narrator": "語り", "sfx": "擬音"}.get(who) or (speaker if who.startswith("n") and who[1:].isdigit() else None))
        if speaker in PLAIN_NAMES and who in CHAR_NAMES:
            self.name_votes[who][speaker] += 1
        if voice_path:
            line["voiceFile"] = voice_path
            self.counts["voiced"] += 1
        # expression: the sprite of the speaker that is on screen
        if who in CHAR_NAMES:
            sname = None
            for k in reversed(list(self.sprites)):
                if CODE_ALIASES.get(self.sprites[k][:3], self.sprites[k][:3]) == who:
                    sname = self.sprites[k]
                    break
            if sname:
                p = self.sprite_path(sname)
                if p:
                    expr = sname[3:] or "normal"
                    self.chars[who].setdefault("expressions", {}).setdefault(expr, {"image": p})
                    line["expression"] = expr
        # sound effects that were waiting for this line
        if sfx_path:
            self.pending_sfx.append(sfx_path)
        if self.pending_sfx:
            first = self.pending_sfx.pop(0)
            self.flush_sfx()  # any more become directions before the line
            line["sfx"] = first
            self.counts["sfxOnLines"] += 1
        self.add_line(line)

    # -- the script ----------------------------------------------------------------------------------------
    def script(self) -> dict:
        for cid, votes in self.name_votes.items():
            if cid in self.chars and votes:
                self.chars[cid]["name"] = votes.most_common(1)[0][0]
        for cid in self.needs_pov:
            self.ensure_char(cid)
        for c in self.chars.values():
            exps = c.get("expressions")
            if exps and "normal" in exps:
                c["defaultExpression"] = "normal"
        scenes = []
        for sc in self.scenes:
            lines = sc["lines"]
            sc = {k: v for k, v in sc.items() if k not in ("_key", "lines")}
            if sc.get("pov") and sc["pov"] not in self.chars:
                del sc["pov"]
            sc["lines"] = lines
            scenes.append(sc)
        return {
            "version": 1,
            "title": self.title,
            "language": "Japanese",
            "characters": dict(sorted(self.chars.items())),
            "scenes": scenes,
            "route": [s["id"] for s in scenes],
        }


# -- the whole conversion --------------------------------------------------------------------------------
def find_scenario_folder(root: str) -> str:
    for cand in (os.path.join(root, "01_シナリオ"), root):
        if os.path.isdir(cand) and any(f.lower().endswith(".json") for _dp, _dn, fn in os.walk(cand) for f in fn):
            return cand
    return os.path.join(root, "01_シナリオ")


def parse_start(start: str):
    """``a01`` / ``a01:0`` (a label number) -> (scenario, label or None)."""
    if ":" in start:
        s, _, lab = start.partition(":")
        try:
            return s, int(lab)
        except ValueError:
            raise SystemExit(f"--start {start!r}: the part after ':' must be a label number")
    return start, None


def convert(root: str, out_path: str, start: str = "kojirou", opts: Options | None = None, assets=None,
            fps: float = 60.0, terminal: str = "narration", subcg: str = "ignore", title: str | None = None,
            scenario_dir: str | None = None, movies: str = "skip"):
    """Run the playthrough and build the script. Returns (script dict, report dict)."""
    opts = opts or Options()
    out_dir = os.path.dirname(os.path.abspath(out_path)) or "."
    store = ScenarioStore(scenario_dir or find_scenario_folder(root))
    if start in ROUTES:
        scn, label = ROUTES[start]
    else:
        scn, label = parse_start(start)
    scn = normalize_scenario(scn)
    assets = assets if assets is not None else load_assets(root)
    interp = Interp(store, opts)
    interp.run(scn, label)
    b = Builder(assets, root, out_dir, fps, terminal, subcg, title or f"EVE burst error PLUS ({start})", movies)
    b.feed(interp.events or [])
    script = b.script()
    report = make_report(interp, b, script, start, assets, store)
    return script, report


def make_report(interp: Interp, b: Builder, script: dict, start: str, assets, store: ScenarioStore) -> dict:
    scenes = script["scenes"]
    lines = [ln for s in scenes for ln in s["lines"]]
    run_text = total_text = 0
    for name in store.files:
        sc = store.get(name)
        if sc is None:
            continue
        total_text += sum(1 for op in sc.ops if op == 0x8035)
    for (sname, k), _n in interp.visits.items():
        sc = store.cache.get(sname)
        if sc is not None and sc.ops[k] == 0x8035:
            run_text += 1
    spoken = [ln for ln in lines if ln.get("kind") != "direction"]
    return {
        "start": start,
        "endedBecause": interp.end_reason,
        "assetsModule": bool(getattr(assets, "available", False)),
        "scenes": len(scenes),
        "lines": len(lines),
        "spokenLines": len(spoken),
        "voicedLines": sum(1 for ln in spoken if ln.get("voiceFile") and ln.get("speaker") != "sfx"),
        "soundEffectLines": sum(1 for ln in spoken if ln.get("speaker") == "sfx"),
        "linesWithSfx": sum(1 for ln in lines if ln.get("sfx")),
        "directions": len(lines) - len(spoken),
        "characters": {k: v.get("name") for k, v in script["characters"].items()},
        "scenariosVisited": len(interp.trace),
        "distinctScenarios": len(set(interp.trace)),
        "textInstructionsRun": run_text,
        "textInstructionsInGame": total_text,
        "scenarioTrace": interp.trace[:REPORT_LIMIT] + (["..."] if len(interp.trace) > REPORT_LIMIT else []),
        "steps": interp.steps,
        "flagChanges": interp.flag_ver,
        "teleports": len(interp.teleports),
        "teleportList": interp.teleports[:REPORT_LIMIT],
        "policy": interp.o.policy,
        "walks": len(interp.walks),
        "walkList": interp.walks[:REPORT_LIMIT],
        "zapping": interp.partner_report(),
        "counts": dict(b.counts),
        "unknownOps": dict(interp.unknown_ops),
        "unknownOpExamples": interp.unknown_examples,
        "ignoredCosmetic": {f"0x{k:04x}": v for k, v in sorted(interp.cosmetic.items())},
        "missingAssets": b.missing,
        "movies": b.movies,
        "unmatchedSpriteSwaps": b.sprite_unmatched,
        "notes": interp.notes[:REPORT_LIMIT] + store.errors[:REPORT_LIMIT],
        "menuChoices": interp.menu_log[:REPORT_LIMIT],
        "menuChoicesTotal": len(interp.menu_log),
        "days": day_summary(script),
        "dateMarkers": b.markers,
        "dateMarkersTotal": b.n_markers,
        "finale": {"firstScene": b.finale_first, "requires": b.last_before_finale if b.finale_first else None},
    }


# -- story time and episodes ------------------------------------------------------------------------------
def day_label(day: int) -> str:
    return "序章" if day <= 0 else f"12月{day}日"


def scene_days(scenes) -> list:
    """The day of each scene, from its ``when`` (``D03-0012``); a scene without one is on the day
    of the scene before it (the first ones on day 0)."""
    out, day = [], 0
    for sc in scenes:
        w = sc.get("when") if isinstance(sc, dict) else None
        m = WHEN_RE.match(w) if isinstance(w, str) else None
        if m:
            day = int(m.group(1))
        out.append(day)
    return out


def scenes_in_route(script: dict) -> list:
    """The scenes in the order they are shown: ``route`` if it is given, else file order."""
    scenes = [sc for sc in script.get("scenes") or [] if isinstance(sc, dict)]
    route = script.get("route") or []
    if not route:
        return scenes
    by_id: dict = {}
    for sc in scenes:
        if isinstance(sc.get("id"), str):
            by_id.setdefault(sc["id"], sc)
    seen, out = set(), []
    for r in route:
        if isinstance(r, str) and r in by_id and r not in seen:
            seen.add(r)
            out.append(by_id[r])
    return out


def day_summary(script: dict) -> list:
    scenes = scenes_in_route(script)
    out: list = []
    for sc, d in zip(scenes, scene_days(scenes)):
        if not out or out[-1]["day"] != d:
            out.append({"day": d, "label": day_label(d), "firstScene": sc.get("id"), "scenes": 0, "lines": 0})
        out[-1]["scenes"] += 1
        out[-1]["lines"] += len(sc.get("lines") or [])
    return out[:REPORT_LIMIT]


def characters_used(script: dict, scenes) -> dict:
    """The characters a set of scenes needs: speakers and viewpoints (whole entries, so their
    expressions fall back the same way as in the full script)."""
    want = set()
    for sc in scenes:
        if isinstance(sc.get("pov"), str):
            want.add(sc["pov"])
        for ln in sc.get("lines") or []:
            if isinstance(ln, dict) and isinstance(ln.get("speaker"), str):
                want.add(ln["speaker"])
    chars = script.get("characters") or {}
    return {k: v for k, v in sorted(chars.items()) if k in want}


def split_episodes(script: dict, max_lines: int | None = None, label: str = "") -> list:
    """Cut a script into episodes: one per day of its ``when``; with ``max_lines`` a day that has
    more lines is cut further, between scenes (a scene is never cut, so one longer than the cap
    is an episode of its own). Returns a list of {script, day, part, parts, scenes, lines}."""
    if max_lines is not None and max_lines < 1:
        raise ValueError("the line cap of an episode must be at least 1")
    scenes = scenes_in_route(script)
    groups: list = []  # [day, [scenes]]
    for sc, d in zip(scenes, scene_days(scenes)):
        if not groups or groups[-1][0] != d:
            groups.append([d, []])
        groups[-1][1].append(sc)
    parts: list = []  # (day, part, parts of that day, scenes)
    for d, scs in groups:
        chunks: list = [[]]
        n = 0
        for sc in scs:
            k = len(sc.get("lines") or [])
            if max_lines is not None and chunks[-1] and n + k > max_lines:
                chunks.append([])
                n = 0
            chunks[-1].append(sc)
            n += k
        for i, ch in enumerate(chunks):
            parts.append((d, i + 1, len(chunks), ch))
    if len(parts) > MAX_EPISODES:
        raise ValueError(f"{len(parts)} episodes: more than {MAX_EPISODES}; raise the line cap")
    out = []
    for n, (d, part, nparts, scs) in enumerate(parts, 1):
        ids = {sc.get("id") for sc in scs}
        new_scenes = []
        for sc in scs:
            sc = dict(sc)
            # a prerequisite in an earlier episode is shown before this one anyway
            req = [r for r in sc.get("requires") or [] if r in ids]
            if req:
                sc["requires"] = req
            else:
                sc.pop("requires", None)
            new_scenes.append(sc)
        title = f"第 {n} 話 {day_label(d)}" + (f"（{part}）" if nparts > 1 else "")
        if label:
            title = f"{label} {title}"
        ep = {
            "version": script.get("version", 1),
            "title": title,
            "language": script.get("language", "Japanese"),
            "characters": characters_used(script, new_scenes),
            "scenes": new_scenes,
            "route": [sc.get("id") for sc in new_scenes],
        }
        out.append({"script": ep, "day": d, "part": part, "parts": nparts, "scenes": len(new_scenes),
                    "lines": sum(len(sc.get("lines") or []) for sc in new_scenes)})
    return out


def episode_path(out_path: str, n: int, total: int) -> str:
    """``out/kojirou.fcscript.json`` -> ``out/kojirou.ep01.fcscript.json``."""
    d, base = os.path.split(out_path)
    if base.lower().endswith(".fcscript.json"):
        stem = base[: -len(".fcscript.json")]
    else:
        stem = os.path.splitext(base)[0]
    width = max(2, len(str(total)))
    return os.path.join(d, f"{stem}.ep{n:0{width}d}.fcscript.json")


def write_json(path: str, data) -> None:
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(data, f, ensure_ascii=False, indent=1)
        f.write("\n")


def load_choices(path: str) -> list:
    with open(path, "rb") as f:
        data = json.loads(f.read().decode("utf-8-sig"))
    if not isinstance(data, list):
        raise SystemExit(f"{path}: expected a JSON list of selections")
    for x in data:
        if not (x is None or isinstance(x, (int, str))) or isinstance(x, bool):
            raise SystemExit(f"{path}: a selection is a 1-based number, a menu text, \"auto\", \"cancel\" or null, not {x!r}")
    return data


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--root", required=True, help="the decoded dump (the folder holding 01_シナリオ, 02_画像, 03_音声); read only")
    ap.add_argument("--route", default=None, help="kojirou (a01, default) or marina (b01)")
    ap.add_argument("--start", default=None, help="start at a scenario instead of a route: a001, a01:0 (scenario:label number)")
    ap.add_argument("--out", required=True, help="the script to write (*.fcscript.json); paths inside are relative to it")
    ap.add_argument("--report", help="also write the report (unknown instructions, missing files, menu choices...) here")
    ap.add_argument("--choices", help="JSON list of selections for the menus, in the order they come up")
    ap.add_argument("--choices-only", action="store_true", help="end the path when the list is used up instead of falling back to the policy")
    ap.add_argument("--policy", choices=POLICIES, default="days", help="menu policy once --choices is used up: days (default: explore, the story days in order, walking, the other side alongside), explore or order")
    ap.add_argument("--no-zapping", action="store_true", help="days: do not play the other side alongside (the sides share the flags F0-F255)")
    ap.add_argument("--on-stuck", choices=("teleport", "stop"), default="teleport", help="when no menu entry has anything new: jump to a scenario that has (default), or end the path")
    ap.add_argument("--max-steps", type=int, default=DEFAULT_MAX_STEPS)
    ap.add_argument("--max-lines", type=int, default=DEFAULT_MAX_LINES)
    ap.add_argument("--max-moves", type=int, default=DEFAULT_MAX_MOVES)
    ap.add_argument("--fps", type=float, default=60.0, help="frames per second of the game's wait instruction (assumed 60)")
    ap.add_argument("--terminal", choices=("narration", "skip"), default="narration", help="text printed on hacking terminals")
    ap.add_argument("--subcg", choices=("ignore", "background"), default="ignore", help="small-window CGs: ignore them or show them as the background")
    ap.add_argument("--movies", choices=("skip", "direction"), default="skip", help="event movies: leave them out (they are listed in the report) or put an empty direction line where they play")
    ap.add_argument("--scenarios", help="folder of scenario .json files (default: <root>/01_シナリオ)")
    ap.add_argument("--title", default=None)
    ap.add_argument("--episodes", action="store_true", help="also write one script per day of the story: <out stem>.ep01.fcscript.json, ...")
    ap.add_argument("--episode-lines", type=int, default=None, metavar="N", help="cut a day with more than N lines into several episodes, between scenes (implies --episodes)")
    a = ap.parse_args(argv)
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except Exception:  # noqa: BLE001
            pass
    if a.route and a.start:
        raise SystemExit("give --route or --start, not both")
    start = a.start or a.route or "kojirou"
    if not (a.fps > 0):
        raise SystemExit("--fps must be positive")
    opts = Options(policy=a.policy, max_steps=a.max_steps, max_lines=a.max_lines, max_moves=a.max_moves, choices_only=a.choices_only, on_stuck=a.on_stuck,
                   zapping=not a.no_zapping)
    if a.choices:
        opts.choices = load_choices(a.choices)
    if a.episode_lines is not None:
        if a.episode_lines < 1:
            raise SystemExit("--episode-lines must be at least 1")
        a.episodes = True
    if not os.path.isdir(a.root):
        raise SystemExit(f"--root {a.root}: not a folder")
    out_dir = os.path.dirname(os.path.abspath(a.out)) or "."
    os.makedirs(out_dir, exist_ok=True)
    script, report = convert(a.root, a.out, start, opts, fps=a.fps, terminal=a.terminal, subcg=a.subcg, title=a.title, scenario_dir=a.scenarios, movies=a.movies)
    write_json(a.out, script)
    episodes = []
    if a.episodes and script["scenes"]:
        eps = split_episodes(script, a.episode_lines, ROUTE_LABELS.get(start, ""))
        for n, ep in enumerate(eps, 1):
            path = episode_path(a.out, n, len(eps))
            write_json(path, ep["script"])
            episodes.append({"file": os.path.basename(path), "title": ep["script"]["title"], "day": ep["day"],
                             "scenes": ep["scenes"], "lines": ep["lines"]})
        report["episodes"] = episodes
    if a.report:
        write_json(a.report, report)
    brief = {k: report[k] for k in ("start", "endedBecause", "scenes", "lines", "spokenLines", "voicedLines", "scenariosVisited", "steps")}
    brief.update({"out": os.path.abspath(a.out), "unknownOps": report["unknownOps"], "missing": {k: len(v) for k, v in report["missingAssets"].items()}})
    brief["days"] = {d["label"]: d["lines"] for d in report["days"]}
    if episodes:
        brief["episodes"] = len(episodes)
        brief["firstEpisode"] = episodes[0]
    print(json.dumps(brief, ensure_ascii=False))
    if not report["assetsModule"]:
        print("note: assets.py was not found or did not load: the script has no pictures or audio", file=sys.stderr)
    if script["scenes"] == []:
        print("nothing was produced: " + report["endedBecause"], file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
