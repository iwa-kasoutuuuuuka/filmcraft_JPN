"""Put the voice references chosen by voice_refs.py into a script made by eve_to_script.py.

    python apply_voices.py --script story.fcscript.json --refs voice_refs.json \
        --root "E:/filmcraft_JPN/PS2_EVE_Bursterror解析データ" [--out other.json] [--fallback Ono_Anna]

Every character with a reference gets `voice: {refAudio, refText}` (the first choice; the path is
written relative to the script file, as the script format wants). A character with no reference
(the narrator, minor roles with no recordings) gets the preset speaker `--fallback`, which
FilmCraft's Qwen3-TTS script can speak without a recording. A character that already has a voice
is left alone. Nothing is copied: the audio stays where it is.
"""

import argparse
import json
import os
import sys

DEFAULT_FALLBACK = "Ono_Anna"


def apply(script: dict, refs: dict, root: str, script_dir: str, fallback: str | None = DEFAULT_FALLBACK) -> dict:
    """Returns a report; changes `script` in place."""
    chars = script.get("characters")
    report = {"cloned": [], "preset": [], "kept": [], "missingFile": []}
    if not isinstance(chars, dict):
        return report
    for cid in sorted(chars):
        c = chars[cid]
        if not isinstance(c, dict):
            continue
        if c.get("voice"):
            report["kept"].append(cid)
            continue
        ref = refs.get(cid) if isinstance(refs, dict) else None
        path = ref.get("refAudio") if isinstance(ref, dict) else None
        text = ref.get("refText") if isinstance(ref, dict) else None
        if path and text:
            full = path if os.path.isabs(path) else os.path.join(root, path)
            if not os.path.isfile(full):
                report["missingFile"].append(cid)
            else:
                try:
                    shown = os.path.relpath(full, script_dir).replace("\\", "/")
                except ValueError:  # another drive: keep it absolute
                    shown = os.path.abspath(full).replace("\\", "/")
                c["voice"] = {"refAudio": shown, "refText": text}
                report["cloned"].append(cid)
                continue
        if fallback:
            c["voice"] = {"speaker": fallback}
            report["preset"].append(cid)
    return report


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--script", required=True)
    ap.add_argument("--refs", required=True)
    ap.add_argument("--root", required=True, help="the extracted game folder the refs are relative to")
    ap.add_argument("--out", help="write here instead of changing --script")
    ap.add_argument("--fallback", default=DEFAULT_FALLBACK, help="preset speaker for characters with no reference ('' for none)")
    a = ap.parse_args(argv)
    try:
        with open(a.script, encoding="utf-8") as f:
            script = json.load(f)
        with open(a.refs, encoding="utf-8") as f:
            refs = json.load(f)
    except (OSError, ValueError) as e:
        print(f"cannot read input: {e}", file=sys.stderr)
        return 2
    out = a.out or a.script
    rep = apply(script, refs, a.root, os.path.dirname(os.path.abspath(out)), a.fallback or None)
    try:
        with open(out, "w", encoding="utf-8", newline="\n") as f:
            json.dump(script, f, ensure_ascii=False, indent=1)
    except OSError as e:
        print(f"cannot write {out}: {e}", file=sys.stderr)
        return 2
    print(json.dumps({k: v for k, v in rep.items()}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
