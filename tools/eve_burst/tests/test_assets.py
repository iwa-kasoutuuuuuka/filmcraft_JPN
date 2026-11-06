"""Tests for tools/eve_burst/assets.py.

Run:  python -m unittest discover -s tools/eve_burst/tests
Tests that need the real converted data skip when the folder is absent (set EVE_BURST_DATA to
override the default location).  The others build a tiny fake tree in a temp directory.
"""
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import assets  # noqa: E402
from assets import Assets, parse_sprite_name  # noqa: E402

REAL_ROOT = os.environ.get("EVE_BURST_DATA", "E:/filmcraft_JPN/PS2_EVE_Bursterror解析データ")


def touch(root, rel, data=b""):
    p = os.path.join(root, *rel.split("/"))
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "wb") as f:
        f.write(data)


class FakeTreeTests(unittest.TestCase):
    def setUp(self):
        self._td = tempfile.TemporaryDirectory()
        self.root = self._td.name
        for rel in [
            "02_画像/背景/01______.png",
            "02_画像/背景/02_N____.png",
            "02_画像/イベントCG/s0010.png",
            "02_画像/サブCG/s13.png",
            "02_画像/ハッキング画面/hack_m.png",
            "02_画像/日付画面/1203md.png",
            "02_画像/立ち絵/kyo_氷室恭子/kyo00ad.png",
            "02_画像/立ち絵/kyo_氷室恭子/kyo02bd.png",
            "02_画像/立ち絵/koj_天城小次郎/koj00ad.png",
            "03_音声/BGM/eve021_Some Title.wav",
            "03_音声/BGM/eve003.wav",
            "03_音声/ボイス/kyo_氷室恭子/kyo0123.wav",
            "03_音声/ボイス/j_擬音/j356.wav",
            "03_音声/ボイス/eve_ジングル/eve_51b.wav",
            "03_音声/効果音/j_番号付き効果音/j356.wav",
            "03_音声/効果音/se_一般効果音/se01.wav",
            "03_音声/効果音/sysse_システム音/sysse_00.wav",
            "03_音声/その他/ADX形式のボイス/may3000.wav",
            "04_動画/イベントムービー/scene02.mp4",
            "04_動画/日付表示/1203.mp4",
            "04_動画/オープニング/opening.mp4",
            "04_動画/表情アニメ/kyo_氷室恭子/kyo00ad01.mp4",
        ]:
            touch(self.root, rel)
        self.a = Assets(self.root)

    def tearDown(self):
        self._td.cleanup()

    def test_no_side_effects_on_missing_root(self):
        a = Assets(os.path.join(self.root, "nope"))
        self.assertIsNone(a.background("01______"))
        self.assertIsNone(a.voice("kyo0123"))
        self.assertEqual(a.coverage()["scenarios"], 0)

    def test_images_case_insensitive(self):
        self.assertEqual(self.a.background("01______"), "02_画像/背景/01______.png")
        self.assertEqual(self.a.background("02_n____"), "02_画像/背景/02_N____.png")
        self.assertEqual(self.a.background("01______.bmp"), "02_画像/背景/01______.png")
        self.assertIsNone(self.a.background("99______"))

    def test_background_falls_back_to_event_cg_and_screens(self):
        self.assertEqual(self.a.background("s0010"), "02_画像/イベントCG/s0010.png")
        self.assertEqual(self.a.background("hack_m"), "02_画像/ハッキング画面/hack_m.png")
        self.assertEqual(self.a.background("1203md"), "02_画像/日付画面/1203md.png")
        self.assertEqual(self.a.event_cg("S0010"), "02_画像/イベントCG/s0010.png")
        self.assertEqual(self.a.event_cg("01______"), "02_画像/背景/01______.png")

    def test_sub_cg(self):
        self.assertEqual(self.a.sub_cg("S13"), "02_画像/サブCG/s13.png")
        self.assertIsNone(self.a.sub_cg("s14"))

    def test_sprite_full_name_and_parts(self):
        want = "02_画像/立ち絵/kyo_氷室恭子/kyo02bd.png"
        self.assertEqual(self.a.sprite("kyo02bd"), want)
        self.assertEqual(self.a.sprite("KYO02BD"), want)
        self.assertEqual(self.a.sprite("kyo", 2, "b"), want)
        self.assertEqual(self.a.sprite("kyo", "0"), "02_画像/立ち絵/kyo_氷室恭子/kyo00ad.png")
        self.assertIsNone(self.a.sprite("kyo", 9))
        self.assertIsNone(self.a.sprite("zzz00ad"))
        self.assertIsNone(self.a.sprite("kyo", "x"))
        self.assertEqual(self.a.sprite_names("kyo"), ["kyo00ad", "kyo02bd"])
        self.assertEqual(self.a.sprite_names("nope"), [])

    def test_parse_sprite_name(self):
        self.assertEqual(parse_sprite_name("kyo02bd"), {"code": "kyo", "pose": 2, "outfit": "b", "suffix": "d"})
        self.assertEqual(parse_sprite_name("may00cd.pbd")["outfit"], "c")
        self.assertIsNone(parse_sprite_name("scene01"))

    def test_sprite_transition(self):
        self.assertEqual(self.a.sprite_transition("kyo00ad", "kyo01ad"),
                         "04_動画/表情アニメ/kyo_氷室恭子/kyo00ad01.mp4")
        self.assertIsNone(self.a.sprite_transition("kyo00ad", "kyo03ad"))
        self.assertIsNone(self.a.sprite_transition("bogus", "kyo03ad"))

    def test_bgm_title_suffix(self):
        self.assertEqual(self.a.bgm("eve021"), "03_音声/BGM/eve021_Some Title.wav")
        self.assertEqual(self.a.bgm("EVE021.adx"), "03_音声/BGM/eve021_Some Title.wav")
        self.assertEqual(self.a.bgm("eve003"), "03_音声/BGM/eve003.wav")
        self.assertIsNone(self.a.bgm("eve02"))  # no prefix confusion with eve021
        self.assertIsNone(self.a.bgm("eve099"))

    def test_voice_folders_by_code(self):
        self.assertEqual(self.a.voice("kyo0123"), "03_音声/ボイス/kyo_氷室恭子/kyo0123.wav")
        self.assertEqual(self.a.voice("KYO0123.ahx"), "03_音声/ボイス/kyo_氷室恭子/kyo0123.wav")
        self.assertEqual(self.a.voice("eve_51b"), "03_音声/ボイス/eve_ジングル/eve_51b.wav")
        self.assertEqual(self.a.voice("may3000"), "03_音声/その他/ADX形式のボイス/may3000.wav")
        self.assertIsNone(self.a.voice("kyo9999"))

    def test_same_name_voice_vs_se(self):
        # j356 exists as .ahx (ボイス) and .adx (効果音): dialogue wants the first, effect commands the second.
        self.assertEqual(self.a.voice("j356"), "03_音声/ボイス/j_擬音/j356.wav")
        self.assertEqual(self.a.se("j356"), "03_音声/効果音/j_番号付き効果音/j356.wav")
        self.assertEqual(self.a.resolve("音声", "j356"), self.a.se("j356"))

    def test_se_and_fallback(self):
        self.assertEqual(self.a.se("SE01"), "03_音声/効果音/se_一般効果音/se01.wav")
        self.assertEqual(self.a.se("sysse_00"), "03_音声/効果音/sysse_システム音/sysse_00.wav")
        self.assertEqual(self.a.se("kyo0123"), "03_音声/ボイス/kyo_氷室恭子/kyo0123.wav")
        self.assertIsNone(self.a.se("se99"))

    def test_movie(self):
        self.assertEqual(self.a.movie("scene02"), "04_動画/イベントムービー/scene02.mp4")
        self.assertEqual(self.a.movie("1203"), "04_動画/日付表示/1203.mp4")
        self.assertEqual(self.a.movie("opening"), "04_動画/オープニング/opening.mp4")
        self.assertIsNone(self.a.movie("scene04"))

    def test_resolve_by_scenario_type(self):
        self.assertEqual(self.a.resolve("背景", "01______"), "02_画像/背景/01______.png")
        self.assertEqual(self.a.resolve("演出ムービー", "1203"), "04_動画/日付表示/1203.mp4")
        self.assertEqual(self.a.resolve("立ち絵", "koj00ad"), "02_画像/立ち絵/koj_天城小次郎/koj00ad.png")
        self.assertIsNone(self.a.resolve("???", "x"))

    def test_coverage(self):
        ins = [
            {"op": "0x8014", "asset": {"type": "背景", "name": "01______"}},
            {"op": "0x8014", "asset": {"type": "背景", "name": "77______"}},
            {"op": "0x8014", "asset": {"type": "背景", "name": "77______"}},
            {"op": "0x8048", "asset": {"type": "立ち絵", "name": "kyo00ad"}},
            {"op": "0x8026", "asset": {"type": "BGM", "name": "eve999"}},
            {"op": "0x8035", "voices": [{"name": "kyo0123"}, {"name": "kyo0124"}]},
            "garbage",
        ]
        os.makedirs(os.path.join(self.root, "01_シナリオ", "小次郎編"))
        with open(os.path.join(self.root, "01_シナリオ", "小次郎編", "a001.json"), "w", encoding="utf-8") as f:
            json.dump({"instructions": ins}, f)
        with open(os.path.join(self.root, "01_シナリオ", "小次郎編", "broken.json"), "w") as f:
            f.write("{not json")
        touch(self.root, "00_目録/全ファイル一覧.tsv",
              "path\tpak\n03_音声\\ボイス\\kyo_氷室恭子\\kyo0124.wav\tX\n".encode("utf-8"))
        rep = Assets(self.root).coverage()
        self.assertEqual(rep["scenarios"], 2)
        self.assertEqual(rep["total"]["背景"], 3)
        self.assertEqual(rep["distinct"]["背景"], 2)
        self.assertEqual(rep["unresolved"]["背景"]["77______"]["count"], 2)
        self.assertEqual(rep["unresolved"]["背景"]["77______"]["scenarios"], ["a001"])
        self.assertNotIn("立ち絵", rep["unresolved"])
        self.assertIn("eve999", rep["unresolved"]["BGM"])
        self.assertEqual(list(rep["unresolved"]["ボイス"]), ["kyo0124"])
        self.assertTrue(rep["unresolved"]["ボイス"]["kyo0124"]["in_catalog"])
        self.assertFalse(rep["unresolved"]["BGM"]["eve999"]["in_catalog"])
        self.assertEqual(rep["unresolved_refs"]["背景"], 2)
        self.assertIn("77______", Assets.format_coverage(rep))

    def test_import_has_no_side_effects(self):
        self.assertTrue(hasattr(assets, "Assets"))


@unittest.skipUnless(os.path.isdir(os.path.join(REAL_ROOT, "01_シナリオ")), "real data folder not present")
class RealDataTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.a = Assets(REAL_ROOT)
        cls.rep = cls.a.coverage()

    def test_scans_all_87_scenarios(self):
        self.assertEqual(self.rep["scenarios"], 87)

    def test_known_names_resolve(self):
        a = self.a
        self.assertEqual(a.background("01______"), "02_画像/背景/01______.png")
        self.assertEqual(a.sprite("kyo00bd"), "02_画像/立ち絵/kyo_氷室恭子/kyo00bd.png")
        self.assertEqual(a.sprite("kyo", 1, "b"), "02_画像/立ち絵/kyo_氷室恭子/kyo01bd.png")
        self.assertTrue(a.bgm("eve039").startswith("03_音声/BGM/eve039"))
        self.assertTrue(a.sub_cg("s13").endswith("s13.png"))
        self.assertTrue(a.background("hack_m").endswith("hack_m.png"))

    def test_images_and_bgm_fully_covered(self):
        for typ in ("背景", "背景（端末画面）", "立ち絵", "サブCG", "BGM"):
            self.assertNotIn(typ, self.rep["unresolved"], typ)

    def test_unresolved_are_only_files_the_catalog_lists(self):
        # Anything missing must be a copy still in progress, never a name the catalog does not know.
        for typ, names in self.rep["unresolved"].items():
            for n, e in names.items():
                self.assertTrue(e["in_catalog"], "%s %s is not in the catalog" % (typ, n))

    def test_resolved_files_exist(self):
        for rel in (self.a.background("01______"), self.a.sprite("aka00ad"), self.a.bgm("eve001")):
            self.assertTrue(os.path.isfile(os.path.join(REAL_ROOT, *rel.split("/"))), rel)


if __name__ == "__main__":
    unittest.main()
