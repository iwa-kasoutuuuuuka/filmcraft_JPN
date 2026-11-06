"""Tests for tools/eve_burst/episode_plan.py and check_adaptation.py.

Run:  python -m unittest discover -s tools/eve_burst/tests
Scripts and media are made by hand in a temp directory. The test on the real dump skips when it is
absent (EVE_BURST_DATA overrides the location); it never writes inside it.
"""
import copy
import json
import os
import struct
import subprocess
import sys
import tempfile
import time
import unittest
import wave

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
sys.path.insert(0, HERE)
import check_adaptation as C  # noqa: E402
import episode_plan as P  # noqa: E402
import eve_to_script as E  # noqa: E402
import merge_routes as M  # noqa: E402
from test_eve_to_script import REAL_ROOT, requires_in_order, validate_by_hand  # noqa: E402

TOOLS = os.path.join(HERE, "..")


def write_wav(path, seconds, rate=8000):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(b"\0\0" * int(seconds * rate))


def write_mp4(path, seconds, scale=1000, version=0):
    if version == 0:
        mvhd_body = bytes([0, 0, 0, 0]) + struct.pack(">IIII", 0, 0, scale, int(seconds * scale)) + b"\0" * 80
    else:
        mvhd_body = bytes([1, 0, 0, 0]) + struct.pack(">QQIQ", 0, 0, scale, int(seconds * scale)) + b"\0" * 80
    mvhd = struct.pack(">I4s", 8 + len(mvhd_body), b"mvhd") + mvhd_body
    moov = struct.pack(">I4s", 8 + len(mvhd), b"moov") + mvhd
    ftyp = struct.pack(">I4s", 16, b"ftyp") + b"isom\0\0\0\0"
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(ftyp + moov)


def line(lid, speaker="koj", text="あいうえおかきくけこ", **kw):  # 10 characters: 10 / 7 s spoken
    d = {"id": lid, "speaker": speaker, "text": text}
    d.update(kw)
    return d


def scene(sid, when, pov, n=3, title="a01 bg", **kw):
    sc = {"id": sid, "title": title, "pov": pov, "when": when,
          "lines": [line(f"{sid}-{k:03d}", pov, f"{sid} の {k} 行目です") for k in range(n)]}
    sc.update(kw)
    return sc


def series_script():
    """A merged script: per day Kojirou then Marina, then the meeting part."""
    scenes = []
    for day, nk, nm in ((2, 0, 4), (3, 12, 8), (4, 10, 10), (5, 1, 0)):
        for k in range(nk):
            scenes.append(scene(f"k-d{day}-{k}", f"D{day:02d}-01-{k + 1:04d}", "koj", 10))
        for k in range(nm):
            scenes.append(scene(f"m-d{day}-{k}", f"D{day:02d}-02-{k + 1:04d}", "mar", 10, title="b01 bg"))
    scenes.append(scene("k-f1", "D05-03-0001", "koj", 10, title="ab01 bg", requires=["k-d5-0", "m-d4-9"]))
    scenes.append(scene("k-f2", "D09-01-0001", "koj", 10, title="c02 bg"))
    return {"version": 1, "title": "T", "language": "Japanese",
            "characters": {"koj": {"name": "小次郎"}, "mar": {"name": "まりな"}, "kyo": {"name": "恭子"}},
            "scenes": scenes, "route": [s["id"] for s in scenes]}


class Base(unittest.TestCase):
    def setUp(self):
        self._td = tempfile.TemporaryDirectory()
        self.dir = self._td.name

    def tearDown(self):
        self._td.cleanup()

    def est(self, **kw):
        return P.Estimator(self.dir, **kw)


class Arithmetic(unittest.TestCase):
    def test_largest_remainder(self):
        self.assertEqual(P.largest_remainder([1, 1, 1], 6), [2, 2, 2])
        self.assertEqual(P.largest_remainder([272, 174], 6, 1), [4, 2])
        self.assertEqual(P.largest_remainder([59, 400, 400, 300, 300, 400], 22, 1), [1, 5, 5, 3, 3, 5])
        out = P.largest_remainder([1, 1000], 3, 1)
        self.assertEqual(out, [1, 2])
        self.assertEqual(P.largest_remainder([0, 0], 3), [2, 1])
        self.assertEqual(sum(P.largest_remainder([3.3, 2.2, 9.9, 0.1], 17, 1)), 17)
        with self.assertRaises(P.PlanError):
            P.largest_remainder([1, 1, 1], 2, 1)

    def test_cut_even(self):
        items = list(range(10))
        runs = P.cut_even(items, [1] * 10, 3)
        self.assertEqual(len(runs), 3)
        self.assertEqual(sum(runs, []), items)
        self.assertTrue(all(3 <= len(r) <= 4 for r in runs))
        runs = P.cut_even(items, [10, 1, 1, 1, 1, 1, 1, 1, 1, 1], 2)
        self.assertEqual(runs[0], [0])
        self.assertEqual(P.cut_even([1, 2], [1, 1], 5), [[1], [2]])  # never more runs than items
        self.assertEqual(P.cut_even([7], [1], 1), [[7]])
        runs = P.cut_even(items, [0] * 10, 4)  # nothing has a length: still cut, never empty
        self.assertEqual(sum(runs, []), items)
        self.assertTrue(all(runs))


class Estimate(Base):
    def test_lines_and_scenes_as_anime_build_lays_them_out(self):
        write_wav(os.path.join(self.dir, "v", "a.wav"), 2.0)
        write_wav(os.path.join(self.dir, "se", "b.wav"), 0.5)
        write_mp4(os.path.join(self.dir, "m", "c.mp4"), 12.5)
        e = self.est()
        self.assertAlmostEqual(e.line(line("1", voiceFile="v/a.wav")), 2.35, places=3)
        self.assertAlmostEqual(e.line(line("2")), 10 / 7 + 0.35, places=3)
        self.assertAlmostEqual(e.line(line("3", text="あ")), 0.8 + 0.35, places=3)  # at least 0.8 s
        self.assertAlmostEqual(e.line(line("4", pauseAfter=1.0)), 10 / 7 + 1.0, places=3)
        self.assertAlmostEqual(e.line(line("5", text="｜漢字《かんじかんじかんじか》")), 10 / 7 + 0.35, places=3)  # ruby is read out
        self.assertAlmostEqual(e.line(line("6", reading="あいうえおかきくけこさしすせそ")), 15 / 7 + 0.35, places=3)
        self.assertAlmostEqual(e.line({"id": "7", "kind": "direction"}), 1.0)
        self.assertAlmostEqual(e.line({"id": "8", "kind": "direction", "sfx": "se/b.wav", "pauseAfter": 0.2}), 0.7)
        self.assertAlmostEqual(e.line({"id": "9", "kind": "direction", "video": "m/c.mp4"}), 12.5)
        self.assertAlmostEqual(e.line(line("10", voiceFile="v/missing.wav")), 10 / 7 + 0.35, places=3)  # missing: estimated
        self.assertAlmostEqual(e.scene({"lines": [{"id": "7", "kind": "direction"}]}), 1.6)
        with self.assertRaises(P.PlanError):
            P.Estimator(self.dir, cps=0)

    def test_media_lengths_from_hostile_files(self):
        write_mp4(os.path.join(self.dir, "v1.mp4"), 3.0, version=1)
        self.assertAlmostEqual(P.mp4_seconds(os.path.join(self.dir, "v1.mp4")), 3.0)
        bad = os.path.join(self.dir, "bad")
        for content in (b"", b"garbage" * 100, struct.pack(">I4s", 0xFFFFFFF0, b"moov") + b"\0" * 32,
                        struct.pack(">I4s", 4, b"moov"), struct.pack(">I4s", 1, b"moov") + b"\0\0",
                        (struct.pack(">I4s", 8, b"free")) * 10000):
            with open(bad, "wb") as f:
                f.write(content)
            t = time.time()
            self.assertIsNone(P.mp4_seconds(bad))
            self.assertIsNone(P.wav_seconds(bad))
            self.assertLess(time.time() - t, 2)
        write_mp4(os.path.join(self.dir, "z.mp4"), 1.0, scale=0)
        self.assertIsNone(P.mp4_seconds(os.path.join(self.dir, "z.mp4")))
        self.assertIsNone(P.mp4_seconds(os.path.join(self.dir, "nope.mp4")))


class Trim(unittest.TestCase):
    def test_passages_shown_again_and_dots(self):
        a = {"id": "s1", "lines": [line("1", text="一"), line("2", text="二"), line("3", text="三"), line("4", text="四")]}
        b = {"id": "s2", "lines": [line("5", text="一"), {"id": "d", "kind": "direction"}, line("6", text="二"), line("7", text="三"),
                                   line("8", text="新しい"), line("9", text="一"), line("10", text="二")]}
        c = {"id": "s3", "lines": [line("11", text="‥‥。"), line("12", text="‥‥", voiceFile="v.wav"), line("13", text="はい")]}
        d = {"id": "s4", "lines": [line("14", text="一"), line("15", text="二"), line("16", text="三")]}
        e = {"id": "s5", "requires": ["s4"], "lines": [line("17", text="終わり")]}
        before = json.dumps([a, b, c, d, e])
        out, st = P.trim([a, b, c, d, e])
        self.assertEqual(json.dumps([a, b, c, d, e]), before, "the input is not changed")
        ids = [[ln["id"] for ln in sc["lines"]] for sc in out]
        # 5-7 are a run of three repeats (the direction in between does not break it); 9-10 only two
        self.assertEqual(ids, [["1", "2", "3", "4"], ["d", "8", "9", "10"], ["12", "13"], ["17"]])
        self.assertEqual(st, {"replayedLines": 6, "dotLines": 1, "scenesDropped": 1})
        self.assertAlmostEqual(out[1]["lines"][-1]["pauseAfter"], 0.95)  # the dots became a pause before them
        self.assertEqual(out[3]["requires"], ["s3"])  # s4 went: what required it requires the scene before
        out, st = P.trim([a, b], min_run=0, drop_dots=False)
        self.assertEqual(st["replayedLines"], 0)


class Plan(Base):
    def run_plan(self, script=None, **kw):
        script = script or series_script()
        return script, P.plan(script, self.est(), **kw)

    def test_the_series_has_the_episodes_asked_for(self):
        script, res = self.run_plan(episodes=8, minutes=1, final_minutes=2)
        eps = res["episodes"]
        self.assertEqual(len(eps), 8)
        flat = [u["scene"]["id"] for ep in eps for u in ep]
        self.assertEqual(sorted(flat), sorted(s["id"] for s in script["scenes"]))  # every scene once
        for side in ("k-", "m-"):  # each side keeps its order
            mine = [x for x in flat if x.startswith(side) and "-f" not in x]
            self.assertEqual(mine, [s["id"] for s in script["scenes"] if s["id"] in mine])
        final = [u["scene"]["id"] for u in eps[-1]]
        self.assertEqual(final[-2:], ["k-f1", "k-f2"])
        self.assertTrue(0 < res["keep"] < 1)
        # the regular episodes have one viewpoint each and alternate within a day
        povs = [{u["scene"]["pov"] for u in ep} for ep in eps[:-1]]
        self.assertTrue(all(len(p) == 1 for p in povs[1:]), povs)
        self.assertIn({"mar"}, povs[1:4])
        whole = {"scenes": [u["scene"] for ep in eps for u in ep], "route": flat}
        self.assertEqual(requires_in_order(whole), [])

    def test_a_short_day_goes_with_the_next_and_too_few_episodes_join_days(self):
        _s, res = self.run_plan(episodes=4, minutes=6, final_minutes=0)
        self.assertEqual(len(res["episodes"]), 4)
        first_days = {u["day"] for u in res["episodes"][0]}
        self.assertIn(2, first_days)
        _s, res = self.run_plan(episodes=2, minutes=10, final_minutes=0)
        self.assertEqual(len(res["episodes"]), 2)
        _s, res = self.run_plan(episodes=1, minutes=10, final_minutes=30)  # one episode: no separate last one
        self.assertEqual(len(res["episodes"]), 1)

    def test_bad_requests_are_refused(self):
        for kw in ({"episodes": 0}, {"episodes": 100}, {"minutes": 0}, {"final_minutes": -1}):
            with self.assertRaises(P.PlanError, msg=kw):
                self.run_plan(**kw)
        with self.assertRaises(P.PlanError):
            P.plan({"scenes": []}, self.est())
        only_end = {"scenes": [scene("x", "D09-0001", "koj", title="c02 bg")], "route": []}
        with self.assertRaises(P.PlanError):
            P.plan(only_end, self.est(), episodes=3, final_minutes=10)
        _s, res = self.run_plan(series_script(), episodes=3, final_minutes=0)  # the same without a last episode is fine
        self.assertEqual(len(res["episodes"]), 3)

    def test_written_files(self):
        script = series_script()
        script["characters"]["koj"]["voice"] = {"refAudio": "v/koj.wav", "refText": "x"}
        script["scenes"][0]["background"] = "bg/a.png"
        src = os.path.join(self.dir, "src", "both.fcscript.json")
        os.makedirs(os.path.dirname(src))
        with open(src, "w", encoding="utf-8") as f:
            json.dump(script, f, ensure_ascii=False)
        res = P.plan(script, P.Estimator(os.path.dirname(src)), episodes=8, minutes=3, final_minutes=4)
        out = os.path.join(self.dir, "series")
        summary = P.write_outputs(script, res, src, out, "eve", P.Estimator(out))
        files = sorted(f for f in os.listdir(out) if f.startswith("eve.ep"))
        self.assertEqual(files, [f"eve.ep{n:02d}.fcscript.json" for n in range(1, 9)])
        self.assertTrue(os.path.isfile(os.path.join(out, "eve.plan.json")))
        with open(os.path.join(out, "eve.plan.md"), encoding="utf-8") as f:
            self.assertIn("録音のある行", f.read())
        eps = []
        for name in files:
            with open(os.path.join(out, name), encoding="utf-8") as f:
                eps.append(json.load(f))
        for ep in eps:
            self.assertEqual(validate_by_hand(ep), [])
            self.assertEqual(requires_in_order(ep), [])
            whens = [sc["when"] for sc in ep["scenes"]]
            self.assertEqual(whens, sorted(whens))
        self.assertEqual(eps[0]["title"], "第 1 話 12月2日（まりな）")
        self.assertNotIn("kyo", eps[0]["characters"])  # only who it uses
        self.assertEqual(eps[0]["scenes"][0]["background"] if eps[0]["scenes"][0]["id"] == "k-d2-0" else "../src/bg/a.png", "../src/bg/a.png")
        koj_ep = next(ep for ep in eps if "koj" in ep["characters"])
        self.assertEqual(koj_ep["characters"]["koj"]["voice"]["refAudio"], "../src/v/koj.wav")
        self.assertTrue(eps[-1]["title"].startswith("第 8 話 12月"))
        self.assertEqual(summary["episodes"][-1]["targetMinutes"], 4)
        self.assertTrue(summary["episodes"][-1]["final"])

    def test_cli(self):
        src = os.path.join(self.dir, "both.fcscript.json")
        with open(src, "w", encoding="utf-8") as f:
            json.dump(series_script(), f, ensure_ascii=False)
        out = os.path.join(self.dir, "o")
        tool = os.path.join(TOOLS, "episode_plan.py")
        r = subprocess.run([sys.executable, "-X", "utf8", tool, "--script", src, "--out-dir", out, "--episodes", "6", "--minutes", "3", "--final-minutes", "4"],
                           capture_output=True, text=True, encoding="utf-8")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(len(json.loads(r.stdout)["episodes"]), 6)
        for extra in (["--stem", "../x"], ["--episodes", "0"], ["--min-run", "-1"]):
            r = subprocess.run([sys.executable, "-X", "utf8", tool, "--script", src, "--out-dir", out] + extra, capture_output=True, text=True, encoding="utf-8")
            self.assertEqual(r.returncode, 2, extra)
        with open(src, "w", encoding="utf-8") as f:
            f.write("{not json")
        r = subprocess.run([sys.executable, "-X", "utf8", tool, "--script", src, "--out-dir", out], capture_output=True, text=True, encoding="utf-8")
        self.assertEqual(r.returncode, 2)


class Adaptation(Base):
    def draft(self):
        return {"version": 1, "title": "第 1 話", "characters": {"koj": {"name": "小次郎"}, "kyo": {"name": "恭子"}},
                "scenes": [
                    {"id": "s1", "lines": [line("s1-1"), line("s1-2", "kyo", "録音の台詞", voiceFile="v/1.wav"), line("s1-3"), line("s1-4")]},
                    {"id": "s2", "lines": [line("s2-1"), line("s2-2", "kyo", "二つ目", voiceFile="v/2.wav")]},
                    {"id": "s3", "lines": [line("s3-1")]},
                ], "route": ["s1", "s2", "s3"]}

    def check(self, edited, **kw):
        return C.check(self.draft(), edited, self.dir, self.dir, est=self.est(), **kw)

    def test_a_good_adaptation(self):
        ed = self.draft()
        s1 = ed["scenes"][0]["lines"]
        s1[0] = dict(s1[0], text="まとめた一行", generated=True)  # s1-1 rewritten, s1-3 merged into it
        s1[1] = dict(s1[1], reading="ろくおんのせりふ")  # a reading may be added
        del s1[2]
        ed["scenes"][1]["lines"].insert(1, line("s2-g01", text="つなぎ", generated=True))
        del ed["scenes"][2]
        ed["route"] = ["s1", "s2"]
        r = self.check(ed)
        self.assertTrue(r["ok"], r["errors"])
        self.assertEqual((r["kept"], r["rewritten"], r["new"], r["deleted"]), (4, 1, 1, 2))
        self.assertEqual((r["recordedKept"], r["recordedDeleted"], r["scenesDeleted"]), (2, 0, 1))
        self.assertLessEqual(r["minutes"], r["draftMinutes"])

    def test_broken_rules(self):
        def broken(change, needle):
            ed = self.draft()
            change(ed)
            r = self.check(ed)
            self.assertFalse(r["ok"], needle)
            self.assertTrue(any(needle in e for e in r["errors"]), (needle, r["errors"]))

        broken(lambda ed: ed["scenes"][0]["lines"][1].update(text="変えた"), "recorded, so its text")
        broken(lambda ed: ed["scenes"][0]["lines"][1].update(voiceFile="v/9.wav"), "recorded, so its voiceFile")
        broken(lambda ed: ed["scenes"][0]["lines"][0].update(text="変えた"), "not marked \"generated\"")
        broken(lambda ed: ed["scenes"][0]["lines"][0].update(speaker="kyo"), "its speaker changed")
        broken(lambda ed: ed["scenes"][0]["lines"][0].update(kind="narration"), "its kind changed")
        broken(lambda ed: ed["scenes"][0]["lines"][0].update(voiceFile="v/1.wav"), "now has one")
        broken(lambda ed: ed["scenes"][0]["lines"].append(line("x1")), "new, but not marked")
        broken(lambda ed: ed["scenes"][0]["lines"].append(line("x2", generated=True, voiceFile="v/1.wav")), "new, but has a recording")
        broken(lambda ed: ed["scenes"][0]["lines"].reverse(), "moved before a line")
        broken(lambda ed: ed["scenes"].reverse() or ed.update(route=["s3", "s2", "s1"]), "moved before a scene")
        broken(lambda ed: ed["scenes"][1]["lines"].append(ed["scenes"][0]["lines"].pop()), "moved from scene")
        broken(lambda ed: (ed["scenes"].append({"id": "s9", "lines": []}), ed["route"].append("s9")), "not in the draft")
        broken(lambda ed: ed["scenes"][0]["lines"].append(copy.deepcopy(ed["scenes"][0]["lines"][0])), "appears twice")
        r = self.check(self.draft(), min_minutes=5)
        self.assertTrue(any("shorter than 5" in e for e in r["errors"]))
        r = self.check(self.draft(), max_minutes=0.01)
        self.assertTrue(any("longer than" in e for e in r["errors"]))

    def test_recording_paths_are_compared_as_files(self):
        sub = os.path.join(self.dir, "adapted")
        os.makedirs(sub)
        ed = self.draft()
        for sc in ed["scenes"]:
            for ln in sc["lines"]:
                if ln.get("voiceFile"):
                    ln["voiceFile"] = "../" + ln["voiceFile"]
        r = C.check(self.draft(), ed, self.dir, sub, est=P.Estimator(sub))
        self.assertTrue(r["ok"], r["errors"])

    def test_cli_exit_codes(self):
        src = os.path.join(self.dir, "d.fcscript.json")
        ed_path = os.path.join(self.dir, "e.fcscript.json")
        with open(src, "w", encoding="utf-8") as f:
            json.dump(self.draft(), f, ensure_ascii=False)
        tool = os.path.join(TOOLS, "check_adaptation.py")

        def run(ed):
            with open(ed_path, "w", encoding="utf-8") as f:
                f.write(ed if isinstance(ed, str) else json.dumps(ed, ensure_ascii=False))
            return subprocess.run([sys.executable, "-X", "utf8", tool, "--source", src, "--edited", ed_path], capture_output=True, text=True, encoding="utf-8")

        r = run(self.draft())
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertTrue(json.loads(r.stdout)["ok"])
        bad = self.draft()
        bad["scenes"][0]["lines"][1]["text"] = "x"
        self.assertEqual(run(bad).returncode, 1)
        self.assertEqual(run("[]").returncode, 2)


@unittest.skipUnless(os.path.isdir(os.path.join(REAL_ROOT, "01_シナリオ")), "the decoded dump is not here")
class RealData(Base):
    def test_both_routes_plan_into_29_valid_episodes(self):
        paths = []
        for route in ("kojirou", "marina"):
            p = os.path.join(self.dir, f"{route}.fcscript.json")
            s, _ = E.convert(REAL_ROOT, p, route, E.Options())
            with open(p, "w", encoding="utf-8") as f:
                json.dump(s, f, ensure_ascii=False)
            paths.append(p)
        both = os.path.join(self.dir, "both.fcscript.json")
        s, _ = M.merge([paths[0]], [paths[1]], both)
        with open(both, "w", encoding="utf-8") as f:
            json.dump(s, f, ensure_ascii=False)
        res = P.plan(s, P.Estimator(self.dir))
        self.assertEqual(len(res["episodes"]), 29)
        self.assertGreater(res["trim"]["replayedLines"], 0)
        out = os.path.join(self.dir, "series")
        summary = P.write_outputs(s, res, both, out, "eve", P.Estimator(out))
        for row in summary["episodes"]:
            with open(os.path.join(out, row["file"]), encoding="utf-8") as f:
                ep = json.load(f)
            self.assertEqual(validate_by_hand(ep), [], row["file"])
            self.assertEqual(requires_in_order(ep), [], row["file"])
        self.assertTrue(summary["episodes"][-1]["final"])


if __name__ == "__main__":
    unittest.main()
