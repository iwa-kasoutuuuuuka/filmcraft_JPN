import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))

import sprite_variants as sv  # noqa: E402


def touch(path):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(b"png")


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


def make_script():
    return {
        "version": 1,
        "characters": {
            "kyo": {"name": "恭子", "expressions": {
                "01bd": {"image": "../data/02_画像/立ち絵/kyo_氷室恭子/kyo01bd.png"},
                "00bd": {"image": "../data/02_画像/立ち絵/kyo_氷室恭子/kyo00bd.png"},
                "unused": {"image": "../data/x/kyo05ad.png"},
            }},
            "koj": {"name": "小次郎"},
            "narrator": {"name": "語り"},
        },
        "scenes": [{"id": "s1", "lines": [
            {"id": "l1", "speaker": "kyo", "text": "こんにちは", "expression": "01bd"},
            {"id": "l2", "speaker": "kyo", "text": "うん", "expression": "01bd"},
            {"id": "l3", "speaker": "kyo", "text": "", "expression": "00bd"},
            {"id": "l4", "kind": "direction", "speaker": "kyo", "expression": "unused"},
            {"id": "l5", "speaker": "narrator", "kind": "narration", "text": "夜。"},
        ]}],
        "route": ["s1"],
    }


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.t = self.tmp.name
        self.var = os.path.join(self.t, "eve_sprites")
        self.sdir = os.path.join(self.t, "out")
        os.makedirs(self.sdir)

    def tearDown(self):
        self.tmp.cleanup()

    def var_file(self, name, code="kyo"):
        p = os.path.join(self.var, code, name) if code else os.path.join(self.var, name)
        touch(p)
        return p


class Names(unittest.TestCase):
    def test_base_and_code(self):
        self.assertEqual(sv.base_of("a/b/kyo01bd.png"), "kyo01bd")
        self.assertEqual(sv.base_of("a\\b\\kyo01bd_closed.png"), "kyo01bd")
        self.assertIsNone(sv.base_of(""))
        self.assertIsNone(sv.base_of(3))
        self.assertIsNone(sv.base_of("x/.png"))
        self.assertIsNone(sv.base_of("x" * 5000))
        self.assertEqual(sv.code_of("kyo01bd", "kyo"), "kyo")
        self.assertEqual(sv.code_of("pur02cd", "pur"), "pur")
        self.assertEqual(sv.code_of("marina_normal", "marina"), "marina")
        self.assertEqual(sv.code_of("x", "../evil"), "")

    def test_default_folder_is_beside_the_dump(self):
        d = sv.default_variants_dir(os.path.join("E:", os.sep, "a", "dump"))
        self.assertEqual(os.path.basename(d), "eve_sprites")
        self.assertEqual(os.path.basename(os.path.dirname(d)), "a")


class Apply(Base):
    def test_full_set_is_written_relative_with_the_blink_on_the_sprite(self):
        for k in ("open", "a", "i", "u", "e", "o", "blink"):
            self.var_file(f"kyo01bd_{k}.png")
        s = make_script()
        rep = sv.apply(s, sv.Folder(self.var), self.sdir)
        e = s["characters"]["kyo"]["expressions"]
        self.assertEqual(e["01bd"]["openImage"], "../eve_sprites/kyo/kyo01bd_open.png")
        self.assertEqual(e["01bd"]["vowelImages"]["u"], "../eve_sprites/kyo/kyo01bd_u.png")
        self.assertEqual(sorted(e["01bd"]["vowelImages"]), ["a", "e", "i", "o", "u"])
        self.assertEqual(e["01bd"]["blink"], "../eve_sprites/kyo/kyo01bd_blink.png")
        self.assertNotIn("01bd_blink", e)
        self.assertEqual(e["01bd"]["image"], "../data/02_画像/立ち絵/kyo_氷室恭子/kyo01bd.png")
        # 00bd is shown (an empty line still shows the sprite) but has nothing; `unused` only on a direction
        self.assertEqual([(m["expression"], m["kinds"]) for m in rep["missing"]], [("00bd", ["a", "i", "u", "e", "o", "blink"])])
        self.assertNotIn("openImage", e["unused"])
        self.assertEqual(rep["counts"]["expressionsShown"], 2)
        self.assertEqual(rep["counts"]["complete"], 1)
        self.assertEqual(rep["counts"]["filesWritten"], 7)

    def test_only_what_exists_open_falls_back_to_a_and_flat_folder_and_case(self):
        self.var_file("KYO01BD_A.PNG", code=None)
        self.var_file("kyo01bd_blink.png", code=None)
        s = make_script()
        rep = sv.apply(s, sv.Folder(self.var), self.sdir)
        sp = s["characters"]["kyo"]["expressions"]["01bd"]
        self.assertTrue(sp["openImage"].lower().endswith("kyo01bd_a.png"))
        self.assertEqual(list(sp["vowelImages"]), ["a"])
        self.assertIn("kyo/01bd", rep["openFromA"])
        miss = {m["expression"]: m["kinds"] for m in rep["missing"]}
        self.assertEqual(miss["01bd"], ["i", "u", "e", "o"])

    def test_existing_fields_are_kept_unless_overwrite(self):
        for k in ("open", "a", "blink"):
            self.var_file(f"kyo01bd_{k}.png")
        s = make_script()
        e = s["characters"]["kyo"]["expressions"]
        e["01bd"]["openImage"] = "mine.png"
        e["01bd"]["vowelImages"] = {"a": "mine_a.png"}
        e["01bd_blink"] = {"image": "mine_blink.png"}  # the older way counts as set
        rep = sv.apply(s, sv.Folder(self.var), self.sdir)
        self.assertEqual(e["01bd"]["openImage"], "mine.png")
        self.assertEqual(e["01bd"]["vowelImages"], {"a": "mine_a.png"})
        self.assertNotIn("blink", e["01bd"])
        self.assertEqual(sorted(rep["kept"][0]["kinds"]), ["a", "blink", "open"])
        e["00bd"]["blink"] = "mine00.png"
        self.var_file("kyo00bd_blink.png")
        sv.apply(s, sv.Folder(self.var), self.sdir)
        self.assertEqual(e["00bd"]["blink"], "mine00.png")
        sv.apply(s, sv.Folder(self.var), self.sdir, overwrite=True)
        self.assertTrue(e["01bd"]["openImage"].endswith("kyo01bd_open.png"))
        self.assertTrue(e["01bd"]["blink"].endswith("kyo01bd_blink.png"))
        self.assertTrue(e["00bd"]["blink"].endswith("kyo00bd_blink.png"))

    def test_closed_variant_replaces_the_picture_and_a_second_run_is_stable(self):
        for k in ("closed", "open", "blink"):
            self.var_file(f"kyo01bd_{k}.png")
        s = make_script()
        sv.apply(s, sv.Folder(self.var), self.sdir)
        sp = s["characters"]["kyo"]["expressions"]["01bd"]
        self.assertEqual(sp["image"], "../eve_sprites/kyo/kyo01bd_closed.png")
        before = json.dumps(s, sort_keys=True)
        rep = sv.apply(s, sv.Folder(self.var), self.sdir)
        self.assertEqual(json.dumps(s, sort_keys=True), before)
        self.assertEqual(rep["written"], [])

    def test_missing_folder_and_hostile_scripts_do_not_fail(self):
        nowhere = sv.Folder(os.path.join(self.t, "does", "not", "exist"))
        s = make_script()
        rep = sv.apply(s, nowhere, self.sdir)
        self.assertEqual(rep["counts"]["filesWritten"], 0)
        self.assertFalse(nowhere.listed_any)
        for bad in ({}, {"characters": 3}, {"characters": {"a": 1, "b": {"expressions": [1]}}, "scenes": 5},
                    {"characters": {"b": {"expressions": {"x": {"image": None}, "y": 7}}},
                     "scenes": [None, {"lines": "no"}, {"lines": [None, {"speaker": 5}, {"speaker": "b", "expression": ["x"]}]}]}):
            rep = sv.apply(bad, sv.Folder(self.var), self.sdir)
            self.assertIsInstance(rep["counts"], dict)
        bad = {"characters": {"b": {"expressions": {"x": {"image": "../../etc/..png"}}}},
               "scenes": [{"lines": [{"speaker": "b", "text": "t", "expression": "x"}]}]}
        rep = sv.apply(bad, sv.Folder(self.var), self.sdir)
        self.assertEqual(len(rep["unusable"]), 1)

    def test_expression_fallback_matches_anime_build(self):
        self.var_file("kyo00bd_open.png")
        s = make_script()
        s["characters"]["kyo"]["defaultExpression"] = "00bd"
        s["scenes"][0]["lines"] = [{"id": "x", "speaker": "kyo", "text": "あ"}]  # no expression
        sv.apply(s, sv.Folder(self.var), self.sdir)
        self.assertIn("openImage", s["characters"]["kyo"]["expressions"]["00bd"])


class Main(Base):
    def write_script(self):
        p = os.path.join(self.sdir, "s.fcscript.json")
        with open(p, "w", encoding="utf-8") as f:
            json.dump(make_script(), f, ensure_ascii=False)
        return p

    def run_main(self, args):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = sv.main(args)
        return code, out.getvalue(), err.getvalue()

    def test_list_writes_nothing_and_names_every_file(self):
        p = self.write_script()
        self.var_file("kyo01bd_a.png")
        before = read(p)
        code, out, err = self.run_main(["--script", p, "--variants", self.var, "--list"])
        self.assertEqual(code, 0)
        self.assertEqual(read(p), before)
        rows = [r.split("\t") for r in out.strip().splitlines()[1:]]
        self.assertEqual(len(rows), 2 * len(sv.KINDS))
        st = {(r[1], r[5]): r[6] for r in rows}
        self.assertEqual(st[("01bd", "a")], "have")
        self.assertEqual(st[("01bd", "open")], "optional")
        self.assertEqual(st[("00bd", "open")], "optional")
        self.assertEqual(st[("00bd", "a")], "missing")
        self.assertIn("11 required files missing", err)  # 01bd: i u e o blink; 00bd: all six
        self.assertEqual(st[("00bd", "closed")], "optional")
        self.assertEqual(rows[0][2:4], ["1", "0"])  # 00bd: shown once, never spoken
        self.assertTrue(any(r[4].endswith("eve_sprites/kyo/kyo00bd_blink.png") for r in rows))
        self.assertIn("required files missing", err)

    def test_out_report_and_default_folder_from_root(self):
        p = self.write_script()
        root = os.path.join(self.t, "data")
        os.makedirs(root)
        self.var_file("kyo01bd_open.png")
        dest = os.path.join(self.sdir, "with.fcscript.json")
        rp = os.path.join(self.sdir, "rep.json")
        code, out, err = self.run_main(["--script", p, "--root", root, "--out", dest, "--report", rp])
        self.assertEqual(code, 0)
        self.assertNotIn("openImage", json.loads(read(p))["characters"]["kyo"]["expressions"]["01bd"])
        got = json.loads(read(dest))["characters"]["kyo"]["expressions"]["01bd"]
        self.assertEqual(got["openImage"], "../eve_sprites/kyo/kyo01bd_open.png")
        self.assertEqual(json.loads(read(rp))["counts"]["filesWritten"], 1)
        self.assertIn("missing: kyo/00bd", err)

    def test_bad_input_returns_2(self):
        self.assertEqual(self.run_main(["--script", os.path.join(self.t, "none.json"), "--variants", self.var])[0], 2)
        bad = os.path.join(self.t, "bad.json")
        with open(bad, "w") as f:
            f.write("[1, 2")
        self.assertEqual(self.run_main(["--script", bad, "--variants", self.var])[0], 2)
        with open(bad, "w") as f:
            f.write("[1, 2]")
        self.assertEqual(self.run_main(["--script", bad, "--variants", self.var])[0], 2)
        deep = os.path.join(self.t, "deep.json")
        with open(deep, "w") as f:
            f.write("[" * 100000)
        self.assertEqual(self.run_main(["--script", deep, "--variants", self.var])[0], 2)
        self.assertEqual(self.run_main(["--script", bad])[0], 2)  # neither --root nor --variants

    def test_missing_variants_folder_is_a_warning(self):
        p = self.write_script()
        code, _, err = self.run_main(["--script", p, "--variants", os.path.join(self.t, "nope")])
        self.assertEqual(code, 0)
        self.assertIn("does not exist", err)


if __name__ == "__main__":
    unittest.main()
