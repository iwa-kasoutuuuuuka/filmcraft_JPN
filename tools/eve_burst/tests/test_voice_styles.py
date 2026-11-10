import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))

import voice_styles as vs  # noqa: E402

DUMP = "E:/dump/03_音声/ボイス"
STYLES = {
    "aka": {"怒り": {"refAudio": f"{DUMP}/aka_茜/aka0643.wav", "refText": "あーっ！", "files": ["aka0643.wav"]},
            "静か": {"refAudio": f"{DUMP}/aka_茜/aka0457.wav", "refText": "二階堂君は…"},
            "普通": {"refAudio": "x.wav", "refText": "not a style label"},
            "驚き": {"refAudio": 5, "refText": "bad"}},
    "koj": {"驚き": {"refAudio": "E:/out/koj_驚き.wav", "refText": "け、毛！？"}},
}


def script():
    return {"characters": {
        "aka": {"name": "茜", "voice": {"refAudio": "../../dump/03_音声/ボイス/aka_茜/aka0140.wav", "refText": "…"}},
        "koj": {"name": "小次郎", "voice": {"refAudio": "../voices/kojtest/koj_joined.wav", "refText": "…"}},
        "narrator": {"name": "語り", "voice": {"refAudio": "../voices/kojtest/koj_joined.wav", "refText": "…"}},
        "n001": {"name": "男", "voice": {"refAudio": "../voices/extras/man-a.wav", "refText": "…"}},
        "anna": {"name": "プリセット", "voice": {"speaker": "Ono_Anna"}},
        "plain": {"name": "声なし"}, "junk": "x",
    }, "scenes": []}


class Folders(unittest.TestCase):
    def test_folder_and_prefix(self):
        self.assertEqual(vs.folder_of("../../PS2/03_音声/ボイス/aka_茜/aka0140.wav"), "aka_茜")
        self.assertEqual(vs.folder_of("E:\\PS2\\03_音声\\ボイス\\pul_プルシア\\pul0260.wav"), "pul_プルシア")
        self.assertIsNone(vs.folder_of("../voices/extras/man-a.wav"))
        self.assertIsNone(vs.folder_of(None))
        self.assertEqual(vs.prefix_of("pul_プルシア"), "pul")


class Ranking(unittest.TestCase):
    def test_strong_labels_prefer_strong_clips_and_pictures_over_text(self):
        a = {"strength": "中", "src": "expression", "dur": 7.0, "per": 0.0}
        b = {"strength": "強", "src": "text", "dur": 9.0, "per": 0.05}
        c = {"strength": "強", "src": "expression", "dur": 11.0, "per": 0.05}
        self.assertEqual(sorted([a, b, c], key=lambda x: vs.rank_key(x, "怒り")), [c, b, a])
        # a quiet label prefers a clip that is not strong, then the length nearest 7 s (7.0 before 6.5)
        d = {"strength": "中", "src": "expression", "dur": 6.5, "per": 0.0}
        self.assertEqual(sorted([c, d, a], key=lambda x: vs.rank_key(x, "静か")), [a, d, c])

    def test_join_plan(self):
        shorts = [{"dur": 1.7}, {"dur": 2.0}, {"dur": 3.0}]
        self.assertEqual(vs.plan_join(shorts), shorts[:2])  # 1.7 + 0.3 + 2.0 + 0.3 >= 4
        self.assertEqual(vs.plan_join([{"dur": 1.0}, {"dur": 1.0}, {"dur": 1.0}, {"dur": 3.0}]), [])  # three are not enough
        self.assertEqual(vs.plan_join([]), [])
        same = [{"wav": "se35.wav", "dur": 2.5}, {"wav": "se35.wav", "dur": 2.5}]
        self.assertEqual(vs.plan_join(same), [])  # the same recording twice is not a join


class Apply(unittest.TestCase):
    def test_cloned_characters_get_their_folders_styles(self):
        s = script()
        done = vs.apply_styles(s, STYLES, {"koj": "koj", "narrator": "koj"})
        c = s["characters"]
        self.assertEqual(set(c["aka"]["voice"]["styles"]), {"怒り", "静か"})  # 普通 is not a style, 驚き is broken
        self.assertEqual(c["aka"]["voice"]["styles"]["怒り"], {"refAudio": f"{DUMP}/aka_茜/aka0643.wav", "refText": "あーっ！"})
        self.assertEqual(c["koj"]["voice"]["styles"]["驚き"]["refText"], "け、毛！？")
        self.assertIn("styles", c["narrator"]["voice"])
        for k in ("n001", "anna", "plain"):
            self.assertNotIn("styles", c[k].get("voice") or {}, k)
        self.assertEqual(c["aka"]["voice"]["refAudio"], "../../dump/03_音声/ボイス/aka_茜/aka0140.wav")  # the plain voice stays
        self.assertEqual(sorted(done), [("aka", "aka", 2), ("koj", "koj", 1), ("narrator", "koj", 1)])

    def test_a_narrator_sharing_a_recording_takes_its_folder(self):
        s = script()
        s["characters"]["narrator-aka"] = {"name": "語り", "voice": {"refAudio": "../../dump/03_音声/ボイス/aka_茜/aka0140.wav", "refText": "…"}}
        vs.apply_styles(s, STYLES, {"koj": "koj"})  # no alias for the narrators
        self.assertIn("styles", s["characters"]["narrator"]["voice"])  # same file as koj
        self.assertEqual(set(s["characters"]["narrator-aka"]["voice"]["styles"]), {"怒り", "静か"})

    def test_without_an_alias_a_voice_outside_the_dump_gets_none(self):
        s = script()
        vs.apply_styles(s, STYLES, {})
        self.assertNotIn("styles", s["characters"]["koj"]["voice"])

    def test_hostile(self):
        self.assertEqual(vs.apply_styles({"characters": "x"}, STYLES, {}), [])
        self.assertEqual(vs.apply_styles(script(), {"aka": "x"}, {}), [])


class Cli(unittest.TestCase):
    def test_apply_round_trip_and_bad_input(self):
        with tempfile.TemporaryDirectory() as d:
            sp, st, op = (os.path.join(d, n) for n in ("s.json", "st.json", "o.json"))
            for p, v in ((sp, script()), (st, STYLES)):
                with io.open(p, "w", encoding="utf-8") as f:
                    f.write(json.dumps(v, ensure_ascii=False))
            out = io.StringIO()
            with redirect_stdout(out):
                self.assertEqual(vs.main(["apply", "--styles", st, "--script", sp, "--alias", "narrator=koj", "--alias", "koj=koj", "--out", op]), 0)
            self.assertEqual(len(json.loads(out.getvalue())["styled"]), 3)
            with io.open(op, encoding="utf-8") as f:
                self.assertIn("styles", json.load(f)["characters"]["aka"]["voice"])
            with io.open(sp, "w", encoding="utf-8") as f:
                f.write("[")
            with redirect_stderr(io.StringIO()):
                self.assertEqual(vs.main(["apply", "--styles", st, "--script", sp, "--out", op]), 2)


if __name__ == "__main__":
    unittest.main()
