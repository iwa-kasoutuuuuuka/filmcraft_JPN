import argparse
import io
import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))

import onomatopoeia as on  # noqa: E402

SE = "../../DATA/03_音声/効果音/se_一般効果音/"
VOICE = "../../DATA/03_音声/ボイス/"


def script():
    lines = [
        {"id": "a", "kind": "narration", "speaker": "narrator", "text": "ガシャーン！", "voiceFile": SE + "se17.wav"},
        {"id": "b", "speaker": "koj", "text": "ぐぅ～。", "voiceFile": VOICE + "j_擬音/j302.wav"},
        {"id": "c", "kind": "narration", "speaker": "sfx", "text": "ズダダダッ！", "voiceFile": VOICE + "j_擬音/j303.wav"},
        {"id": "d", "kind": "narration", "speaker": "narrator", "text": "ジリッ‥‥。"},
        {"id": "e", "speaker": "koj", "text": "ギクッ！"},
        {"id": "f", "speaker": "aka", "text": "フン、だ。", "voiceFile": VOICE + "aka_茜/aka0194.wav"},
        {"id": "g", "speaker": "aka", "text": "エヘヘーン。", "voiceFile": VOICE + "aka_茜/aka0037.wav"},
        {"id": "h", "speaker": "koj", "text": "フン‥‥。"},
        {"id": "i", "speaker": "koj", "text": "ぐるぐる、ぐるぐる。"},
        {"id": "j", "speaker": "koj", "text": "（ガクッ‥‥。）"},
        {"id": "k", "kind": "narration", "speaker": "narrator", "text": "ガチャッ。", "voiceFile": SE + "se03.wav"},
        {"id": "l", "kind": "narration", "speaker": "narrator", "text": "ガチャッ！"},
        {"id": "m", "speaker": "aka", "text": "よくぞ気づいたっ！", "voiceFile": VOICE + "aka_茜/aka0195.wav", "sfx": SE + "se12.wav"},
        {"id": "n", "speaker": "koj", "text": "バカ！"},
        {"id": "o", "speaker": "koj", "text": "プリシア！"},
        {"id": "p", "speaker": "koj", "text": "ムカムカムカムカムカムカムカ‥‥。"},
        {"id": "q", "speaker": "koj", "text": "ドカバキグシャメキャボコ！"},
        {"id": "r", "kind": "direction", "text": "‥‥", "speaker": "koj"},
        {"id": "s", "speaker": "koj", "text": "なんだよ。"},
        {"id": "t", "kind": "narration", "speaker": "narrator", "text": "ピンポーン。", "voiceFile": SE + "se05.wav", "sfx": SE + "se99.wav"},
        {"id": "u", "speaker": "koj", "text": "ぐぅ。", "sfx": VOICE + "j_擬音/j302.wav"},
        {"id": "v", "kind": "narration", "speaker": "narrator", "text": "コンコン。"},
        {"id": "w", "kind": "narration", "speaker": "narrator", "text": "ガタッ。", "spoken": False},
    ]
    return {
        "version": 1,
        "characters": {"koj": {"name": "小次郎"}, "aka": {"name": "茜"}, "narrator": {"name": "語り"}, "sfx": {"name": "擬音"}, "pul": {"name": "プリシア"}},
        "scenes": [{"id": "s1", "lines": lines}],
    }


def opts(**kw):
    o = dict(max_kana=8, keep=set(), mark=set(), table=None)
    o.update(kw)
    return argparse.Namespace(**o)


def by_id(s):
    return {l["id"]: l for sc in s["scenes"] for l in sc["lines"]}


class Rules(unittest.TestCase):
    def setUp(self):
        self.s = script()
        self.r = on.process(self.s, opts(table={on.key("コンコン"): SE + "se40.wav"}))
        self.m = {m["id"]: m for m in self.r["marked"]}
        self.l = by_id(self.s)

    def test_game_sounds_used_as_voices_move_to_the_effects(self):
        for i, f in [("a", "se17.wav"), ("b", "j302.wav"), ("c", "j303.wav"), ("k", "se03.wav")]:
            self.assertEqual(self.m[i]["rule"], "clip")
            self.assertIs(self.l[i]["spoken"], False)
            self.assertNotIn("voiceFile", self.l[i])
            self.assertTrue(self.l[i]["sfx"].endswith(f), self.l[i])

    def test_a_line_with_its_own_sound_keeps_it_and_drops_the_clip(self):
        self.assertTrue(self.l["t"]["sfx"].endswith("se99.wav"))
        self.assertNotIn("voiceFile", self.l["t"])
        self.assertTrue(self.m["t"]["dropped"].endswith("se05.wav"))

    def test_katakana_sounds_without_a_clip_become_silent_or_borrow_one(self):
        for i in ["d", "e", "j"]:
            self.assertIs(self.l[i]["spoken"], False, i)
            self.assertNotIn("sfx", self.l[i], i)
        self.assertEqual(self.m["j"]["rule"], "bracket")
        # the same word with a sound elsewhere lends it
        self.assertTrue(self.l["l"]["sfx"].endswith("se03.wav"))
        self.assertEqual(self.m["l"]["source"], "borrowed")
        # and the table, for the rest
        self.assertTrue(self.l["v"]["sfx"].endswith("se40.wav"))
        self.assertEqual(self.m["v"]["source"], "table")

    def test_voices_words_names_and_interjections_stay_spoken(self):
        for i in ["f", "g", "h", "i", "m", "n", "o", "s", "q"]:
            self.assertNotIn("spoken", self.l[i], i)
            self.assertNotIn(i, self.m, i)
        self.assertEqual(self.l["m"]["voiceFile"].split("/")[-1], "aka0195.wav")
        near = {n["id"]: n["why"] for n in self.r["nearMisses"]}
        self.assertIn("h", near)
        self.assertIn("i", near)
        # twelve kana that are not one piece repeated: too long to be a sound word
        self.assertIn("q", near)

    def test_a_repeated_word_counts_once(self):
        self.assertIs(self.l["p"]["spoken"], False)
        self.assertEqual(on.repeated_piece("イライライラ"), "イラ")
        self.assertEqual(on.repeated_piece("ドテッ"), "")
        # a rumble of one kana is a sound word however long
        self.assertEqual(on.classify({"text": "ドドドドドドドドドドドド！", "speaker": "koj"}, {}, opts(), set())[0], "katakana")

    def test_hiragana_with_a_sound_and_directions(self):
        self.assertEqual(self.m["u"]["rule"], "with-sfx")
        self.assertNotIn("spoken", self.l["r"])
        self.assertIn("w", self.r["alreadyUnspoken"])
        self.assertEqual(self.r["byRule"]["clip"], 5)

    def test_keep_and_mark(self):
        s = script()
        r = on.process(s, opts(keep={"e"}, mark={"s"}))
        l = by_id(s)
        self.assertNotIn("spoken", l["e"])
        self.assertIs(l["s"]["spoken"], False)
        self.assertIn("mark", r["byRule"])

    def test_sound_files_are_told_from_voices(self):
        for p in [SE + "se17.wav", VOICE + "j_擬音/j301.wav", "x/j_番号付き効果音/j305.wav", "se02.wav", "m201.wav", "eve_51b.wav"]:
            self.assertTrue(on.is_sound_file(p), p)
        for p in [VOICE + "aka_茜/aka0002.wav", "v/koj0001.wav", "", None, "voice/marina_0012.wav"]:
            self.assertFalse(on.is_sound_file(p), p)

    def test_hostile_input_is_not_a_crash(self):
        s = {"scenes": [{"id": "x", "lines": [None, 3, {"id": "a", "text": None}, {"id": "b", "text": "ガタッ", "speaker": 5}, {"id": "c", "text": "ドン", "speaker": ["x"], "voiceFile": 7}]}], "characters": {"x": "y"}}
        r = on.process(s, opts())
        self.assertEqual([m["id"] for m in r["marked"]], ["b", "c"])
        self.assertEqual(on.process({}, opts())["marked"], [])


class Cli(unittest.TestCase):
    def test_dry_run_writes_nothing_and_a_run_writes_the_script(self):
        with tempfile.TemporaryDirectory() as d:
            src, dst = os.path.join(d, "in.json"), os.path.join(d, "out.json")
            with io.open(src, "w", encoding="utf-8") as f:
                json.dump(script(), f, ensure_ascii=False)
            out = io.StringIO()
            old, sys.stdout = sys.stdout, out
            try:
                self.assertEqual(on.main([src, "--dry-run"]), 0)
                self.assertFalse(os.path.exists(dst))
                self.assertEqual(on.main([src, dst, "--json"]), 0)
            finally:
                sys.stdout = old
            self.assertIn("sound words:", out.getvalue())
            with io.open(dst, encoding="utf-8") as f:
                got = by_id(json.load(f))
            self.assertIs(got["a"]["spoken"], False)
            with self.assertRaises(SystemExit):
                on.main([src])


if __name__ == "__main__":
    unittest.main()
