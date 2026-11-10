import io
import json
import os
import sys
import tempfile
import time
import unittest
from contextlib import redirect_stderr, redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))

import episode_run as er  # noqa: E402


def write(p, v):
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with io.open(p, "w", encoding="utf-8") as f:
        f.write(v if isinstance(v, str) else json.dumps(v, ensure_ascii=False))


def read(p):
    with io.open(p, encoding="utf-8") as f:
        return json.load(f)


def script(voiced=True):
    return {"characters": {"koj": {"name": "小次郎", **({"voice": {"refAudio": "../voice/k.wav", "refText": "…"}} if voiced else {}),
                                   "expressions": {"00": {"image": "../pics/k.png"}}}},
            "scenes": [{"id": "s1", "background": "../pics/bg.png", "lines": [
                {"id": "l1", "speaker": "koj", "text": "おい！！"},
                {"id": "l2", "speaker": "koj", "text": "ガシャーン。", "spoken": False},
                {"id": "l3", "speaker": "koj", "text": "録音あり", "voiceFile": "../voice/l3.wav"}]}]}


class Helpers(unittest.TestCase):
    def test_source_order(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertIsNone(er.source_for(2, d))
            write(f"{d}/series/eve.ep02.fcscript.json", "{}")
            self.assertTrue(er.source_for(2, d).endswith("series/eve.ep02.fcscript.json"))
            write(f"{d}/voiced/eve.ep02.voiced.fcscript.json", "{}")
            self.assertTrue(er.source_for(2, d).endswith("eve.ep02.voiced.fcscript.json"))
            write(f"{d}/voiced/eve.ep02.adapted.voiced.fcscript.json", "{}")
            self.assertTrue(er.source_for(2, d).endswith("adapted.voiced.fcscript.json"))

    def test_a_stale_source_is_refused(self):
        def sc(title, ids):
            return {"title": title, "scenes": [{"id": "s", "lines": [{"id": i} for i in ids]}]}
        with tempfile.TemporaryDirectory() as d:
            write(f"{d}/adapted/eve.ep05.fcscript.json", sc("第 5 話", ["a", "b", "c"]))
            good = f"{d}/voiced/eve.ep05.adapted.voiced.fcscript.json"
            write(good, sc("第 5 話", ["a", "b", "c"]))
            self.assertIsNone(er.check_source(5, good, d))
            # the adapted script itself is always fine
            self.assertIsNone(er.check_source(5, f"{d}/adapted/eve.ep05.fcscript.json", d))
            # same title, other lines (the copy kept from before the renumbering)
            write(good, sc("第 5 話", ["x", "y"]))
            self.assertIn("stale", er.check_source(5, good, d))
            # another episode's title
            write(good, sc("第 6 話", ["a", "b", "c"]))
            self.assertIn("episode 6", er.check_source(5, good, d))
            # no adapted/ to compare with: only the title counts
            write(f"{d}/voiced/eve.ep07.voiced.fcscript.json", sc("第 7 話", ["q"]))
            self.assertIsNone(er.check_source(7, f"{d}/voiced/eve.ep07.voiced.fcscript.json", d))
            # unreadable and hostile input is a reason, not a crash
            write(f"{d}/voiced/eve.ep08.voiced.fcscript.json", "not json")
            self.assertIn("cannot read", er.check_source(8, f"{d}/voiced/eve.ep08.voiced.fcscript.json", d))
            self.assertEqual(er.line_ids([1, 2]), [])
            self.assertEqual(er.line_ids({"scenes": [5, {"lines": 3}, {"lines": [7, {"id": 9}, {"id": "z"}]}]}), ["z"])

    def test_rebase_rewrites_existing_relative_paths_only(self):
        with tempfile.TemporaryDirectory() as d:
            write(f"{d}/voice/k.wav", "x")
            write(f"{d}/pics/k.png", "x")
            s = er.rebase(script(), f"{d}/voiced", f"{d}/work/ep02")
            self.assertEqual(s["characters"]["koj"]["voice"]["refAudio"], "../../voice/k.wav")
            self.assertEqual(s["characters"]["koj"]["expressions"]["00"]["image"], "../../pics/k.png")
            self.assertEqual(s["scenes"][0]["background"], "../pics/bg.png")  # missing file: left as it is
            self.assertEqual(s["scenes"][0]["lines"][0]["text"], "おい！！")  # not a path key

    def test_fresh(self):
        with tempfile.TemporaryDirectory() as d:
            a, b = f"{d}/a", f"{d}/b"
            self.assertFalse(er.fresh(b, [a]))
            write(a, "x")
            write(b, "y")
            os.utime(a, (time.time() - 10, time.time() - 10))
            self.assertTrue(er.fresh(b, [a, f"{d}/missing"]))
            os.utime(a, (time.time() + 10, time.time() + 10))
            self.assertFalse(er.fresh(b, [a]))

    def test_all_voiced(self):
        self.assertTrue(er.all_voiced(script()))
        self.assertFalse(er.all_voiced(script(voiced=False)))
        self.assertTrue(er.all_voiced({"scenes": "x"}))

    def test_labels_to_review(self):
        rows = [["a", "koj", "問い", "中", "text", "ほぉ～、それでぇ？"], ["b", "koj", "怒り", "強", "text", "この大馬鹿者！！"],
                ["c", "aka", "問い", "中", "expression", "ほぉ？"], ["d", "koj", "静か", "弱", "text", "‥‥やれやれ、大したもんだな。"], ["short"]]
        self.assertEqual([r["id"] for r in er.labels_to_review(rows)], ["a", "d"])
        sigh = [["e", "koj", "静か", "弱", "text", "‥‥はぁぁ‥‥。"]]
        self.assertEqual(er.labels_to_review(sigh), [])  # a sigh is quiet, nothing to review

    def test_missing_variants_and_summary(self):
        rep = {"missing": [{"base": "aka00ad", "kinds": ["blink"]}, {"base": "aka05ad", "kinds": ["open", "a", "blink"]}, "junk", {"kinds": ["a"]}]}
        self.assertEqual(er.missing_variants(rep), {"noMouth": ["aka05ad"], "noBlink": ["aka00ad", "aka05ad"]})
        self.assertEqual(er.missing_variants("not json"), {"noMouth": [], "noBlink": []})
        build = {"ok": True, "steps": [{"step": "check", "errors": 0, "warnings": 2, "problems": 2}, {"step": "build", "seconds": 600.5},
                                       {"step": "export", "ok": True, "path": "x.mp4"}]}
        r = er.summarize(build, {"lines": {"l1": {"words": {"絵画": "かいが"}}, "l2": {"words": {}}}}, [], {"noMouth": ["x"], "noBlink": []}, "y.mp4")
        self.assertEqual((r["ok"], r["checkWarnings"], r["seconds"], r["mp4"], r["misreadsLeft"]), (True, 2, 600.5, "x.mp4", {"l1": {"絵画": "かいが"}}))
        self.assertEqual(r["spritesWithoutMouth"], ["x"])
        self.assertFalse(er.summarize({}, {}, [], {}, "y.mp4")["ok"])


class FakeRun(er.Run):
    """Records every tool call and writes the file a call would write."""

    calls: list = []

    def call(self, args, out=None):
        name = os.path.basename(args[3]) if args[0].endswith(("python.exe", "python")) and len(args) > 3 else os.path.basename(args[0])
        rest = args[4:] if name.endswith(".py") else args
        FakeRun.calls.append(name if not name.endswith(".exe") else next(x for x in args if x in ("script.synthesize", "anime.run")))
        target = None
        if "--out" in rest:
            target = rest[rest.index("--out") + 1]
        elif name == "onomatopoeia.py":
            target = rest[1]
        if target:
            src = next((rest[rest.index(k) + 1] for k in ("--script", "--scripts") if k in rest), rest[0] if name == "onomatopoeia.py" else None)
            with io.open(src, encoding="utf-8") as f:
                data = f.read()
            with io.open(target, "w", encoding="utf-8") as f:
                f.write(data)
        if out:
            with io.open(out, "w", encoding="utf-8") as f:
                f.write("{}")
        if name == "tts_readings.py" and args[4] == "check":
            rep = rest[rest.index("--report") + 1]
            with io.open(rep, "w", encoding="utf-8") as f:
                f.write(json.dumps({"lines": {}}))
            return json.dumps({"checked": 1, "flagged": 0})
        if name.endswith(".exe") and "anime.run" in args:
            mp4 = json.loads(args[-1])["export"]
            with io.open(mp4, "w", encoding="utf-8") as f:
                f.write("mp4")
            res = {"ok": True, "steps": [{"step": "check", "errors": 0, "warnings": 1}, {"step": "build", "seconds": 12.0}, {"step": "export", "ok": True, "path": mp4}]}
            with io.open(out, "w", encoding="utf-8") as f:
                f.write(json.dumps(res))
            return json.dumps(res)
        return "{}"


class Stages(unittest.TestCase):
    def setUp(self):
        self._out = redirect_stdout(io.StringIO())
        self._out.__enter__()

    def tearDown(self):
        self._out.__exit__(None, None, None)

    def args(self, d, src):
        return er.main.__globals__["argparse"].Namespace(
            episode=2, script=src, work_dir=f"{d}/ep02", out=d, root=d, python="python.exe", cli="filmcraft-cli.exe", whisper="w", ctc="c",
            names="n.json", sprites=f"{d}/sprites", expressions="e.json", styles=f"{d}/styles.json", overrides="o.json", force=None, dry_run=False, seed_cache=None,
            screen_donor=None, siblings=None, no_screen=False, ignore_gpu=True, allow_stale_cli=False)

    def test_every_stage_runs_once_then_everything_is_skipped(self):
        with tempfile.TemporaryDirectory() as d:
            src = f"{d}/voiced/eve.ep02.adapted.voiced.fcscript.json"
            write(src, script())
            write(f"{d}/styles.json", "{}")
            FakeRun.calls = []
            r = FakeRun(self.args(d, src)).run()
            self.assertEqual(FakeRun.calls, ["protagonist_sprites.py", "onomatopoeia.py", "sprite_variants.py", "emotion_labels.py",
                                             "tts_readings.py", "voice_styles.py", "script.synthesize", "tts_readings.py", "anime.run"])
            self.assertTrue(r["ok"])
            self.assertEqual(r["seconds"], 12.0)
            self.assertTrue(os.path.isfile(f"{d}/ep02/ep02.result.json"))
            self.assertNotIn("extras_voices.py", FakeRun.calls)  # every speaker had a voice
            # a second run: nothing to do
            FakeRun.calls = []
            FakeRun(self.args(d, src)).run()
            self.assertEqual(FakeRun.calls, [])
            # a newer labels file (a hand fix) redoes what comes after it, not what comes before
            time.sleep(0.05)
            p = f"{d}/ep02/ep02.6-labels.fcscript.json"
            os.utime(p, None)
            FakeRun.calls = []
            FakeRun(self.args(d, src)).run()
            self.assertEqual(FakeRun.calls, ["tts_readings.py", "voice_styles.py", "script.synthesize", "tts_readings.py", "anime.run"])

    def test_screen_stage_runs_when_a_conversion_with_screen_fields_is_there(self):
        with tempfile.TemporaryDirectory() as d:
            src = f"{d}/voiced/eve.ep02.adapted.voiced.fcscript.json"
            write(src, script())
            write(f"{d}/styles.json", "{}")
            write(f"{d}/routes_screen/kojirou.fcscript.json", script())
            FakeRun.calls = []
            r = FakeRun(self.args(d, src)).run()
            self.assertEqual(FakeRun.calls[-3:], ["tts_readings.py", "screen_fields.py", "anime.run"])
            self.assertTrue(r["script"].endswith("ep02.10-screen.fcscript.json"))
            self.assertEqual(r["source"], src)
            self.assertEqual(len(r["sourceSha256"]), 64)
            self.assertEqual(r["sourceLines"], len(er.line_ids(script())))
            FakeRun.calls = []
            FakeRun(self.args(d, src)).run()
            self.assertEqual(FakeRun.calls, [])
            # a newer conversion: the screen and the build again, no synthesis
            time.sleep(0.05)
            os.utime(f"{d}/routes_screen/kojirou.fcscript.json", None)
            FakeRun.calls = []
            FakeRun(self.args(d, src)).run()
            self.assertEqual(FakeRun.calls, ["screen_fields.py", "anime.run"])
            # --no-screen: built from the spoken script
            a = self.args(d, src)
            a.no_screen = True
            a.force = ["build"]
            FakeRun.calls = []
            r = FakeRun(a).run()
            self.assertEqual(FakeRun.calls, ["anime.run"])
            self.assertFalse(r["script"].endswith("10-screen.fcscript.json"))

    def test_a_stale_cli_stops_the_run_before_anything_is_made(self):
        with tempfile.TemporaryDirectory() as d:
            src = f"{d}/adapted/eve.ep02.fcscript.json"
            write(src, script())
            a = self.args(d, src)
            a.cli = f"{d}/old-cli.exe"
            write(a.cli, "x")
            os.utime(a.cli, (1000, 1000))  # older than any source of the repository
            FakeRun.calls = []
            with self.assertRaises(RuntimeError) as e:
                FakeRun(a).run()
            self.assertIn("was built", str(e.exception))
            self.assertEqual(FakeRun.calls, [])
            self.assertFalse(os.path.exists(f"{d}/ep02"))
            a.allow_stale_cli = True
            write(f"{d}/styles.json", "{}")
            self.assertTrue(FakeRun(a).run()["ok"])

    def test_a_source_with_an_id_twice_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            src = f"{d}/adapted/eve.ep02.fcscript.json"
            s2 = script()
            s2["scenes"][0]["lines"].append(dict(s2["scenes"][0]["lines"][0]))
            write(src, s2)
            FakeRun.calls = []
            with self.assertRaises(ValueError):
                FakeRun(self.args(d, src)).run()
            self.assertEqual(FakeRun.calls, [])

    def test_unvoiced_speakers_call_extras_voices(self):
        with tempfile.TemporaryDirectory() as d:
            src = f"{d}/adapted/eve.ep02.fcscript.json"
            write(src, script(voiced=False))
            write(f"{d}/styles.json", "{}")
            FakeRun.calls = []
            FakeRun(self.args(d, src)).run()
            self.assertEqual(FakeRun.calls[0], "extras_voices.py")


class RunHelpers(unittest.TestCase):
    def test_gpu_reason(self):
        self.assertEqual(er.gpu_reason("0, 1589\n"), "")
        self.assertIn("97 %", er.gpu_reason("97, 9800\n3, 100\n"))
        self.assertIn("9000 MiB", er.gpu_reason(" 2 , 9000 "))
        for text in ("", "N/A, N/A", "garbage", None, "12"):
            self.assertEqual(er.gpu_reason(text), "")

    def test_duplicate_ids(self):
        self.assertEqual(er.duplicate_ids(["a", "b", "a", "c", "a", "b"]), ["a", "b"])
        self.assertEqual(er.duplicate_ids([]), [])

    def test_default_cli_prefers_a_release_build_that_is_not_older(self):
        with tempfile.TemporaryDirectory() as d:
            dbg = f"{d}/target/agent-main/debug/filmcraft-cli.exe"
            rel = f"{d}/target/agent-main/release/filmcraft-cli.exe"
            self.assertEqual(er.default_cli(d), dbg)  # neither is there: the debug path is named
            write(rel, "x")
            self.assertEqual(er.default_cli(d), rel)
            write(dbg, "x")
            os.utime(rel, (1, 1))
            self.assertEqual(er.default_cli(d), dbg)  # a stale release build is not used
            os.utime(dbg, (1, 1))
            self.assertEqual(er.default_cli(d), rel)

    def test_rebase_to_another_drive_keeps_the_full_path(self):
        with tempfile.TemporaryDirectory() as d:
            write(f"{d}/a/v.wav", "x")
            real = os.path.relpath

            def other_drive(path, start=None):
                raise ValueError("path is on mount C:, start on mount F:")
            os.path.relpath = other_drive
            try:
                got = er.rebase({"voiceFile": "v.wav"}, f"{d}/a", f"{d}/b")
            finally:
                os.path.relpath = real
            self.assertEqual(got["voiceFile"], os.path.abspath(f"{d}/a/v.wav").replace(os.sep, "/"))

    def test_a_cli_older_than_the_sources_is_named(self):
        with tempfile.TemporaryDirectory() as d:
            cli = f"{d}/cli.exe"
            write(cli, "x")
            for rel in er.CLI_SOURCES:
                write(f"{d}/{rel}", "x")
                os.utime(f"{d}/{rel}", (1000, 1000))
            os.utime(cli, (2000, 2000))
            self.assertEqual(er.stale_cli(cli, d), "")
            os.utime(f"{d}/{er.CLI_SOURCES[0]}", (3000, 3000))
            self.assertIn(er.CLI_SOURCES[0], er.stale_cli(cli, d))
            self.assertNotIn(er.CLI_SOURCES[1], er.stale_cli(cli, d))
            self.assertEqual(er.stale_cli(f"{d}/none.exe", d), "")  # cannot be told: not refused here
            self.assertEqual(er.stale_cli(cli, f"{d}/nowhere"), "")

    def test_default_donors(self):
        self.assertEqual(er.default_donors("X"), ["k=X/routes_screen/kojirou.fcscript.json", "m=X/routes_screen/marina.fcscript.json"])


class Cli(unittest.TestCase):
    def test_arguments(self):
        with redirect_stderr(io.StringIO()):
            self.assertEqual(er.main([]), 2)
            self.assertEqual(er.main(["--episode", "0"]), 2)
        with tempfile.TemporaryDirectory() as d, redirect_stderr(io.StringIO()), redirect_stdout(io.StringIO()):
            self.assertEqual(er.main(["--episode", "3", "--out", d]), 1)  # no script for it


if __name__ == "__main__":
    unittest.main()
