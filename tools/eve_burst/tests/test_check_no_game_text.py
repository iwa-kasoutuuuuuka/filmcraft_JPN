"""check_no_game_text.py against a tiny made-up dump (no real data)."""
import io
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import check_no_game_text as ck  # noqa: E402

LINE = "これは見本のための作り物の長い台詞です。"
NOTE = "（効果音命令で再生される作り物の注記）"


def dump(root):
    os.makedirs(os.path.join(root, "00_目録"))
    os.makedirs(os.path.join(root, "01_シナリオ", "a"))
    with io.open(os.path.join(root, ck.CATALOG), "w", encoding="utf-8-sig", newline="") as f:
        f.write("ボイスファイル\t収録PAK\tシナリオ\t命令位置\t話者\t台詞\t種別\n")
        f.write(f"x0001.ahx\tP\ta01\t0x1\t太郎\t{LINE}\t台詞（0x8035）\n")
        f.write(f"j301.ahx\tP\ta01\t0x2\t\t{NOTE}\t効果音命令（0x800b）\n")
        f.write("x0002.ahx\tP\ta01\t0x3\t太郎\tはい。\t台詞（0x8035）\n")
    with io.open(os.path.join(root, "01_シナリオ", "a", "a01.json"), "w", encoding="utf-8") as f:
        body = {"instructions": [{"op": 1, "text": "地の文として出てくる別の作り物の一文‥‥である。"}, {"name": "名前だけの欄は数えない長い文字列です"}]}
        json.dump(body, f, ensure_ascii=False)


class Lines(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = os.path.join(self.tmp.name, "dump")
        dump(self.root)

    def tearDown(self):
        self.tmp.cleanup()

    def test_what_counts(self):
        lines = ck.game_lines(self.root, 12)
        self.assertIn(ck.norm(LINE), lines)
        self.assertIn("地の文として出てくる別の作り物の一文……である。", lines)  # the two kinds of dots are one
        self.assertNotIn(ck.norm(NOTE), lines)  # a note of the catalog on an effect row
        self.assertFalse(any("名前だけ" in line for line in lines))
        self.assertNotIn("はい。", lines)  # too short to be anybody's

    def test_missing_dump(self):
        self.assertIsNone(ck.game_lines(os.path.join(self.tmp.name, "nowhere"), 12))
        self.assertEqual(ck.main(["--root", os.path.join(self.tmp.name, "nowhere")]), 2)
        self.assertEqual(ck.main(["--root", ""]), 2)

    def test_find(self):
        lines = ck.game_lines(self.root, 12)
        index = {}
        for line in lines:
            index.setdefault(line[: ck.KEY], []).append(line)
        text = f'first\n  "text": "{LINE}",\nこれは見本のための別の文です。\n地の文として 出てくる別の作り物の一文…である。\n'
        hits = ck.find(text, index)
        self.assertEqual([n for n, _ in hits], [2])  # inside quotes; a line with half the dots is another line
        self.assertEqual(len(ck.find("地の文として 出てくる別の作り物の一文‥‥である。", index)), 1)  # spaces ignored
        self.assertEqual(ck.find("これは見本のための作り物の", index), [])  # only a part of the line

    def test_files(self):
        lines = ck.game_lines(self.root, 12)
        clean = os.path.join(self.tmp.name, "clean.md")
        dirty = os.path.join(self.tmp.name, "dirty.py")
        binary = os.path.join(self.tmp.name, "x.png")
        with io.open(clean, "w", encoding="utf-8") as f:
            f.write("何も引用していない文書です。\n")
        with io.open(dirty, "w", encoding="utf-8") as f:
            f.write(f'X = "{LINE}"\n')
        with open(binary, "wb") as f:
            f.write(b"\xff\xfe" + LINE.encode("utf-8"))
        hits = ck.scan(lines, None, [clean, dirty, binary, os.path.join(self.tmp.name, "missing.txt")])
        self.assertEqual(list(hits), [dirty])
        self.assertEqual(hits[dirty][0][0], 1)


if __name__ == "__main__":
    unittest.main()
