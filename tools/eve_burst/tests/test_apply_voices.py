import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))

import apply_voices  # noqa: E402


class Apply(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = os.path.join(self.tmp.name, "data")
        self.sdir = os.path.join(self.tmp.name, "out")
        os.makedirs(os.path.join(self.root, "v"))
        os.makedirs(self.sdir)
        with open(os.path.join(self.root, "v", "koj0003.wav"), "wb") as f:
            f.write(b"x")

    def tearDown(self):
        self.tmp.cleanup()

    def test_refs_become_voices_with_relative_paths_and_others_get_the_preset(self):
        script = {"characters": {"koj": {"name": "小次郎"}, "narrator": {"name": "語り"}, "aka": {"name": "茜", "voice": {"speaker": "x"}}, "mar": {"name": "m"}}}
        refs = {"koj": {"refAudio": "v/koj0003.wav", "refText": "text"}, "mar": {"refAudio": "v/gone.wav", "refText": "t"}}
        rep = apply_voices.apply(script, refs, self.root, self.sdir)
        self.assertEqual(rep["cloned"], ["koj"])
        self.assertEqual(rep["kept"], ["aka"])
        self.assertEqual(sorted(rep["preset"]), ["mar", "narrator"])
        self.assertEqual(rep["missingFile"], ["mar"])
        self.assertEqual(script["characters"]["koj"]["voice"], {"refAudio": "../data/v/koj0003.wav", "refText": "text"})
        self.assertEqual(script["characters"]["aka"]["voice"], {"speaker": "x"})
        self.assertEqual(script["characters"]["narrator"]["voice"], {"speaker": "Ono_Anna"})

    def test_no_fallback_leaves_voiceless_characters_alone_and_bad_input_is_harmless(self):
        script = {"characters": {"n": {"name": "n"}, "bad": 3}}
        rep = apply_voices.apply(script, {}, self.root, self.sdir, fallback=None)
        self.assertNotIn("voice", script["characters"]["n"])
        self.assertEqual(rep["preset"], [])
        self.assertEqual(apply_voices.apply({}, [], self.root, self.sdir)["cloned"], [])

    def test_main_rewrites_the_file_and_reports_unreadable_input(self):
        sp = os.path.join(self.sdir, "s.json")
        rp = os.path.join(self.tmp.name, "refs.json")
        with open(sp, "w", encoding="utf-8") as f:
            json.dump({"characters": {"koj": {"name": "k"}}}, f)
        with open(rp, "w", encoding="utf-8") as f:
            json.dump({"koj": {"refAudio": "v/koj0003.wav", "refText": "t"}}, f)
        self.assertEqual(apply_voices.main(["--script", sp, "--refs", rp, "--root", self.root]), 0)
        with open(sp, encoding="utf-8") as f:
            self.assertIn("refAudio", json.load(f)["characters"]["koj"]["voice"])
        self.assertEqual(apply_voices.main(["--script", sp, "--refs", os.path.join(self.tmp.name, "none.json"), "--root", self.root]), 2)


if __name__ == "__main__":
    unittest.main()
