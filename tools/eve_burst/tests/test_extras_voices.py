import array
import json
import math
import os
import sys
import tempfile
import unittest
import wave

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))

import extras_voices as ev  # noqa: E402


def tone(path, f0, seconds=1.0, sr=24000, amp=0.3):
    """A buzzy voiced tone (five harmonics) with a short silent lead-in."""
    n = int(sr * seconds)
    a = array.array("h")
    for i in range(n):
        t = i / sr
        v = 0.0 if i < sr // 10 else sum(math.sin(2 * math.pi * f0 * h * t) / h for h in range(1, 6))
        a.append(int(max(-1.0, min(1.0, amp * v)) * 32000))
    if sys.byteorder == "big":
        a.byteswap()
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sr)
        w.writeframes(a.tobytes())


def script():
    return {
        "version": 1, "title": "t",
        "characters": {
            "koj": {"name": "小次郎"}, "mar": {"name": "まりな"}, "narrator": {"name": "語り"},
            "n001": {"name": "守衛"}, "n002": {"name": "男"}, "n003": {"name": "サラリーマン"},
            "n004": {"name": "女"}, "n005": {"name": "謎の影"}, "sfx": {"name": "擬音"},
            "n006": {"name": "受付", "voice": {"speaker": "Ono_Anna"}},
        },
        "route": ["s1", "s2", "s3", "s4"],
        "scenes": [
            {"id": "s1", "pov": "koj", "background": "bg/a.png", "lines": [
                {"id": "1", "speaker": "narrator", "text": "夜だ。"},
                {"id": "2", "speaker": "n001", "text": "誰だ。"},
                {"id": "3", "speaker": "n002", "text": "俺だ。"},
                {"id": "3b", "speaker": "n002", "text": "本当だ。"},
                {"id": "4", "speaker": "n003", "text": "私です。"},
                {"id": "5", "speaker": "sfx", "text": "ガシャーン", "voiceFile": "v/x.wav"},
            ]},
            {"id": "s2", "pov": "mar", "lines": [
                {"id": "6", "speaker": "narrator", "text": "朝だ。"},
                {"id": "7", "speaker": "n004", "text": "あら。"},
            ]},
            {"id": "s3", "lines": [{"id": "8", "speaker": "narrator", "text": "まだ朝だ。"}]},
            {"id": "s4", "pov": "koj", "lines": [
                {"id": "9", "speaker": "narrator", "text": "昼。"},
                {"id": "10", "speaker": "narrator", "text": "まだ昼。"},
                {"id": "11", "speaker": "n005", "text": "ふふ。"},
                {"id": "12", "kind": "direction", "sfx": "se/a.wav"},
            ]},
        ],
    }


class Classify(unittest.TestCase):
    def test_keywords_pick_the_class_and_marks_pick_the_variant(self):
        self.assertEqual(ev.guess_class("守衛")[0], "gatekeeper")
        self.assertEqual(ev.guess_class("警備員")[0], "security")
        self.assertEqual(ev.guess_class("女警備員")[0], "female-mid")
        self.assertEqual(ev.guess_class("警官の声")[0], "police")
        self.assertEqual(ev.guess_class("サラリーマン")[0], "male-mid")
        self.assertEqual(ev.guess_class("フロント")[0], "front")
        self.assertEqual(ev.guess_class("ウエイトレス")[0], "female-young")
        self.assertEqual(ev.guess_class("男の子")[0], "child")
        self.assertEqual(ev.guess_class("若い男")[0], "male-young")
        self.assertEqual(ev.guess_class("声２")[0], "unknown")
        self.assertEqual(ev.guess_class("ＴＶ")[0], "announcer")
        self.assertEqual(ev.guess_class("")[0], "unknown")
        self.assertEqual([ev.variant_of(n) for n in ("男１", "男２", "声Ｂ", "左の女", "右の女", "男")], [0, 1, 1, 0, 2, 0])

    def test_role_of_prefers_the_table_and_survives_a_broken_one(self):
        t = ev.normalize_table({"names": {"男": {"class": "male-old", "voice": "man-c"}, "女": "bad", "X": {"class": "nope"}}, "classes": 7})
        self.assertEqual(ev.role_of(t, "男")["voice"], "man-c")
        self.assertEqual(ev.role_of(t, "男")["class"], "male-old")
        self.assertEqual(ev.role_of(t, "女")["class"], "female-mid")  # not a dict: guessed
        self.assertFalse(ev.role_of(t, "女")["listed"])
        self.assertEqual(ev.role_of(t, "X")["class"], "unknown")
        self.assertIn("male-mid", t["classes"])  # a broken classes value falls back to the defaults
        self.assertEqual(ev.normalize_table(None)["aliases"], {"pur": "pul"})


class Pitch(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()

    def tearDown(self):
        self.tmp.cleanup()

    def test_f0_of_a_low_and_a_high_tone_and_bad_files(self):
        lo, hi = os.path.join(self.tmp.name, "lo.wav"), os.path.join(self.tmp.name, "hi.wav")
        tone(lo, 110, 1.0)
        tone(hi, 220, 1.0, sr=48000)
        a, b = ev.f0_stats(lo), ev.f0_stats(hi)
        self.assertAlmostEqual(a["f0"], 110, delta=6)
        self.assertAlmostEqual(b["f0"], 220, delta=10)
        self.assertAlmostEqual(a["seconds"], 1.0, delta=0.01)
        junk = os.path.join(self.tmp.name, "junk.wav")
        with open(junk, "wb") as f:
            f.write(b"RIFF\x00\x00")
        with self.assertRaises(ValueError):
            ev.f0_stats(junk)
        silent = os.path.join(self.tmp.name, "silent.wav")
        tone(silent, 110, 0.5, amp=0.0)
        self.assertEqual(ev.f0_stats(silent)["f0"], 0.0)

    def test_pick_prefers_a_fitting_steady_long_candidate(self):
        c = [
            {"file": "a", "f0": 200.0, "p10": 190, "p90": 210, "seconds": 8.0},   # too high for a man
            {"file": "b", "f0": 110.0, "p10": 80, "p90": 160, "seconds": 8.0},    # fits, wobbly
            {"file": "c", "f0": 115.0, "p10": 105, "p90": 125, "seconds": 7.0},   # fits, steady
            {"file": "d", "f0": 100.0, "p10": 99, "p90": 101, "seconds": 1.0},    # too short
        ]
        first = dict(c[0])
        self.assertEqual(ev.pick_candidate(c, "male")["file"], "c")
        self.assertEqual(ev.pick_candidate([first], "female")["file"], "a")
        self.assertIsNone(ev.pick_candidate([], "male"))

    def test_choose_copies_the_pick_and_records_it(self):
        cand = os.path.join(self.tmp.name, "cand")
        os.makedirs(cand)
        tone(os.path.join(cand, "man-a-1.wav"), 240, 4.0)
        tone(os.path.join(cand, "man-a-2.wav"), 120, 4.0)
        t = ev.default_table()
        entry, msg = ev.choose(t, "man-a", cand, self.tmp.name)
        self.assertTrue(entry["refAudio"].endswith("/man-a.wav"))
        self.assertTrue(os.path.isfile(os.path.join(self.tmp.name, "man-a.wav")))
        self.assertAlmostEqual(entry["f0"], 120, delta=6)
        self.assertTrue(entry["why"].startswith("fits"))
        self.assertEqual(len(entry["alternatives"]), 1)
        self.assertEqual(entry["refText"], t["voices"]["man-a"]["refText"])
        self.assertIsNone(ev.choose(t, "child", cand, self.tmp.name)[0])


class DesignJobs(unittest.TestCase):
    def test_jobs_per_voice_skip_existing_candidates_and_report_bad_voices(self):
        with tempfile.TemporaryDirectory() as d:
            t = ev.default_table()
            t["voices"]["broken"] = {"design": 3}
            open(os.path.join(d, "thug-2.wav"), "wb").close()
            jobs, problems = ev.design_jobs(t, ["thug", "broken"], d, 3, set())
            self.assertEqual([j["id"] for j in jobs], ["thug-1", "thug-3"])
            self.assertEqual(jobs[0]["design"], t["voices"]["thug"]["design"])
            self.assertEqual(jobs[0]["text"], t["voices"]["thug"]["refText"])
            self.assertTrue(jobs[0]["out"].endswith("cand/thug-1.wav") or jobs[0]["out"].endswith("/thug-1.wav"))
            self.assertEqual(len(problems), 1)
            jobs, _ = ev.design_jobs(t, ["thug"], d, 99, {"thug"})
            self.assertEqual(len(jobs), ev.MAX_CANDIDATES)

    def test_used_voices_follow_the_names_in_the_scripts(self):
        t = ev.default_table()
        names = {"守衛": {"how": "role", "lines": 3}, "(ref:may) 真弥子": {"how": "ref:may", "lines": 9}}
        self.assertEqual(ev.used_voices(t, names), ["man-b", "gatekeeper"])


class Apply(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        r = self.tmp.name
        self.root = os.path.join(r, "data")
        os.makedirs(os.path.join(self.root, "v"))
        for n in ("koj.wav", "mar.wav"):
            open(os.path.join(self.root, "v", n), "wb").close()
        self.refs = {"koj": {"refAudio": "v/koj.wav", "refText": "k"}, "mar": {"refAudio": "v/mar.wav", "refText": "m"}}
        self.xdir = os.path.join(r, "voices", "extras")
        os.makedirs(self.xdir)
        self.xrefs = {}
        for v in ("gatekeeper", "man-b", "man-a", "male-mid-2", "woman-a", "unknown"):
            p = os.path.join(self.xdir, v + ".wav")
            open(p, "wb").close()
            self.xrefs[v] = {"refAudio": p.replace("\\", "/"), "refText": "t-" + v}
        self.src = os.path.join(r, "series")
        self.out = os.path.join(r, "out", "voiced")
        os.makedirs(self.src)

    def tearDown(self):
        self.tmp.cleanup()

    def test_named_narrator_and_extras_get_fixed_voices_without_sharing_in_a_scene(self):
        s = script()
        t = ev.default_table()
        rep = ev.apply_script(s, self.refs, self.root, t, self.xrefs, self.src, self.out)
        c = s["characters"]
        self.assertEqual(sorted(rep["cloned"]), ["koj", "mar"])
        self.assertEqual(c["koj"]["voice"]["refAudio"], "../../data/v/koj.wav")
        # narration: koj has 3 lines (s1, s4 x2), mar 2 (s2 and s3, which follows s2): koj is the narrator
        self.assertEqual(c["narrator"]["voice"]["refText"], "k")
        self.assertEqual(c["narrator-mar"]["voice"]["refText"], "m")
        self.assertEqual(c["narrator-mar"]["name"], "語り")
        speakers = {ln["id"]: ln.get("speaker") for sc in s["scenes"] for ln in sc["lines"]}
        self.assertEqual((speakers["6"], speakers["8"], speakers["9"]), ("narrator-mar", "narrator-mar", "narrator"))
        self.assertEqual(rep["narratorMoved"], {"narrator-mar": 2})
        # extras by name: 守衛 -> gatekeeper; 男 and サラリーマン share s1, so the second one moves on
        self.assertEqual(rep["extras"]["守衛"], "gatekeeper")
        self.assertEqual(rep["extras"]["男"], "man-a")
        self.assertEqual(rep["extras"]["サラリーマン"], "male-mid-2")
        self.assertEqual(rep["moved"], ["サラリーマン: man-a -> male-mid-2"])
        self.assertEqual(c["n001"]["voice"], {"refAudio": "../../voices/extras/gatekeeper.wav", "refText": "t-gatekeeper"})
        self.assertEqual(rep["extras"]["女"], "woman-a")
        self.assertEqual(rep["extras"]["謎の影"], "unknown")
        self.assertIn("謎の影", rep["unlisted"])
        # kept, skipped, nothing left over, and the background path rebased to the new folder
        self.assertEqual(c["n006"]["voice"], {"speaker": "Ono_Anna"})
        self.assertNotIn("voice", c["sfx"])
        self.assertEqual(rep["remaining"], {})
        self.assertEqual(s["scenes"][0]["background"], "../../series/bg/a.png")
        self.assertEqual(s["scenes"][0]["lines"][5]["voiceFile"], "../../series/v/x.wav")

    def test_a_name_bigger_over_the_series_keeps_its_voice_in_a_clash(self):
        t = ev.default_table()
        s = script()  # here サラリーマン has 1 line and 男 2, but over the series サラリーマン is bigger
        rep = ev.apply_script(s, self.refs, self.root, t, self.xrefs, self.src, self.out, priority={"サラリーマン": 90, "男": 5})
        self.assertEqual(rep["extras"]["サラリーマン"], "man-a")
        self.assertEqual(rep["extras"]["男"], "male-mid-2")

    def test_the_same_name_gets_the_same_voice_in_every_script(self):
        t = ev.default_table()
        got = set()
        for extra in ({}, {"n009": {"name": "警官"}}):
            s = script()
            s["characters"].update(extra)
            s["characters"]["n042"] = s["characters"].pop("n001")  # another id for 守衛
            for sc in s["scenes"]:
                for ln in sc["lines"]:
                    if ln.get("speaker") == "n001":
                        ln["speaker"] = "n042"
            ev.apply_script(s, self.refs, self.root, t, self.xrefs, self.src, self.out)
            got.add(s["characters"]["n042"]["voice"]["refAudio"])
        self.assertEqual(len(got), 1)

    def test_missing_references_and_hostile_scripts_do_not_crash(self):
        t = ev.default_table()
        rep = ev.apply_script(script(), {}, self.root, t, {}, self.src, self.out)
        self.assertIn("守衛 [n001]", rep["remaining"])
        self.assertIn("narrator", rep["voiceless"])
        self.assertTrue(rep["missingRef"])
        for bad in ({}, {"characters": []}, {"characters": {"a": 1}, "scenes": [3, {"lines": [None, {"speaker": 5}]}]}, []):
            ev.apply_script(bad, self.refs, self.root, t, self.xrefs, self.src, self.out)
        self.assertEqual(ev.collect([("x", {"characters": {"a": 2}, "scenes": "no"})], {}, t), {})

    def test_main_init_list_and_apply_write_copies(self):
        sp = os.path.join(self.src, "eve.ep01.fcscript.json")
        with open(sp, "w", encoding="utf-8") as f:
            json.dump(script(), f, ensure_ascii=False)
        rp = os.path.join(self.tmp.name, "refs.json")
        with open(rp, "w", encoding="utf-8") as f:
            json.dump(self.refs, f)
        vdir = os.path.dirname(self.xdir)
        with open(os.path.join(self.xdir, "extras_refs.json"), "w", encoding="utf-8") as f:
            json.dump(self.xrefs, f)
        glob_ = os.path.join(self.src, "*.fcscript.json")
        self.assertEqual(ev.main(["--scripts", glob_, "--voices-dir", vdir, "--init"]), 0)
        with open(os.path.join(vdir, "extras_roles.json"), encoding="utf-8") as f:
            table = json.load(f)
        self.assertEqual(table["names"]["守衛"]["class"], "gatekeeper")
        self.assertNotIn("真弥子", table["names"])
        table["names"]["男"]["voice"] = "man-b"  # a user edit survives a second --init
        with open(os.path.join(vdir, "extras_roles.json"), "w", encoding="utf-8") as f:
            json.dump(table, f, ensure_ascii=False)
        self.assertEqual(ev.main(["--scripts", glob_, "--voices-dir", vdir, "--init"]), 0)
        with open(os.path.join(vdir, "extras_roles.json"), encoding="utf-8") as f:
            self.assertEqual(json.load(f)["names"]["男"]["voice"], "man-b")
        self.assertEqual(ev.main(["--scripts", glob_, "--voices-dir", vdir, "--list"]), 0)
        rc = ev.main(["--scripts", glob_, "--voices-dir", vdir, "--apply", "--refs", rp, "--root", self.root,
                      "--out-dir", self.out, "--tag", "voiced", "--report", os.path.join(self.out, "r.json")])
        self.assertEqual(rc, 0)
        with open(os.path.join(self.out, "eve.ep01.voiced.fcscript.json"), encoding="utf-8") as f:
            done = json.load(f)
        self.assertEqual(done["characters"]["n002"]["voice"]["refText"], "t-man-b")
        with open(sp, encoding="utf-8") as f:
            self.assertNotIn("voice", json.load(f)["characters"]["koj"])  # the input is unchanged
        # writing over the input needs --in-place; no action at all is an error
        self.assertEqual(ev.main(["--scripts", sp, "--voices-dir", vdir, "--apply", "--refs", rp, "--root", self.root, "--out-dir", self.src]), 2)
        self.assertEqual(ev.main(["--scripts", os.path.join(self.tmp.name, "none", "*.json"), "--voices-dir", vdir, "--apply", "--out-dir", self.out]), 2)
        with self.assertRaises(SystemExit):
            ev.main(["--scripts", sp])

    def test_design_dry_run_writes_jobs_for_missing_voices_only(self):
        sp = os.path.join(self.src, "eve.ep01.fcscript.json")
        with open(sp, "w", encoding="utf-8") as f:
            json.dump(script(), f, ensure_ascii=False)
        vdir = os.path.dirname(self.xdir)
        with open(os.path.join(self.xdir, "extras_refs.json"), "w", encoding="utf-8") as f:
            json.dump(self.xrefs, f)
        os.remove(os.path.join(self.xdir, "gatekeeper.wav"))  # its reference is gone: design it again
        self.xrefs.pop("unknown")
        rc = ev.main(["--scripts", sp, "--voices-dir", vdir, "--design", "--dry-run", "--voices", "gatekeeper,man-a,thug"])
        self.assertEqual(rc, 0)
        with open(os.path.join(self.xdir, "design_jobs.extras.jsonl"), encoding="utf-8") as f:
            ids = [json.loads(x)["id"] for x in f]
        self.assertEqual(ids, ["gatekeeper-1", "gatekeeper-2", "gatekeeper-3", "thug-1", "thug-2", "thug-3"])


if __name__ == "__main__":
    unittest.main()
