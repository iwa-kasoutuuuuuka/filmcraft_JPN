"""Give the extras of every episode script one fixed voice per role (personal study only).

The converter numbers unnamed speakers per script (`n001`, `m-n003`, ...), so the same 守衛 is a
different id in every episode. This tool keys them by their DISPLAY NAME instead, maps each name
to a voice class through a table the user can edit, designs one reference per voice with
Qwen3-TTS ("design, then clone") and writes the voices into copies of the scripts.

    python extras_voices.py --scripts "eve_out/series/eve.ep*.fcscript.json" --voices-dir eve_out/voices --list
    python extras_voices.py --scripts ... --voices-dir eve_out/voices --init      # writes extras_roles.json
    python extras_voices.py --scripts ... --voices-dir eve_out/voices --design    # GPU, one process, serial
    python extras_voices.py --scripts ... --voices-dir eve_out/voices --apply \
        --refs tools/eve_burst/voice_refs.json --root DATA --out-dir eve_out/voiced --tag voiced

Files (all outside the repository, under --voices-dir):
    extras_roles.json         the table: voices (design + refText + gender), classes (ordered voice
                              lists), names (display name -> class, optional pinned voice), aliases
                              (character id -> voice_refs id, e.g. pur -> pul), skipIds (never voiced)
    extras/extras_refs.json   designed references, {voice: {refAudio, refText, seconds, f0, design,
                              why, alternatives}} (the shape of voice_refs.json); existing entries
                              are reused, never redesigned unless --redo names them
    extras/cand/<voice>-<k>.wav   the candidates; extras/<voice>.wav the pick

--apply, per script (a character that already has a voice is never touched):
    1. characters whose id is in --refs (or an alias of one) get that clone (apply_voices.apply);
    2. `narrator` gets the clone of the scene's point of view (koj or mar); a script whose
       narration spans both gets `narrator-<pov>` for the minority, its lines moved to it;
    3. every other character (the extras) by display name -> class -> voice -> reference. Two
       extras that share a scene never share a voice: the one with fewer lines takes the next
       voice of its class (then any voice of the same gender), and the report says so.
Paths are written relative to the output script; other relative paths of the script are rebased
when the output folder differs from the input folder. Standard library only; F0 is measured with
`wave` + `array`.
"""

import argparse
import array
import glob
import json
import os
import re
import shutil
import subprocess
import sys
import unicodedata
import wave

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import apply_voices  # noqa: E402

MAX_SCRIPTS = 1000
MAX_FILE_BYTES = 256 * 1024 * 1024
MAX_NAMES = 5000
MAX_CANDIDATES = 9
MAX_WAV_SECONDS = 60.0
MAX_NAME_CHARS = 200
NARRATOR = "narrator"
PROTAGONISTS = ("koj", "mar")

# F0 windows a pick must fall in, by gender (median of the voiced frames, Hz)
F0_RANGE = {"male": (60.0, 160.0), "female": (165.0, 300.0), "child": (200.0, 450.0), "any": (90.0, 300.0)}
SECONDS_RANGE = (3.0, 15.0)

# Built-in voices. The first six were designed earlier (eve_out/voices/extras) and are reused
# as they are; the others are designed by --design. All sentences are original.
DEFAULT_VOICES = {
    "man-a": {"gender": "male", "design": "30代の日本人男性。低めで普通の声、感情を抑えた淡々とした話し方。",
              "refText": "ええ、その件なら聞いています。詳しいことは上の者に確認してから、改めてご連絡します。"},
    "man-b": {"gender": "male", "design": "50代の日本人男性。低くしわがれ気味の声、ゆっくりで重々しい話し方。",
              "refText": "昔のことだ。もう誰も覚えていないだろうが、あの夜のことは今でもはっきりと覚えている。"},
    "man-c": {"gender": "male", "design": "20代の日本人男性。低めだが若い声、やや早口で軽い話し方。",
              "refText": "あ、すみません、ちょっといいですか。さっきここに来た人、どっちに行ったか分かります？"},
    "woman-a": {"gender": "female", "design": "30代の日本人女性。落ち着いた中くらいの高さの声、丁寧でゆっくりした話し方。",
                "refText": "お待たせいたしました。こちらが本日の資料になります。ご不明な点があればお知らせください。"},
    "front": {"gender": "male", "design": "30代の日本人男性のホテルのフロント係。低めで柔らかい声、丁寧でゆっくり、落ち着いた話し方。",
              "refText": "いらっしゃいませ。ご予約のお名前を伺ってもよろしいでしょうか。お部屋の準備はできております。"},
    "police": {"gender": "male", "design": "50代の日本人男性の警官。低く太い声、威圧的だが落ち着いて、はっきりした話し方。",
               "refText": "そこの君、少し止まりなさい。この辺りで不審な人物を見なかったか。話を聞かせてもらう。"},
    "gatekeeper": {"gender": "male", "design": "60代の日本人男性の守衛。低くややしゃがれた声、のんびりとした落ち着いた話し方。",
                   "refText": "ああ、面会の方かね。そこの帳簿に名前を書いてから、中に入ってくれ。帰りにもう一度声をかけてな。"},
    "security": {"gender": "male", "design": "40代の日本人男性の警備員。低めで落ち着いた声、事務的できびきびした話し方。",
                 "refText": "こちらは警備室です。関係者以外の立ち入りはご遠慮いただいています。身分証をお願いします。"},
    "male-mid-2": {"gender": "male", "design": "40代の日本人男性。やや低くかすれ気味の声、ぶっきらぼうで気だるい話し方。",
                   "refText": "まあ、そういうことなら仕方ないな。こっちも仕事なんでね、用件は手短に頼むよ。"},
    "male-young-2": {"gender": "male", "design": "20代の日本人男性。明るく張りのある声、はきはきとした丁寧な話し方。",
                     "refText": "はい、わかりました。すぐに確認してきますので、こちらで少々お待ちください。"},
    "thug": {"gender": "male", "design": "30代の日本人男性のチンピラ。低くドスの利いた声、乱暴で威圧的な話し方。",
             "refText": "おい、てめえ、どこ見て歩いてんだ。ここは俺たちの縄張りなんだよ、わかってんのか。"},
    "female-young": {"gender": "female", "design": "20代前半の日本人女性。明るくやや高めの声、親しみやすく柔らかな話し方。",
                     "refText": "えっと、それなら向こうの建物だと思います。よかったら、途中までご案内しましょうか？"},
    "female-mid-2": {"gender": "female", "design": "40代の日本人女性。少し低めで落ち着いた声、きびきびとした話し方。",
                     "refText": "あら、こんな時間にどうしたの。用があるなら、まずは事務所の方に声をかけてちょうだい。"},
    "female-old": {"gender": "female", "design": "60代の日本人女性。やや低くかすれた声、気さくでゆっくりとした話し方。",
                   "refText": "あらまあ、ずいぶん遅かったじゃないの。外は寒かったでしょう、ほら、早くお上がりなさい。"},
    "child": {"gender": "child", "design": "10歳くらいの日本人の男の子。高く元気な声、無邪気な話し方。",
              "refText": "ねえねえ、それなあに？ぼくにもちょっとだけ見せてよ。ねえ、いいでしょう？"},
    "unknown": {"gender": "any", "design": "30代の日本人。中性的で落ち着いた声、感情を抑えた淡々とした話し方。",
                "refText": "もしもし、聞こえますか。申し訳ありませんが、こちらからはそれ以上のことはお話しできません。"},
}

# class -> gender and the voices in the order they are handed out (a clash takes the next)
DEFAULT_CLASSES = {
    "male-young": {"gender": "male", "voices": ["man-c", "male-young-2"]},
    "male-mid": {"gender": "male", "voices": ["man-a", "male-mid-2", "male-young-2"]},
    "male-old": {"gender": "male", "voices": ["man-b", "gatekeeper"]},
    "female-young": {"gender": "female", "voices": ["female-young", "woman-a"]},
    "female-mid": {"gender": "female", "voices": ["woman-a", "female-mid-2", "female-young"]},
    "female-old": {"gender": "female", "voices": ["female-old", "female-mid-2"]},
    "child": {"gender": "child", "voices": ["child"]},
    "gatekeeper": {"gender": "male", "voices": ["gatekeeper", "man-b"]},
    "security": {"gender": "male", "voices": ["security", "male-mid-2"]},
    "police": {"gender": "male", "voices": ["police", "man-b", "male-mid-2"]},
    "front": {"gender": "male", "voices": ["front", "male-young-2"]},
    "thug": {"gender": "male", "voices": ["thug", "man-c", "male-mid-2"]},
    "announcer": {"gender": "any", "voices": ["man-a"]},
    "unknown": {"gender": "any", "voices": ["unknown", "man-a", "woman-a"]},
}

# Generic role words, checked in order on the NFKC form of the name (first hit wins).
KEYWORD_RULES = [
    (("子供", "子ども", "こども", "男の子", "少年"), "child"),
    (("おばちゃん", "おばあ", "婆", "老婆"), "female-old"),
    (("女の子", "少女", "娘", "ウエイトレス", "ウェイトレス", "若い女", "生徒", "女子", "女学生"), "female-young"),
    (("女", "婦", "嬢", "おばさん", "ママ", "母", "受付"), "female-mid"),
    (("守衛",), "gatekeeper"),
    (("警備員", "ガードマン"), "security"),
    (("警官", "警察", "巡査", "刑事"), "police"),
    (("フロント",), "front"),
    (("野郎", "チンピラ", "ヤクザ", "不良"), "thug"),
    (("親父", "おやじ", "老人", "じい", "爺", "老", "社長", "会長"), "male-old"),
    (("若い", "ボーイ", "学生", "青年", "兄ちゃん"), "male-young"),
    (("TV", "テレビ", "放送", "ラジオ", "アナウンス"), "announcer"),
    (("男", "員", "人", "マン", "手", "パイロット", "主人", "客", "係", "店", "医者", "先生", "記者"), "male-mid"),
]

_MARK_RE = re.compile(r"(?:[0-9]+|[A-Ja-j])$")


def nfkc(s) -> str:
    return unicodedata.normalize("NFKC", str(s or "")).strip()


def guess_class(name: str) -> tuple[str, str]:
    """(class, reason) for a display name, from the keyword rules; 'unknown' if none fits."""
    n = nfkc(name)
    for words, cls in KEYWORD_RULES:
        for w in words:
            if w in n:
                return cls, "keyword " + w
    return "unknown", "no keyword"


def variant_of(name: str) -> int:
    """男１ -> 0, 男２ -> 1, 声Ｂ -> 1, 左の女 -> 0, 右の女 -> 2: a hint for which voice of the class."""
    n = nfkc(name)
    m = _MARK_RE.search(n)
    if m:
        t = m.group(0)
        if t.isdigit():
            return max(0, min(int(t[-3:]), 999) - 1)
        return max(0, ord(t.upper()) - ord("A"))
    for k, v in (("左", 0), ("中央", 1), ("右", 2)):
        if k in n:
            return v
    return 0


# ---- reading -----------------------------------------------------------------------------------

def read_json(path: str, limit: int = MAX_FILE_BYTES):
    """The parsed file, or raises OSError/ValueError (too big, unreadable, not JSON)."""
    if os.path.getsize(path) > limit:
        raise ValueError(f"{path}: larger than {limit} bytes")
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def write_json(path: str, data) -> None:
    d = os.path.dirname(os.path.abspath(path))
    os.makedirs(d, exist_ok=True)
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8", newline="\n") as f:
        json.dump(data, f, ensure_ascii=False, indent=1)
    os.replace(tmp, path)


def expand_scripts(patterns) -> list[str]:
    out, seen = [], set()
    for p in patterns or []:
        hits = sorted(glob.glob(p)) if any(c in p for c in "*?[") else ([p] if os.path.isfile(p) else [])
        for h in hits:
            k = os.path.normcase(os.path.abspath(h))
            if k not in seen and os.path.isfile(h):
                seen.add(k)
                out.append(h)
            if len(out) >= MAX_SCRIPTS:
                return out
    return out


def scenes_of(script) -> list[dict]:
    sc = script.get("scenes") if isinstance(script, dict) else None
    return [s for s in sc if isinstance(s, dict)] if isinstance(sc, list) else []


def lines_of(scene: dict) -> list[dict]:
    ln = scene.get("lines")
    return [x for x in ln if isinstance(x, dict)] if isinstance(ln, list) else []


def chars_of(script) -> dict:
    c = script.get("characters") if isinstance(script, dict) else None
    return c if isinstance(c, dict) else {}


def needs_voice(line: dict) -> bool:
    """A spoken line with no recording of its own: it will be synthesized in the speaker's voice."""
    if line.get("kind") == "direction" or line.get("voiceFile"):
        return False
    t = line.get("text")
    return isinstance(t, str) and bool(t.strip())


def char_usage(script) -> dict:
    """{id: {"lines", "unvoiced", "scenes": set}}"""
    out: dict = {}
    for sc in scenes_of(script):
        sid = str(sc.get("id") or "")
        for ln in lines_of(sc):
            who = ln.get("speaker")
            if not isinstance(who, str) or not who:
                continue
            u = out.setdefault(who, {"lines": 0, "unvoiced": 0, "scenes": set()})
            u["lines"] += 1
            u["unvoiced"] += 1 if needs_voice(ln) else 0
            u["scenes"].add(sid)
    return out


def display_name(cid: str, c) -> str:
    n = c.get("name") if isinstance(c, dict) else None
    n = n if isinstance(n, str) and n.strip() else cid
    return n.strip()[:MAX_NAME_CHARS]


# ---- the role table ----------------------------------------------------------------------------

def default_table() -> dict:
    return {
        "version": 1,
        "note": "Edit freely: names.<display name>.class picks a class, .voice pins one voice. "
                "classes.<class>.voices are handed out in order. Re-run --init to add new names (edits are kept).",
        "voices": json.loads(json.dumps(DEFAULT_VOICES, ensure_ascii=False)),
        "classes": json.loads(json.dumps(DEFAULT_CLASSES, ensure_ascii=False)),
        "names": {},
        "aliases": {"pur": "pul"},
        "skipIds": ["sfx"],
    }


def normalize_table(t) -> dict:
    """A usable table from whatever was read (missing parts filled in with the defaults)."""
    base = default_table()
    if not isinstance(t, dict):
        return base
    for k in ("voices", "classes", "names", "aliases"):
        v = t.get(k)
        if isinstance(v, dict):
            base[k] = {str(a): b for a, b in list(v.items())[:MAX_NAMES]}
    if isinstance(t.get("skipIds"), list):
        base["skipIds"] = [str(x) for x in t["skipIds"][:MAX_NAMES]]
    if isinstance(t.get("note"), str):
        base["note"] = t["note"]
    return base


def class_voices(table: dict, cls: str) -> list[str]:
    c = table["classes"].get(cls)
    v = c.get("voices") if isinstance(c, dict) else None
    return [str(x) for x in v if isinstance(x, str)] if isinstance(v, list) else []


def class_gender(table: dict, cls: str) -> str:
    c = table["classes"].get(cls)
    g = c.get("gender") if isinstance(c, dict) else None
    return g if g in F0_RANGE else "any"


def voice_gender(table: dict, vid: str) -> str:
    v = table["voices"].get(vid)
    g = v.get("gender") if isinstance(v, dict) else None
    if g in F0_RANGE:
        return g
    d = str(v.get("design") or "") if isinstance(v, dict) else ""
    return "female" if "女性" in d else ("male" if "男性" in d else "any")


def role_of(table: dict, name: str) -> dict:
    """{"class", "voice" (pinned or None), "variant", "listed"} for a display name."""
    e = table["names"].get(name)
    if not isinstance(e, dict):
        key = nfkc(name)
        e = next((v for k, v in table["names"].items() if nfkc(k) == key and isinstance(v, dict)), None)
    listed = isinstance(e, dict)
    e = e if listed else {}
    cls = e.get("class") if isinstance(e.get("class"), str) else None
    if not cls or cls not in table["classes"]:
        cls = guess_class(name)[0] if not cls else "unknown"
    pin = e.get("voice") if isinstance(e.get("voice"), str) and e.get("voice") else None
    var = e.get("variant")
    var = var if isinstance(var, int) and 0 <= var < 1000 else variant_of(name)
    return {"class": cls, "voice": pin, "variant": var, "listed": listed}


# ---- collecting ------------------------------------------------------------------------------

def load_all(paths: list[str]) -> tuple[list[tuple[str, dict]], list[str]]:
    good, bad = [], []
    for p in paths:
        try:
            s = read_json(p)
        except (OSError, ValueError) as e:
            bad.append(f"{p}: {e}")
            continue
        if isinstance(s, dict):
            good.append((p, s))
        else:
            bad.append(f"{p}: not a script object")
    return good, bad


def how_voiced(cid: str, refs: dict, table: dict) -> str:
    if cid == NARRATOR or cid.startswith(NARRATOR + "-"):
        return "narrator"
    if cid in table["skipIds"]:
        return "skip"
    if cid in refs:
        return "ref:" + cid
    a = table["aliases"].get(cid)
    if isinstance(a, str) and a in refs:
        return "ref:" + a
    return "role"


def collect(scripts: list[tuple[str, dict]], refs: dict, table: dict) -> dict:
    """{display name: {lines, unvoiced, scripts, ids, how}} for characters without a voice."""
    out: dict = {}
    for path, s in scripts:
        use = char_usage(s)
        for cid, c in chars_of(s).items():
            if not isinstance(c, dict) or c.get("voice"):
                continue
            how = how_voiced(cid, refs, table)
            name = display_name(cid, c) if how == "role" else ("(" + how + ") " + display_name(cid, c))
            u = use.get(cid, {"lines": 0, "unvoiced": 0})
            e = out.setdefault(name, {"lines": 0, "unvoiced": 0, "scripts": 0, "ids": [], "how": how})
            e["lines"] += u["lines"]
            e["unvoiced"] += u["unvoiced"]
            e["scripts"] += 1
            if cid not in e["ids"] and len(e["ids"]) < 8:
                e["ids"].append(cid)
            if len(out) >= MAX_NAMES:
                return out
    return out


def init_table(existing, names: dict) -> tuple[dict, list[str]]:
    """The table with every role name of `names` present (user edits kept); returns (table, added)."""
    t = normalize_table(existing)
    added = []
    for name, e in sorted(names.items(), key=lambda kv: (-kv[1]["lines"], kv[0])):
        if e.get("how") != "role":
            continue
        cur = t["names"].get(name)
        if isinstance(cur, dict):
            cur["lines"] = e["lines"]
            continue
        cls, why = guess_class(name)
        t["names"][name] = {"class": cls, "lines": e["lines"], "guess": why}
        if variant_of(name):
            t["names"][name]["variant"] = variant_of(name)
        added.append(name)
    return t, added


# ---- pitch ---------------------------------------------------------------------------------------

def read_wav_mono(path: str, max_seconds: float = MAX_WAV_SECONDS) -> tuple[int, list[float]]:
    """(rate, samples in -1..1) of a 16- or 32-bit PCM WAV, channels averaged; raises ValueError."""
    try:
        with wave.open(path, "rb") as w:
            nch, sw, sr, n = w.getnchannels(), w.getsampwidth(), w.getframerate(), w.getnframes()
            if sr < 4000 or sr > 384000 or nch < 1 or nch > 8 or sw not in (2, 4):
                raise ValueError(f"{path}: unsupported WAV ({nch} ch, {sw * 8} bit, {sr} Hz)")
            n = min(n, int(sr * max_seconds))
            raw = w.readframes(n)
    except (wave.Error, EOFError) as e:
        raise ValueError(f"{path}: {e}") from e
    a = array.array("h" if sw == 2 else "i")
    raw = raw[: len(raw) - len(raw) % (a.itemsize * nch)]
    a.frombytes(raw)
    if sys.byteorder == "big":
        a.byteswap()
    scale = 32768.0 if sw == 2 else 2147483648.0
    if nch == 1:
        return sr, [v / scale for v in a]
    return sr, [sum(a[i:i + nch]) / (nch * scale) for i in range(0, len(a) - nch + 1, nch)]


def f0_stats(path: str) -> dict:
    """{"seconds", "f0" (median Hz), "p10", "p90", "voiced" (frames)} by autocorrelation of 40 ms
    frames every 20 ms, after averaging the signal down to about 8 kHz. 0 Hz when nothing is voiced."""
    sr, x = read_wav_mono(path)
    seconds = len(x) / sr if sr else 0.0
    f = max(1, sr // 8000)
    if f > 1:
        x = [sum(x[i:i + f]) / f for i in range(0, len(x) - f + 1, f)]
    r = sr / f
    frame, hop = max(8, int(r * 0.04)), max(1, int(r * 0.02))
    lo, hi = max(2, int(r / 500)), min(frame - 1, int(r / 60))
    f0s = []
    mul = float.__mul__
    for i in range(0, len(x) - frame, hop):
        s = x[i:i + frame]
        m = sum(s) / frame
        s = [v - m for v in s]
        e = sum(map(mul, s, s))
        if (e / frame) ** 0.5 < 0.02 or e <= 0:
            continue
        best_k, best = 0, -1.0
        for k in range(lo, hi + 1):
            c = sum(map(mul, s, s[k:])) / e
            if c > best:
                best_k, best = k, c
        if best > 0.5 and best_k:
            f0s.append(r / best_k)
    if not f0s:
        return {"seconds": round(seconds, 2), "f0": 0.0, "p10": 0.0, "p90": 0.0, "voiced": 0}
    f0s.sort()

    def pct(p):
        return f0s[min(len(f0s) - 1, max(0, int(round(p / 100 * (len(f0s) - 1)))))]
    return {"seconds": round(seconds, 2), "f0": pct(50), "p10": pct(10), "p90": pct(90), "voiced": len(f0s)}


def pick_candidate(cands: list[dict], gender: str) -> dict | None:
    """Fitting first (F0 in the gender's window, 3-15 s), then the steadiest pitch, then the longest."""
    lo, hi = F0_RANGE.get(gender, F0_RANGE["any"])
    for c in cands:
        c["fits"] = lo <= c["f0"] <= hi and SECONDS_RANGE[0] <= c["seconds"] <= SECONDS_RANGE[1]
        c["spread"] = c["p90"] - c["p10"]
    if not cands:
        return None
    cands.sort(key=lambda c: (not c["fits"], c["spread"], -c["seconds"], c["file"]))
    return cands[0]


# ---- designing -------------------------------------------------------------------------------------

def used_voices(table: dict, names: dict) -> list[str]:
    """Voices the role names can be given (pinned ones and their classes' lists), in table order."""
    want = set()
    for name, e in names.items():
        if e.get("how") != "role":
            continue
        r = role_of(table, name)
        if r["voice"]:
            want.add(r["voice"])
        want.update(class_voices(table, r["class"]))
    return [v for v in table["voices"] if v in want] + sorted(v for v in want if v not in table["voices"])


def design_jobs(table: dict, voices: list[str], cand_dir: str, n: int, redo: set) -> tuple[list[dict], list[str]]:
    """Jobs for filmcraft_tts.py --jobs (candidates already on disk are kept unless redone)."""
    jobs, problems = [], []
    n = max(1, min(int(n), MAX_CANDIDATES))
    for vid in voices:
        v = table["voices"].get(vid)
        if not isinstance(v, dict) or not isinstance(v.get("design"), str) or not isinstance(v.get("refText"), str):
            problems.append(f"voice {vid}: needs `design` and `refText` in the table")
            continue
        for k in range(1, n + 1):
            out = os.path.join(cand_dir, f"{vid}-{k}.wav").replace("\\", "/")
            if os.path.isfile(out) and vid not in redo:
                continue
            jobs.append({"id": f"{vid}-{k}", "text": v["refText"], "out": out, "design": v["design"]})
    return jobs, problems


def choose(table: dict, vid: str, cand_dir: str, out_dir: str) -> tuple[dict | None, str]:
    """Measure the candidates of one voice, copy the pick to <out_dir>/<vid>.wav; (entry, message)."""
    v = table["voices"].get(vid) or {}
    gender = voice_gender(table, vid)
    cands = []
    for k in range(1, MAX_CANDIDATES + 1):
        p = os.path.join(cand_dir, f"{vid}-{k}.wav")
        if not os.path.isfile(p):
            continue
        try:
            st = f0_stats(p)
        except (OSError, ValueError) as e:
            print(f"  {vid}: {e}", file=sys.stderr)
            continue
        st["file"] = p.replace("\\", "/")
        cands.append(st)
    best = pick_candidate(cands, gender)
    if best is None:
        return None, f"{vid}: no candidates"
    dst = os.path.join(out_dir, f"{vid}.wav")
    shutil.copyfile(best["file"], dst)
    entry = {
        "refAudio": os.path.abspath(dst).replace("\\", "/"), "refText": v.get("refText", ""),
        "seconds": best["seconds"], "f0": round(best["f0"]), "design": v.get("design", ""), "gender": gender,
        "why": ("fits" if best["fits"] else "NO candidate in range") + f", F0 {round(best['f0'])} Hz, 10-90 % [{round(best['p10'])}, {round(best['p90'])}]",
        "alternatives": [{"refAudio": c["file"], "f0": round(c["f0"]), "seconds": c["seconds"], "fits": c["fits"]} for c in cands if c is not best],
    }
    others = ", ".join(f"{os.path.basename(c['file'])} {round(c['f0'])}" for c in cands if c is not best)
    return entry, f"{vid:13s} -> {os.path.basename(best['file'])}  F0 {round(best['f0']):4d} Hz  {'ok' if best['fits'] else 'OUT OF RANGE'}   others: {others}"


def default_tts_python() -> str:
    env = os.environ.get("FILMCRAFT_TTS_PYTHON")
    if env:
        return env
    venv = os.path.join(HERE, "..", "qwen3-tts", ".venv", "Scripts" if os.name == "nt" else "bin", "python.exe" if os.name == "nt" else "python")
    return os.path.abspath(venv) if os.path.isfile(venv) else sys.executable


def gpu_busy() -> int | None:
    """GPU utilization in % from nvidia-smi, or None when it cannot be asked."""
    try:
        r = subprocess.run(["nvidia-smi", "--query-gpu=utilization.gpu", "--format=csv,noheader,nounits"],
                           capture_output=True, text=True, timeout=20)
        return int(r.stdout.strip().splitlines()[0])
    except (OSError, ValueError, IndexError, subprocess.SubprocessError):
        return None


def run_tts(jobs_path: str, python: str, script: str, device: str, log_dir: str) -> int:
    """One synthesizer process for all jobs (the GPU is used serially). Output goes to design.out/err."""
    cmd = [python, script, "--jobs", jobs_path]
    if device:
        cmd += ["--device", device]
    with open(os.path.join(log_dir, "design.out"), "w", encoding="utf-8") as out, \
            open(os.path.join(log_dir, "design.err"), "w", encoding="utf-8") as err:
        try:
            p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=err, text=True, encoding="utf-8", errors="replace")
        except OSError as e:
            print(f"cannot start {python}: {e}", file=sys.stderr)
            return 2
        for line in p.stdout or []:
            out.write(line)
            out.flush()
            try:
                o = json.loads(line)
            except ValueError:
                continue
            if isinstance(o, dict) and o.get("id") is not None and ("ok" in o or "error" in o):
                print(f"  {o.get('id')}: {'ok ' + str(o.get('seconds')) + ' s' if o.get('ok') else o.get('error')}", flush=True)
        return p.wait()


# ---- applying ---------------------------------------------------------------------------------------

PATH_KEYS = {"background", "bgm", "voiceFile", "sfx", "video", "subCg", "image", "openImage", "blink", "refAudio"}


def _rebase_one(p, src: str, dst: str):
    if not isinstance(p, str) or not p or os.path.isabs(p) or re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", p):
        return p
    full = os.path.normpath(os.path.join(src, p))
    try:
        return os.path.relpath(full, dst).replace("\\", "/")
    except ValueError:  # another drive
        return full.replace("\\", "/")


def rebase_paths(node, src: str, dst: str, depth: int = 0):
    """Rewrite the script's relative file paths (known keys only) from folder `src` to folder `dst`."""
    if depth > 12 or os.path.normcase(os.path.abspath(src)) == os.path.normcase(os.path.abspath(dst)):
        return
    if isinstance(node, dict):
        for k, v in list(node.items()):
            if k in PATH_KEYS and isinstance(v, str):
                node[k] = _rebase_one(v, src, dst)
            elif k == "vowelImages" and isinstance(v, dict):
                for vk, vv in list(v.items()):
                    v[vk] = _rebase_one(vv, src, dst)
            elif k == "vowelImages" and isinstance(v, list):
                node[k] = [_rebase_one(x, src, dst) for x in v]
            elif isinstance(v, (dict, list)):
                rebase_paths(v, src, dst, depth + 1)
    elif isinstance(node, list):
        for v in node:
            rebase_paths(v, src, dst, depth + 1)


def rel_to(path: str, out_dir: str) -> str:
    full = os.path.abspath(path)
    try:
        return os.path.relpath(full, out_dir).replace("\\", "/")
    except ValueError:
        return full.replace("\\", "/")


def route_scenes(script) -> list[dict]:
    """Scenes in route order (the scenes not in the route follow in file order)."""
    sc = scenes_of(script)
    route = script.get("route") if isinstance(script, dict) else None
    if not isinstance(route, list):
        return sc
    by_id = {str(s.get("id")): s for s in sc}
    out, seen = [], set()
    for r in route[:100000]:
        s = by_id.get(str(r))
        if s is not None and id(s) not in seen:
            seen.add(id(s))
            out.append(s)
    return out + [s for s in sc if id(s) not in seen]


def scene_povs(script) -> dict:
    """{id(scene): pov}, a scene without a pov taking the one before it (else after it) in the route."""
    order = route_scenes(script)
    povs = [s.get("pov") if isinstance(s.get("pov"), str) and s.get("pov") else None for s in order]
    last = None
    filled = []
    for p in povs:
        last = p or last
        filled.append(last)
    nxt = None
    for i in range(len(filled) - 1, -1, -1):
        nxt = povs[i] or nxt
        if filled[i] is None:
            filled[i] = nxt
    return {id(s): p for s, p in zip(order, filled)}


def apply_narrator(script: dict, refs: dict, root: str, out_dir: str, rep: dict) -> None:
    chars = chars_of(script)
    nar = chars.get(NARRATOR)
    if not isinstance(nar, dict) or nar.get("voice"):
        return
    povs = scene_povs(script)
    count: dict = {}
    for sc in scenes_of(script):
        n = sum(1 for ln in lines_of(sc) if ln.get("speaker") == NARRATOR)
        if n:
            p = povs.get(id(sc))
            count[p] = count.get(p, 0) + n
    if not count:
        return

    def voice_for(pov):
        r = refs.get(pov) if isinstance(pov, str) else None
        if not isinstance(r, dict) or not r.get("refAudio") or not r.get("refText"):
            return None
        full = r["refAudio"] if os.path.isabs(r["refAudio"]) else os.path.join(root, r["refAudio"])
        return {"refAudio": rel_to(full, out_dir), "refText": r["refText"]} if os.path.isfile(full) else None

    known = sorted((p for p in count if voice_for(p)), key=lambda p: (-count[p], str(p)))
    if not known:
        rep["voiceless"].append(NARRATOR)
        return
    main = known[0]
    nar["voice"] = voice_for(main)
    rep["narrator"][NARRATOR] = main
    for pov in known[1:]:
        nid = f"{NARRATOR}-{pov}"
        if nid in chars:
            continue
        chars[nid] = {"name": nar.get("name") or "語り", "voice": voice_for(pov)}
        moved = 0
        for sc in scenes_of(script):
            if povs.get(id(sc)) != pov:
                continue
            for ln in lines_of(sc):
                if ln.get("speaker") == NARRATOR:
                    ln["speaker"] = nid
                    moved += 1
        rep["narrator"][nid] = pov
        rep["narratorMoved"][nid] = moved


def apply_extras(script: dict, table: dict, xrefs: dict, out_dir: str, rep: dict, priority: dict | None = None) -> None:
    """`priority` (display name -> lines in all scripts) decides who keeps the first voice in a clash,
    so a name loses its voice only to a name that is bigger over the whole series."""
    priority = priority if isinstance(priority, dict) else {}
    chars = chars_of(script)
    use = char_usage(script)
    todo = []
    for cid, c in chars.items():
        if not isinstance(c, dict) or c.get("voice"):
            continue
        if cid == NARRATOR or cid.startswith(NARRATOR + "-"):
            continue
        u = use.get(cid, {"lines": 0, "unvoiced": 0, "scenes": set()})
        if cid in table["skipIds"]:
            if u["unvoiced"]:
                rep["voiceless"].append(cid)
            continue
        name = display_name(cid, c)
        r = role_of(table, name)
        if not r["listed"]:
            rep["unlisted"].append(name)
        big = priority.get(name)
        big = big if isinstance(big, int) else 0
        todo.append((0 if r["voice"] else 1, -big, -u["unvoiced"], -u["lines"], name, cid, r, u))
    todo.sort(key=lambda t: t[:6])
    taken: dict = {}  # scene id -> set of voice ids held by extras there

    def ok(vid):
        e = xrefs.get(vid)
        return isinstance(e, dict) and e.get("refAudio") and e.get("refText") and os.path.isfile(e["refAudio"])

    for _pin, _big, _u, _l, name, cid, r, u in todo:
        scenes = u["scenes"]
        if r["voice"]:
            cands = [r["voice"]]
        else:
            lst = class_voices(table, r["class"])
            k = r["variant"] % len(lst) if lst else 0
            cands = lst[k:] + lst[:k]
            g = class_gender(table, r["class"])
            for cls in table["classes"]:
                if class_gender(table, cls) == g:
                    cands += [v for v in class_voices(table, cls) if v not in cands]
        usable = [v for v in cands if ok(v)]
        if not usable:
            rep["missingRef"].append(f"{name} ({', '.join(cands) or 'no voices'})")
            if u["unvoiced"]:
                rep["voiceless"].append(cid)
            continue
        free = next((v for v in usable if not any(v in taken.get(s, ()) for s in scenes)), None)
        vid = free or usable[0]
        if free is None and scenes:
            rep["clash"].append(f"{name}={vid}")
        elif vid != usable[0]:
            rep["moved"].append(f"{name}: {usable[0]} -> {vid}")
        for s in scenes:
            taken.setdefault(s, set()).add(vid)
        e = xrefs[vid]
        c = chars[cid]
        c["voice"] = {"refAudio": rel_to(e["refAudio"], out_dir), "refText": e["refText"]}
        rep["extras"][name] = vid


def apply_script(script: dict, refs: dict, root: str, table: dict, xrefs: dict, src_dir: str, out_dir: str,
                 priority: dict | None = None) -> dict:
    """Voices every character it can; changes `script` in place and returns the report."""
    rep = {"cloned": [], "narrator": {}, "narratorMoved": {}, "extras": {}, "moved": [], "clash": [],
           "unlisted": [], "missingRef": [], "kept": [], "voiceless": [], "remaining": {}}
    if not isinstance(script, dict) or not isinstance(script.get("characters"), dict):
        return rep
    rebase_paths(script, src_dir, out_dir)
    named = {k: v for k, v in refs.items() if isinstance(v, dict)} if isinstance(refs, dict) else {}
    for a, b in table["aliases"].items():
        if isinstance(b, str) and b in named and a not in named:
            named[a] = named[b]
    sub = apply_voices.apply(script, named, root, out_dir, fallback=None)
    rep["cloned"] = sub["cloned"]
    rep["kept"] = sub["kept"]
    rep["missingRef"] += [f"{c} (voice_refs file missing)" for c in sub["missingFile"]]
    apply_narrator(script, named, root, out_dir, rep)
    apply_extras(script, table, xrefs, out_dir, rep, priority)
    use = char_usage(script)
    for cid, c in chars_of(script).items():
        if isinstance(c, dict) and not c.get("voice") and use.get(cid, {}).get("unvoiced"):
            rep["remaining"][display_name(cid, c) + f" [{cid}]"] = use[cid]["unvoiced"]
    rep["unlisted"] = sorted(set(rep["unlisted"]))
    rep["voiceless"] = sorted(set(rep["voiceless"]))
    return rep


def out_path_for(src: str, out_dir: str | None, tag: str | None) -> str:
    base = os.path.basename(src)
    if tag:
        m = re.match(r"^(.*?)(\.fcscript\.json|\.json)$", base, re.I)
        base = f"{m.group(1)}.{tag}{m.group(2)}" if m else f"{base}.{tag}"
    return os.path.join(out_dir or os.path.dirname(src), base)


# ---- command line ---------------------------------------------------------------------------------------

def print_list(names: dict, table: dict, xrefs: dict) -> None:
    print("name\tlines\tunvoiced\tscripts\thow\tclass\tvoice\tids")
    for name, e in sorted(names.items(), key=lambda kv: (-kv[1]["unvoiced"], -kv[1]["lines"], kv[0])):
        cls = voice = ""
        if e["how"] == "role":
            r = role_of(table, name)
            cls = r["class"] + ("" if r["listed"] else " (guess)")
            lst = class_voices(table, r["class"])
            voice = r["voice"] or (lst[r["variant"] % len(lst)] if lst else "")
            voice += "" if voice in xrefs else " (not designed)"
        print(f"{name}\t{e['lines']}\t{e['unvoiced']}\t{e['scripts']}\t{e['how']}\t{cls}\t{voice}\t{','.join(e['ids'])}")


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--scripts", action="append", default=[], help="script file or glob (repeatable)")
    ap.add_argument("--voices-dir", help="folder of extras_roles.json and extras/ (outside the repository)")
    ap.add_argument("--roles", help="role table (default <voices-dir>/extras_roles.json)")
    ap.add_argument("--extras-refs", help="designed references (default <voices-dir>/extras/extras_refs.json)")
    ap.add_argument("--refs", default=os.path.join(HERE, "voice_refs.json"), help="voice_refs.json of the named characters")
    ap.add_argument("--root", help="data root the --refs paths are relative to")
    ap.add_argument("--list", action="store_true", help="print the voiceless characters by display name")
    ap.add_argument("--json", action="store_true", help="--list as JSON")
    ap.add_argument("--init", action="store_true", help="create the role table, or add new names to it")
    ap.add_argument("--design", action="store_true", help="design the missing voices (GPU) and pick by F0")
    ap.add_argument("--dry-run", action="store_true", help="--design: write the jobs file only")
    ap.add_argument("--voices", help="--design: only these voice ids (comma separated)")
    ap.add_argument("--redo", default="", help="--design: redesign these voice ids (comma separated)")
    ap.add_argument("--candidates", type=int, default=3)
    ap.add_argument("--tts-python", default=None)
    ap.add_argument("--tts-script", default=os.path.abspath(os.path.join(HERE, "..", "qwen3-tts", "filmcraft_tts.py")))
    ap.add_argument("--device", default="cuda:0")
    ap.add_argument("--apply", action="store_true", help="write voices into the scripts")
    ap.add_argument("--out", help="--apply with one script: write here")
    ap.add_argument("--out-dir", help="--apply: write the scripts into this folder")
    ap.add_argument("--tag", help="--apply: insert .<tag> before .fcscript.json in the output name")
    ap.add_argument("--in-place", action="store_true", help="--apply: overwrite the inputs")
    ap.add_argument("--report", help="--apply: write the full report here (JSON)")
    ap.add_argument("--rank-scripts", action="append", default=[],
                    help="--apply: more scripts (file or glob) whose lines count when ranking names for a clash, not written")
    a = ap.parse_args(argv)

    if not (a.list or a.init or a.design or a.apply):
        ap.error("pass --list, --init, --design or --apply")
    if (a.init or a.design or a.apply) and not (a.voices_dir or (a.roles and a.extras_refs)):
        ap.error("--voices-dir (or --roles and --extras-refs) is needed")
    vdir = a.voices_dir or os.path.dirname(os.path.abspath(a.roles or "."))
    roles_path = a.roles or os.path.join(vdir, "extras_roles.json")
    xrefs_path = a.extras_refs or os.path.join(vdir, "extras", "extras_refs.json")

    paths = expand_scripts(a.scripts)
    scripts, bad = load_all(paths)
    for b in bad:
        print("skipped " + b, file=sys.stderr)
    try:
        refs = read_json(a.refs, 16 * 1024 * 1024) if a.refs and os.path.isfile(a.refs) else {}
    except (OSError, ValueError) as e:
        print(f"cannot read --refs: {e}", file=sys.stderr)
        return 2
    refs = refs if isinstance(refs, dict) else {}
    try:
        table_raw = read_json(roles_path, 16 * 1024 * 1024) if os.path.isfile(roles_path) else None
        xrefs = read_json(xrefs_path, 16 * 1024 * 1024) if os.path.isfile(xrefs_path) else {}
    except (OSError, ValueError) as e:
        print(f"cannot read the role table or extras refs: {e}", file=sys.stderr)
        return 2
    xrefs = xrefs if isinstance(xrefs, dict) else {}
    table = normalize_table(table_raw)
    names = collect(scripts, refs, table)

    if a.init:
        table, added = init_table(table_raw, names)
        write_json(roles_path, table)
        print(f"{roles_path}: {len(table['names'])} names ({len(added)} added)")
    if a.list:
        if a.json:
            print(json.dumps(names, ensure_ascii=False, indent=1))
        else:
            print_list(names, table, xrefs)
    if a.design:
        rc = do_design(a, table, names, xrefs, xrefs_path)
        if rc:
            return rc
        try:
            xrefs = read_json(xrefs_path) if os.path.isfile(xrefs_path) else {}
        except (OSError, ValueError):
            xrefs = {}
    if a.apply:
        ranked = names
        if a.rank_scripts:
            more, _ = load_all([p for p in expand_scripts(a.rank_scripts) if p not in paths])
            ranked = collect(scripts + more, refs, table)
        return do_apply(a, scripts, refs, table, xrefs, {k: v["lines"] for k, v in ranked.items() if v.get("how") == "role"})
    return 0


def do_design(a, table: dict, names: dict, xrefs: dict, xrefs_path: str) -> int:
    xdir = os.path.dirname(os.path.abspath(xrefs_path))
    cand = os.path.join(xdir, "cand")
    os.makedirs(cand, exist_ok=True)
    redo = {v for v in a.redo.split(",") if v}
    want = [v for v in a.voices.split(",") if v] if a.voices else used_voices(table, names)
    todo = []
    for vid in want:
        e = xrefs.get(vid)
        if vid not in redo and isinstance(e, dict) and e.get("refAudio") and os.path.isfile(str(e["refAudio"])):
            print(f"{vid}: reused ({e.get('why', '')})")
            continue
        dst = os.path.join(xdir, f"{vid}.wav")
        if os.path.isfile(dst) and vid not in redo:
            print(f"{vid}: {dst} exists but is not in {os.path.basename(xrefs_path)}; pass --redo {vid} or rename the voice", file=sys.stderr)
            continue
        todo.append(vid)
    jobs, problems = design_jobs(table, todo, cand, a.candidates, redo)
    for p in problems:
        print(p, file=sys.stderr)
    todo = [v for v in todo if isinstance(table["voices"].get(v), dict)]
    if not todo:
        print("nothing to design")
        return 0
    jobs_path = os.path.join(xdir, "design_jobs.extras.jsonl")
    with open(jobs_path, "w", encoding="utf-8", newline="\n") as f:
        for j in jobs:
            f.write(json.dumps(j, ensure_ascii=False) + "\n")
    print(f"{len(jobs)} jobs for {len(todo)} voices -> {jobs_path}")
    if a.dry_run:
        return 0
    if jobs:
        busy = gpu_busy()
        if busy is not None and busy > 20:
            print(f"warning: the GPU is {busy} % busy; another process slows synthesis down a lot", file=sys.stderr)
        rc = run_tts(jobs_path, a.tts_python or default_tts_python(), a.tts_script, a.device, xdir)
        if rc != 0:
            print(f"the synthesizer exited with {rc} (see design.err); picking from what was made", file=sys.stderr)
    for vid in todo:
        entry, msg = choose(table, vid, cand, xdir)
        print(msg)
        if entry:
            xrefs[vid] = entry
    write_json(xrefs_path, xrefs)
    return 0


def do_apply(a, scripts, refs, table, xrefs, priority=None) -> int:
    if not scripts:
        print("no scripts", file=sys.stderr)
        return 2
    if a.out and len(scripts) != 1:
        print("--out needs exactly one script", file=sys.stderr)
        return 2
    if not (a.out or a.out_dir or a.in_place):
        print("pass --out, --out-dir or --in-place", file=sys.stderr)
        return 2
    root = a.root or ""
    full: dict = {}
    remaining: dict = {}
    rc = 0
    for src, s in scripts:
        dst = a.out or (src if a.in_place and not a.out_dir else out_path_for(src, a.out_dir, a.tag))
        if os.path.normcase(os.path.abspath(dst)) == os.path.normcase(os.path.abspath(src)) and not a.in_place:
            print(f"{src}: refusing to overwrite the input without --in-place", file=sys.stderr)
            rc = 2
            continue
        out_dir = os.path.dirname(os.path.abspath(dst))
        rep = apply_script(s, refs, root, table, xrefs, os.path.dirname(os.path.abspath(src)), out_dir, priority)
        try:
            write_json(dst, s)
        except OSError as e:
            print(f"cannot write {dst}: {e}", file=sys.stderr)
            rc = 2
            continue
        full[dst.replace("\\", "/")] = rep
        for k, v in rep["remaining"].items():
            remaining[k] = remaining.get(k, 0) + v
        print(f"{os.path.basename(dst)}: {len(rep['cloned'])} cloned, narrator {rep['narrator'] or '-'}, "
              f"{len(rep['extras'])} extras, {len(rep['moved'])} moved, {len(rep['clash'])} clashes, "
              f"{sum(rep['remaining'].values())} lines without a voice")
    if a.report:
        write_json(a.report, {"scripts": full, "remaining": remaining})
    if remaining:
        print("still without a voice: " + ", ".join(f"{k} {v}" for k, v in sorted(remaining.items(), key=lambda kv: -kv[1])))
    else:
        print("every spoken line has a voice")
    return rc


if __name__ == "__main__":
    sys.exit(main())
