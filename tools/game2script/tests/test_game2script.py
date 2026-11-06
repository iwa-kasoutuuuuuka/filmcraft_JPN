"""Tests for game2script.py: `python -m unittest discover tools/game2script/tests`."""

import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))

import game2script  # noqa: E402

SCENARIO = """// a comment
■ s01 屋上
@pov marina
@when 1998-04-12T10:00
@bg roof
@bgm calm
夜だった。
@face まりな smile
[v001]まりな「待って、[漢字/かんじ]が読めない。」
まりな（どうして…）
@se door
@wait 0.5
小次郎「俺に任せろ。」
@bg room
[v003]まりな「着いた。」
■ s02 朝
@requires s01
@bg roof
朝になった。
"""


def touch(path):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(b"x")


class Game(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = self.tmp.name
        # a Shift-JIS scenario, as old games have
        os.makedirs(os.path.join(self.dir, "scenario"))
        with open(os.path.join(self.dir, "scenario", "ch1.txt"), "wb") as f:
            f.write(SCENARIO.encode("cp932"))
        for f in ["bg/roof.png", "bg/room.png", "bgm/calm.ogg", "se/door.wav", "voice/v001.wav", "voice/v003.wav", "chara/marina_smile.png", "chara/marina_smile_open.png", "chara/marina_smile_blink.png", "chara/marina_smile_a.png", "chara/marina_smile_i.png", "chara/marina_normal.png", "chara/kojiro_normal.png"]:
            touch(os.path.join(self.dir, f))
        self.out = os.path.join(self.dir, "out", "story.fcscript.json")

    def tearDown(self):
        self.tmp.cleanup()

    def convert(self, profile="plain", extra=()):
        rc = game2script.main(["--input", os.path.join(self.dir, "scenario"), "--assets", self.dir, "--out", self.out, "--profile", profile, *extra])
        self.assertEqual(rc, 0)
        with open(self.out, encoding="utf-8") as f:
            return json.load(f)

    def profile(self, **over):
        with open(os.path.join(os.path.dirname(HERE), "profiles", "plain.json"), encoding="utf-8") as f:
            p = json.load(f)
        p.update(over)
        path = os.path.join(self.dir, "profile.json")
        with open(path, "w", encoding="utf-8") as f:
            json.dump(p, f, ensure_ascii=False)
        return path

    def test_scenes_lines_and_commands(self):
        s = self.convert()
        ids = [x["id"] for x in s["scenes"]]
        # the background change inside s01 starts a part; the header starts s02
        self.assertEqual(ids, ["s01", "s01-p2", "s02"])
        self.assertEqual(s["route"], ids)
        a, b, c = s["scenes"]
        self.assertEqual(a["title"], "屋上")
        self.assertEqual((a["background"], a["bgm"]), ("../bg/roof.png", "../bgm/calm.ogg"))
        self.assertEqual((a["when"], b["when"]), ("1998-04-12T10:00", "1998-04-12T10:00"))
        self.assertEqual(b["background"], "../bg/room.png")
        self.assertEqual(b["bgm"], "../bgm/calm.ogg", "music carries across a part")
        self.assertEqual(b["requires"], ["s01"])
        self.assertEqual(c["requires"], ["s01"])
        kinds = [(x["kind"], x.get("speaker")) for x in a["lines"]]
        self.assertEqual(kinds, [("narration", "narrator"), ("dialogue", "c1"), ("monologue", "c1"), ("direction", None), ("dialogue", "c2")])
        self.assertEqual(a["lines"][1]["text"], "待って、｜漢字《かんじ》が読めない。")
        self.assertEqual(a["lines"][1]["voiceFile"], "../voice/v001.wav")
        self.assertEqual(a["lines"][3]["sfx"], "../se/door.wav")
        # @wait goes on the line before it (the sound effect), not on the next
        self.assertEqual(a["lines"][3]["pauseAfter"], 0.5)
        self.assertEqual([x["id"] for x in a["lines"]], ["s01-001", "s01-002", "s01-003", "s01-004", "s01-005"])
        self.assertEqual(s["characters"]["narrator"]["name"], "語り")

    def test_named_characters_and_their_pictures(self):
        prof = self.profile(characters={"まりな": {"id": "marina", "name": "まりな", "voice": {"speaker": "Ono_Anna"}}, "小次郎": {"id": "kojiro"}})
        s = self.convert(prof)
        marina = s["characters"]["marina"]
        self.assertEqual(marina["voice"], {"speaker": "Ono_Anna"})
        self.assertEqual(marina["expressions"]["smile"], {"image": "../chara/marina_smile.png", "openImage": "../chara/marina_smile_open.png", "vowelImages": {"a": "../chara/marina_smile_a.png", "i": "../chara/marina_smile_i.png"}})
        self.assertEqual(marina["expressions"]["smile_blink"], {"image": "../chara/marina_smile_blink.png"})
        self.assertEqual(marina["defaultExpression"], "normal")
        self.assertIn("kojiro", s["characters"])
        # @face set smile for marina from then on
        lines = s["scenes"][0]["lines"]
        self.assertEqual(lines[1]["speaker"], "marina")
        self.assertEqual(lines[1]["expression"], "smile")
        self.assertNotIn("expression", lines[4])
        self.assertEqual(s["scenes"][0]["pov"], "marina")

    def test_the_report_lists_what_the_profile_missed(self):
        # no narration rule and no voice folder: unmatched lines and missing files are reported
        prof = self.profile(rules=[{"match": "^(?:\\[(?P<voice>[A-Za-z0-9_]+)\\])?(?P<speaker>[^「『（(\\[\\]]+?)[ \\u3000]*「(?P<text>.*)」$", "kind": "dialogue"}], assets={"voice": "nowhere"})
        rep = os.path.join(self.dir, "report.json")
        s = self.convert(prof, ["--report", rep])
        with open(rep, encoding="utf-8") as f:
            r = json.load(f)
        self.assertIn("夜だった。", r["unmatched"])
        self.assertIn("まりな（どうして…）", r["unmatched"])
        self.assertEqual(sorted(r["missing"]["voice"]), ["v001", "v003"])
        self.assertEqual(r["unknownSpeakers"], ["marina", "まりな", "小次郎"])  # the name in @pov is a different name from the one in the dialogue
        self.assertNotIn("voiceFile", s["scenes"][0]["lines"][0])

    def test_a_scene_without_a_header_gets_one_and_unknown_files_are_not_invented(self):
        with open(os.path.join(self.dir, "scenario", "ch1.txt"), "wb") as f:
            f.write("@bg nowhere\n誰か「はい」\n".encode("utf-8"))
        s = self.convert()
        self.assertEqual([x["id"] for x in s["scenes"]], ["s001"])
        self.assertNotIn("background", s["scenes"][0])

    def test_files_are_read_in_natural_order_and_utf8_works(self):
        for n, body in (("ch10.txt", "十「あ」\n"), ("ch2.txt", "二「い」\n")):
            with open(os.path.join(self.dir, "scenario", n), "w", encoding="utf-8") as f:
                f.write(body)
        os.remove(os.path.join(self.dir, "scenario", "ch1.txt"))
        s = self.convert()
        texts = [ln["text"] for sc in s["scenes"] for ln in sc["lines"]]
        self.assertEqual(texts, ["い", "あ"])

    def test_a_profile_can_extend_another_and_change_only_what_differs(self):
        path = os.path.join(self.dir, "mine.json")
        with open(path, "w", encoding="utf-8") as f:
            json.dump({"extends": "plain", "characters": {"まりな": {"id": "marina"}}, "assets": {"voice": "nowhere"}}, f, ensure_ascii=False)
        s = self.convert(path)
        self.assertIn("marina", s["characters"])
        self.assertNotIn("voiceFile", s["scenes"][0]["lines"][1], "the voice folder was changed")
        self.assertEqual(s["scenes"][0]["lines"][0]["kind"], "narration", "the rules came from plain")
        loop = os.path.join(self.dir, "loop.json")
        with open(loop, "w", encoding="utf-8") as f:
            json.dump({"extends": "loop.json"}, f)
        with self.assertRaises(SystemExit):
            game2script.load_profile(loop)

    def test_bad_input_is_refused_with_a_message(self):
        with self.assertRaises(SystemExit):
            game2script.main(["--input", os.path.join(self.dir, "empty"), "--out", self.out])
        bad = self.profile(rules=[{"match": "(", "kind": "dialogue"}])
        with self.assertRaises(SystemExit) as cm:
            game2script.main(["--input", os.path.join(self.dir, "scenario"), "--assets", self.dir, "--out", self.out, "--profile", bad])
        self.assertIn("bad regular expression", str(cm.exception))
        with self.assertRaises(SystemExit):
            game2script.main(["--input", os.path.join(self.dir, "scenario"), "--out", self.out, "--profile", "no-such-profile"])


if __name__ == "__main__":
    unittest.main()
