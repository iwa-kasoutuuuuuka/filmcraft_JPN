"""Make mouth (and, when asked, blink) variants of the sprites by cutting them out of other expressions.

    python mouth_cutouts.py --root "E:/filmcraft_JPN/PS2_EVE_Bursterror解析データ" aka koj \
        [--out-dir DIR] [--blink-source aka 05] [--overwrite] [--report FILE] [--dry-run]

The game has no open/closed pairs of one face (docs/eve-burst-assets.md, "Sprites"), but the
expressions of one costume are often drawn on the same body: `aka00ad` (mouth closed) and `aka01ad`
(mouth open, other eyebrows) differ only around the eyes and the mouth. This tool finds those
differences and pastes the rectangles over each other, so the variants keep the canvas, the place
and the line work of the original exactly (the rule of "Mouth and blink variants").

For one code and outfit (`aka..a`), all pictures are read and compared pairwise. A pair whose
difference stays inside the face (the top `--face-frac` of the figure) and is small is "the same
pose"; pictures in the same pose form a cluster. In a cluster the changed pixels are split into
connected components and the components into horizontal bands: the lowest band is the mouth, the
bands above are the eyes (and eyebrows). The base of a cluster is `00`, else the picture with the
fewest dark pixels in the mouth band.

For each expression X of a cluster:

- `<X>_closed.png`: when X's mouth differs from the base by at least `--open-min-h` rows, the base's
  mouth rectangle pasted onto X (its resting face; only for a picture drawn mouth-open).
- `<X>_a/_i/_u/_e/_o.png` and `_open`: X's resting face with the mouth rectangle of each open
  member Y. The candidates are sorted by how far the mouth opens (rectangle height, then dark
  pixels): the widest is `_a` (and `_open`), then by rank `_o`, `_e`, `_u`, `_i`; with fewer
  candidates they repeat (one candidate -> all five are the same mouth).
- `<X>_blink.png`: only with `--blink-source <code> <NN>` naming a picture drawn with closed eyes;
  its eye rectangle is pasted onto each resting face of its cluster. Without it no blink is made.

Rectangles get `--pad` pixels of margin and their outer `--feather` pixels are blended into the
picture below, so the edge does not show. Existing files are kept unless `--overwrite`. A review
sheet per code is written to `<out>/_review/<code>.png`: one row per picture, tiles of the face
in this order: original with the rectangles (red = mouth, blue = eyes), `_closed`, `_a _i _u _e _o`,
`_blink`, and the difference between the original and `_a` (red pixels). A skipped picture shows
only its original, framed in red. The JSON report says, per picture, the donors and rectangles, the
files written and why something was skipped.

Limits: the vowels are not real mouth shapes, only different openings taken from other
expressions. A cluster of one picture gets nothing (a picture in another pose has its head redrawn,
not moved, so its mouth cannot be taken over; a hand over the mouth shows as a too large mouth
difference or another pose). Standard library only (PNG via zlib + struct:
8-bit RGB/RGBA, filters 0-4, no interlace; anything else is reported and skipped). Input is treated
as hostile: file sizes, image sizes, counts and names are capped; a broken PNG is skipped with a
warning; a missing folder is reported. The dump is only read.
"""

import argparse
import json
import os
import re
import struct
import sys
import zlib

MAX_FILE_BYTES = 32 * 1024 * 1024
MAX_DIM = 4096
MAX_PIXELS = 2048 * 2048        # a decoded picture
MAX_SHEET_PIXELS = 4096 * 4096  # a picture made here (the review sheet)
MAX_GROUP_FILES = 64
MAX_DIR_ENTRIES = 50_000
MAX_CODES = 64
MAX_COMPONENTS = 20_000
PNG_SIG = b"\x89PNG\r\n\x1a\n"
CODE_RE = re.compile(r"^[a-z]{3}$")
VOWELS = ("a", "i", "u", "e", "o")
# vowel -> rank of openness (1 = widest candidate, 0 = smallest)
VOWEL_RANK = {"a": 1.0, "o": 0.75, "e": 0.5, "u": 0.25, "i": 0.0}
SPRITE_DIR = os.path.join("02_画像", "立ち絵")


class PngError(Exception):
    pass


class Image:
    """8-bit RGBA picture; `px` is a bytearray of w*h*4."""

    __slots__ = ("w", "h", "px")

    def __init__(self, w: int, h: int, px=None):
        if w <= 0 or h <= 0 or w > 2 * MAX_DIM or h > 2 * MAX_DIM or w * h > MAX_SHEET_PIXELS:
            raise PngError(f"image size {w}x{h} out of range")
        self.w = w
        self.h = h
        self.px = bytearray(w * h * 4) if px is None else bytearray(px)
        if len(self.px) != w * h * 4:
            raise PngError("pixel buffer has the wrong size")

    def copy(self) -> "Image":
        return Image(self.w, self.h, self.px)

    def pixel(self, x: int, y: int) -> tuple:
        i = (y * self.w + x) * 4
        return tuple(self.px[i:i + 4])

    def put(self, x: int, y: int, rgba) -> None:
        if 0 <= x < self.w and 0 <= y < self.h:
            i = (y * self.w + x) * 4
            self.px[i:i + 4] = bytes(rgba)

    def crop(self, x0: int, y0: int, x1: int, y1: int) -> "Image":
        x0, y0 = max(0, x0), max(0, y0)
        x1, y1 = min(self.w, x1), min(self.h, y1)
        if x1 <= x0 or y1 <= y0:
            raise PngError("empty crop")
        out = Image(x1 - x0, y1 - y0)
        rw = (x1 - x0) * 4
        for y in range(y0, y1):
            s = (y * self.w + x0) * 4
            d = (y - y0) * rw
            out.px[d:d + rw] = self.px[s:s + rw]
        return out


# ---------------------------------------------------------------- PNG

def _paeth(a: int, b: int, c: int) -> int:
    p = a + b - c
    pa = p - a if p >= a else a - p
    pb = p - b if p >= b else b - p
    pc = p - c if p >= c else c - p
    if pa <= pb and pa <= pc:
        return a
    return b if pb <= pc else c


def decode_png(data: bytes) -> Image:
    if len(data) > MAX_FILE_BYTES:
        raise PngError(f"file larger than {MAX_FILE_BYTES} bytes")
    if data[:8] != PNG_SIG:
        raise PngError("not a PNG (bad signature)")
    pos = 8
    ihdr = None
    idat = []
    idat_len = 0
    seen_end = False
    while pos + 8 <= len(data):
        (length,) = struct.unpack(">I", data[pos:pos + 4])
        ctype = data[pos + 4:pos + 8]
        if length > len(data) - pos - 12:
            raise PngError(f"chunk {ctype!r} runs past the end of the file")
        body = data[pos + 8:pos + 8 + length]
        (crc,) = struct.unpack(">I", data[pos + 8 + length:pos + 12 + length])
        if zlib.crc32(ctype + body) & 0xFFFFFFFF != crc:
            raise PngError(f"bad CRC in chunk {ctype!r}")
        pos += 12 + length
        if ctype == b"IHDR":
            if length != 13:
                raise PngError("bad IHDR")
            ihdr = struct.unpack(">IIBBBBB", body)
        elif ctype == b"IDAT":
            idat_len += length
            if idat_len > MAX_FILE_BYTES:
                raise PngError("too much image data")
            idat.append(body)
        elif ctype == b"IEND":
            seen_end = True
            break
    if ihdr is None:
        raise PngError("no IHDR")
    w, h, depth, color, comp, filt, interlace = ihdr
    if w == 0 or h == 0 or w > MAX_DIM or h > MAX_DIM or w * h > MAX_PIXELS:
        raise PngError(f"image size {w}x{h} out of range")
    if depth != 8 or color not in (2, 6):
        raise PngError(f"unsupported PNG (bit depth {depth}, colour type {color}); only 8-bit RGB/RGBA")
    if comp != 0 or filt != 0 or interlace != 0:
        raise PngError("unsupported PNG (interlaced or unknown method)")
    if not idat or not seen_end:
        raise PngError("truncated PNG (no image data or no IEND)")
    bpp = 4 if color == 6 else 3
    stride = w * bpp
    expected = h * (stride + 1)
    dec = zlib.decompressobj()
    try:
        raw = dec.decompress(b"".join(idat), expected + 1)
    except zlib.error as e:
        raise PngError(f"broken image data: {e}") from None
    if len(raw) != expected:
        raise PngError(f"image data has {len(raw)} bytes, expected {expected}")
    out = bytearray(w * h * 4)
    prev = bytearray(stride)
    for y in range(h):
        o = y * (stride + 1)
        ft = raw[o]
        line = bytearray(raw[o + 1:o + 1 + stride])
        if ft == 0:
            pass
        elif ft == 1:
            for x in range(bpp, stride):
                line[x] = (line[x] + line[x - bpp]) & 255
        elif ft == 2:
            line = bytearray((a + b) & 255 for a, b in zip(line, prev))
        elif ft == 3:
            for x in range(stride):
                left = line[x - bpp] if x >= bpp else 0
                line[x] = (line[x] + ((left + prev[x]) >> 1)) & 255
        elif ft == 4:
            for x in range(stride):
                if x >= bpp:
                    line[x] = (line[x] + _paeth(line[x - bpp], prev[x], prev[x - bpp])) & 255
                else:
                    line[x] = (line[x] + prev[x]) & 255
        else:
            raise PngError(f"bad filter type {ft} in row {y}")
        prev = line
        d = y * w * 4
        if bpp == 4:
            out[d:d + stride] = line
        else:
            for x in range(w):
                out[d + 4 * x:d + 4 * x + 3] = line[3 * x:3 * x + 3]
                out[d + 4 * x + 3] = 255
    return Image(w, h, out)


def read_png(path: str) -> Image:
    try:
        size = os.path.getsize(path)
        if size > MAX_FILE_BYTES:
            raise PngError(f"file larger than {MAX_FILE_BYTES} bytes")
        with open(path, "rb") as f:
            data = f.read(MAX_FILE_BYTES + 1)
    except OSError as e:
        raise PngError(f"cannot read: {e}") from None
    return decode_png(data)


def _chunk(ctype: bytes, body: bytes) -> bytes:
    return struct.pack(">I", len(body)) + ctype + body + struct.pack(">I", zlib.crc32(ctype + body) & 0xFFFFFFFF)


def encode_png(img: Image) -> bytes:
    stride = img.w * 4
    raw = bytearray()
    prev = bytes(stride)
    for y in range(img.h):
        line = bytes(img.px[y * stride:(y + 1) * stride])
        # filter 2 (Up) when the row repeats a lot of the previous one, else 0; keeps files small and fast
        up = bytes((a - b) & 255 for a, b in zip(line, prev)) if y else line
        if y and up.count(0) > line.count(0):
            raw.append(2)
            raw += up
        else:
            raw.append(0)
            raw += line
        prev = line
    ihdr = struct.pack(">IIBBBBB", img.w, img.h, 8, 6, 0, 0, 0)
    return PNG_SIG + _chunk(b"IHDR", ihdr) + _chunk(b"IDAT", zlib.compress(bytes(raw), 6)) + _chunk(b"IEND", b"")


def write_png(path: str, img: Image) -> None:
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    tmp = path + ".tmp"
    with open(tmp, "wb") as f:
        f.write(encode_png(img))
    os.replace(tmp, path)


# ---------------------------------------------------------------- analysis

class Options:
    def __init__(self, **kw):
        self.t_rgb = 40          # a channel difference that counts as changed
        self.t_alpha = 40
        self.face_frac = 0.30    # the face is the top part of the figure
        self.max_outside = 40    # changed pixels allowed outside the face for "same pose"
        self.max_face_frac = 0.08  # changed share of the face area allowed for "same pose"
        self.min_component = 3   # smaller components are noise
        self.band_gap = 3        # rows between components of one band
        self.mouth_min_pos = 0.5  # the mouth band's centre must be below this share of the face
        self.mouth_max_w = 60
        self.mouth_max_h = 40
        self.eyes_max_h = 60
        self.open_min_h = 3      # a mouth differing over at least this many rows is drawn open
        self.dark_luma = 90
        self.pad = 3
        self.feather = 2
        for k, v in kw.items():
            if not hasattr(self, k):
                raise ValueError(f"unknown option {k}")
            setattr(self, k, v)


def opaque_bbox(img: Image, alpha_min: int = 16):
    """(x0, y0, x1, y1) of the visible pixels, exclusive end; None when all transparent."""
    w, h, px = img.w, img.h, img.px
    x0, y0, x1, y1 = w, h, -1, -1
    for y in range(h):
        row = px[y * w * 4:(y + 1) * w * 4]
        alphas = row[3::4]
        if max(alphas) < alpha_min:
            continue
        xs = [x for x, a in enumerate(alphas) if a >= alpha_min]
        y0 = min(y0, y)
        y1 = y
        x0 = min(x0, xs[0])
        x1 = max(x1, xs[-1])
    if y1 < 0:
        return None
    return (x0, y0, x1 + 1, y1 + 1)


def _differs(pa, pb, i, t_rgb, t_a) -> bool:
    a1, a2 = pa[i + 3], pb[i + 3]
    if abs(a1 - a2) >= t_a:
        return True
    if a1 < t_a and a2 < t_a:
        return False
    return (abs(pa[i] - pb[i]) >= t_rgb or abs(pa[i + 1] - pb[i + 1]) >= t_rgb
            or abs(pa[i + 2] - pb[i + 2]) >= t_rgb)


def compare(a: Image, b: Image, face: tuple, opt: Options):
    """Changed pixels of a against b: (set of (x, y) inside the face rows, count outside, face count).

    Stops counting outside the face once over the limit (a different pose)."""
    w = a.w
    fy0, fy1 = face
    inside = set()
    outside = 0
    pa, pb = a.px, b.px
    ra, rb = memoryview(pa), memoryview(pb)
    for y in range(a.h):
        s = y * w * 4
        if ra[s:s + w * 4] == rb[s:s + w * 4]:
            continue
        in_face = fy0 <= y < fy1
        for x in range(w):
            i = s + 4 * x
            if pa[i:i + 4] == pb[i:i + 4]:
                continue
            if _differs(pa, pb, i, opt.t_rgb, opt.t_alpha):
                if in_face:
                    inside.add((x, y))
                    if len(inside) > MAX_PIXELS:
                        break
                else:
                    outside += 1
        if outside > opt.max_outside:
            break
    return inside, outside


def components(pixels: set, min_size: int) -> list:
    """8-connected components as (x0, y0, x1, y1, count), exclusive end; tiny ones dropped."""
    left = set(pixels)
    out = []
    while left and len(out) < MAX_COMPONENTS:
        start = left.pop()
        stack = [start]
        x0 = x1 = start[0]
        y0 = y1 = start[1]
        n = 0
        while stack:
            x, y = stack.pop()
            n += 1
            x0, x1 = min(x0, x), max(x1, x)
            y0, y1 = min(y0, y), max(y1, y)
            for dy in (-1, 0, 1):
                for dx in (-1, 0, 1):
                    q = (x + dx, y + dy)
                    if q in left:
                        left.discard(q)
                        stack.append(q)
        if n >= min_size:
            out.append((x0, y0, x1 + 1, y1 + 1, n))
    return out


def bands(comps: list, gap: int) -> list:
    """Components grouped by rows: [(y0, y1, [components])] from top to bottom."""
    out = []
    for c in sorted(comps, key=lambda c: c[1]):
        if out and c[1] <= out[-1][1] + gap:
            y0, y1, cs = out[-1]
            out[-1] = (y0, max(y1, c[3]), cs + [c])
        else:
            out.append((c[1], c[3], [c]))
    return out


def union_box(boxes):
    boxes = list(boxes)
    if not boxes:
        return None
    return (min(b[0] for b in boxes), min(b[1] for b in boxes), max(b[2] for b in boxes), max(b[3] for b in boxes))


def grow(box, pad: int, w: int, h: int):
    return (max(0, box[0] - pad), max(0, box[1] - pad), min(w, box[2] + pad), min(h, box[3] + pad))


def paste(dst: Image, src: Image, rect, feather: int) -> Image:
    """dst with src's pixels inside rect; the outer `feather` pixels blend towards dst."""
    out = dst.copy()
    x0, y0, x1, y1 = rect
    w = dst.w
    for y in range(max(0, y0), min(dst.h, y1)):
        dy = min(y - y0, y1 - 1 - y)
        for x in range(max(0, x0), min(w, x1)):
            d = min(dy, x - x0, x1 - 1 - x)
            k = 256 if d >= feather else (d + 1) * 256 // (feather + 1)
            i = (y * w + x) * 4
            for c in range(4):
                out.px[i + c] = (src.px[i + c] * k + dst.px[i + c] * (256 - k) + 128) >> 8
    return out


def dark_count(img: Image, box, opt: Options) -> int:
    if box is None:
        return 0
    n = 0
    for y in range(box[1], box[3]):
        for x in range(box[0], box[2]):
            r, g, b, a = img.pixel(x, y)
            if a >= 128 and (r * 299 + g * 587 + b * 114) // 1000 < opt.dark_luma:
                n += 1
    return n


def pick_ranked(cands: list, vowel: str):
    """cands sorted widest first; the vowel's rank picks one."""
    n = len(cands)
    idx = round((1.0 - VOWEL_RANK[vowel]) * (n - 1))
    return cands[max(0, min(n - 1, idx))]


def analyse(images: dict, opt: Options, blink_nn=None) -> dict:
    """images: {name: Image} of one code+outfit (same canvas). Returns the plan with the pictures made.

    Result: {"face": [y0, y1], "clusters": [...], "expressions": {name: {...}}, "made": {name: {kind: Image}}}
    """
    names = sorted(images)
    res = {"face": None, "clusters": [], "expressions": {}, "made": {}, "zones": {}}
    if not names:
        return res
    first = images[names[0]]
    w, h = first.w, first.h
    boxes = [b for b in (opaque_bbox(images[n]) for n in names) if b]
    if not boxes:
        for n in names:
            res["expressions"][n] = {"skipped": "fully transparent"}
        return res
    fig = union_box(boxes)
    fy0 = fig[1]
    fy1 = min(h, fy0 + max(1, int(round((fig[3] - fig[1]) * opt.face_frac))))
    res["face"] = [fy0, fy1]
    face_area = max(1, (fy1 - fy0) * (fig[2] - fig[0]))
    # pairwise: same pose?
    diffs = {}
    same = {n: {n} for n in names}
    for i, a in enumerate(names):
        for b in names[i + 1:]:
            inside, outside = compare(images[a], images[b], (fy0, fy1), opt)
            ok = outside <= opt.max_outside and len(inside) <= opt.max_face_frac * face_area
            diffs[(a, b)] = diffs[(b, a)] = (inside, outside, ok)
            if ok:
                same[a].add(b)
                same[b].add(a)
    # clusters = connected parts of the "same pose" graph
    seen = set()
    clusters = []
    for n in names:
        if n in seen:
            continue
        stack, part = [n], set()
        while stack:
            m = stack.pop()
            if m in part:
                continue
            part.add(m)
            stack.extend(same[m] - part)
        seen |= part
        clusters.append(sorted(part))
    for cl in clusters:
        _plan_cluster(cl, images, diffs, (fy0, fy1), w, h, opt, blink_nn, res)
    return res


def _nn(name: str) -> str:
    m = re.match(r"^[a-z]{3}(\d\d)", name)
    return m.group(1) if m else ""


def _plan_cluster(cl, images, diffs, face, w, h, opt, blink_nn, res):
    fy0, fy1 = face
    info = {"members": cl}
    res["clusters"].append(info)
    if len(cl) == 1:
        res["expressions"][cl[0]] = {"skipped": "no other picture in the same pose (nothing to cut a mouth from)"}
        return
    # zones from every pair in the cluster
    comps_of = {}
    allc = []
    for i, a in enumerate(cl):
        for b in cl[i + 1:]:
            cs = components(diffs[(a, b)][0], opt.min_component)
            comps_of[(a, b)] = comps_of[(b, a)] = cs
            allc += cs
    bs = bands(allc, opt.band_gap)
    mouth_zone = None
    eye_zone = None
    if bs:
        low = bs[-1]
        centre = (low[0] + low[1]) / 2
        if centre >= fy0 + opt.mouth_min_pos * (fy1 - fy0):
            mouth_zone = (low[0], low[1])
            above = bs[:-1]
        else:
            above = bs
        if above:
            eye_zone = (above[0][0], above[-1][1])
    info["mouthZone"] = list(mouth_zone) if mouth_zone else None
    info["eyeZone"] = list(eye_zone) if eye_zone else None

    def zone_box(a, b, zone):
        if zone is None or a == b:
            return None
        cs = [c for c in comps_of[(a, b)] if c[1] < zone[1] and c[3] > zone[0]]
        return union_box(cs)

    # base: 00, else the fewest dark pixels in the mouth zone
    mouth_area = None
    if mouth_zone:
        xs = [c for c in allc if c[1] < mouth_zone[1] and c[3] > mouth_zone[0]]
        mouth_area = grow(union_box(xs), opt.pad, w, h) if xs else None
    darks = {n: dark_count(images[n], mouth_area, opt) for n in cl}
    zeros = [n for n in cl if _nn(n) == "00"]
    base = zeros[0] if zeros else min(cl, key=lambda n: (darks[n], n))
    info["base"] = base
    if mouth_zone is None:
        for n in cl:
            res["expressions"][n] = {"skipped": "no mouth difference in this pose (only the eyes change)", "base": base}
        return
    # mouth of each member against the base
    mouth = {}
    for n in cl:
        if n == base:
            continue
        box = zone_box(base, n, mouth_zone)
        if box is None:
            continue
        bw, bh = box[2] - box[0], box[3] - box[1]
        if bw > opt.mouth_max_w or bh > opt.mouth_max_h:
            mouth[n] = {"box": box, "tooBig": True}
            continue
        mouth[n] = {"box": box, "height": bh, "dark": darks[n], "open": bh >= opt.open_min_h}
    donors = sorted((n for n, m in mouth.items() if m.get("open") and not m.get("tooBig")),
                    key=lambda n: (-mouth[n]["height"], -mouth[n]["dark"], n))
    # drop donors whose mouth patch is the same picture as an earlier one
    uniq = []
    for n in donors:
        r = grow(mouth[n]["box"], opt.pad, w, h)
        patch = images[n].crop(*r).px
        if any(images[u].crop(*r).px == patch for u in uniq):
            continue
        uniq.append(n)
    donors = uniq
    info["donors"] = donors
    blink_src = None
    if blink_nn:
        blink_src = next((n for n in cl if _nn(n) == blink_nn), None)
    info["blinkSource"] = blink_src
    eye_rect = None
    if blink_src and blink_src != base and eye_zone:
        eb = zone_box(base, blink_src, eye_zone)
        if eb and eb[3] - eb[1] <= opt.eyes_max_h:
            eye_rect = grow(eb, opt.pad, w, h)
    for n in cl:
        e = {"base": base}
        res["expressions"][n] = e
        made = {}
        m = mouth.get(n)
        if m and m.get("tooBig"):
            e["skipped"] = f"the mouth difference is too large ({m['box'][2] - m['box'][0]}x{m['box'][3] - m['box'][1]} px; hand or pose)"
            continue
        rest = images[n]
        if m and m.get("open"):
            r = grow(m["box"], opt.pad, w, h)
            rest = paste(images[n], images[base], r, opt.feather)
            made["closed"] = rest
            e["closedRect"] = list(r)
        if not donors:
            e["skipped"] = "no open mouth in this pose to cut from"
            if made:
                res["made"][n] = made
            continue
        cands = []
        for d in donors:
            r = grow(mouth[d]["box"], opt.pad, w, h)
            cands.append((d, r, paste(rest, images[d], r, opt.feather)))
        e["donors"] = [{"from": d, "rect": list(r), "height": mouth[d]["height"], "dark": mouth[d]["dark"]}
                       for d, r, _ in cands]
        e["vowels"] = {}
        for v in VOWELS:
            d, r, img = pick_ranked(cands, v)
            made[v] = img
            e["vowels"][v] = d
        made["open"] = made["a"]
        if blink_nn:
            if eye_rect is not None:
                made["blink"] = paste(rest, images[blink_src], eye_rect, opt.feather)
                e["blinkRect"] = list(eye_rect)
            elif blink_src is None:
                e["blink"] = f"no picture {blink_nn} in this pose"
            else:
                e["blink"] = "no eye difference found for the blink source"
        else:
            e["blink"] = "none (no --blink-source)"
        res["made"][n] = made


# ---------------------------------------------------------------- review sheet

def _blend_over(dst: Image, dx: int, dy: int, src: Image, scale: int = 1) -> None:
    for y in range(src.h * scale):
        ty = dy + y
        if not 0 <= ty < dst.h:
            continue
        sy = y // scale
        for x in range(src.w * scale):
            tx = dx + x
            if not 0 <= tx < dst.w:
                continue
            i = (sy * src.w + x // scale) * 4
            a = src.px[i + 3]
            if a == 0:
                continue
            j = (ty * dst.w + tx) * 4
            for c in range(3):
                dst.px[j + c] = (src.px[i + c] * a + dst.px[j + c] * (255 - a) + 127) // 255
            dst.px[j + 3] = 255


def _rect_outline(img: Image, r, colour) -> None:
    x0, y0, x1, y1 = r
    for x in range(x0, x1):
        img.put(x, y0, colour)
        img.put(x, y1 - 1, colour)
    for y in range(y0, y1):
        img.put(x0, y, colour)
        img.put(x1 - 1, y, colour)


def _checker(w: int, h: int) -> Image:
    img = Image(w, h)
    for y in range(h):
        for x in range(w):
            v = 205 if ((x // 8) + (y // 8)) % 2 else 235
            i = (y * w + x) * 4
            img.px[i:i + 4] = bytes((v, v, v, 255))
    return img


def _diff_tile(a: Image, b: Image, opt: Options) -> Image:
    out = a.copy()
    for i in range(0, len(out.px), 4):
        if _differs(a.px, b.px, i, opt.t_rgb, opt.t_alpha):
            out.px[i:i + 4] = b"\xff\x00\x00\xff"
        else:
            out.px[i + 3] = out.px[i + 3] // 3
    return out


TILE_KINDS = ("orig", "closed", "a", "i", "u", "e", "o", "blink", "diff")


def review_sheet(groups: list, opt: Options, scale: int = 2):
    """groups: [(images, result)]; one row per picture. None when there is nothing to show."""
    rows = []
    for images, res in groups:
        if not res.get("face") or not images:
            continue
        fy0, fy1 = res["face"]
        any_img = next(iter(images.values()))
        w = any_img.w
        rects = []
        for e in res["expressions"].values():
            for d in e.get("donors", []):
                rects.append(d["rect"])
            for k in ("closedRect", "blinkRect"):
                if k in e:
                    rects.append(e[k])
        if rects:
            u = union_box(rects)
            cx = (u[0] + u[2]) // 2
        else:
            cx = w // 2
        half = max(40, min(w // 2, (fy1 - fy0) // 2 + 10))
        box = (max(0, cx - half), fy0, min(w, cx + half), fy1)
        for name in sorted(images):
            e = res["expressions"].get(name, {})
            made = res["made"].get(name, {})
            orig = images[name].copy()
            for r in ([e["closedRect"]] if "closedRect" in e else []) + [d["rect"] for d in e.get("donors", [])]:
                _rect_outline(orig, r, (255, 0, 0, 255))
            if "blinkRect" in e:
                _rect_outline(orig, e["blinkRect"], (0, 80, 255, 255))
            tiles = []
            for k in TILE_KINDS:
                if k == "orig":
                    tiles.append(orig.crop(*box))
                elif k == "diff":
                    tiles.append(_diff_tile(images[name], made["a"], opt).crop(*box) if "a" in made else None)
                else:
                    tiles.append(made[k].crop(*box) if k in made else None)
            rows.append((tiles, "skipped" in e and not made))
    if not rows:
        return None
    tw = max(t.w for r, _ in rows for t in r if t) * scale
    th = max(t.h for r, _ in rows for t in r if t) * scale
    gap = 4
    sheet = _checker(len(TILE_KINDS) * (tw + gap) + gap, len(rows) * (th + gap) + gap)
    for ri, (tiles, skipped) in enumerate(rows):
        y = gap + ri * (th + gap)
        for ci, t in enumerate(tiles):
            x = gap + ci * (tw + gap)
            if t is not None:
                _blend_over(sheet, x, y, t, scale)
        if skipped:
            _rect_outline(sheet, (gap - 2, y - 2, gap + tw + 2, y + th + 2), (255, 0, 0, 255))
    return sheet


# ---------------------------------------------------------------- files

def sprite_folder(root: str, code: str):
    base = os.path.join(root, SPRITE_DIR)
    try:
        entries = os.listdir(base)[:MAX_DIR_ENTRIES]
    except OSError:
        return None
    for e in sorted(entries):
        if e.lower() == code or e.lower().startswith(code + "_"):
            p = os.path.join(base, e)
            if os.path.isdir(p):
                return p
    return None


def sprite_files(folder: str, code: str) -> dict:
    """{outfit: [file names]} for `<code><NN><outfit>d.png`."""
    rx = re.compile(r"^" + re.escape(code) + r"(\d\d)([a-z])d\.png$", re.IGNORECASE)
    out = {}
    try:
        entries = os.listdir(folder)[:MAX_DIR_ENTRIES]
    except OSError:
        return out
    for e in sorted(entries):
        m = rx.match(e)
        if m:
            out.setdefault(m.group(2).lower(), []).append(e)
    return out


def run(root: str, codes: list, out_dir: str, opt: Options, blink: dict, overwrite=False, dry_run=False) -> dict:
    rep = {"root": root.replace("\\", "/"), "outDir": out_dir.replace("\\", "/"), "codes": {}, "warnings": []}
    if not os.path.isdir(root):
        rep["warnings"].append(f"the dump folder {root} does not exist")
        return rep
    for code in codes[:MAX_CODES]:
        c = {"groups": [], "written": [], "kept": [], "skipped": {}, "counts": {}}
        rep["codes"][code] = c
        if not CODE_RE.match(code):
            c["error"] = "a code is three lowercase letters"
            continue
        folder = sprite_folder(root, code)
        if folder is None:
            c["error"] = f"no sprite folder for {code} under {os.path.join(root, SPRITE_DIR)}"
            continue
        c["folder"] = folder.replace("\\", "/")
        dest = os.path.join(out_dir, code)
        review = []
        for outfit, files in sorted(sprite_files(folder, code).items()):
            if len(files) > MAX_GROUP_FILES:
                rep["warnings"].append(f"{code}..{outfit}: {len(files)} files, only the first {MAX_GROUP_FILES} used")
                files = files[:MAX_GROUP_FILES]
            images = {}
            size = None
            for f in files:
                name = os.path.splitext(f)[0].lower()
                try:
                    img = read_png(os.path.join(folder, f))
                except PngError as e:
                    rep["warnings"].append(f"{f}: skipped: {e}")
                    c["skipped"][name] = f"cannot read: {e}"
                    continue
                if size is None:
                    size = (img.w, img.h)
                if (img.w, img.h) != size:
                    c["skipped"][name] = f"canvas {img.w}x{img.h} differs from {size[0]}x{size[1]} of its outfit"
                    continue
                images[name] = img
            res = analyse(images, opt, blink.get(code))
            g = {"outfit": outfit, "size": list(size) if size else None, "face": res["face"],
                 "clusters": res["clusters"], "expressions": res["expressions"]}
            c["groups"].append(g)
            review.append((images, res))
            for name, e in res["expressions"].items():
                if "skipped" in e:
                    c["skipped"][name] = e["skipped"]
                files_out = {}
                for kind, img in sorted(res["made"].get(name, {}).items()):
                    p = os.path.join(dest, f"{name}_{kind}.png")
                    files_out[kind] = p.replace("\\", "/")
                    if os.path.exists(p) and not overwrite:
                        c["kept"].append(p.replace("\\", "/"))
                        continue
                    if not dry_run:
                        try:
                            write_png(p, img)
                        except OSError as err:
                            rep["warnings"].append(f"cannot write {p}: {err}")
                            continue
                    c["written"].append(p.replace("\\", "/"))
                if files_out:
                    e["files"] = files_out
        try:
            sheet = review_sheet(review, opt)
        except PngError as err:
            rep["warnings"].append(f"{code}: no review sheet: {err}")
            sheet = None
        if sheet is not None and not dry_run:
            p = os.path.join(out_dir, "_review", f"{code}.png")
            try:
                write_png(p, sheet)
                c["review"] = p.replace("\\", "/")
            except OSError as err:
                rep["warnings"].append(f"cannot write {p}: {err}")
        kinds = {}
        for g in c["groups"]:
            for e in g["expressions"].values():
                for k in e.get("files", {}):
                    kinds[k] = kinds.get(k, 0) + 1
        c["counts"] = {"written": len(c["written"]), "kept": len(c["kept"]), "skipped": len(c["skipped"]), "byKind": kinds}
    return rep


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("codes", nargs="+", help="sprite codes (aka koj ...)")
    ap.add_argument("--root", required=True, help="the dump folder (read only)")
    ap.add_argument("--out-dir", help="where the variants go, one subfolder per code (default <root>/../eve_sprites)")
    ap.add_argument("--blink-source", nargs=2, action="append", metavar=("CODE", "NN"), default=[],
                    help="a picture drawn with closed eyes; its eyes make the _blink of its pose")
    ap.add_argument("--overwrite", action="store_true", help="replace files that already exist")
    ap.add_argument("--dry-run", action="store_true", help="analyse and report, write no picture")
    ap.add_argument("--report", help="also write the JSON report here")
    ap.add_argument("--threshold", type=int, default=40, help="channel difference that counts as changed (1-255)")
    ap.add_argument("--face-frac", type=float, default=0.30)
    ap.add_argument("--pad", type=int, default=3)
    ap.add_argument("--feather", type=int, default=2)
    a = ap.parse_args(argv)
    if not 1 <= a.threshold <= 255 or not 0.05 <= a.face_frac <= 1.0 or not 0 <= a.pad <= 32 or not 0 <= a.feather <= 16:
        print("--threshold 1..255, --face-frac 0.05..1, --pad 0..32, --feather 0..16", file=sys.stderr)
        return 2
    opt = Options(t_rgb=a.threshold, t_alpha=a.threshold, face_frac=a.face_frac, pad=a.pad,
                  feather=min(a.feather, a.pad))
    blink = {}
    for code, nn in a.blink_source:
        if not CODE_RE.match(code.lower()) or not re.match(r"^\d\d$", nn):
            print(f"--blink-source wants a code and two digits, got {code} {nn}", file=sys.stderr)
            return 2
        blink[code.lower()] = nn
    codes = [c.lower() for c in a.codes]
    out_dir = a.out_dir or os.path.normpath(os.path.join(os.path.abspath(a.root), os.pardir, "eve_sprites"))
    rep = run(a.root, codes, out_dir, opt, blink, a.overwrite, a.dry_run)
    for w in rep["warnings"]:
        print(f"warning: {w}", file=sys.stderr)
    status = 0
    for code, c in rep["codes"].items():
        if "error" in c:
            print(f"{code}: {c['error']}", file=sys.stderr)
            status = 2
            continue
        print(f"{code}: written {c['counts']['written']}, kept {c['counts']['kept']}, "
              f"by kind {json.dumps(c['counts']['byKind'])}; review {c.get('review', '-')}", file=sys.stderr)
        for name, why in sorted(c["skipped"].items()):
            print(f"  {name}: {why}", file=sys.stderr)
        if code not in blink:
            print(f"  {code}: no blink (give --blink-source {code} NN for a picture with closed eyes)", file=sys.stderr)
    if a.report:
        try:
            os.makedirs(os.path.dirname(os.path.abspath(a.report)), exist_ok=True)
            with open(a.report, "w", encoding="utf-8", newline="\n") as f:
                json.dump(rep, f, ensure_ascii=False, indent=1)
        except OSError as e:
            print(f"cannot write the report: {e}", file=sys.stderr)
            return 2
    return status


if __name__ == "__main__":
    sys.exit(main())
