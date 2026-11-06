"""Sound words (擬音) in a script: subtitled, never read aloud, their sound on the effects track.

A game writes its noises as lines (ガシャーン！, ガチャッ。, ぐぅ～。): the text box shows the word and
the game plays a sound with it, or nothing. Read aloud by a synthesized voice they sound like a
person reciting a noise. This tool finds those lines and marks them `"spoken": false` (FilmCraft
then subtitles them, synthesizes nothing and lays out no voice; docs/script.md), and puts their
sound where sounds go: `sfx` (A3 Effects, levelled under the voices by anime.build).

    python onomatopoeia.py IN.fcscript.json OUT.fcscript.json
    python onomatopoeia.py IN.fcscript.json --dry-run            # the list only, nothing written
    python onomatopoeia.py IN OUT --keep s0003-014 --mark s0002-007 --table sounds.json --json

A line is a sound word when (first match wins; directions, lines already `spoken: false` and
`--keep` ids are skipped):

  clip      its voiceFile is a game sound, not a voice: a folder whose name has 効果音 or 擬音
            (ボイス/j_擬音, 効果音/se_一般効果音, ...) or a file named like a sound (se17, j304, m201,
            eve_51). The file moves to `sfx` (a line that already has an `sfx` keeps that one).
            A line with a real recording (aka0002.wav) is never touched.
  speaker   its speaker is the converter's sound character (id `sfx`, or the name 擬音).
  katakana  its text, without punctuation, is katakana only (small っぁぃぅぇぉ and ー～ allowed:
            ドッカーーーンっ) with 1..--max-kana kana (8; a word said over and over, イライライラ,
            counts once), and is not a word people say: an
            interjection (フン, ウン, エッ, アハハ, ...), a common word written in katakana (コレ,
            ウソ, バカ, ...) or a character's name.
  bracket   its whole text is in （…） or (…) and the inside is a katakana sound word of any length.
  with-sfx  it has an `sfx` and its text is kana only (hiragana too: ぐぅ～) and short.
  mark      its id is given with --mark.

A sound word without a sound borrows one: the same word (punctuation and ー～ ignored) elsewhere
in the script with a sound, else an entry of --table (a JSON object, word -> sound file path as
written in the script); otherwise it is a silent beat with its subtitle.

Reported, not changed (look at them): hiragana sound-like repetitions (ぐるぐる、ぐるぐる。), short
katakana interjections that were kept, katakana lines longer than --max-kana.

Standard library only. Paths are written as found.
"""

import argparse
import collections
import io
import json
import os
import re
import sys

KATAKANA = set(chr(c) for c in range(0x30A1, 0x30FB)) | {"ヽ", "ヾ"}
HIRAGANA = set(chr(c) for c in range(0x3041, 0x3097))
SMALL = set("ァィゥェォッャュョヮぁぃぅぇぉっゃゅょゎ")
LONG = set("ー～〜")
# punctuation and symbols a sound word may carry (they are not counted)
PUNCT = set("。、．，！？!?‥…・「」『』　 \t♪☆★♡♥～〜ー―－-") - LONG
BRACKETS = [("（", "）"), ("(", ")")]

# things people say that are written in katakana: kept spoken
INTERJECTIONS = {
    "フン", "フフ", "フフフ", "ウフフ", "ウン", "ウウン", "ハイ", "エ", "ア", "アア", "オイ", "オウ", "ン",
    "ハア", "ハ", "ヘ", "ホ", "ヘエ", "ホウ", "ウム", "ム", "ムウ", "ナニ", "エヘヘ", "エヘヘン", "アハハ", "ハハ",
    "ハハハ", "ヘヘ", "ヘヘヘ", "イヤ", "ヤダ", "オオ", "ウワ", "ワア", "キャ", "ヒイ", "ヒ", "ウ", "ウウ", "オ",
    "ギャ", "ヤア", "ネ", "ヨ", "サア", "ホラ", "ハテ", "エト", "アノ", "マア", "ソウ", "ヨシ", "ヨオ",
}
WORDS = {
    "コレ", "ソレ", "アレ", "ドレ", "ココ", "ソコ", "アソコ", "ウソ", "オレ", "ボク", "ダメ", "バカ", "アホ", "ホント",
    "マジ", "オツケ", "オケ", "サンキユ", "イイ", "ドウゾ", "イエ", "ノ", "イエス", "ゴメン", "スマン", "アリガト",
    "アリガトウ", "オハヨウ", "サヨナラ", "バイバイ", "ヤバイ", "スゴイ", "ナルホド", "ダレ", "ナゼ", "ドウシテ",
}

SOUND_FILE = re.compile(r"^(se\d+|j\d+|m\d+|eve_\d+)[a-z]?(_.*)?\.(wav|adx|ahx|ogg|mp3|flac)$", re.I)


def is_sound_file(path):
    """A game sound rather than a voice: by folder (効果音, 擬音) or by name (se17, j304)."""
    if not isinstance(path, str):
        return False
    parts = re.split(r"[\\/]", path)
    folders = parts[:-1]
    if any("効果音" in f or "擬音" in f for f in folders):
        return True
    return bool(SOUND_FILE.match(parts[-1] if parts else ""))


def strip_punct(text):
    return "".join(ch for ch in text if ch not in PUNCT and not ch.isspace())


def key(text):
    """The word for matching and the word lists: no punctuation, no ー～, no small kana."""
    return "".join(ch for ch in strip_punct(text) if ch not in LONG and ch not in SMALL)


def to_katakana(text):
    return "".join(chr(ord(c) + 0x60) if 0x3041 <= ord(c) <= 0x3096 else c for c in text)


def kana_count(core):
    return sum(1 for ch in core if (ch in KATAKANA or ch in HIRAGANA) and ch not in LONG)


def katakana_word(core):
    """Katakana only (small hiragana and ー～ allowed), at least one katakana."""
    if not core or not any(ch in KATAKANA and ch not in SMALL for ch in core):
        return False
    return all(ch in KATAKANA or ch in LONG or (ch in SMALL and ch in HIRAGANA) for ch in core)


def kana_only(core):
    return bool(core) and all(ch in KATAKANA or ch in HIRAGANA or ch in LONG for ch in core)


def repeated_piece(core):
    """イライライラ -> イラ: the piece of two or more kana that the word repeats, else ''."""
    k = key(core)
    for n in range(2, len(k) // 2 + 1):
        if len(k) % n == 0 and k == k[:n] * (len(k) // n):
            return k[:n]
    return ""


def repetition(core):
    """ごろごろごろごろ: one short piece said again (a mimetic word)."""
    return bool(repeated_piece(core))


def in_brackets(text):
    t = text.strip()
    for a, b in BRACKETS:
        if t.startswith(a) and t.endswith(b) and len(t) > len(a) + len(b):
            return t[len(a):-len(b)]
    return None


def names_in_katakana(script):
    out = set()
    chars = script.get("characters")
    for c in (chars.values() if isinstance(chars, dict) else []):
        n = key(str(c.get("name", "")) if isinstance(c, dict) else "")
        if n and all(ch in KATAKANA for ch in n):
            out.add(n)
    return out


def classify(line, chars, opts, names):
    """(rule, note) when the line is a sound word, ('', why) when it is a near miss, else None."""
    text = str(line.get("text") or "")
    vf = line.get("voiceFile")
    if not isinstance(vf, str):
        vf = None
    if line.get("id") in opts.mark:
        return ("mark", "")
    if vf and is_sound_file(vf):
        return ("clip", "")
    if vf:
        return None  # a real recording: spoken as it is
    sp = line.get("speaker")
    who = chars.get(sp) if isinstance(sp, str) else None
    if sp == "sfx" or (isinstance(who, dict) and who.get("name") == "擬音"):
        return ("speaker", "")
    core = strip_punct(text)
    inner = in_brackets(text)
    if inner is not None and katakana_word(strip_punct(inner)):
        return ("bracket", "")
    if katakana_word(core):
        k = key(core)
        if k in INTERJECTIONS or k in WORDS or k in names:
            return ("", "katakana interjection or word, kept spoken")
        # a word said over and over (イライライライラ) counts once
        n = kana_count(core)
        piece = repeated_piece(core)
        if piece:
            n = kana_count(piece)
        if n > opts.max_kana:
            return ("", f"katakana only but {n} kana (more than {opts.max_kana}), kept spoken")
        return ("katakana", "")
    if line.get("sfx") and kana_only(core) and kana_count(core) <= opts.max_kana:
        return ("with-sfx", "")
    if kana_only(core) and repetition(core) and kana_count(core) <= 2 * opts.max_kana:
        return ("", "a repeated kana word (mimetic?), kept spoken")
    return None


def process(script, opts):
    """Mark the sound words in `script` (changed in place). Returns the report."""
    chars = script.get("characters") if isinstance(script.get("characters"), dict) else {}
    names = names_in_katakana(script)
    table = dict(opts.table or {})
    scenes = script.get("scenes") if isinstance(script.get("scenes"), list) else []
    lines = [l for sc in scenes if isinstance(sc, dict) for l in (sc.get("lines") if isinstance(sc.get("lines"), list) else []) if isinstance(l, dict)]
    # the sounds the script itself gives each word
    known = collections.defaultdict(collections.Counter)
    for l in lines:
        if l.get("kind") == "direction" or not l.get("text"):
            continue
        snd = l.get("sfx") or (l.get("voiceFile") if is_sound_file(l.get("voiceFile")) else None)
        if snd:
            known[key(str(l["text"]))][snd] += 1
    marked, near, already = [], [], []
    for l in lines:
        lid = l.get("id")
        if l.get("kind") == "direction" or not str(l.get("text") or "").strip():
            continue
        if l.get("spoken") is False:
            already.append(lid)
            continue
        if not isinstance(lid, str) or lid in opts.keep:
            continue
        c = classify(l, chars, opts, names)
        if c is None:
            continue
        rule, why = c
        entry = {"id": lid, "kind": l.get("kind", "dialogue"), "speaker": l.get("speaker"), "text": l.get("text")}
        if not rule:
            near.append(dict(entry, why=why))
            continue
        sound, source = l.get("sfx"), "sfx" if l.get("sfx") else ""
        vf = l.get("voiceFile")
        if is_sound_file(vf):
            if sound:
                entry["dropped"] = vf
            else:
                sound, source = vf, "voiceFile"
        if not sound:
            k = key(str(l.get("text")))
            if known.get(k):
                sound, source = known[k].most_common(1)[0][0], "borrowed"
            elif k in table or str(l.get("text")) in table:
                sound, source = table.get(k) or table.get(str(l.get("text"))), "table"
        l["spoken"] = False
        if is_sound_file(vf):
            l.pop("voiceFile", None)
        if sound:
            l["sfx"] = sound
        entry.update(rule=rule, sound=sound, source=source or "none")
        marked.append(entry)
    by_rule = collections.Counter(m["rule"] for m in marked)
    by_source = collections.Counter(m["source"] for m in marked)
    return {"marked": marked, "nearMisses": near, "alreadyUnspoken": already, "byRule": dict(by_rule), "bySound": dict(by_source)}


def print_report(r, out=None):
    w = (out or sys.stdout).write
    w(f"sound words: {len(r['marked'])}  by rule {r['byRule']}  sound from {r['bySound']}\n")
    for m in r["marked"]:
        snd = os.path.basename(str(m["sound"])) if m.get("sound") else "(silent)"
        extra = f"  (kept its sfx, dropped {os.path.basename(m['dropped'])})" if m.get("dropped") else ""
        w(f"  {m['id']:<14} {m['rule']:<9} {str(m['speaker'] or '-'):<9} {m['text']}  -> {snd} [{m['source']}]{extra}\n")
    if r["alreadyUnspoken"]:
        w(f"already spoken: false: {len(r['alreadyUnspoken'])}\n")
    w(f"near misses (left spoken, check them): {len(r['nearMisses'])}\n")
    for m in r["nearMisses"]:
        w(f"  {m['id']:<14} {str(m['speaker'] or '-'):<9} {m['text']}  ({m['why']})\n")


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("src")
    ap.add_argument("dst", nargs="?")
    ap.add_argument("--dry-run", action="store_true", help="list only, write nothing")
    ap.add_argument("--max-kana", type=int, default=8)
    ap.add_argument("--keep", nargs="*", default=[], help="line ids never marked")
    ap.add_argument("--mark", nargs="*", default=[], help="line ids always marked")
    ap.add_argument("--table", help="JSON object: word -> sound file, for words without a sound")
    ap.add_argument("--json", action="store_true", help="print the report as JSON")
    a = ap.parse_args(argv)
    if not a.dry_run and not a.dst:
        ap.error("give OUT, or --dry-run")
    if a.max_kana < 1 or a.max_kana > 64:
        ap.error("--max-kana must be 1..64")
    with io.open(a.src, encoding="utf-8") as f:
        script = json.load(f)
    table = None
    if a.table:
        with io.open(a.table, encoding="utf-8") as f:
            table = json.load(f)
        if not isinstance(table, dict):
            ap.error("--table must be a JSON object")
        table = {key(str(k)): str(v) for k, v in table.items()} | {str(k): str(v) for k, v in table.items()}
    opts = argparse.Namespace(max_kana=a.max_kana, keep=set(a.keep), mark=set(a.mark), table=table)
    report = process(script, opts)
    if a.json:
        sys.stdout.write(json.dumps(report, ensure_ascii=False, indent=1) + "\n")
    else:
        print_report(report)
    if not a.dry_run:
        with io.open(a.dst, "w", encoding="utf-8", newline="\n") as f:
            f.write(json.dumps(script, ensure_ascii=False, indent=1))
    return 0


if __name__ == "__main__":
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    sys.exit(main())
