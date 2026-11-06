"""Tests for tools/eve_burst/adapt.py (run: python -m unittest discover -s tools/eve_burst/tests)."""
import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
sys.path.insert(0, HERE)
import adapt as A  # noqa: E402
import check_adaptation as C  # noqa: E402
import episode_plan as P  # noqa: E402
from test_eve_to_script import validate_by_hand  # noqa: E402

TOOL = os.path.join(HERE, "..", "adapt.py")


def draft():
    def ln(lid, speaker="mar", text="まりなの台詞です", **kw):
        d = {"id": lid, "kind": "dialogue", "speaker": speaker, "text": text}
        d.update(kw)
        return d
    return {
        "version": 1, "title": "第 1 話", "language": "Japanese",
        "characters": {"mar": {"name": "まりな"}, "hon": {"name": "本部長"}, "narrator": {"name": "語り"}},
        "scenes": [
            {"id": "s1", "title": "b01 bg", "pov": "mar", "when": "D02-01-0001", "background": "bg/a.png",
             "lines": [ln("s1-001"), ln("s1-002", "hon", "録音", voiceFile="v/h1.wav"), ln("s1-003"), ln("s1-004"), ln("s1-005"),
                       {"id": "s1-006", "kind": "direction", "sfx": "se/x.wav"}]},
            {"id": "s2", "title": "b01 bg2", "pov": "mar", "when": "D02-01-0002",
             "lines": [ln("s2-001", "narrator", "夜だった。", kind="narration"), ln("s2-002"), ln("s2-003", "hon", "二", voiceFile="v/h2.wav")]},
            {"id": "s3", "title": "b01 bg3", "pov": "mar", "when": "D02-01-0003", "requires": ["s2"], "lines": [ln("s3-001")]},
        ],
        "route": ["s1", "s2", "s3"],
    }


class Apply(unittest.TestCase):
    def setUp(self):
        self._td = tempfile.TemporaryDirectory()
        self.dir = self._td.name

    def tearDown(self):
        self._td.cleanup()

    def test_every_kind_of_edit(self):
        d = draft()
        edits = {
            "delete": ["s1-004", "s2-001..s2-002"],
            "rewrite": {"s1-001": "短く。"},
            "merge": [{"ids": ["s1-003", "s1-005"], "text": "二行を一行に。"}],
            "insert": [{"after": "s1-002", "speaker": "mar", "text": "つなぎ。", "kind": "monologue", "pauseAfter": 0.5},
                       {"before": "s3-001", "speaker": "narrator", "text": "翌朝。"}],
            "reading": {"s1-002": "ろくおん"},
            "pause": {"s2-003": 1.5},
            "note": "free text is allowed",
        }
        r = A.apply_edits(d, edits)
        self.assertEqual(d, draft(), "the draft is not changed")
        ids = [[x["id"] for x in sc["lines"]] for sc in r["scenes"]]
        self.assertEqual(ids, [["s1-001", "s1-002", "s1-g01", "s1-003", "s1-006"], ["s2-003"], ["s3-g01", "s3-001"]])
        lines = {x["id"]: x for sc in r["scenes"] for x in sc["lines"]}
        self.assertEqual((lines["s1-001"]["text"], lines["s1-001"]["generated"]), ("短く。", True))
        self.assertEqual(lines["s1-003"]["text"], "二行を一行に。")
        self.assertEqual(lines["s1-g01"], {"id": "s1-g01", "kind": "monologue", "speaker": "mar", "text": "つなぎ。", "generated": True, "pauseAfter": 0.5})
        self.assertEqual(lines["s3-g01"]["kind"], "narration")
        self.assertEqual(lines["s1-002"]["reading"], "ろくおん")
        self.assertEqual(lines["s2-003"]["pauseAfter"], 1.5)
        self.assertEqual(r["route"], ["s1", "s2", "s3"])
        self.assertEqual(r["scenes"][2]["requires"], ["s2"])
        self.assertEqual(validate_by_hand(r), [])
        res = C.check(d, r, self.dir, self.dir, est=P.Estimator(self.dir))
        self.assertTrue(res["ok"], res["errors"])
        self.assertEqual((res["rewritten"], res["new"]), (2, 2))

    def test_an_emptied_scene_goes_and_what_required_it_lets_go(self):
        r = A.apply_edits(draft(), {"delete": ["s2-001..s2-003"]})
        self.assertEqual(r["route"], ["s1", "s3"])
        self.assertNotIn("requires", r["scenes"][1])
        r = A.apply_edits(draft(), {"deleteScenes": ["s1"]})
        self.assertEqual(r["route"], ["s2", "s3"])

    def test_edits_that_cannot_be_applied(self):
        bad = [
            ({"rewrite": {"s1-002": "x"}}, "has a recording"),
            ({"merge": [{"ids": ["s1-001", "s1-002"], "text": "x"}]}, "has a recording"),
            ({"merge": [{"ids": ["s1-005", "s2-002"], "text": "x"}]}, "another scene"),
            ({"merge": [{"ids": ["s1-005", "s1-003"], "text": "x"}]}, "draft's order"),
            ({"merge": [{"ids": ["s1-003"], "text": "x"}]}, "at least two"),
            ({"merge": [{"ids": ["s1-001", "s1-006"], "text": "x"}]}, "same speaker"),
            ({"rewrite": {"s1-006": "x"}}, "direction"),
            ({"rewrite": {"nope": "x"}}, "not a line"),
            ({"rewrite": {"s1-001": ""}}, "non-empty"),
            ({"rewrite": {"s1-001": "x" * 5000}}, "longer than"),
            ({"rewrite": {"s1-001": "x"}, "delete": ["s1-001"]}, "also deleted"),
            ({"delete": ["s2-002..s1-001"]}, "backwards"),
            ({"delete": ["s1-001..nope"]}, "not a line"),
            ({"delete": "s1-001"}, "list of ids"),
            ({"deleteScenes": ["s9"]}, "not a scene"),
            ({"insert": [{"after": "s1-001", "speaker": "kyo", "text": "x"}]}, "not a character"),
            ({"insert": [{"after": "s1-004", "speaker": "mar", "text": "x"}], "delete": ["s1-004"]}, "is deleted"),
            ({"insert": [{"speaker": "mar", "text": "x"}]}, "after"),
            ({"insert": [{"after": "s1-001", "speaker": "mar", "text": "x", "kind": "direction"}]}, "kind must be"),
            ({"pause": {"s1-001": 99}}, "0..60"),
            ({"pause": {"s1-001": True}}, "0..60"),
            ({"remove": []}, "unknown edit kinds"),
            ([], "JSON object"),
        ]
        for edits, needle in bad:
            with self.assertRaises(A.EditError, msg=edits) as cm:
                A.apply_edits(draft(), edits)
            self.assertIn(needle, str(cm.exception), edits)

    def test_a_recording_played_without_its_text(self):
        d = draft()
        d["characters"]["n001"] = {"name": "男"}
        d["scenes"][1]["lines"].append({"id": "s2-004", "kind": "dialogue", "speaker": "n001", "text": "名無しの台詞"})
        # s1-002 alone; s2-003 in a deleted scene (only it stays); s2-004's speaker is named
        r = A.apply_edits(d, {"voiceOnly": ["s1-002", "s2-003"], "delete": ["s1-002"], "deleteScenes": ["s2"],
                              "speaker": {"s2-004": "hon"}})
        lines = {x["id"]: x for sc in r["scenes"] for x in sc["lines"]}
        self.assertEqual(lines["s1-002"], {"id": "s1-002", "kind": "direction", "sfx": "v/h1.wav"})
        self.assertEqual([x["id"] for x in r["scenes"][1]["lines"]], ["s2-003"])
        self.assertEqual(validate_by_hand(r), [])
        res = C.check(d, r, self.dir, self.dir, est=P.Estimator(self.dir))
        self.assertTrue(res["ok"], res["errors"])
        self.assertEqual(res["voiceOnly"], 2)
        r = A.apply_edits(d, {"speaker": {"s2-004": "hon"}})
        self.assertEqual(r["scenes"][1]["lines"][-1]["speaker"], "hon")
        res = C.check(d, r, self.dir, self.dir, est=P.Estimator(self.dir))
        self.assertTrue(res["ok"], res["errors"])
        self.assertEqual(res["speakersNamed"], 1)
        for edits, needle in (({"voiceOnly": ["s1-001"]}, "no recording"), ({"voiceOnly": ["s1-006"]}, "no recording"),
                              ({"speaker": {"s1-001": "hon"}}, "numbered minor"), ({"speaker": {"s1-002": "mar"}}, "has a recording"),
                              ({"speaker": {"s2-004": "kyo"}}, "not a character"), ({"voiceOnly": ["s1-002"], "rewrite": {"s1-002": "x"}}, "direction")):
            with self.assertRaises(A.EditError, msg=edits) as cm:
                A.apply_edits(d, edits)
            self.assertIn(needle, str(cm.exception))
        # the checker still refuses a named line that was not a numbered character, or a sound that is another file
        bad = A.apply_edits(d, {"voiceOnly": ["s1-002"]})
        bad["scenes"][0]["lines"][1]["sfx"] = "v/other.wav"
        self.assertFalse(C.check(d, bad, self.dir, self.dir, est=P.Estimator(self.dir))["ok"])

    def test_view_is_compact(self):
        text = A.view(draft(), P.Estimator(self.dir))
        self.assertIn("## s1", text)
        self.assertIn("s1-002 hon R 録音", text)
        self.assertIn("s2-001 narrator - 夜だった。", text)
        self.assertIn("（direction sfx）", text)
        self.assertIn("estimated", text)

    def test_cli(self):
        src = os.path.join(self.dir, "series", "ep.fcscript.json")
        os.makedirs(os.path.dirname(src))
        with open(src, "w", encoding="utf-8") as f:
            json.dump(draft(), f, ensure_ascii=False)
        edits = os.path.join(self.dir, "e.json")
        out = os.path.join(self.dir, "adapted", "ep.fcscript.json")

        def run(e, *extra):
            with open(edits, "w", encoding="utf-8") as f:
                f.write(e if isinstance(e, str) else json.dumps(e, ensure_ascii=False))
            return subprocess.run([sys.executable, "-X", "utf8", TOOL, "apply", "--script", src, "--edits", edits, "--out", out, *extra],
                                  capture_output=True, text=True, encoding="utf-8")

        r = run({"delete": ["s1-004"]})
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(out, encoding="utf-8") as f:
            res = json.load(f)
        self.assertEqual(res["scenes"][0]["background"], "../series/bg/a.png")  # paths right from the new folder
        self.assertEqual(res["scenes"][0]["lines"][1]["voiceFile"], "../series/v/h1.wav")
        self.assertEqual(run({"delete": ["s1-004"]}, "--min", "30").returncode, 1)  # written, but too short
        self.assertEqual(run({"rewrite": {"s1-002": "x"}}).returncode, 2)
        self.assertEqual(run("{bad json").returncode, 2)
        r = subprocess.run([sys.executable, "-X", "utf8", TOOL, "view", "--script", src], capture_output=True, text=True, encoding="utf-8")
        self.assertEqual(r.returncode, 0)
        self.assertIn("s1-001 mar -", r.stdout)


if __name__ == "__main__":
    unittest.main()
