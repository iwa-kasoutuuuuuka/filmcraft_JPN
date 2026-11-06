"""Tests for voice_refs.py: `python -m unittest discover tools/eve_burst/tests`.

A tiny fake data tree is generated into a temp folder (WAVs are synthetic tones, no game data).
Tests against the real extracted data run only when it exists (EVE_BURST_DATA or the default
folder) and are skipped otherwise."""

import json
import math
import os
import random
import struct
import sys
import tempfile
import unittest
import wave

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))

import voice_refs as vr  # noqa: E402

RATE = 8000
REAL = os.environ.get("EVE_BURST_DATA", r"E:\filmcraft_JPN\PS2_EVE_Bursterror解析データ")


def tone(seconds, amp=0.4, noise=0.0, seed=1, gaps=(), clip=False):
    """Samples of a wobbling 'voice' tone, with optional background noise, silent gaps (start, len) and clipping."""
    rnd = random.Random(seed)
    n = int(seconds * RATE)
    out = []
    for i in range(n):
        t = i / RATE
        env = 0.6 + 0.4 * math.sin(2 * math.pi * 3.0 * t)
        v = amp * env * math.sin(2 * math.pi * 180.0 * t) + amp * 0.3 * env * math.sin(2 * math.pi * 540.0 * t)
        if any(a <= t < a + d for a, d in gaps):
            v = 0.0
        v += noise * (rnd.random() * 2 - 1)
        if clip:
            v *= 6.0
        out.append(max(-32768, min(32767, int(v * 32767))))
    return out


def write_wav(path, samples, rate=RATE):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(struct.pack("<%dh" % len(samples), *samples))


def tsv(path, header, rows):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8-sig", newline="") as f:
        for r in [header] + rows:
            f.write("\t".join(r) + "\r\n")


GOOD_TEXT = "そうね、少し落ち着いて話しましょう。"


def make_tree(root):
    tsv(os.path.join(root, vr.CODES_TSV), ["コード", "素材", "人物名", "補足", "根拠", "確度", "フォルダ名"], [
        ["abc", "ボイス", "テスト太郎", "", "", "確実", "abc_太郎"],
        ["xyz", "ボイス", "無声花子", "", "", "確実", "xyz_花子"],
        ["j", "ボイス", "（人物ではない）擬音", "", "", "確実", "j_擬音"],
    ])
    clips = {  # stem -> (samples, text)
        "abc0001": (tone(5.0, seed=1), GOOD_TEXT),                        # clean, the best
        "abc0002": (tone(5.0, seed=2, noise=0.25), "もう少しだけ待ってくれないか。"),  # noisy
        "abc0003": (tone(5.0, seed=3, clip=True), "それは本当に大丈夫なのかい。"),      # clipped
        "abc0004": (tone(5.0, seed=4, gaps=[(2.0, 1.5)]), "今日はここまでにしておこう。"),  # long silence
        "abc0005": (tone(2.0, seed=5), "短すぎる参照はだめだよね。"),                  # too short
        "abc0006": (tone(5.0, seed=6), "ははは、そうだったのか。"),                    # laugh
        "abc0007": (tone(5.0, seed=7), "待って！何をするつもりだ！"),                   # shouting: scores lower than 0001
        "abc0008": (tone(5.0, seed=8), "それでもあなたは、行くのですね。きっと後悔しますよ。"),              # second good clip
    }
    rows = []
    for stem, (smp, text) in clips.items():
        write_wav(os.path.join(root, vr.VOICE_DIR, "abc_太郎", stem + ".wav"), smp)
        rows.append([stem + ".ahx", "KOJIROU", "a01", "0x0001", "太郎", text, "台詞（0x8035）"])
    write_wav(os.path.join(root, vr.VOICE_DIR, "j_擬音", "j301.wav"), tone(1.0))
    rows.append(["j301.ahx", "KOJIROU", "a01", "0x0002", "", "（効果音命令で再生・台詞行なし）", "効果音命令（0x800b）"])
    tsv(os.path.join(root, vr.LINES_TSV), ["ボイスファイル", "収録PAK", "シナリオ", "命令位置", "話者", "台詞", "種別"], rows)

    def msg(speaker, voices=()):
        return {"op": vr.MESSAGE_OP, "speaker": speaker, "text": "x", "voices": [{"name": v} for v in voices]}

    ins = [msg("太郎", ["abc0001"]), msg("太郎", ["abc0002"]), msg("太郎", ["abc0008"]), msg("太郎"), msg("太郎"), msg("太郎"),
           msg("花子"), msg("花子"), msg(""), msg("", ["j301"]), {"op": "0x8001"}]
    p = os.path.join(root, "01_シナリオ", "小次郎編", "a01.json")
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w", encoding="utf-8-sig") as f:
        json.dump({"scenario": "a01", "instructions": ins}, f, ensure_ascii=False)
    # a second scenario file that is broken must not stop anything
    with open(os.path.join(root, "01_シナリオ", "小次郎編", "bad.json"), "w", encoding="utf-8") as f:
        f.write("{not json")


class TextRules(unittest.TestCase):
    def test_plain_sentence_scores_full(self):
        s, rej, _ = vr.text_check(GOOD_TEXT)
        self.assertEqual(rej, [])
        self.assertEqual(s, 40.0)

    def test_rejects(self):
        for t, why in [("‥‥‥‥。", "no words"), ("はい。", "too short"), ("ははは、そうだったのか。", "laugh"),
                       ("《その時間は、延びる。》", "aside"), ("ＳＯＳを出すんだ。", "digits"),
                       ("‥‥‥‥‥‥そうか‥‥‥‥‥‥", "ellipsis")]:
            self.assertTrue(vr.text_check(t)[1], why)

    def test_exclamation_and_ellipsis_lower_the_score(self):
        self.assertLess(vr.text_check("待ってくれ！話を聞け！")[0], vr.text_check("待ってくれ、話を聞いてほしい。")[0])
        self.assertLess(vr.text_check("‥‥そうかもしれないな‥‥。")[0], 40.0)

    def test_clean_ref_text_and_moras(self):
        self.assertEqual(vr.clean_ref_text("だが目は‥‥慣れてきた　ぜ。"), "だが目は……慣れてきたぜ。")
        self.assertEqual(vr.moras("きゃっ"), 2)       # ゃ is fused, っ counts
        self.assertEqual(vr.moras("今日は。"), 5)     # two kanji count 2 each, は 1, 。 none


class Analysis(unittest.TestCase):
    def test_clean_tone(self):
        m = vr.analyse(RATE, tone(5.0))
        self.assertAlmostEqual(m["seconds"], 5.0, places=2)
        self.assertLess(m["clip_frac"], 0.001)
        self.assertGreater(m["snr_db"], 12)
        self.assertEqual(m["max_gap_s"], 0.0)

    def test_gap_noise_and_clipping_are_measured(self):
        gap = vr.analyse(RATE, tone(5.0, gaps=[(2.0, 1.5)]))
        self.assertGreaterEqual(gap["max_gap_s"], 1.2)
        self.assertGreater(gap["silence_ratio"], 0.15)
        noisy = vr.analyse(RATE, tone(5.0, noise=0.3))
        self.assertLess(noisy["snr_db"], vr.analyse(RATE, tone(6.0))["snr_db"])
        self.assertGreater(vr.analyse(RATE, tone(5.0, clip=True))["clip_frac"], 0.01)

    def test_too_short_or_empty_is_an_error_not_a_crash(self):
        with self.assertRaises(ValueError):
            vr.analyse(RATE, [])
        with self.assertRaises(ValueError):
            vr.analyse(RATE, [0] * 100)

    def test_unreadable_file(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "x.wav")
            with open(p, "wb") as f:
                f.write(b"not a wav")
            with self.assertRaises(ValueError):
                vr.read_samples(p)
            self.assertIsNone(vr.wav_info(p))


class FakeTree(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.root = cls.tmp.name
        make_tree(cls.root)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def test_inventory_counts(self):
        inv = vr.inventory(self.root)
        self.assertEqual(inv["scenarios"], 1)
        self.assertEqual(inv["unreadable_scenarios"], ["bad.json"])
        self.assertEqual(inv["lines"], 10)
        self.assertEqual(inv["voiced_lines"], 4)
        self.assertEqual(inv["sfx_only_lines"], 1)
        abc = inv["codes"]["abc"]
        self.assertEqual((abc["lines"], abc["voiced"], abc["unvoiced"]), (6, 3, 3))
        self.assertEqual(abc["clips_present"], 8)
        self.assertEqual(abc["clips_referenced"], 3)
        # a speaker with too few voiced lines to be mapped stays unattributed
        self.assertEqual(inv["codes"]["xyz"]["lines"], 0)
        self.assertEqual(inv["unattributed_unvoiced"]["花子"], 2)
        self.assertEqual(inv["unattributed_unvoiced"][vr.NARRATION], 1)
        self.assertEqual(inv["speakers"]["太郎"]["lines"], 6)
        self.assertEqual(inv["formats"], {f"{RATE}Hz/1ch": 9})
        self.assertIn("| abc | テスト太郎 | 6 | 3 | 3 | 50.0 | 8 | 3 |", vr.inventory_markdown(inv))

    def test_generic_speakers_are_not_attributed_to_one_actor(self):
        with tempfile.TemporaryDirectory() as d:
            make_tree(d)

            def msg(speaker, voices=()):
                return {"op": vr.MESSAGE_OP, "speaker": speaker, "voices": [{"name": v} for v in voices]}

            ins = [msg("太郎", ["abc0001"]) for _ in range(30)] + [msg("男", ["abc0002"]) for _ in range(3)]                 + [msg("男") for _ in range(20)]
            with open(os.path.join(d, "01_シナリオ", "小次郎編", "b.json"), "w", encoding="utf-8") as f:
                json.dump({"instructions": ins}, f)
            inv = vr.inventory(d)
            self.assertEqual(inv["speakers"]["男"]["code"], "")
            self.assertEqual(inv["speakers"]["太郎"]["code"], "abc")
            self.assertEqual(inv["unattributed_unvoiced"]["男"], 20)
            self.assertEqual(inv["codes"]["abc"]["unvoiced"], 3)

    def test_the_clean_calm_clip_wins_and_bad_ones_are_dropped(self):
        refs, report = vr.build_refs(self.root)
        self.assertEqual(set(refs), {"abc"})                    # xyz has no clips; j is not a person
        r = refs["abc"]
        self.assertIn(r["refAudio"], ("03_音声/ボイス/abc_太郎/abc0001.wav", "03_音声/ボイス/abc_太郎/abc0008.wav"))
        self.assertIn(r["refText"], (GOOD_TEXT, "それでもあなたは、行くのですね。きっと後悔しますよ。"))
        self.assertAlmostEqual(r["seconds"], 5.0, places=1)
        self.assertIn("score", r["why"])
        used = {r["refAudio"]} | {a["refAudio"] for a in r["alternatives"]}
        for bad in ("abc0002", "abc0003", "abc0004", "abc0005", "abc0006"):
            self.assertFalse(any(bad in u for u in used), (bad, used))
        self.assertTrue(any("abc0008" in u for u in used) and any("abc0001" in u for u in used))
        self.assertLessEqual(len(r["alternatives"]), 2)
        self.assertEqual(report["xyz"]["picked"], 0)

    def test_deterministic_and_stdlib_path_agrees(self):
        a = vr.build_refs(self.root)[0]
        self.assertEqual(a, vr.build_refs(self.root)[0])
        saved = vr.np
        try:
            vr.np = None
            b = vr.build_refs(self.root)[0]
        finally:
            vr.np = saved
        self.assertEqual(a["abc"]["refAudio"], b["abc"]["refAudio"])
        self.assertEqual(a["abc"]["refText"], b["abc"]["refText"])

    def test_absolute_paths_and_json_roundtrip(self):
        refs = vr.build_refs(self.root, per_char=1, absolute=True)[0]
        self.assertTrue(os.path.isabs(refs["abc"]["refAudio"]))
        self.assertNotIn("alternatives", refs["abc"])
        self.assertEqual(json.loads(json.dumps(refs, ensure_ascii=False)), refs)

    def test_cli_writes_the_files_and_never_writes_into_the_data(self):
        with tempfile.TemporaryDirectory() as out:
            before = sorted(os.listdir(self.root))
            o, md = os.path.join(out, "r.json"), os.path.join(out, "i.md")
            self.assertEqual(vr.main(["--root", self.root, "--out", o, "--inventory-md", md]), 0)
            with open(o, encoding="utf-8") as f:
                self.assertIn("abc", json.load(f))
            with open(md, encoding="utf-8") as f:
                self.assertTrue(f.read().startswith("| code"))
            self.assertEqual(before, sorted(os.listdir(self.root)))
        self.assertEqual(vr.main(["--root", os.path.join(self.root, "missing")]), 2)

    def test_relaxed_limits_rescue_a_character_with_only_short_clips(self):
        with tempfile.TemporaryDirectory() as d:
            make_tree(d)
            for n in range(1, 9):
                os.remove(os.path.join(d, vr.VOICE_DIR, "abc_太郎", f"abc000{n}.wav"))
            write_wav(os.path.join(d, vr.VOICE_DIR, "abc_太郎", "abc0005.wav"), tone(3.0, seed=5))
            refs = vr.build_refs(d)[0]
            self.assertIn("RELAXED", refs["abc"]["why"])


@unittest.skipUnless(os.path.isdir(os.path.join(REAL, "00_目録")), "real EVE burst error data not present")
class RealData(unittest.TestCase):
    def test_catalog_loads(self):
        codes = vr.load_codes(REAL)
        for c in ("koj", "mar", "kyo", "yay", "may"):
            self.assertIn(c, codes)
        self.assertGreater(len(vr.load_clip_lines(REAL)), 10000)

    def test_voice_clips_are_mono_pcm_at_known_rates(self):
        codes = vr.load_codes(REAL)
        present = vr.voice_files(REAL, codes)
        if not present:
            self.skipTest("voice folder not copied yet")
        for stem in sorted(present)[:: max(1, len(present) // 40)]:
            info = vr.wav_info(present[stem])
            self.assertIsNotNone(info, stem)
            self.assertEqual(info[1], 1, stem)
            self.assertIn(info[0], (8000, 16000, 22050, 24000, 44100, 48000), stem)

    def test_a_picked_reference_measures_as_promised(self):
        codes = vr.load_codes(REAL)
        lines = vr.load_clip_lines(REAL)
        present = vr.voice_files(REAL, codes)
        picks, _, _ = vr.pick_for_code("kyo", codes, lines, present, per_char=1, pool=10)
        if not picks:
            self.skipTest("no kyo clips on disk yet")
        rate, smp = vr.read_samples(picks[0]["path"])
        m = vr.analyse(rate, smp)
        self.assertTrue(4.0 <= m["seconds"] <= 12.0)
        self.assertLess(m["clip_frac"], 0.001)
        self.assertLess(m["max_gap_s"], 0.8)


if __name__ == "__main__":
    unittest.main()
