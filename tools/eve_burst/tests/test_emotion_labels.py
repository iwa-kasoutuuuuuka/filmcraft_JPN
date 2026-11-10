import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))

import emotion_labels as el  # noqa: E402

AKA = {"aka": {"00": "普通", "01": "動揺", "02": "静か", "03": "呆れ", "04": "喜び", "05": "甘え", "06": "怒り", "07": "動揺"}}


def write(path, text):
    with io.open(path, "w", encoding="utf-8") as f:
        f.write(text)


def read_json(path):
    with io.open(path, encoding="utf-8") as f:
        return json.load(f)


class FromText(unittest.TestCase):
    def test_marks(self):
        self.assertEqual(el.label_from_text("（さっきまでと、ずいぶん態度が違うな‥‥。）", "monologue"), "静か")
        self.assertEqual(el.label_from_text("この大馬鹿者！　ここは休憩所じゃないっ！！", None), "怒り")
        self.assertEqual(el.label_from_text("電話！？　今度こそ仕事かっ！", None), "驚き")
        self.assertEqual(el.label_from_text("君、そんな噂を本気にしているのか？", None), "問い")
        self.assertEqual(el.label_from_text("えへへ。", None), "喜び")
        self.assertEqual(el.label_from_text("‥‥つまらん‥‥。", None), "静か")
        self.assertEqual(el.label_from_text("事務所の扉が閉まった。", None), "普通")

    def test_short_exclamation_is_surprise_long_one_is_anger(self):
        self.assertEqual(el.label_from_text("えっ！", None), "驚き")
        self.assertEqual(el.label_from_text("さっさと仕事に行けと言っているんだ！", None), "怒り")

    def test_strength(self):
        self.assertEqual(el.strength_of("うちは出前持ちじゃないっ！！", "怒り"), "強")
        self.assertEqual(el.strength_of("ほぉ～～、それでぇ？", "問い"), "強")
        self.assertEqual(el.strength_of("（まったくまいるぜ‥‥。）", "静か"), "弱")
        self.assertEqual(el.strength_of("お前、信じてんのか？", "問い"), "中")


class FromExpression(unittest.TestCase):
    def test_expression_wins_over_text(self):
        line = {"speaker": "aka", "text": "なんだヨ、それ。", "expression": "03ad"}
        self.assertEqual(el.label_line(line, AKA), ("呆れ", "中", "expression"))

    def test_neutral_face_still_asks(self):
        line = {"speaker": "aka", "text": "それで、どうするの？", "expression": "00ad"}
        self.assertEqual(el.label_line(line, AKA)[0], "問い")

    def test_unknown_expression_falls_back_to_text(self):
        line = {"speaker": "aka", "text": "小次郎！　キミねっ！！", "expression": "99zz"}
        self.assertEqual(el.label_line(line, AKA)[:2], ("怒り", "強"))
        self.assertEqual(el.label_line({"speaker": "koj", "text": "おい！", "expression": "00ad"}, AKA)[2], "text")

    def test_existing_label_is_kept(self):
        line = {"speaker": "koj", "text": "おい！", "instruct": "静か（弱）"}
        self.assertEqual(el.label_line(line, {}), ("静か", "弱", "instruct"))

    def test_parse_and_format(self):
        self.assertEqual(el.parse_instruct("怒り（強）: 怒鳴って"), ("怒り", "強"))
        self.assertIsNone(el.parse_instruct("怒った声で"))
        self.assertIsNone(el.parse_instruct(None))
        self.assertEqual(el.format_label("驚き", "中"), "驚き（中）")


def script():
    return {"scenes": [{"lines": [
        {"id": "a", "speaker": "koj", "text": "この大馬鹿者！！"},
        {"id": "b", "speaker": "aka", "text": "なんだヨ、それ。", "expression": "03ad", "voiceFile": "x.wav"},
        {"id": "c", "speaker": "koj", "text": "黙れ", "instruct": "怒った声で、低く"},
        {"id": "d", "kind": "direction", "text": "――"},
        {"id": "e", "speaker": "koj", "text": "ガシャーン！", "spoken": False},
        {"id": "f", "speaker": "koj", "text": "おい", "instruct": "静か（弱）"},
        "junk", {"id": "g", "text": "x" * (el.MAX_TEXT + 1)},
    ]}, "junk"]}


class Script(unittest.TestCase):
    def test_labels_only_synthesized_lines_and_keeps_hand_directions(self):
        s = script()
        rows = el.label_script(s, AKA)
        lines = {l["id"]: l for l in s["scenes"][0]["lines"] if isinstance(l, dict)}
        self.assertEqual(lines["a"]["instruct"], "怒り（強）")
        self.assertNotIn("instruct", lines["b"])  # recorded
        self.assertEqual(lines["c"]["instruct"], "怒った声で、低く")  # hand-written direction kept
        self.assertNotIn("instruct", lines["d"])
        self.assertNotIn("instruct", lines["e"])  # unspoken
        self.assertEqual(lines["f"]["instruct"], "静か（弱）")
        self.assertEqual([r[0] for r in rows], ["a", "f"])

    def test_all_and_overwrite(self):
        s = script()
        el.label_script(s, AKA, overwrite=True, recorded=True)
        lines = {l["id"]: l for l in s["scenes"][0]["lines"] if isinstance(l, dict)}
        self.assertEqual(lines["b"]["instruct"], "呆れ（中）")
        self.assertEqual(lines["f"]["instruct"], "普通（中）")

    def test_skip_neutral_leaves_plain_lines_alone(self):
        s = {"scenes": [{"lines": [{"id": "a", "speaker": "koj", "text": "事務所に戻る。"}, {"id": "b", "speaker": "koj", "text": "おい！！"}]}]}
        rows = el.label_script(s, {}, skip_neutral=True)
        lines = s["scenes"][0]["lines"]
        self.assertNotIn("instruct", lines[0])
        self.assertEqual(lines[1]["instruct"], "怒り（強）")
        self.assertEqual([r[2] for r in rows], ["普通", "怒り"])  # still reported

    def test_hostile(self):
        self.assertEqual(el.label_script({"scenes": "x"}, AKA), [])
        self.assertEqual(el.label_script({}, {}), [])


class Cli(unittest.TestCase):
    def test_round_trip_and_bad_input(self):
        with tempfile.TemporaryDirectory() as d:
            sp, ep, op, rp = (os.path.join(d, n) for n in ("s.json", "e.json", "o.json", "r.tsv"))
            write(sp, json.dumps(script(), ensure_ascii=False))
            write(ep, json.dumps(AKA, ensure_ascii=False))
            out = io.StringIO()
            with redirect_stdout(out):
                self.assertEqual(el.main(["--script", sp, "--expressions", ep, "--all", "--report", rp, "--out", op]), 0)
            self.assertEqual(json.loads(out.getvalue())["labelled"], 3)
            self.assertEqual(read_json(op)["scenes"][0]["lines"][1]["instruct"], "呆れ（中）")
            write(sp, "[1")
            with redirect_stderr(io.StringIO()):
                self.assertEqual(el.main(["--script", sp, "--out", op]), 2)
            write(sp, "[1]")
            with redirect_stderr(io.StringIO()):
                self.assertEqual(el.main(["--script", sp, "--out", op]), 2)


if __name__ == "__main__":
    unittest.main()
