import io
import json
import os
import struct
import sys
import tempfile
import unittest
import zlib
from contextlib import redirect_stderr

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))

import mouth_cutouts as mc  # noqa: E402

W, H = 40, 60
SKIN = (250, 220, 200, 255)
DARK = (40, 20, 20, 255)
OPT = dict(face_frac=0.5, max_face_frac=0.2, pad=2, feather=1)


def figure(mouth="closed", eyes="open", shift=0):
    """A tiny sprite: transparent canvas, a skin-coloured body from row 5, eyes at row 12, mouth near row 24."""
    img = mc.Image(W, H)
    for y in range(5, H):
        for x in range(8 + shift, 32 + shift):
            img.put(x, y, SKIN)
    for ex in (14, 24):
        if eyes == "open":
            for y in range(11, 14):
                for x in range(ex, ex + 3):
                    img.put(x + shift, y, DARK)
        else:
            for x in range(ex - 1, ex + 4):
                img.put(x + shift, 13, DARK)
    if mouth == "closed":
        for x in range(16, 24):
            img.put(x + shift, 24, DARK)
    elif mouth == "wide":
        for y in range(22, 28):
            for x in range(16, 24):
                img.put(x + shift, y, DARK)
    elif mouth == "small":
        for y in range(23, 27):
            for x in range(17, 23):
                img.put(x + shift, y, DARK)
    elif mouth == "frown":  # closed, another shape: a line as long as the closed one, moved
        for x in range(12, 20):
            img.put(x + shift, 26, DARK)
    return img


def chunk(t, body):
    return struct.pack(">I", len(body)) + t + body + struct.pack(">I", zlib.crc32(t + body) & 0xFFFFFFFF)


def png_bytes(w, h, rows, color=6, depth=8, idat=None):
    raw = b"".join(rows)
    body = zlib.compress(raw) if idat is None else idat
    return (mc.PNG_SIG + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, depth, color, 0, 0, 0))
            + chunk(b"IDAT", body) + chunk(b"IEND", b""))


def filtered_rows(pixels, w, bpp, filters):
    """Encode rows (lists of bytes) with the given filter type per row, as a PNG encoder would."""
    stride = w * bpp
    out = []
    prev = bytes(stride)
    for y, line in enumerate(pixels):
        ft = filters[y % len(filters)]
        enc = bytearray()
        for x in range(stride):
            a = line[x - bpp] if x >= bpp else 0
            b = prev[x]
            c = prev[x - bpp] if x >= bpp else 0
            pred = (0, a, b, (a + b) >> 1, mc._paeth(a, b, c))[ft]
            enc.append((line[x] - pred) & 255)
        out.append(bytes([ft]) + bytes(enc))
        prev = line
    return out


class PngTest(unittest.TestCase):
    def test_round_trip(self):
        img = figure("wide")
        img.put(0, 0, (1, 2, 3, 4))
        back = mc.decode_png(mc.encode_png(img))
        self.assertEqual((back.w, back.h), (W, H))
        self.assertEqual(back.px, img.px)

    def test_all_filters_rgb_and_rgba(self):
        w, h = 5, 6
        for bpp, color in ((3, 2), (4, 6)):
            pixels = [bytes((x * 37 + y * 11 + k * 5) & 255 for x in range(w) for k in range(bpp)) for y in range(h)]
            data = png_bytes(w, h, filtered_rows(pixels, w, bpp, [0, 1, 2, 3, 4, 2]), color=color)
            img = mc.decode_png(data)
            for y in range(h):
                for x in range(w):
                    want = tuple(pixels[y][x * bpp:x * bpp + bpp]) + ((255,) if bpp == 3 else ())
                    self.assertEqual(img.pixel(x, y), want, (bpp, x, y))

    def test_broken_inputs(self):
        good = mc.encode_png(figure())
        cases = {
            "signature": b"GIF89a" + good[6:],
            "truncated": good[: len(good) // 2],
            "crc": good[:30] + bytes([good[30] ^ 0xFF]) + good[31:],
            "16-bit": png_bytes(2, 2, [b"\x00" + b"\x00" * 16] * 2, depth=16),
            "palette": png_bytes(2, 2, [b"\x00\x00\x00"] * 2, color=3),
            "huge": png_bytes(100_000, 100_000, [b"\x00"]),
            "too much data": png_bytes(2, 2, [b"\x00" + b"\x01" * 8] * 50),
            "bad filter": png_bytes(2, 1, [b"\x09" + b"\x00" * 8]),
            "not zlib": png_bytes(2, 1, [], idat=b"not zlib at all"),
            "empty": b"",
        }
        for name, data in cases.items():
            with self.assertRaises(mc.PngError, msg=name):
                mc.decode_png(data)

    def test_read_missing_file(self):
        with self.assertRaises(mc.PngError):
            mc.read_png(os.path.join(tempfile.gettempdir(), "no-such-dir-xyz", "a.png"))


class AnalyseTest(unittest.TestCase):
    def setUp(self):
        self.opt = mc.Options(**OPT)

    def test_rect_detection_paste_and_closed(self):
        imgs = {"aka00ad": figure("closed"), "aka01ad": figure("wide")}
        res = mc.analyse(imgs, self.opt)
        self.assertEqual(res["clusters"][0]["members"], ["aka00ad", "aka01ad"])
        self.assertEqual(res["clusters"][0]["base"], "aka00ad")
        e0, e1 = res["expressions"]["aka00ad"], res["expressions"]["aka01ad"]
        # the mouth rectangle covers rows 22..27 and columns 16..23 with 2 px margin
        rect = e0["donors"][0]["rect"]
        self.assertEqual(rect, [14, 20, 26, 30])
        made0 = res["made"]["aka00ad"]
        self.assertEqual(set(made0), {"a", "i", "u", "e", "o", "open"})  # base: no _closed, no blink
        # inside the rectangle (beyond the feather) the open mouth, outside the base untouched
        self.assertEqual(made0["a"].pixel(20, 25), DARK)
        self.assertEqual(made0["a"].pixel(20, 12), imgs["aka00ad"].pixel(20, 12))
        self.assertEqual(made0["a"].pixel(5, 40), imgs["aka00ad"].pixel(5, 40))
        # one candidate: all vowels the same mouth
        self.assertEqual(made0["i"].px, made0["a"].px)
        # the open picture gets its closed version and the open mouth back as vowels
        made1 = res["made"]["aka01ad"]
        self.assertIn("closed", made1)
        self.assertEqual(made1["closed"].px, imgs["aka00ad"].px)
        self.assertEqual(made1["a"].px, imgs["aka01ad"].px)
        self.assertEqual(e1["closedRect"], rect)
        self.assertIn("none", e1["blink"])

    def test_feathered_edge_blends(self):
        a, b = figure("closed"), figure("closed")
        for y in range(H):
            for x in range(W):
                b.put(x, y, (0, 0, 0, 255))
        out = mc.paste(a, b, (10, 10, 20, 20), 2)
        self.assertEqual(out.pixel(15, 15), (0, 0, 0, 255))
        edge = out.pixel(10, 15)
        self.assertTrue(0 < edge[0] < SKIN[0], edge)  # the outer row is a mix
        self.assertEqual(out.pixel(9, 15), a.pixel(9, 15))

    def test_vowels_sorted_by_opening(self):
        imgs = {"aka00ad": figure("closed"), "aka01ad": figure("small"), "aka02ad": figure("wide")}
        res = mc.analyse(imgs, self.opt)
        e = res["expressions"]["aka00ad"]
        self.assertEqual([d["from"] for d in e["donors"]], ["aka02ad", "aka01ad"])
        self.assertEqual(e["vowels"]["a"], "aka02ad")
        self.assertEqual(e["vowels"]["o"], "aka02ad")
        self.assertEqual(e["vowels"]["i"], "aka01ad")
        self.assertEqual(e["vowels"]["u"], "aka01ad")

    def test_closed_mouth_in_another_shape(self):
        imgs = {"aka00ad": figure("closed"), "aka01ad": figure("wide"), "aka02ad": figure("frown")}
        res = mc.analyse(imgs, self.opt)
        e2 = res["expressions"]["aka02ad"]
        self.assertEqual(e2["mouth"], "closed, another shape")
        self.assertEqual([d["from"] for d in e2["donors"]], ["aka01ad"])  # the frown is no donor
        made2 = res["made"]["aka02ad"]
        self.assertNotIn("closed", made2)  # its own mouth is already closed
        a = made2["a"]
        self.assertEqual(a.pixel(20, 25), DARK)  # the open mouth
        self.assertEqual(a.pixel(12, 26), SKIN)  # no end of the frown left beside it
        self.assertEqual(a.pixel(13, 26), SKIN)

    def test_other_pose_is_skipped(self):
        imgs = {"aka00ad": figure("closed"), "aka01ad": figure("wide"), "aka02ad": figure("wide", shift=6)}
        res = mc.analyse(imgs, self.opt)
        self.assertIn("same pose", res["expressions"]["aka02ad"]["skipped"])
        self.assertNotIn("aka02ad", res["made"])
        self.assertIn("aka00ad", res["made"])

    def test_blink_only_when_asked(self):
        imgs = {"aka00ad": figure("closed"), "aka01ad": figure("wide"), "aka02ad": figure("closed", eyes="shut")}
        res = mc.analyse(imgs, self.opt)
        self.assertTrue(all("blink" not in m for m in res["made"].values()))
        res = mc.analyse(imgs, self.opt, blink_nn="02")
        made1 = res["made"]["aka01ad"]
        self.assertIn("blink", made1)
        blink = made1["blink"]
        self.assertEqual(blink.pixel(20, 24), DARK)        # mouth closed (the resting face)
        self.assertEqual(blink.pixel(20, 26), SKIN)
        self.assertEqual(blink.pixel(15, 12), SKIN)        # eye open part gone
        self.assertEqual(blink.pixel(15, 13), DARK)        # closed-eye line

    def test_only_eyes_change(self):
        imgs = {"aka00ad": figure("closed"), "aka01ad": figure("closed", eyes="shut")}
        res = mc.analyse(imgs, self.opt)
        self.assertIn("only the eyes", res["expressions"]["aka00ad"]["skipped"])
        self.assertEqual(res["made"], {})


class RunTest(unittest.TestCase):
    def make_dump(self, tmp, pictures):
        folder = os.path.join(tmp, "dump", mc.SPRITE_DIR, "aka_茜")
        os.makedirs(folder)
        for name, img in pictures.items():
            with open(os.path.join(folder, name + ".png"), "wb") as f:
                f.write(img if isinstance(img, bytes) else mc.encode_png(img))
        return os.path.join(tmp, "dump")

    def test_files_named_by_rule(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self.make_dump(tmp, {"aka00ad": figure("closed"), "aka01ad": figure("wide"),
                                        "aka02ad": b"\x89PNG broken", "aka00bd": figure("closed")})
            out = os.path.join(tmp, "eve_sprites")
            rep = mc.run(root, ["aka"], out, mc.Options(**OPT), {})
            got = sorted(os.listdir(os.path.join(out, "aka")))
            want = sorted([f"aka00ad_{k}.png" for k in ("a", "i", "u", "e", "o", "open")]
                          + [f"aka01ad_{k}.png" for k in ("a", "i", "u", "e", "o", "open", "closed")])
            self.assertEqual(got, want)
            c = rep["codes"]["aka"]
            self.assertIn("cannot read", c["skipped"]["aka02ad"])
            self.assertIn("same pose", c["skipped"]["aka00bd"])
            self.assertTrue(os.path.isfile(os.path.join(out, "_review", "aka.png")))
            back = mc.read_png(os.path.join(out, "aka", "aka01ad_closed.png"))
            self.assertEqual(back.px, figure("closed").px)
            # existing files are kept, then replaced with overwrite
            rep2 = mc.run(root, ["aka"], out, mc.Options(**OPT), {})
            self.assertEqual(rep2["codes"]["aka"]["counts"]["written"], 0)
            self.assertEqual(rep2["codes"]["aka"]["counts"]["kept"], 13)
            rep3 = mc.run(root, ["aka"], out, mc.Options(**OPT), {}, overwrite=True)
            self.assertEqual(rep3["codes"]["aka"]["counts"]["written"], 13)

    def test_missing_folders_and_bad_codes(self):
        with tempfile.TemporaryDirectory() as tmp:
            rep = mc.run(os.path.join(tmp, "nothing"), ["aka"], tmp, mc.Options(), {})
            self.assertTrue(rep["warnings"])
            root = self.make_dump(tmp, {"aka00ad": figure()})
            rep = mc.run(root, ["kyo", "../x", "aka"], tmp, mc.Options(), {})
            self.assertIn("no sprite folder", rep["codes"]["kyo"]["error"])
            self.assertIn("error", rep["codes"]["../x"])
            self.assertIn("same pose", rep["codes"]["aka"]["skipped"]["aka00ad"])

    def test_cli(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self.make_dump(tmp, {"aka00ad": figure("closed"), "aka01ad": figure("wide")})
            report = os.path.join(tmp, "r.json")
            err = io.StringIO()
            with redirect_stderr(err):
                code = mc.main(["aka", "--root", root, "--face-frac", "0.5", "--pad", "2", "--report", report, "--dry-run"])
            self.assertEqual(code, 0)
            self.assertIn("no blink", err.getvalue())
            with open(report, encoding="utf-8") as f:
                rep = json.load(f)
            self.assertEqual(rep["codes"]["aka"]["counts"]["written"], 13)
            self.assertFalse(os.path.exists(os.path.join(tmp, "eve_sprites")))  # dry run wrote nothing
            with redirect_stderr(io.StringIO()):
                self.assertEqual(mc.main(["aka", "--root", root, "--threshold", "0"]), 2)
                self.assertEqual(mc.main(["aka", "--root", root, "--blink-source", "aka", "x"]), 2)
                self.assertEqual(mc.main(["zzz", "--root", root, "--dry-run"]), 2)


if __name__ == "__main__":
    unittest.main()
