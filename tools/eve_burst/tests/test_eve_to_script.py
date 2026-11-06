"""Tests for tools/eve_burst/eve_to_script.py.

Run:  python -m unittest discover -s tools/eve_burst/tests
The small scenarios are made by hand in a temp directory, with a fake asset resolver. The tests
that need the real decoded dump skip when it is absent (set EVE_BURST_DATA to override the
default location); they never write inside it.
"""
import copy
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
import eve_to_script as E  # noqa: E402

REAL_ROOT = os.environ.get("EVE_BURST_DATA", "E:/filmcraft_JPN/PS2_EVE_Bursterror解析データ")
REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))


class FakeAssets:
    """Resolves every name to ``<kind>/<name>`` except the ones listed as missing."""

    available = True

    def __init__(self, missing=()):
        self.missing = set(missing)

    def _p(self, kind, name, ext):
        return None if name in self.missing else f"{kind}/{name}.{ext}"

    def background(self, name):
        return self._p("bg", name, "png")

    def sub_cg(self, name):
        return self._p("sub", name, "png")

    def sprite(self, code, *a, **k):
        return self._p("chara", code, "png")

    def bgm(self, name):
        return self._p("bgm", name, "wav")

    def se(self, name):
        return self._p("se", name, "wav")

    def voice(self, name):
        return self._p("voice", name, "wav")

    def movie(self, name):
        return self._p("movie", name, "mp4")


class Sc:
    """Builds one decoded scenario (the .json of the dump) with symbolic jump targets."""

    def __init__(self, name):
        self.name = name
        self.ins = []
        self.marks = {}

    def mark(self, name):
        self.marks[name] = len(self.ins)
        return self

    def add(self, op, **kw):
        self.ins.append({"op": f"0x{op:04x}", **kw})
        return self

    # -- shorthands
    def label(self, n=1):
        return self.add(0x8001, args=[n], label=n)

    def say(self, speaker, text, voices=(), suffix=""):
        return self.add(0x8035, speaker=speaker, text=text, control_suffix=suffix, voices=[{"name": v, "file": v + ".ahx"} for v in voices])

    def bg(self, name):
        return self.add(0x8014, asset={"type": "背景", "name": name})

    def bgm(self, name):
        return self.add(0x8026, asset={"type": "BGM", "name": name})

    def flag(self, f, v=1):
        return self.add(0x8007, flag=f, value=v)

    def move(self, to):
        return self.add(0x8020, scenario=to, scenario_file=to + ".sco")

    def jump(self, to):
        return self.add(0x8005, target=f"L:{to}")

    def iff(self, conds, else_to):
        return self.add(0x8003, conditions=[list(c) for c in conds], target=f"L:{else_to}")

    def endif(self):
        return self.add(0x8004)

    def menu(self, *items):
        return self.add(0x8002, items=[{"text": t, "conditions": [list(c) for c in cond], "target": f"L:{tg}"} for t, cond, tg in items])

    def back(self):
        return self.add(0x8006)

    def end(self):
        return self.add(0x8000)

    def clear_game(self):
        return self.add(0x800D)

    def data(self):
        ins = []
        for k, d in enumerate(self.ins):
            d = copy.deepcopy(d)
            d["off"] = hex(k)
            if isinstance(d.get("target"), str):
                d["target"] = hex(self.marks[d["target"][2:]])
            for it in d.get("items", []):
                it["target"] = hex(self.marks[it["target"][2:]])
            ins.append(d)
        return {"scenario": self.name, "instructions": ins}


def write_scenarios(folder, *scs):
    os.makedirs(os.path.join(folder, "01_シナリオ", "小次郎編"), exist_ok=True)
    for sc in scs:
        with open(os.path.join(folder, "01_シナリオ", "小次郎編", sc.name + ".json"), "w", encoding="utf-8") as f:
            json.dump(sc.data(), f, ensure_ascii=False)


def validate_by_hand(script):
    """The checks of crates/project/src/script.rs that need no files; returns error strings."""
    errs = []
    if script.get("version") != 1:
        errs.append("version")
    chars = script.get("characters", {})
    if len(chars) > 500:
        errs.append("too many characters")
    scene_ids, line_ids = set(), set()
    n_lines = 0
    for sc in script.get("scenes", []):
        if not sc["id"] or len(sc["id"]) > 128 or sc["id"] in scene_ids:
            errs.append(f"scene id {sc['id']!r}")
        scene_ids.add(sc["id"])
        if sc.get("pov") and sc["pov"] not in chars:
            errs.append("pov " + sc["pov"])
        for ln in sc["lines"]:
            n_lines += 1
            lid = ln["id"]
            if not lid or len(lid) > 128 or lid in line_ids:
                errs.append(f"line id {lid!r}")
            line_ids.add(lid)
            kind = ln.get("kind", "dialogue")
            text = ln.get("text", "")
            if len(text) > 4000:
                errs.append("text too long " + lid)
            if kind in ("dialogue", "monologue") and ln.get("speaker") not in chars:
                errs.append(f"speaker of {lid}")
            if kind == "narration" and ln.get("speaker") is not None and ln["speaker"] not in chars:
                errs.append(f"narrator of {lid}")
            if kind != "direction" and not text.strip():
                errs.append("empty text " + lid)
            p = ln.get("pauseAfter")
            if p is not None and not (0 <= p <= 60):
                errs.append("pause " + lid)
    if n_lines > 200000 or len(scene_ids) > 10000:
        errs.append("too big")
    for r in script.get("route", []):
        if r not in scene_ids:
            errs.append("route " + r)
    if len(set(script.get("route", []))) != len(script.get("route", [])):
        errs.append("route repeats")
    return errs


class Base(unittest.TestCase):
    def setUp(self):
        self._td = tempfile.TemporaryDirectory()
        self.root = self._td.name
        self.out = os.path.join(self.root, "x.fcscript.json")

    def tearDown(self):
        self._td.cleanup()

    def convert(self, *scs, start="a01", opts=None, assets=None, **kw):
        write_scenarios(self.root, *scs)
        script, report = E.convert(self.root, self.out, start, opts or E.Options(), assets=assets or FakeAssets(), **kw)
        self.assertEqual(validate_by_hand(script), [])
        return script, report

    @staticmethod
    def lines(script):
        return [ln for sc in script["scenes"] for ln in sc["lines"]]


class LinesAndScenes(Base):
    def test_speakers_voices_and_narration(self):
        a = Sc("a01").label().bg("01______").bgm("eve001")
        a.say("小次郎", "やあ。", ["koj0001"])
        a.say("", "静かな夜だった。")
        a.say("声", "誰だ？", ["kyo0042"])  # a name that says nothing: the voice file says who
        a.say("", "ドテッ。", ["j301"])  # onomatopoeia with a sound effect as its voice
        a.say("", "（まだ眠い）", ["may0001"])  # unspoken text with a character's voice: their thoughts
        a.say("まりな", "（ガクッ‥‥。）")  # parenthesised: a thought
        a.say("誰か", "どうも。")  # a minor character
        a.clear_game()
        s, rep = self.convert(a)
        ln = self.lines(s)
        self.assertEqual([x["speaker"] for x in ln], ["koj", "narrator", "kyo", "sfx", "may", "mar", "n001"])
        self.assertEqual([x["kind"] for x in ln], ["dialogue", "narration", "dialogue", "narration", "monologue", "monologue", "dialogue"])
        self.assertEqual(ln[0]["voiceFile"], "voice/koj0001.wav")
        self.assertEqual(ln[2]["voiceFile"], "voice/kyo0042.wav")
        self.assertEqual(ln[3]["voiceFile"], "voice/j301.wav")
        self.assertNotIn("voiceFile", ln[1])
        self.assertEqual(s["characters"]["narrator"]["name"], "語り")
        self.assertEqual(s["characters"]["n001"]["name"], "誰か")
        self.assertEqual(rep["endedBecause"], "game clear (the ending was reached)")
        self.assertEqual(rep["voicedLines"], 3)
        self.assertEqual(s["scenes"][0]["pov"], "koj")
        self.assertEqual(s["route"], [sc["id"] for sc in s["scenes"]])

    def test_scenes_split_at_background_or_music_change(self):
        a = Sc("a01").label().bg("b1").bgm("eve001").say("小次郎", "1")
        a.bg("b1").say("小次郎", "2")  # the same picture again: no new scene
        a.bg("b2").say("小次郎", "3")
        a.bgm("eve002").say("小次郎", "4")
        a.add(0x8024).say("小次郎", "5")  # music stops
        a.clear_game()
        s, _ = self.convert(a)
        self.assertEqual([[x["text"] for x in sc["lines"]] for sc in s["scenes"]], [["1", "2"], ["3"], ["4"], ["5"]])
        self.assertEqual(s["scenes"][0]["background"], "bg/b1.png")
        self.assertEqual(s["scenes"][0]["bgm"], "bgm/eve001.wav")
        self.assertNotIn("bgm", s["scenes"][3])
        self.assertEqual(s["scenes"][3]["background"], "bg/b2.png")
        ids = [x["id"] for x in self.lines(s)]
        self.assertEqual(len(ids), len(set(ids)))

    def test_missing_assets_are_reported_and_the_last_picture_stays(self):
        a = Sc("a01").label().bg("b1").say("小次郎", "1", ["koj0001"]).bg("nope").say("小次郎", "2").clear_game()
        s, rep = self.convert(a, assets=FakeAssets(missing={"nope", "koj0001"}))
        self.assertEqual(rep["missingAssets"]["background"], ["nope"])
        self.assertEqual(rep["missingAssets"]["voice"], ["koj0001"])
        self.assertEqual(s["scenes"][1]["background"], "bg/b1.png")
        self.assertNotIn("voiceFile", self.lines(s)[0])

    def test_sprite_expression_follows_the_picture_on_screen(self):
        a = Sc("a01").label().bg("b1")
        a.add(0x8048, slot=2, asset={"type": "立ち絵", "name": "aka00ad"})
        a.say("茜", "a")
        a.add(0x804B, asset={"type": "立ち絵", "name": "aka03ad"})
        a.say("茜", "b")
        a.say("小次郎", "c")
        a.add(0x8049, fade_ms=0)
        a.say("茜", "d")
        a.clear_game()
        s, _ = self.convert(a)
        ln = self.lines(s)
        self.assertEqual([x.get("expression") for x in ln], ["00ad", "03ad", None, None])
        self.assertEqual(sorted(s["characters"]["aka"]["expressions"]), ["00ad", "03ad"])

    def test_waits_become_pause_after_at_60_fps(self):
        a = Sc("a01").label().bg("b1").say("小次郎", "1").add(0x802F, frames=30).add(0x802F, frames=60).say("小次郎", "2").clear_game()
        s, _ = self.convert(a)
        self.assertEqual(self.lines(s)[0]["pauseAfter"], 1.5)
        self.assertNotIn("pauseAfter", self.lines(s)[1])
        s, _ = self.convert(a, fps=30)
        self.assertEqual(self.lines(s)[0]["pauseAfter"], 3.0)

    def test_sound_effect_goes_with_the_next_line(self):
        a = Sc("a01").label().bg("b1").add(0x800B, asset={"type": "効果音", "name": "se01"}).say("小次郎", "バン").add(0x800B, asset={"type": "効果音", "name": "se02"}).bg("b2").say("小次郎", "x").clear_game()
        s, _ = self.convert(a)
        ln = self.lines(s)
        self.assertEqual(ln[0]["sfx"], "se/se01.wav")
        self.assertEqual(ln[1]["kind"], "direction")  # a sound with no line after it, before a cut
        self.assertEqual(ln[1]["sfx"], "se/se02.wav")

    def test_aozora_ruby_marks_in_text_are_neutralised(self):
        a = Sc("a01").label().bg("b1").say("男の声１", "《おい、聞こえるか？》").clear_game()
        s, _ = self.convert(a)
        self.assertEqual(self.lines(s)[0]["text"], "〈おい、聞こえるか？〉")

    def test_empty_text_is_dropped(self):
        a = Sc("a01").label().bg("b1").say("", "　").say("小次郎", "ok").clear_game()
        s, rep = self.convert(a)
        self.assertEqual(len(self.lines(s)), 1)
        self.assertEqual(rep["counts"]["emptyTextSkipped"], 1)

    def test_terminal_text_and_movies(self):
        a = Sc("a01").label().bg("b1").add(0x8041, args=[0], asset={"name": "hack_k"}).add(0x8043, text="\\c1ナイフ\\nデータ\\w1\\n\\q消去後").add(0x8008, asset={"name": "scene01"}).clear_game()
        s, rep = self.convert(a)
        self.assertEqual([x["text"] for x in self.lines(s)], ["ナイフ", "データ", "消去後"])
        self.assertTrue(all(x["speaker"] == "narrator" for x in self.lines(s)))
        self.assertEqual(rep["movies"][0]["name"], "scene01")
        s, _ = self.convert(a, movies="direction")
        self.assertEqual(self.lines(s)[-1]["kind"], "direction")
        s, _ = self.convert(a, terminal="skip")
        self.assertEqual(len(self.lines(s)), 0)


class Interpreter(Base):
    def test_conditional_chain_takes_one_branch_and_skips_the_rest(self):
        a = Sc("a01").label().bg("b1")
        a.flag(5, 2)
        a.iff([(5, 1)], "else1").say("小次郎", "one").endif()
        a.mark("else1").iff([(5, 2)], "else2").say("小次郎", "two").endif()
        a.mark("else2").iff([(5, 2)], "end").say("小次郎", "again two").endif()
        a.mark("end").say("小次郎", "after").clear_game()
        s, _ = self.convert(a)
        # "two" runs, its end-of-branch skips the chain that follows ("again two"), "after" runs
        self.assertEqual([x["text"] for x in self.lines(s)], ["two", "after"])

    def test_flags_below_256_are_shared_and_the_rest_are_per_side(self):
        a = Sc("a01").label().bg("b1").flag(10).flag(300).add(0x802A, args=[2]).iff([(10, 1)], "n1").say("小次郎", "shared").mark("n1").iff([(300, 1)], "n2").say("小次郎", "own").mark("n2").say("小次郎", "end").clear_game()
        s, _ = self.convert(a)
        self.assertEqual([x["text"] for x in self.lines(s)], ["shared", "end"])  # F300 was set for side 1

    def test_clearing_the_side_flags(self):
        a = Sc("a01").label().bg("b1").flag(300).flag(10).add(0x8011).iff([(300, 0)], "n").say("小次郎", "cleared").mark("n").iff([(10, 1)], "m").say("小次郎", "kept").mark("m").clear_game()
        s, _ = self.convert(a)
        self.assertEqual([x["text"] for x in self.lines(s)], ["cleared", "kept"])

    def test_menu_every_entry_once_and_returning_ones_before_leaving_ones(self):
        a = Sc("a01").label().bg("b1")
        a.menu(("出る", [], "go"), ("調べる", [], "look"), ("話す", [], "talk"), ("聞く", [(7, 1)], "ask"))
        a.mark("go").say("小次郎", "go").move("a02").back()
        a.mark("look").say("小次郎", "look").back()
        a.mark("talk").say("小次郎", "talk").flag(7).back()
        a.mark("ask").say("小次郎", "ask").back()
        b = Sc("a02").label().bg("b2").say("小次郎", "arrived").clear_game()
        s, rep = self.convert(a, b)
        # the entry that leaves goes last; "ask" appears once "talk" has set its flag
        self.assertEqual([x["text"] for x in self.lines(s)], ["look", "talk", "ask", "go", "arrived"])
        self.assertEqual(rep["scenariosVisited"], 2)

    def test_policy_order_takes_the_first_entry(self):
        a = Sc("a01").label().bg("b1")
        a.menu(("出る", [], "go"), ("調べる", [], "look"))
        a.mark("go").say("小次郎", "go").clear_game()
        a.mark("look").say("小次郎", "look").back()
        s, _ = self.convert(a, opts=E.Options(policy="order"))
        self.assertEqual([x["text"] for x in self.lines(s)], ["go"])

    def test_entry_is_repeated_while_it_still_has_something_new(self):
        # talking again after another entry changed a flag shows new lines; once nothing is new the path ends
        a = Sc("a01").label().bg("b1")
        a.menu(("A", [], "a"), ("B", [], "b"))
        a.mark("a").iff([(1, 1)], "a2").say("小次郎", "A late").flag(9).endif()
        a.mark("a2").iff([(1, 0)], "a3").say("小次郎", "A early").endif()
        a.mark("a3").back()
        a.mark("b").say("小次郎", "B").flag(1).back()
        s, rep = self.convert(a, opts=E.Options(on_stuck="stop"))
        self.assertEqual([x["text"] for x in self.lines(s)], ["A early", "B", "A late"])
        self.assertIn("menu exhausted", rep["endedBecause"])

    def test_nested_menu_cancels_back_to_the_parent(self):
        a = Sc("a01").label().bg("b1")
        a.menu(("見る", [], "look"), ("終わる", [], "fin"))
        a.mark("look").menu(("壁", [], "wall"), ("床", [], "floor"))
        a.mark("wall").say("小次郎", "wall").back()
        a.mark("floor").say("小次郎", "floor").back()
        a.mark("fin").say("小次郎", "fin").clear_game()
        s, _ = self.convert(a)
        self.assertEqual([x["text"] for x in self.lines(s)], ["wall", "floor", "fin"])

    def test_jump_drops_the_open_menus(self):
        a = Sc("a01").label().bg("b1")
        a.menu(("go", [], "go"))
        a.mark("go").jump("next")
        a.mark("next").say("小次郎", "next").back()  # nothing to return to: the path ends here
        s, rep = self.convert(a, opts=E.Options(on_stuck="stop"))
        self.assertEqual([x["text"] for x in self.lines(s)], ["next"])
        self.assertIn("return with no menu open", rep["endedBecause"])

    def test_explicit_choices_by_number_text_and_cancel(self):
        a = Sc("a01").label().bg("b1")
        a.menu(("A", [], "a"), ("B", [], "b"), ("C", [], "c"))
        a.mark("a").say("小次郎", "A").back()
        a.mark("b").say("小次郎", "B").back()
        a.mark("c").say("小次郎", "C").clear_game()
        o = E.Options(choices=[2, "A", "C"], choices_only=True)
        s, _ = self.convert(a, opts=o)
        self.assertEqual([x["text"] for x in self.lines(s)], ["B", "A", "C"])
        o = E.Options(choices=[2], choices_only=True)
        s, rep = self.convert(a, opts=o)
        self.assertEqual([x["text"] for x in self.lines(s)], ["B"])
        self.assertIn("choices exhausted", rep["endedBecause"])
        s, rep = self.convert(a, opts=E.Options(choices=["cancel"], on_stuck="stop"))
        self.assertEqual(len(self.lines(s)), 0)
        self.assertEqual(len(rep["menuChoices"]), 1)

    def test_scenario_move_to_a_missing_scenario_ends_the_path(self):
        a = Sc("a01").label().bg("b1").say("小次郎", "x").move("c01")
        s, rep = self.convert(a)
        self.assertIn("'c01' is not available", rep["endedBecause"])
        self.assertTrue(any("c01" in n for n in rep["notes"]))

    def test_hyphen_in_a_scenario_name_is_an_underscore(self):
        a = Sc("a01").label().bg("b1").move("a02-2")
        b = Sc("a02_2").label().bg("b1").say("小次郎", "there").clear_game()
        s, _ = self.convert(a, b)
        self.assertEqual([x["text"] for x in self.lines(s)], ["there"])

    def test_unknown_instruction_goes_to_the_report(self):
        a = Sc("a01").label().bg("b1").add(0x80FF, name="謎の命令").say("小次郎", "x").clear_game()
        s, rep = self.convert(a)
        self.assertEqual(len(self.lines(s)), 1)
        self.assertEqual(list(rep["unknownOps"]), ["0x80ff unknown instruction"])
        self.assertEqual(rep["unknownOpExamples"]["0x80ff unknown instruction"]["name"], "謎の命令")

    def test_start_at_a_label_and_a_route_name(self):
        a = Sc("a01").label(0).bg("b1").say("小次郎", "label 0").clear_game()
        a.label(2).say("小次郎", "label 2").clear_game()
        s, _ = self.convert(a, start="a01:2")
        self.assertEqual([x["text"] for x in self.lines(s)], ["label 2"])
        s, _ = self.convert(a, start="kojirou")  # the route name means a01 at label 0
        self.assertEqual([x["text"] for x in self.lines(s)], ["label 0"])

    def test_system_menu_entries_are_never_chosen(self):
        a = Sc("a01").label().bg("b1").menu(("ロード", [], "load"), ("最初から", [], "go"))
        a.mark("load").say("小次郎", "load").back()
        a.mark("go").say("小次郎", "go").clear_game()
        s, _ = self.convert(a)
        self.assertEqual([x["text"] for x in self.lines(s)], ["go"])

    def test_stuck_path_teleports_to_a_scenario_with_something_new(self):
        a = Sc("a01").label().bg("b1").menu(("見る", [], "look"))
        a.mark("look").say("小次郎", "look").back()
        b = Sc("a05").label().bg("b2").say("小次郎", "elsewhere").clear_game()
        s, rep = self.convert(a, b)
        self.assertEqual([x["text"] for x in self.lines(s)], ["look", "elsewhere"])
        self.assertEqual(rep["teleports"], 1)
        self.assertEqual(rep["teleportList"][0]["to"], "a05")
        s, rep = self.convert(a, b, opts=E.Options(on_stuck="stop"))
        self.assertEqual([x["text"] for x in self.lines(s)], ["look"])


class NeverHangs(Base):
    def test_endless_loop_is_cut(self):
        a = Sc("a01").label().bg("b1").mark("top").say("小次郎", "again").jump("top")
        t = time.time()
        s, rep = self.convert(a, opts=E.Options(loop_visits=50))
        self.assertLess(time.time() - t, 10)
        self.assertIn("loop", rep["endedBecause"])
        self.assertLessEqual(len(self.lines(s)), 50)

    def test_line_cap(self):
        a = Sc("a01").label().bg("b1").mark("top").say("小次郎", "again").jump("top")
        s, rep = self.convert(a, opts=E.Options(max_lines=20, loop_visits=10**6))
        self.assertEqual(len(self.lines(s)), 20)
        self.assertIn("line cap", rep["endedBecause"])

    def test_step_cap_and_move_cap(self):
        a = Sc("a01").label().bg("b1").mark("top").flag(1).jump("top")
        s, rep = self.convert(a, opts=E.Options(max_steps=500, loop_visits=10**6))
        self.assertIn("step cap", rep["endedBecause"])
        a = Sc("a01").label().bg("b1").say("小次郎", "x").move("a02")
        b = Sc("a02").label().bg("b1").move("a01")
        s, rep = self.convert(a, b, opts=E.Options(max_moves=30, loop_visits=10**6))
        self.assertIn("scenario-move cap", rep["endedBecause"])

    def test_ping_pong_between_menus_ends(self):
        a = Sc("a01").label().bg("b1").menu(("行く", [], "go"))
        a.mark("go").move("a02")
        b = Sc("a02").label().bg("b2").menu(("戻る", [], "back"))
        b.mark("back").move("a01")
        s, rep = self.convert(a, b)
        self.assertLess(rep["steps"], 2000)

    def test_hostile_input_does_not_crash(self):
        write_scenarios(self.root, Sc("a01").label().bg("b1").say("小次郎", "x").clear_game())
        p = os.path.join(self.root, "01_シナリオ", "小次郎編")
        with open(os.path.join(p, "a02.json"), "w", encoding="utf-8") as f:
            f.write("{not json")
        with open(os.path.join(p, "a03.json"), "w", encoding="utf-8") as f:
            json.dump({"instructions": [{"op": "zzz", "off": "0x0"}]}, f)
        with open(os.path.join(p, "a04.json"), "w", encoding="utf-8") as f:
            json.dump({"instructions": [{"op": "0x8003", "off": "0x0", "conditions": [[1]], "target": "0x99"},
                                        {"op": "0x8002", "off": "0x1", "items": [{"text": "x", "conditions": [["a", 1]], "target": "zz"}]},
                                        {"op": "0x8035", "off": "0x2", "text": 5, "voices": "no"}]}, f)
        for st in ("a02", "a03", "a04", "nope"):
            s, rep = E.convert(self.root, self.out, st, E.Options(), assets=FakeAssets())
            self.assertEqual(s["scenes"], [])
            self.assertTrue(rep["endedBecause"], st)
        # a malformed instruction inside a scenario that loads ends the path with a reason
        with open(os.path.join(p, "a05.json"), "w", encoding="utf-8") as f:
            json.dump({"instructions": [{"op": "0x8003", "off": "0x0", "conditions": [["x", 1]], "target": "0x0"}]}, f)
        s, rep = E.convert(self.root, self.out, "a05", E.Options(), assets=FakeAssets())
        self.assertIn("malformed", rep["endedBecause"])


def requires_in_order(script):
    """The route checks of script.validate for prerequisites: every one is a scene shown earlier."""
    errs, seen = [], set()
    ids = {sc["id"] for sc in script["scenes"]}
    by_id = {sc["id"]: sc for sc in script["scenes"]}
    for sid in script["route"]:
        for r in by_id[sid].get("requires", []):
            if r not in ids or r not in seen:
                errs.append(f"{sid} requires {r}")
        seen.add(sid)
    return errs


class StoryTime(Base):
    def day_movie(self, sc, name):
        return sc.add(0x8046, asset={"type": "演出ムービー", "name": name})

    def test_day_movies_and_date_screens_move_the_day(self):
        a = Sc("a01").label().bg("b1").say("小次郎", "前")
        self.day_movie(a, "1203")
        a.say("小次郎", "三日")  # the same picture, but a new day: a new scene
        a.bg("b2").say("小次郎", "三日の二")
        a.bg("1205md").bg("b2").say("小次郎", "五日")  # a date screen
        self.day_movie(a, "1204")  # earlier than today: time does not run back
        a.say("小次郎", "まだ五日")
        self.day_movie(a, "scene01").add(0x8008, asset={"name": "1299"})  # not a day
        a.clear_game()
        s, rep = self.convert(a)
        self.assertEqual([sc["when"] for sc in s["scenes"]], ["D00-0001", "D03-0001", "D03-0002", "D05-0001"])
        self.assertEqual([[ln["text"] for ln in sc["lines"]] for sc in s["scenes"]], [["前"], ["三日"], ["三日の二"], ["五日", "まだ五日"]])
        whens = [sc["when"] for sc in s["scenes"]]
        self.assertEqual(whens, sorted(whens))
        self.assertEqual([(m["name"], m["applied"]) for m in rep["dateMarkers"]], [("1203", True), ("1205md", True), ("1204", False)])
        self.assertEqual([(d["label"], d["lines"]) for d in rep["days"]], [("序章", 1), ("12月3日", 2), ("12月5日", 2)])

    def test_a_sound_before_the_day_changes_stays_on_the_day_before(self):
        a = Sc("a01").label().bg("b1").say("小次郎", "1").add(0x800B, asset={"type": "効果音", "name": "se01"})
        self.day_movie(a, "1203")
        a.say("小次郎", "2").clear_game()
        s, _ = self.convert(a)
        self.assertEqual(s["scenes"][0]["lines"][-1].get("sfx"), "se/se01.wav")
        self.assertNotIn("sfx", s["scenes"][1]["lines"][0])

    def test_where_the_sides_meet_requires_the_last_scene_before_it(self):
        a = Sc("a01").label().bg("b1").say("小次郎", "1").bg("b2").say("小次郎", "2").move("ab01")
        ab = Sc("ab01").bg("b2").say("小次郎", "合流").bg("b3").say("まりな", "次").move("c02")
        c = Sc("c02").bg("b3").say("まりな", "終").clear_game()
        s, rep = self.convert(a, ab, c)
        ids = [sc["id"] for sc in s["scenes"]]
        self.assertEqual(len(ids), 4)  # the same picture across into ab01 is still a new scene
        self.assertEqual(s["scenes"][2]["requires"], [ids[1]])
        self.assertTrue(all("requires" not in sc for k, sc in enumerate(s["scenes"]) if k != 2))
        self.assertEqual(rep["finale"], {"firstScene": ids[2], "requires": ids[1]})
        self.assertEqual(requires_in_order(s), [])

    def test_a_path_that_starts_where_the_sides_meet_requires_nothing(self):
        ab = Sc("ab01").bg("b1").say("小次郎", "合流").clear_game()
        s, rep = self.convert(ab, start="ab01")
        self.assertNotIn("requires", s["scenes"][0])
        self.assertIsNone(rep["finale"]["requires"])

    def test_scene_days_tolerates_anything(self):
        scenes = [{"when": "D04-0001"}, {}, {"when": 7}, {"when": "later"}, {"when": "D12-x"}, "not a scene", {"when": "D9999999-1"}]
        self.assertEqual(E.scene_days(scenes), [4, 4, 4, 4, 12, 12, 12])


def day_movie(sc, name):
    return sc.add(0x8046, asset={"type": "演出ムービー", "name": name})


class DaysPolicy(Base):
    """The "days" policy (the default): the story days in order, walking, the other side."""

    def texts(self, s):
        return [x["text"] for x in self.lines(s)]

    @staticmethod
    def two_places():
        # the harbour has more that is new, but it is the next day; the house is today's
        a = Sc("a01").label().iff([(300, 1), (301, 1)], "m").say("小次郎", "終").clear_game()
        a.mark("m").bg("b1").menu(("港", [], "x"), ("家", [], "y"))
        a.mark("x").move("a02")
        a.mark("y").move("a03")
        b = Sc("a02").label()
        day_movie(b, "1204").bg("b2").say("小次郎", "p1").say("小次郎", "p2").say("小次郎", "p3").flag(300).move("a01")
        c = Sc("a03").label().bg("b3").say("小次郎", "h1").flag(301).move("a01")
        return a, b, c

    def test_an_entry_that_shows_a_later_day_comes_last(self):
        s, rep = self.convert(*self.two_places(), opts=E.Options(policy="explore"))
        self.assertEqual(self.texts(s), ["p1", "p2", "p3", "h1", "終"])  # most new first
        s, rep = self.convert(*self.two_places())
        self.assertEqual(rep["policy"], "days")
        self.assertEqual(self.texts(s), ["h1", "p1", "p2", "p3", "終"])
        self.assertEqual([(m["name"], m["applied"], m["afterLines"]) for m in rep["dateMarkers"]], [("1204", True, 1)])
        self.assertIn("later day", [m["how"] for m in rep["menuChoices"]])
        whens = [sc["when"] for sc in s["scenes"]]
        self.assertEqual(whens, sorted(whens))

    @staticmethod
    def corridor():
        # a01 - a02 - a03 in a row; what is new is at the far end, behind a flag
        a = Sc("a01").label().bg("b1").menu(("出る", [], "o"))
        a.mark("o").move("a02")
        b = Sc("a02").label().bg("b2").menu(("奥へ", [], "o"), ("戻る", [], "h"))
        b.mark("o").move("a03")
        b.mark("h").move("a01")
        c = Sc("a03").label().iff([(300, 1)], "x").say("小次郎", "far").clear_game()
        c.mark("x").bg("b3").menu(("戻る", [], "h"))
        c.mark("h").move("a02")
        return a, b, c

    def stuck_at_a01(self, flags):
        """An interpreter at a01's menu that has run everything there is except the far line."""
        a, b, c = self.corridor()
        write_scenarios(self.root, a, b, c)
        store = E.ScenarioStore(E.find_scenario_folder(self.root))
        it = E.Interp(store, E.Options())
        for name in ("a01", "a02", "a03"):
            sc = store.get(name)
            for k in range(sc.n):
                if not (name == "a03" and sc.ops[k] == 0x8035):
                    it.visits[(name, k)] = 1
        for f, v in flags.items():
            it.set_flag(f, v)
        it.start_name = "a01"
        self.assertTrue(it.enter("a01"))
        sc = it.scn
        pc = next(k for k in range(sc.n) if sc.ops[k] == 0x8002)
        it.pc = pc
        it.stack = [E.Frame(sc, pc)]
        return it, pc

    def test_the_road_search_finds_the_nearest_new_thing(self):
        it, pc = self.stuck_at_a01({300: 1})
        b02 = it.store.get("a02")
        road = it.find_road()
        self.assertEqual(road, [("a01", pc, 0), ("a02", next(k for k in range(b02.n) if b02.ops[k] == 0x8002), 0)])
        self.assertEqual(it.road_later, 0)
        it, pc = self.stuck_at_a01({})
        self.assertIsNone(it.find_road())  # behind the flag: nothing to walk to
        self.assertTrue(it.road_failed())  # until a flag changes
        it.set_flag(300, 1)
        self.assertFalse(it.road_failed())

    def test_a_road_to_a_later_day_is_marked(self):
        a, b, _ = self.corridor()
        c = Sc("a03").label().bg("b3")
        day_movie(c, "1204").say("小次郎", "next day").clear_game()
        write_scenarios(self.root, a, b, c)
        store = E.ScenarioStore(E.find_scenario_folder(self.root))
        it = E.Interp(store, E.Options())
        it.day = 3
        self.assertTrue(it.enter("a01"))
        it.pc = next(k for k in range(it.scn.n) if it.scn.ops[k] == 0x8002)
        it.stack = [E.Frame(it.scn, it.pc)]
        self.assertEqual(len(it.find_road()), 2)
        self.assertEqual(it.road_later, 4)

    @staticmethod
    def long_corridor(n=7):
        """a01 - a02 - ... - a0n. The far end opens 見る in a01 (F301); after looking (F302) the
        far end has something new, too far away for a look-ahead to see."""
        a = Sc("a01").label().iff([(303, 1)], "m").bg("b1").menu(("寝る", [], "z"))
        a.mark("z").say("小次郎", "zzz").clear_game()
        a.mark("m").bg("b1").menu(("見る", [(301, 1)], "l"), ("出る", [], "o"))
        a.mark("l").say("小次郎", "look").flag(302).back()
        a.mark("o").move("a02")
        scs = [a]
        for k in range(2, n):
            c = Sc(f"a0{k}").label().bg(f"b{k}").menu(("奥へ", [], "o"), ("戻る", [], "h"))
            c.mark("o").move(f"a0{k + 1}")
            c.mark("h").move(f"a0{k - 1}")
            scs.append(c)
        end = Sc(f"a0{n}").label().iff([(302, 1)], "x").say("小次郎", "far").flag(303).move("a01")
        end.mark("x").bg("b9").flag(301).menu(("戻る", [], "h"))
        end.mark("h").move(f"a0{n - 1}")
        scs.append(end)
        return scs

    def test_walking_to_what_is_new(self):
        s, rep = self.convert(*self.long_corridor(), opts=E.Options(policy="explore"))
        self.assertGreaterEqual(rep["teleports"], 1)  # explore jumps there
        s, rep = self.convert(*self.long_corridor())
        self.assertEqual(self.texts(s), ["look", "far", "zzz"])
        self.assertEqual(rep["teleports"], 0)  # days walks there, through every place
        self.assertGreaterEqual(rep["walks"], 1)
        self.assertEqual(rep["walkList"][0]["steps"], 6)
        self.assertIn("walk", [m["how"] for m in rep["menuChoices"]])

    def test_a_teleport_does_not_go_back_in_time(self):
        a = Sc("a01").label()
        day_movie(a, "1203").bg("b1").say("小次郎", "s").menu(("見る", [], "l"))
        a.mark("l").say("小次郎", "look").back()
        b = Sc("a02").label()
        day_movie(b, "1203").say("小次郎", "again").clear_game()  # the 3rd once more
        c = Sc("a03").label().bg("b3").say("小次郎", "fresh").clear_game()
        s, rep = self.convert(a, b, c, opts=E.Options(policy="explore"))
        self.assertEqual(self.texts(s), ["s", "look", "again"])
        s, rep = self.convert(a, b, c)
        self.assertEqual(self.texts(s), ["s", "look", "fresh"])
        self.assertEqual(rep["teleportList"][0]["to"], "a03")

    @staticmethod
    def sides():
        # Kojirou waits for the shared flag F14, which only Marina's side sets
        a = Sc("a01").label().bg("b1").say("小次郎", "a1").menu(("待つ", [(14, 1)], "w"), ("見る", [], "l"))
        a.mark("l").say("小次郎", "look").back()
        a.mark("w").say("小次郎", "a-after").clear_game()
        b = Sc("b01").label().bg("b9").say("まりな", "b1").flag(14).end()
        return a, b

    def test_the_other_side_is_played_alongside(self):
        s, rep = self.convert(*self.sides())
        self.assertEqual(self.texts(s), ["a1", "look", "a-after"])  # nothing of Marina's is written
        self.assertEqual(rep["endedBecause"], "game clear (the ending was reached)")
        self.assertEqual(rep["zapping"]["start"], "b01")
        self.assertEqual(rep["zapping"]["lines"], 1)
        s, rep = self.convert(*self.sides(), opts=E.Options(zapping=False))
        self.assertEqual(self.texts(s), ["a1", "look"])
        self.assertIsNone(rep["zapping"])
        self.assertIn("menu exhausted", rep["endedBecause"])

    def test_the_other_side_stops_at_the_end_of_the_main_sides_day(self):
        # Marina's side would go on to the 4th; Kojirou is on the 3rd, waiting for F14
        a = Sc("a01").label()
        day_movie(a, "1203").bg("b1").say("小次郎", "a1").menu(("待つ", [(14, 1)], "w"), ("見る", [], "l"))
        a.mark("l").say("小次郎", "look").back()
        a.mark("w").say("小次郎", "a-after").clear_game()
        b = Sc("b01").label().bg("b9").say("まりな", "b1").flag(14).menu(("翌日", [], "n"))
        b.mark("n")
        day_movie(b, "1204").say("まりな", "b2").flag(15).end()
        s, rep = self.convert(a, b)
        self.assertEqual(self.texts(s), ["a1", "look", "a-after"])
        self.assertEqual(rep["zapping"]["day"], 0)  # never went on to the 4th
        self.assertEqual(rep["dateMarkers"][0]["name"], "1203")
        self.assertEqual(rep["zapping"]["lines"], 1)
        self.assertGreaterEqual(rep["zapping"]["pauses"], 1)

    def test_a_missing_other_side_is_harmless(self):
        a, _ = self.sides()
        s, rep = self.convert(a)
        self.assertEqual(self.texts(s), ["a1", "look"])
        self.assertIn("not available", rep["zapping"]["endedBecause"])

    @staticmethod
    def exchange():
        """The two terminals of the 6th: Kojirou's a01 waits for F7, which Marina's b01 sets
        only after Kojirou's F6, which he sets elsewhere (a03) after Marina's side has looked
        at everything in b01. Marina's b02 (in reach, behind a flag) has something new too, and
        no way back."""
        a = Sc("a01").label().bg("b1").say("小次郎", "a1").menu(("見る", [], "l"), ("解除", [(7, 1)], "u"), ("奥", [(99, 1)], "o"))
        a.mark("l").say("小次郎", "look").back()
        a.mark("u").say("小次郎", "unlocked").clear_game()
        a.mark("o").move("a03")
        c = Sc("a03").label().bg("b3").say("小次郎", "f6").flag(6).move("a01")
        b = Sc("b01").label().bg("b8").say("まりな", "b1").menu(("見る", [], "l"), ("解除", [(6, 1)], "u"), ("外", [(98, 1)], "o"))
        b.mark("l").say("まりな", "b-look").back()
        b.mark("u").say("まりな", "b-unlocked").flag(7).back()
        b.mark("o").move("b02")
        d = Sc("b02").label().bg("b9").say("まりな", "elsewhere").end()
        return a, c, b, d

    def test_the_other_side_waits_for_the_main_side(self):
        s, rep = self.convert(*self.exchange())
        self.assertEqual(self.texts(s), ["a1", "look", "f6", "a1", "unlocked"])
        self.assertEqual(rep["endedBecause"], "game clear (the ending was reached)")
        self.assertGreaterEqual(rep["zapping"]["waits"], 1)
        self.assertEqual(rep["zapping"]["teleports"], 0)  # it stayed at its terminal
        old = E.WAITS
        E.WAITS = 0  # never waiting: Marina's side moves on to b02, and comes back by teleport
        try:
            s, rep = self.convert(*self.exchange())
        finally:
            E.WAITS = old
        self.assertGreaterEqual(rep["zapping"]["teleports"], 1)

    def test_the_main_side_joins_where_the_other_side_waits(self):
        # Kojirou has nothing left (a02 has, but nothing leads there); Marina's side comes to
        # ab01, where both sides meet: Kojirou joins it instead of teleporting
        a = Sc("a01").label().bg("b1").say("小次郎", "a1").menu(("見る", [], "l"))
        a.mark("l").say("小次郎", "look").back()
        b = Sc("a02").label().bg("b2").say("小次郎", "land").end()
        m = Sc("b01").label().bg("b8").say("まりな", "b1").move("ab01")
        j = Sc("ab01").label().bg("b7").say("小次郎", "together").clear_game()
        s, rep = self.convert(a, b, m, j)
        self.assertEqual(self.texts(s), ["a1", "look", "together"])
        self.assertEqual(rep["teleports"], 0)
        self.assertEqual([(x["from"], x["to"]) for x in rep["joins"]], [("a01", "ab01")])
        self.assertEqual(rep["zapping"]["joinedAt"], "ab01")
        self.assertEqual(rep["zapping"]["lines"], 1)  # Marina's side stopped at ab01's door

    def test_a_teleport_goes_where_a_player_could_get_to(self):
        # on the ship (a02) nothing leads back to land (a03); the deck (a04) is behind a flag
        a = Sc("a01").label().bg("b1").menu(("船へ", [], "s"))
        a.mark("s").move("a02")
        b = Sc("a02").label().bg("b2").say("小次郎", "ship").menu(("見る", [], "l"), ("甲板", [(99, 1)], "d"))
        b.mark("l").say("小次郎", "look").back()
        b.mark("d").move("a04")
        c = Sc("a03").label().bg("b3").say("小次郎", "land").end()
        d = Sc("a04").label().bg("b4").say("小次郎", "deck").clear_game()
        s, rep = self.convert(a, b, c, d, opts=E.Options(policy="explore"))
        self.assertEqual(rep["teleportList"][0]["to"], "a03")  # number order
        s, rep = self.convert(a, b, c, d)
        self.assertEqual(self.texts(s), ["ship", "look", "deck"])
        self.assertEqual([(t["from"], t["to"]) for t in rep["teleportList"]], [("a02", "a04")])
        self.assertNotIn("unreachable", rep["teleportList"][0])
        store = E.ScenarioStore(E.find_scenario_folder(self.root))
        self.assertEqual(store.reachable("a02"), {"a02", "a04"})
        self.assertEqual(store.reachable("a01"), {"a01", "a02", "a04"})
        self.assertEqual(store.reachable("nowhere"), {"nowhere"})

    @staticmethod
    def keypad():
        """a01's terminal: a keypad with A and B whose code is B A B. The keys pressed so far are
        flags (F400, F401), and so is which were right (F410, F411); every key comes back to
        the keypad."""
        a = Sc("a01").label().bg("b1").menu(("押す", [], "pad"))
        a.mark("pad").menu(("Ａ", [], "ka"), ("Ｂ", [], "kb"))
        a.mark("ka").iff([(401, 1)], "a2").flag(400, 0).flag(401, 0).flag(410, 0).flag(411, 0).endif()
        a.mark("a2").iff([(400, 1), (410, 1)], "a3").flag(401, 1).flag(411, 1).endif()
        a.mark("a3").iff([(400, 1)], "a4").flag(401, 1).endif()
        a.mark("a4").iff([(0, 0)], "aend").flag(400, 1).endif()
        a.mark("aend").back()
        a.mark("kb").iff([(401, 1), (411, 1)], "b2").say("小次郎", "open").flag(420).endif()
        a.mark("b2").iff([(401, 1)], "b3").flag(400, 0).flag(401, 0).flag(410, 0).flag(411, 0).endif()
        a.mark("b3").iff([(400, 1)], "b4").flag(401, 1).endif()
        a.mark("b4").iff([(0, 0)], "bend").flag(400, 1).flag(410, 1).endif()
        a.mark("bend").back()
        return a

    def test_the_road_search_presses_a_code(self):
        write_scenarios(self.root, self.keypad())
        store = E.ScenarioStore(E.find_scenario_folder(self.root))
        it = E.Interp(store, E.Options())
        sc = store.get("a01")
        for k in range(sc.n):
            if sc.ops[k] != 0x8035:
                it.visits[("a01", k)] = 1  # everything has run but the door opening
        it.start_name = "a01"
        self.assertTrue(it.enter("a01"))
        menus = [k for k in range(sc.n) if sc.ops[k] == 0x8002]
        it.pc = menus[0]
        it.stack = [E.Frame(sc, it.pc)]
        self.assertIsNone(it.search_road(False))  # by places only, the keypad is one state
        road = it.find_road()
        self.assertEqual(road, [("a01", menus[0], 0), ("a01", menus[1], 1), ("a01", menus[1], 0), ("a01", menus[1], 1)])
        self.assertFalse(it.road_failed())
        self.assertEqual(it.flags, {})  # the search does not touch the path's flags


class Episodes(unittest.TestCase):
    @staticmethod
    def script():
        def scene(sid, when, n, speaker="koj", **kw):
            return {"id": sid, "title": "a01 x", "when": when, "pov": "koj",
                    "lines": [{"id": f"{sid}-{k:03d}", "speaker": speaker, "text": "t"} for k in range(n)], **kw}
        return {
            "version": 1, "title": "T", "language": "Japanese",
            "characters": {"koj": {"name": "小次郎"}, "mar": {"name": "まりな"}, "kyo": {"name": "恭子", "expressions": {"a": {"image": "x.png"}}}},
            "scenes": [
                scene("s1", "D00-0001", 2),
                scene("s2", "D03-0001", 3, "kyo"),
                scene("s3", "D03-0002", 4, requires=["s2"]),
                scene("s4", "D04-0001", 1, requires=["s3"]),
                scene("s5", "D04-0002", 9),
            ],
            "route": ["s1", "s2", "s3", "s4", "s5"],
        }

    def test_one_episode_per_day(self):
        eps = E.split_episodes(self.script(), label="小次郎編")
        self.assertEqual([e["script"]["title"] for e in eps], ["小次郎編 第 1 話 序章", "小次郎編 第 2 話 12月3日", "小次郎編 第 3 話 12月4日"])
        self.assertEqual([e["script"]["route"] for e in eps], [["s1"], ["s2", "s3"], ["s4", "s5"]])
        self.assertEqual([e["lines"] for e in eps], [2, 7, 10])
        # only the characters it uses, whole
        self.assertEqual(sorted(eps[1]["script"]["characters"]), ["koj", "kyo"])
        self.assertEqual(eps[1]["script"]["characters"]["kyo"]["expressions"], {"a": {"image": "x.png"}})
        self.assertEqual(sorted(eps[0]["script"]["characters"]), ["koj"])
        # a prerequisite inside the episode stays, one in an earlier episode is dropped
        self.assertEqual(eps[1]["script"]["scenes"][1]["requires"], ["s2"])
        self.assertNotIn("requires", eps[2]["script"]["scenes"][0])
        for e in eps:
            self.assertEqual(validate_by_hand(e["script"]), [])
            self.assertEqual(requires_in_order(e["script"]), [])

    def test_the_source_is_not_changed(self):
        s = self.script()
        before = json.dumps(s, sort_keys=True)
        E.split_episodes(s, 2)
        self.assertEqual(json.dumps(s, sort_keys=True), before)

    def test_a_line_cap_cuts_a_day_between_scenes(self):
        eps = E.split_episodes(self.script(), max_lines=5)
        self.assertEqual([e["script"]["route"] for e in eps], [["s1"], ["s2"], ["s3"], ["s4"], ["s5"]])
        self.assertEqual([e["script"]["title"] for e in eps][1:3], ["第 2 話 12月3日（1）", "第 3 話 12月3日（2）"])
        self.assertEqual(eps[4]["lines"], 9)  # a scene longer than the cap is an episode of its own
        eps = E.split_episodes(self.script(), max_lines=7)
        self.assertEqual([e["script"]["route"] for e in eps], [["s1"], ["s2", "s3"], ["s4"], ["s5"]])
        with self.assertRaises(ValueError):
            E.split_episodes(self.script(), max_lines=0)

    def test_route_order_wins_over_file_order(self):
        s = self.script()
        s["route"] = ["s2", "s3", "s1"]  # s4 and s5 are not shown
        eps = E.split_episodes(s)
        self.assertEqual([e["script"]["route"] for e in eps], [["s2", "s3"], ["s1"]])

    def test_episode_file_names(self):
        self.assertEqual(E.episode_path(os.path.join("o", "kojirou.fcscript.json"), 3, 12), os.path.join("o", "kojirou.ep03.fcscript.json"))
        self.assertEqual(E.episode_path("k.json", 7, 150), "k.ep007.fcscript.json")


class CommandLine(Base):
    def test_cli_writes_episodes(self):
        a = Sc("a01").label(0).bg("b1").say("小次郎", "1")
        a.add(0x8046, asset={"name": "1203"}).say("小次郎", "2").bg("b2").say("小次郎", "3").bg("1204md").bg("b3").say("恭子", "4").clear_game()
        write_scenarios(self.root, a)
        out = os.path.join(self.root, "o", "k.fcscript.json")
        rep = os.path.join(self.root, "o", "r.json")
        cmd = [sys.executable, "-X", "utf8", os.path.join(HERE, "..", "eve_to_script.py"), "--root", self.root, "--route", "kojirou", "--out", out, "--report", rep]
        r = subprocess.run(cmd + ["--episodes"], capture_output=True, text=True, encoding="utf-8")
        self.assertEqual(r.returncode, 0, r.stderr)
        names = sorted(f for f in os.listdir(os.path.join(self.root, "o")) if ".ep" in f)
        self.assertEqual(names, ["k.ep01.fcscript.json", "k.ep02.fcscript.json", "k.ep03.fcscript.json"])
        with open(os.path.join(self.root, "o", "k.ep03.fcscript.json"), encoding="utf-8") as f:
            ep = json.load(f)
        self.assertEqual(ep["title"], "小次郎編 第 3 話 12月4日")
        self.assertEqual(sorted(ep["characters"]), ["koj", "kyo"])  # koj is the viewpoint
        with open(out, encoding="utf-8") as f:
            self.assertEqual(len(json.load(f)["scenes"]), 4)  # the whole script is still written
        with open(rep, encoding="utf-8") as f:
            self.assertEqual([e["lines"] for e in json.load(f)["episodes"]], [1, 2, 1])
        r = subprocess.run(cmd + ["--episode-lines", "1"], capture_output=True, text=True, encoding="utf-8")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(json.loads(r.stdout.strip().splitlines()[-1])["episodes"], 4)
        r = subprocess.run(cmd + ["--episode-lines", "0"], capture_output=True, text=True, encoding="utf-8")
        self.assertNotEqual(r.returncode, 0)

    def test_cli_writes_script_and_report(self):
        a = Sc("a01").label(0).bg("b1").say("小次郎", "こんにちは。").clear_game()
        write_scenarios(self.root, a)
        out = os.path.join(self.root, "o", "s.fcscript.json")
        rep = os.path.join(self.root, "o", "r.json")
        r = subprocess.run([sys.executable, "-X", "utf8", os.path.join(HERE, "..", "eve_to_script.py"), "--root", self.root, "--route", "kojirou", "--out", out, "--report", rep],
                           capture_output=True, text=True, encoding="utf-8")
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(out, encoding="utf-8") as f:
            script = json.load(f)
        self.assertEqual(self.lines(script)[0]["text"], "こんにちは。")
        with open(rep, encoding="utf-8") as f:
            self.assertEqual(json.load(f)["lines"], 1)
        choices = os.path.join(self.root, "c.json")
        with open(choices, "w") as f:
            f.write('{"not": "a list"}')
        r = subprocess.run([sys.executable, "-X", "utf8", os.path.join(HERE, "..", "eve_to_script.py"), "--root", self.root, "--out", out, "--choices", choices],
                           capture_output=True, text=True, encoding="utf-8")
        self.assertNotEqual(r.returncode, 0)


@unittest.skipUnless(os.path.isdir(os.path.join(REAL_ROOT, "01_シナリオ")), "the decoded dump is not here")
class RealData(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls._td = tempfile.TemporaryDirectory()
        cls.out = os.path.join(cls._td.name, "k.fcscript.json")
        cls.runs = {}

    @classmethod
    def tearDownClass(cls):
        cls._td.cleanup()

    def run_route(self, start, **kw):
        # a route takes some seconds: each one runs once (nothing here changes its result)
        key = (start, tuple(sorted(kw.items())))
        if key not in self.runs:
            self.runs[key] = E.convert(REAL_ROOT, self.out, start, E.Options(), **kw)
        return self.runs[key]

    def assert_days_in_order(self, rep, days):
        applied = [m["name"] for m in rep["dateMarkers"] if m["applied"]]
        self.assertEqual(applied, days)
        shown = [m["name"] for m in rep["dateMarkers"] if not m["name"].endswith("md")]
        self.assertEqual(shown, days)  # every day movie once (the ending's date screens do not count)
        self.assertEqual([d["label"] for d in rep["days"]], [f"12月{int(d[2:])}日" for d in days])

    def test_kojirou_route_reaches_the_ending_and_is_valid(self):
        script, rep = self.run_route("kojirou")
        self.assertEqual(validate_by_hand(script), [])
        self.assertEqual(rep["endedBecause"], "game clear (the ending was reached)")
        self.assertEqual(rep["unknownOps"], {})
        self.assertGreater(rep["spokenLines"], 10000)
        self.assertGreater(rep["scenes"], 500)
        self.assertIn("koj", script["characters"])
        self.assertIn("yay", script["characters"])
        self.assertTrue(any(ln.get("kind") == "narration" for sc in script["scenes"] for ln in sc["lines"]))

    def test_kojirou_route_has_story_time_and_episodes(self):
        script, rep = self.run_route("kojirou")
        whens = [sc["when"] for sc in script["scenes"]]
        self.assertEqual(whens, sorted(whens))
        self.assertTrue(whens[0].startswith("D03-"))  # the day movie 1203 plays before the first line
        self.assertIsNotNone(rep["finale"]["requires"])
        self.assertEqual(requires_in_order(script), [])
        # the days come in the game's order, each day movie once
        self.assert_days_in_order(rep, ["1203", "1204", "1205", "1206", "1207", "1209"])
        self.assertEqual(rep["zapping"]["start"], "b01")
        # no teleport: the terminals of the 6th hand flags to each other, the keypad is pressed,
        # and on the 7th Kojirou stays on the ship and joins Marina's side at ab01
        self.assertEqual(rep["teleportList"], [])
        self.assertEqual([j["to"] for j in rep["joins"]], ["ab01"])
        self.assertEqual(rep["zapping"]["teleports"], 0)
        eps = E.split_episodes(script, 600)
        self.assertEqual(sum(e["lines"] for e in eps), rep["lines"])
        for e in eps:
            self.assertEqual(validate_by_hand(e["script"]), [])
            self.assertEqual(requires_in_order(e["script"]), [])

    def test_marina_route_reaches_the_ending(self):
        script, rep = self.run_route("marina")
        self.assertEqual(validate_by_hand(script), [])
        self.assertEqual(rep["endedBecause"], "game clear (the ending was reached)")
        self.assertIn("mar", script["characters"])
        self.assertEqual(script["scenes"][0]["pov"], "mar")
        self.assertEqual(rep["unknownOps"], {})
        self.assert_days_in_order(rep, ["1202", "1203", "1204", "1205", "1206", "1207", "1209"])
        self.assertEqual(rep["teleportList"], [])
        whens = [sc["when"] for sc in script["scenes"]]
        self.assertEqual(whens, sorted(whens))

    def test_a001_alone(self):
        script, rep = self.run_route("a001:1")
        self.assertEqual(validate_by_hand(script), [])
        self.assertGreater(rep["lines"], 0)

    def test_the_repo_validator_accepts_the_output(self):
        exe = os.path.join(REPO, "target", "debug", "filmcraft-cli.exe")
        if not os.path.isfile(exe):
            self.skipTest("filmcraft-cli is not built")
        script, _ = self.run_route("a001:1")
        with open(self.out, "w", encoding="utf-8") as f:
            json.dump(script, f, ensure_ascii=False)
        r = subprocess.run([exe, "--compact", "exec", "script.validate", "path=" + self.out.replace("\\", "/")], capture_output=True, text=True, encoding="utf-8")
        if r.returncode != 0:
            self.skipTest("the CLI could not run script.validate: " + r.stderr[:200])
        res = json.loads(r.stdout)
        self.assertTrue(res["ok"], res.get("problems", [])[:3])
        self.assertEqual(res["errors"], 0)

    def test_every_scenario_loads(self):
        store = E.ScenarioStore(E.find_scenario_folder(REAL_ROOT))
        self.assertEqual(len(store.files), 87)
        for name in store.files:
            self.assertIsNotNone(store.get(name), name)
        self.assertEqual(store.errors, [])


if __name__ == "__main__":
    unittest.main()
