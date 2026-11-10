import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))

import tts_readings as tr  # noqa: E402


def write(path, text):
    with io.open(path, "w", encoding="utf-8") as f:
        f.write(text)


def read_json(path):
    with io.open(path, encoding="utf-8") as f:
        return json.load(f)


def ph(s):
    """Phonemes written with spaces."""
    return s.split()


class Kana(unittest.TestCase):
    def test_conversions(self):
        self.assertEqual(tr.kata2hira("カイガ、ナンパ"), "かいが、なんぱ")
        self.assertEqual(tr.hira2kata("かいがー"), "カイガー")

    def test_spoken_reads_both_ruby_forms(self):
        self.assertEqual(tr.spoken_of("｜絵画《かいが》と東京《とうきょう》"), "かいがととうきょう")

    def test_game_readings_become_ruby(self):
        self.assertEqual(tr.paren_to_ruby("なんだ、楓（かえで）か‥‥。"), "なんだ、｜楓《かえで》か‥‥。")
        # a monologue in parentheses is not a reading
        self.assertEqual(tr.paren_to_ruby("（上手だと？）"), "（上手だと？）")


class Ruby(unittest.TestCase):
    def test_longest_first_and_every_occurrence(self):
        out = tr.ruby_words("天城小次郎と小次郎", {"小次郎": "こじろう", "天城小次郎": "あまぎこじろう"})
        self.assertEqual(out, "｜天城小次郎《あまぎこじろう》と｜小次郎《こじろう》")

    def test_existing_ruby_is_left_alone(self):
        out = tr.ruby_words("｜孔《こう》さんの孔", {"孔": "こう"})
        self.assertEqual(out, "｜孔《こう》さんの｜孔《こう》")
        self.assertEqual(tr.spoken_of(out), "こうさんのこう")

    def test_nothing_to_do(self):
        self.assertEqual(tr.ruby_words("絵画", {}), "絵画")
        self.assertEqual(tr.ruby_words("絵画", {"": "x", "絵画": ""}), "絵画")


class Numbers(unittest.TestCase):
    KANA = {"５０": "ごじゅう", "９": "きゅう"}

    def test_flagged_numerals_become_the_digit_runs(self):
        out = tr.number_words("わかった、５０くれてやる。", {"五": "ご", "十": "じゅう", "絵画": "かいが"}, self.KANA.get)
        self.assertEqual(out, {"絵画": "かいが", "５０": "ごじゅう"})
        self.assertEqual(tr.ruby_words("わかった、５０くれてやる。", out), "わかった、｜５０《ごじゅう》くれてやる。")

    def test_no_numerals_flagged_leaves_digits_alone(self):
        self.assertEqual(tr.number_words("９割がた", {"賭け": "かけ"}, self.KANA.get), {"賭け": "かけ"})

    def test_a_reading_that_is_not_kana_is_dropped(self):
        self.assertEqual(tr.number_words("９割", {"九": "きゅう"}, lambda r: "9"), {})


class Counters(unittest.TestCase):
    def test_people(self):
        self.assertEqual(tr.counter_ruby("１人より２人、２人より３人。"), "｜１人《ひとり》より｜２人《ふたり》、｜２人《ふたり》より｜３人《さんにん》。")
        self.assertEqual(tr.counter_ruby("4人と7人と14人と20人"), "｜4人《よにん》と｜7人《しちにん》と｜14人《じゅうよにん》と｜20人《にじゅうにん》")
        self.assertEqual(tr.counter_ruby("彼氏の２～３人くらい"), "彼氏の｜２～３人《にさんにん》くらい")
        self.assertEqual(tr.counter_ruby("１人前の料理、１人称"), "１人前の料理、１人称")  # いちにんまえ, いちにんしょう
        self.assertEqual(tr.counter_ruby("１００人と１，２００人"), "１００人と１，２００人")  # a long number is left to the synthesizer

    def test_days_and_dates(self):
        self.assertEqual(tr.counter_ruby("１２月６日、昼過ぎ。"), "｜１２月《じゅうにがつ》｜６日《むいか》、昼過ぎ。")
        self.assertEqual(tr.counter_ruby("12月1日と4月20日"), "｜12月《じゅうにがつ》｜1日《ついたち》と｜4月《しがつ》｜20日《はつか》")
        self.assertEqual(tr.counter_ruby("３日前から１０日間"), "｜３日《みっか》前から｜１０日《とおか》間")
        self.assertEqual(tr.counter_ruby("１日だけ"), "１日だけ")  # いちにち or ついたち: not decided here
        self.assertEqual(tr.counter_ruby("１日中、２日中"), "１日中、２日中")
        self.assertEqual(tr.counter_ruby("１５日と１７日と３１日"), "｜１５日《じゅうごにち》と｜１７日《じゅうしちにち》と｜３１日《さんじゅういちにち》")
        self.assertEqual(tr.counter_ruby("１６日"), "｜１６日《じゅうろくにち》")  # not ６日 → むいか
        self.assertEqual(tr.counter_ruby("４０日"), "４０日")

    def test_hours_and_things(self):
        self.assertEqual(tr.counter_ruby("４時と７時と９時と１４時と３時"), "｜４時《よじ》と｜７時《しちじ》と｜９時《くじ》と｜１４時《じゅうよじ》と３時")
        self.assertEqual(tr.counter_ruby("１つ、２つ、９つ"), "｜１つ《ひとつ》、｜２つ《ふたつ》、｜９つ《ここのつ》")

    def test_existing_ruby_and_odd_input(self):
        self.assertEqual(tr.counter_ruby("｜１２月《じゅうにがつ》６日と｜２人《ににん》"), "｜１２月《じゅうにがつ》｜６日《むいか》と｜２人《ににん》")
        self.assertEqual(tr.counter_ruby(""), "")
        self.assertEqual(tr.counter_ruby("3.5人、９９月９９日"), "3.5人、９９月９９日")
        long = "２人" * 3000
        self.assertEqual(len(tr.counter_ruby(long)) > len(long), True)  # capped, no hang
        self.assertEqual([tr.number_kana(n) for n in (0, 1, 10, 11, 21, 99, 100)], ["", "いち", "じゅう", "じゅういち", "にじゅういち", "きゅうじゅうきゅう", ""])

    def test_apply_writes_them_before_synthesis(self):
        script = {"scenes": [{"lines": [{"id": "a", "text": "２人で行こう。"}, {"id": "b", "text": "６日だ。", "voiceFile": "v.wav"},
                                        {"id": "c", "text": "何もない。"}]}]}
        counts = tr.apply_report(script, {}, {})
        ln = script["scenes"][0]["lines"]
        self.assertEqual(ln[0]["reading"], "｜２人《ふたり》で行こう。")
        self.assertNotIn("reading", ln[1])  # a recording is not synthesized
        self.assertNotIn("reading", ln[2])
        self.assertEqual(counts["withCounters"], 1)
        again = tr.apply_report(script, {}, {})
        self.assertEqual(ln[0]["reading"], "｜２人《ふたり》で行こう。")
        self.assertEqual(again["withCounters"], 0)


class Align(unittest.TestCase):
    def test_long_vowels_match_across_word_boundaries(self):
        # くわえて|行く: "te" + "iku" is heard as "teeku" once the boundary is gone
        words = [("くわえ", "くわえ", ph("k u w a e")), ("て", "て", ph("t e")), ("行く", "いく", ph("i k u"))]
        per, bad = tr.misheard_words(words, list("kuwaeteeku"))
        self.assertEqual(bad, set())
        self.assertEqual(per, 0.0)

    def test_a_misread_word_is_found(self):
        # 絵画 read as えいが
        words = [("だが", "だが", ph("d a g a")), ("絵画", "かいが", ph("k a i g a")), ("は", "は", ph("w a"))]
        per, bad = tr.misheard_words(words, ph("d a g a e e g a w a"))
        self.assertEqual(bad, {1})
        self.assertGreater(per, 0)

    def test_silences_and_case_are_ignored(self):
        words = [("試し", "ためし", ph("t a m e sh I"))]
        _, bad = tr.misheard_words(words, ph("pau t a m e sh i sil"))
        self.assertEqual(bad, set())

    def test_hostile_lengths_do_not_hang(self):
        words = [("あ", "あ", ["a"] * 3000)]
        per, bad = tr.misheard_words(words, ["a"] * 3000)
        self.assertEqual(bad, {0})  # too long to align: counted as not heard, not an hour of work
        self.assertGreaterEqual(per, 0)

    def test_empty(self):
        self.assertEqual(tr.misheard_words([], []), (0.0, set()))
        self.assertEqual(tr.align_ok([], ["a"]), (1, []))


def script():
    return {"version": 1, "scenes": [{"id": "s1", "lines": [
        {"id": "a", "speaker": "koj", "text": "だが絵画は猫じゃない。"},
        {"id": "b", "speaker": "koj", "text": "なんだ、楓（かえで）か‥‥。"},
        {"id": "c", "speaker": "aka", "text": "絵画？", "voiceFile": "x.wav"},
        {"id": "d", "kind": "direction", "text": "絵画"},
        {"id": "e", "speaker": "koj", "text": "孔ってヤツの絵画", "reading": "こうってヤツの絵画"},
        {"id": "f", "speaker": "koj", "text": "普通の台詞。"},
        "not a line", {"id": 5, "text": "x"}, {"id": "g", "text": "x" * (tr.MAX_TEXT + 1)},
    ]}, "not a scene"]}


REPORT = {"version": 1, "lines": {
    "a": {"words": {"絵画": "かいが"}},
    "e": {"words": {"絵画": "かいが"}},
    "f": {"words": {"普通": "<script>"}},  # not kana: ignored
    "zz": {"words": {"何": "なに"}},  # unknown id
    "c": {"words": {"絵画": "かいが"}},  # recorded line: never touched
}}


class Apply(unittest.TestCase):
    def test_readings_are_written_only_where_needed(self):
        s = script()
        counts = tr.apply_report(s, REPORT, {"茜": "あかね", "孔": "こう"})
        lines = {l["id"]: l for l in s["scenes"][0]["lines"] if isinstance(l, dict)}
        self.assertEqual(lines["a"]["reading"], "だが｜絵画《かいが》は猫じゃない。")
        self.assertEqual(lines["b"]["reading"], "なんだ、｜楓《かえで》か‥‥。")
        self.assertEqual(lines["e"]["reading"], "こうってヤツの｜絵画《かいが》")  # builds on the existing reading
        self.assertNotIn("reading", lines["c"])
        self.assertNotIn("reading", lines["d"])
        self.assertNotIn("reading", lines["f"])
        self.assertEqual(lines["a"]["text"], "だが絵画は猫じゃない。")  # the subtitle is unchanged
        self.assertEqual(counts, {"lines": 4, "withNames": 1, "withCounters": 0, "fixed": 2})

    def test_katakana_form(self):
        s = script()
        tr.apply_report(s, REPORT, {}, "kata")
        self.assertEqual(s["scenes"][0]["lines"][0]["reading"], "だが｜絵画《カイガ》は猫じゃない。")

    def test_hostile_report(self):
        s = script()
        for bad in ({}, {"lines": []}, {"lines": {"a": "x"}}, {"lines": {"a": {"words": ["x"]}}}):
            tr.apply_report(s, bad, {})
        self.assertNotIn("reading", s["scenes"][0]["lines"][0])
        self.assertEqual(tr.apply_report({"scenes": "x"}, {}, {}), {"lines": 0, "withNames": 0, "withCounters": 0, "fixed": 0})


class Cli(unittest.TestCase):
    def test_apply_round_trip(self):
        with tempfile.TemporaryDirectory() as d:
            sp, rp, np_, op = (os.path.join(d, n) for n in ("s.json", "r.json", "n.json", "o.json"))
            for p, v in ((sp, script()), (rp, REPORT), (np_, {"茜": "あかね"})):
                write(p, json.dumps(v, ensure_ascii=False))
            out = io.StringIO()
            with redirect_stdout(out):
                code = tr.main(["apply", "--script", sp, "--report", rp, "--names", np_, "--out", op])
            self.assertEqual(code, 0)
            self.assertEqual(json.loads(out.getvalue())["fixed"], 2)
            s = read_json(op)
            self.assertEqual(s["scenes"][0]["lines"][1]["reading"], "なんだ、｜楓《かえで》か‥‥。")

    def test_bad_input_is_an_error_not_a_crash(self):
        with tempfile.TemporaryDirectory() as d:
            sp = os.path.join(d, "s.json")
            write(sp, "[1, 2")
            err = io.StringIO()
            with redirect_stderr(err):
                self.assertEqual(tr.main(["apply", "--script", sp, "--out", os.path.join(d, "o.json")]), 2)
                self.assertEqual(tr.main(["apply", "--script", os.path.join(d, "missing.json"), "--out", os.path.join(d, "o.json")]), 2)
            write(sp, "[1, 2]")
            with redirect_stderr(err):
                self.assertEqual(tr.main(["apply", "--script", sp, "--out", os.path.join(d, "o.json")]), 2)


if __name__ == "__main__":
    unittest.main()
