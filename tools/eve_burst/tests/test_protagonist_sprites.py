import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))

import assets as eve_assets  # noqa: E402
import protagonist_sprites as ps  # noqa: E402


def script():
    return {
        "version": 1,
        "characters": {"koj": {"name": "小次郎"}, "mar": {"name": "まりな", "expressions": {"01ad": {"image": "x.png"}}},
                       "aka": {"name": "茜"}},
        "scenes": [
            {"id": "s1", "pov": "koj", "background": "bg/a.png", "lines": [
                {"id": "1", "speaker": "koj", "text": "よし。"},
                {"id": "2", "speaker": "aka", "text": "何よ。"},
            ]},
            {"id": "s2", "pov": "koj", "hideCharacters": True, "lines": [{"id": "3", "speaker": "koj", "text": "回想。"}]},
            {"id": "s3", "pov": "mar", "lines": [{"id": "4", "speaker": "mar", "text": "ふう。"}]},
        ],
    }


class Sprites(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = os.path.join(self.tmp.name, "data")
        d = os.path.join(self.root, "02_画像", "立ち絵", "koj_天城小次郎")
        os.makedirs(d)
        for n in ("koj01bd.png", "koj00ad.png", "koj00bd.png", "koj00ad_open.png", "notes.txt"):
            open(os.path.join(d, n), "wb").close()
        self.out = os.path.join(self.tmp.name, "out")
        self.src = os.path.join(self.tmp.name, "series")
        os.makedirs(self.src)

    def tearDown(self):
        self.tmp.cleanup()

    def test_a_protagonist_without_expressions_gets_the_sprites_and_a_default(self):
        s = script()
        rep = ps.apply(s, eve_assets.Assets(self.root), self.out)
        koj = s["characters"]["koj"]
        self.assertEqual(sorted(koj["expressions"]), ["00ad", "00bd", "01bd"])  # the _open variant is not a sprite
        self.assertEqual(koj["expressions"]["00ad"]["image"], "../data/02_画像/立ち絵/koj_天城小次郎/koj00ad.png")
        self.assertEqual(koj["defaultExpression"], "00ad")
        self.assertEqual(rep["added"], {"koj": 3})
        self.assertEqual(rep["kept"], ["mar"])
        self.assertEqual(rep["shown"], {"koj": 1, "mar": 1})  # the line under hideCharacters is not drawn
        self.assertEqual(s["characters"]["mar"], {"name": "まりな", "expressions": {"01ad": {"image": "x.png"}}})
        self.assertNotIn("expressions", s["characters"]["aka"])  # not a protagonist

    def test_fill_default_missing_sprites_and_hostile_input(self):
        s = script()
        rep = ps.apply(s, eve_assets.Assets(os.path.join(self.tmp.name, "nowhere")), self.out, fill_default=True)
        self.assertEqual(rep["noSprites"], ["koj"])
        self.assertNotIn("expressions", s["characters"]["koj"])
        self.assertEqual(s["characters"]["mar"]["defaultExpression"], "01ad")
        self.assertEqual(ps.pick_default(["01bd", "02ad"]), "02ad")
        self.assertIsNone(ps.pick_default([]))
        for bad in ({}, [], {"characters": 3, "scenes": [{"pov": "koj"}]}, {"characters": {"koj": 1}, "scenes": [{"pov": "koj", "lines": 5}]}):
            ps.apply(bad, eve_assets.Assets(self.root), self.out)

    def test_main_writes_a_copy_with_rebased_paths(self):
        sp = os.path.join(self.src, "eve.ep02.fcscript.json")
        with open(sp, "w", encoding="utf-8") as f:
            json.dump(script(), f, ensure_ascii=False)
        self.assertEqual(ps.main(["--scripts", sp, "--root", self.root, "--out-dir", self.out, "--tag", "voiced"]), 0)
        with open(os.path.join(self.out, "eve.ep02.voiced.fcscript.json"), encoding="utf-8") as f:
            done = json.load(f)
        self.assertEqual(done["characters"]["koj"]["defaultExpression"], "00ad")
        self.assertEqual(done["scenes"][0]["background"], "../series/bg/a.png")
        with open(sp, encoding="utf-8") as f:
            self.assertNotIn("expressions", json.load(f)["characters"]["koj"])
        self.assertEqual(ps.main(["--scripts", sp, "--root", self.root, "--out-dir", self.src]), 2)
        self.assertEqual(ps.main(["--scripts", os.path.join(self.tmp.name, "none*.json"), "--root", self.root, "--in-place"]), 2)


if __name__ == "__main__":
    unittest.main()
