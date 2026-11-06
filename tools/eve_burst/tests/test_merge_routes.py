"""Tests for tools/eve_burst/merge_routes.py.

Run:  python -m unittest discover -s tools/eve_burst/tests
Small scripts are written by hand into a temp directory. The tests on the real dump skip when it
is absent (EVE_BURST_DATA overrides the location); they never write inside it.
"""
import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
sys.path.insert(0, HERE)
import eve_to_script as E  # noqa: E402
import merge_routes as M  # noqa: E402
from test_eve_to_script import REAL_ROOT, REPO, requires_in_order, validate_by_hand  # noqa: E402

TOOL = os.path.join(HERE, "..", "merge_routes.py")


def scene(sid, when, scenario="a01", n=1, speaker="koj", pov="koj", **kw):
    sc = {"id": sid, "title": f"{scenario} bg", "pov": pov, "when": when,
          "lines": [{"id": f"{sid}-{k:03d}", "speaker": speaker, "text": f"{sid} {k}"} for k in range(n)]}
    sc.update(kw)
    return sc


def script(scenes, chars=None, title="T"):
    return {"version": 1, "title": title, "language": "Japanese",
            "characters": chars or {"koj": {"name": "小次郎"}, "mar": {"name": "まりな"}},
            "scenes": scenes, "route": [sc["id"] for sc in scenes]}


class Base(unittest.TestCase):
    def setUp(self):
        self._td = tempfile.TemporaryDirectory()
        self.dir = self._td.name
        self.out = os.path.join(self.dir, "both.fcscript.json")

    def tearDown(self):
        self._td.cleanup()

    def write(self, name, data):
        path = os.path.join(self.dir, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False)
        return path

    def sides(self):
        a = script([
            scene("s1", "D03-0001"), scene("s2", "D03-0002"), scene("s3", "D05-0001"),
            scene("s4", "D07-0001", "ab01", requires=["s3"]), scene("s5", "D09-0001", "c02"),
        ])
        b = script([
            scene("s1", "D02-0001", "b01", speaker="mar", pov="mar"), scene("s2", "D03-0001", "b01", speaker="mar", pov="mar"),
            scene("s3", "D07-0001", "ab01", speaker="mar", pov="mar", requires=["s2"]), scene("s4", "D09-0001", "c02", pov="mar"),
        ])
        return self.write("a/k.fcscript.json", a), self.write("b/m.fcscript.json", b)

    def merge(self, a, b, **kw):
        s, rep = M.merge([a] if isinstance(a, str) else a, [b] if isinstance(b, str) else b, self.out, **kw)
        self.assertEqual(validate_by_hand(s), [])
        self.assertEqual(requires_in_order(s), [])
        whens = [sc["when"] for sc in s["scenes"]]
        self.assertEqual(whens, sorted(whens), "story time runs forward")
        return s, rep


class Interleave(Base):
    def test_day_by_day_then_where_the_sides_meet_once(self):
        a, b = self.sides()
        s, rep = self.merge(a, b)
        self.assertEqual(s["route"], ["m-s1", "k-s1", "k-s2", "m-s2", "k-s3", "k-s4", "k-s5"])
        self.assertEqual([sc["id"] for sc in s["scenes"]], s["route"])
        self.assertEqual(s["scenes"][5]["requires"], ["k-s3", "m-s2"])  # the last scene before it on both sides
        self.assertEqual(rep["finaleScenesLeftOut"], 2)
        self.assertEqual([(x["day"], x["side"]) for x in rep["blocks"]], [(2, "B"), (3, "A"), (3, "B"), (5, "A"), (7, "finale"), (9, "finale")])
        self.assertEqual([sc["when"] for sc in s["scenes"]][:4], ["D02-01-0001", "D03-01-0001", "D03-01-0002", "D03-02-0001"])
        self.assertEqual([ln["id"] for ln in s["scenes"][0]["lines"]], ["m-s1-000"])

    def test_other_orders_and_whose_finale(self):
        a, b = self.sides()
        s, _ = self.merge(a, b, order="ba")
        self.assertEqual(s["route"][:4], ["m-s1", "m-s2", "k-s1", "k-s2"])
        s, rep = self.merge(a, b, finale_from="b")
        self.assertEqual(s["route"][-2:], ["m-s3", "m-s4"])
        self.assertEqual(s["scenes"][-2]["requires"], ["m-s2", "k-s3"])
        s, rep = self.merge(a, b, finale_from="both")
        self.assertEqual(rep["finaleScenesLeftOut"], 0)
        self.assertEqual(s["route"][-4:], ["k-s4", "m-s3", "k-s5", "m-s4"])  # still day by day

    def test_episode_files_and_patterns(self):
        a, b = self.sides()
        with open(a, encoding="utf-8") as f:
            whole = json.load(f)
        paths = []
        for n, ep in enumerate(E.split_episodes(whole), 1):
            paths.append(self.write(f"a/k.ep{n:02d}.fcscript.json", ep["script"]))
        s1, _ = self.merge(a, b)
        s2, _ = self.merge([os.path.join(self.dir, "a", "k.ep*.fcscript.json")], b)
        self.assertEqual(s1["route"], s2["route"])
        with self.assertRaises(M.MergeError):
            M.merge([os.path.join(self.dir, "nothing*.json")], [b], self.out)

    def test_a_prerequisite_shown_later_moves_the_scene_after_it(self):
        a = script([scene("s1", "D03-0001"), scene("s2", "D04-0001")])
        b = script([scene("s1", "D03-0001", "b01", pov="mar"), scene("s2", "D03-0002", "b01", pov="mar")])
        pa, pb = self.write("k.fcscript.json", a), self.write("m.fcscript.json", b)
        s, _ = M.merge([pa], [pb], self.out)
        self.assertEqual(s["route"], ["k-s1", "m-s1", "m-s2", "k-s2"])
        scenes = [dict(x) for x in s["scenes"]]
        scenes[1]["requires"] = ["k-s2"]
        notes = []
        order = [x["id"] for x in M.stable_order(scenes, notes)]
        self.assertEqual(order, ["k-s1", "m-s2", "k-s2", "m-s1"])
        self.assertTrue(notes)

    def test_a_circle_of_prerequisites_does_not_hang(self):
        scenes = [{"id": "a", "requires": ["b"], "lines": []}, {"id": "b", "requires": ["a"], "lines": []}, {"id": "c", "lines": []}]
        notes = []
        self.assertEqual([x["id"] for x in M.stable_order(scenes, notes)], ["c", "a", "b"])
        self.assertIn("circle", notes[0])


class Characters(Base):
    def test_game_codes_are_one_person_and_minor_characters_by_name(self):
        a = script([scene("s1", "D03-0001", speaker="n001"), scene("s2", "D03-0002", speaker="n002"), scene("s3", "D03-0003", speaker="kat")],
                   {"koj": {"name": "小次郎"}, "n001": {"name": "警官"}, "n002": {"name": "男"},
                    "kat": {"name": "桂木", "expressions": {"00ad": {"image": "x.png"}}}})
        b = script([scene("s1", "D03-0001", "b01", speaker="n001", pov="mar"), scene("s2", "D03-0002", "b01", speaker="n002", pov="mar"),
                    scene("s3", "D03-0003", "b01", speaker="kat", pov="mar"), scene("s4", "D03-0004", "b01", speaker="n003", pov="mar")],
                   {"mar": {"name": "まりな"}, "n001": {"name": "男"}, "n002": {"name": "フロント"}, "n003": {"name": "守衛"},
                    "kat": {"name": "鈴木", "expressions": {"01ad": {"image": "y.png"}}, "voice": {"speaker": "X"}}})
        pa, pb = self.write("k.fcscript.json", a), self.write("m.fcscript.json", b)
        s, rep = self.merge(pa, pb)
        sp = {sc["id"]: sc["lines"][0]["speaker"] for sc in s["scenes"]}
        self.assertEqual(sp["m-s1"], "n002")  # 男 is A's n002
        self.assertEqual(sp["m-s2"], "m-n002")  # フロント is nobody on side A: renamed
        self.assertEqual(sp["m-s3"], "kat")
        self.assertEqual(sp["m-s4"], "n003")  # free on side A: kept
        kat = s["characters"]["kat"]
        self.assertEqual(kat["name"], "桂木")
        self.assertEqual(sorted(kat["expressions"]), ["00ad", "01ad"])
        self.assertEqual(kat["voice"], {"speaker": "X"})
        self.assertEqual(s["characters"]["m-n002"]["name"], "フロント")
        self.assertEqual(rep["renamedCharacters"], {"n001": "n002", "n002": "m-n002"})


class Paths(Base):
    def test_paths_stay_right_from_the_output_folder(self):
        a = script([scene("s1", "D03-0001", background="../media/bg.png", bgm="m.wav")],
                   {"koj": {"name": "小次郎", "voice": {"refAudio": "v/koj.wav", "refText": "x"},
                            "expressions": {"n": {"image": "c/k.png", "openImage": "c/k_o.png", "vowelImages": {"a": "c/k_a.png"}}}}})
        a["scenes"][0]["lines"][0].update({"voiceFile": "v/1.wav", "sfx": "/abs/se.wav"})
        b = script([scene("s1", "D03-0001", "b01", pov="mar")])
        pa = self.write("deep/k/k.fcscript.json", a)
        pb = self.write("m.fcscript.json", b)
        s, _ = self.merge(pa, pb)
        sc = s["scenes"][0]
        self.assertEqual(sc["background"], "deep/media/bg.png")
        self.assertEqual(sc["bgm"], "deep/k/m.wav")
        self.assertEqual(sc["lines"][0]["voiceFile"], "deep/k/v/1.wav")
        self.assertEqual(sc["lines"][0]["sfx"], "/abs/se.wav")
        k = s["characters"]["koj"]
        self.assertEqual(k["voice"]["refAudio"], "deep/k/v/koj.wav")
        self.assertEqual(k["expressions"]["n"]["vowelImages"]["a"], "deep/k/c/k_a.png")
        self.assertEqual(k["expressions"]["n"]["openImage"], "deep/k/c/k_o.png")
        self.assertEqual(sc["lines"][0]["text"], "s1 0")  # text is never a path


class Hostile(Base):
    def test_bad_input_is_refused_with_a_message(self):
        a, b = self.sides()
        bad = os.path.join(self.dir, "bad.json")
        for content in ("not json", "[1, 2]", '{"scenes": 5}', '{"version": 2, "scenes": []}', '{"scenes": [{"lines": []}]}',
                        '{"scenes": [{"id": "x", "lines": 3}]}', '{"scenes": [], "characters": []}'):
            with open(bad, "w", encoding="utf-8") as f:
                f.write(content)
            with self.assertRaises(M.MergeError, msg=content):
                M.merge([bad], [b], self.out)
        with self.assertRaises(M.MergeError):
            M.merge([os.path.join(self.dir, "missing.json")], [b], self.out)
        for kw in ({"prefix_a": "k", "prefix_b": "k"}, {"prefix_a": ""}, {"prefix_a": "a/b"}, {"finale_from": "x"}, {"order": "xy"}):
            with self.assertRaises(M.MergeError, msg=kw):
                M.merge([a], [b], self.out, **kw)

    def test_odd_but_readable_input_does_not_crash(self):
        a = script([scene("s1", "D03-0001"), scene("s1", "D03-0002")])  # a repeated id: the later copy is left out
        a["scenes"].append({"id": "s9", "title": 5, "when": None, "lines": ["not a line", {"id": "s9-1", "kind": "direction"}]})
        a["route"] = []  # file order
        a["characters"]["odd"] = "not an object"
        b = script([], {})
        pa, pb = self.write("k.fcscript.json", a), self.write("m.fcscript.json", b)
        s, rep = M.merge([pa], [pb], self.out)
        self.assertEqual(s["route"], ["k-s1", "k-s9"])
        self.assertTrue(any("already" in n for n in rep["notes"]))

    def test_cli(self):
        a, b = self.sides()
        rep = os.path.join(self.dir, "r.json")
        r = subprocess.run([sys.executable, "-X", "utf8", TOOL, "--kojirou", a, "--marina", b, "--out", self.out, "--report", rep],
                           capture_output=True, text=True, encoding="utf-8")
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(self.out, encoding="utf-8") as f:
            self.assertEqual(len(json.load(f)["route"]), 7)
        with open(rep, encoding="utf-8") as f:
            self.assertEqual(json.load(f)["finaleScenesLeftOut"], 2)
        r = subprocess.run([sys.executable, "-X", "utf8", TOOL, "--a", a, "--b", os.path.join(self.dir, "none.json"), "--out", self.out],
                           capture_output=True, text=True, encoding="utf-8")
        self.assertEqual(r.returncode, 2)
        self.assertIn("merge_routes:", r.stderr)


def repo_validate(path):
    """script.validate of the repo's CLI, or None when it is not built or cannot run."""
    exe = os.path.join(REPO, "target", "debug", "filmcraft-cli.exe")
    if not os.path.isfile(exe):
        exe = os.path.join(REPO, "target", "debug", "filmcraft-cli")
    if not os.path.isfile(exe):
        return None
    r = subprocess.run([exe, "--compact", "exec", "script.validate", "path=" + path.replace("\\", "/")], capture_output=True, text=True, encoding="utf-8")
    if r.returncode != 0:
        return None
    return json.loads(r.stdout)


class RepoValidator(Base):
    def test_the_merged_script_passes_script_validate(self):
        a, b = self.sides()
        s, _ = M.merge([a], [b], self.out)
        with open(self.out, "w", encoding="utf-8") as f:
            json.dump(s, f, ensure_ascii=False)
        res = repo_validate(self.out)
        if res is None:
            self.skipTest("filmcraft-cli is not built")
        self.assertEqual(res["errors"], 0, res["problems"][:3])
        self.assertFalse([p for p in res["problems"] if "story time" in p["message"] or "requires" in p["message"]])


@unittest.skipUnless(os.path.isdir(os.path.join(REAL_ROOT, "01_シナリオ")), "the decoded dump is not here")
class RealData(Base):
    def test_both_routes_merge_into_one_valid_order(self):
        paths = []
        for route in ("kojirou", "marina"):
            p = os.path.join(self.dir, f"{route}.fcscript.json")
            s, _ = E.convert(REAL_ROOT, p, route, E.Options())
            with open(p, "w", encoding="utf-8") as f:
                json.dump(s, f, ensure_ascii=False)
            paths.append(p)
        s, rep = self.merge(paths[0], paths[1])
        self.assertGreater(rep["finaleScenesLeftOut"], 0)
        self.assertEqual(len(rep["finaleRequires"]), 2)
        with open(self.out, "w", encoding="utf-8") as f:
            json.dump(s, f, ensure_ascii=False)
        res = repo_validate(self.out)
        if res is not None:
            self.assertEqual(res["errors"], 0, res["problems"][:3])


if __name__ == "__main__":
    unittest.main()
