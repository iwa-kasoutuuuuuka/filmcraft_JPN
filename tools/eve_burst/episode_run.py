#!/usr/bin/env python3
"""Run one EVE episode from its adapted script to an mp4 (personal study; docs/eve-burst-episode-run.md).

    python episode_run.py --episode 2                       # E:/filmcraft_JPN/eve_out/ep02/
    python episode_run.py --script some.fcscript.json --work-dir E:/filmcraft_JPN/eve_out/epXX
    python episode_run.py --episode 2 --dry-run             # what would run, nothing done

Stages (each leaves its file in the work folder; a stage whose file is newer than its input is skipped,
so a stopped run goes on where it stopped):

    fetch      the adapted script (eve_out/voiced -> adapted -> series), copied with its relative paths rebased
    voices     extras_voices.py --apply (skipped when every speaker already has a voice)
    sprites    protagonist_sprites.py (protagonists without pictures get the game's sprites)
    sounds     onomatopoeia.py (sound words: subtitled, not spoken, their sound as sfx)
    variants   sprite_variants.py with the cut-out set in eve_sprites/ (the Gemini set is not used)
    labels     emotion_labels.py --skip-neutral (instruct = 怒り（強） ..., not written for 普通)
    readings   tts_readings.py apply (names and the game's own readings as ruby)
    styles     voice_styles.py apply (a reference per emotion label, from voices/styles/styles.json)
    speak      script.synthesize, then tts_readings check -> apply -> synthesize, at most 3 rounds
    screen     screen_fields.py: movies, sub CG insets, sides of a two-shot, fades and scenes without sprites,
               copied by id from the conversions made with --screen (eve_out/routes_screen/; skipped without them)
    build      anime.run (synthesize from the cache, build, check, export), the project saved
    result     <work>/epNN.result.json: check errors and warnings, misreads left, label lines to review,
               sprites without mouth or blink pictures, length, mp4 path, the source script and its SHA-256

A filmcraft-cli built before the sources that say what a script means (script.rs, script_tools.rs, anime.rs,
synth.rs) is refused: it would read the script by old rules (`--allow-stale-cli` runs it anyway).
Before the first synthesis the GPU is looked at (nvidia-smi): a run stops when something else is using it
(a miner or a loaded LLM makes synthesis some 30 times slower); `--ignore-gpu` goes on anyway.

Standard library only; every step is an existing tool or filmcraft-cli run as a subprocess. Needs the
Qwen3-TTS venv for the synthesizer and the recognisers. One GPU job at a time: run episodes one after the other.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
EVE = "E:/filmcraft_JPN"
sys.path.insert(0, HERE)

from screen_fields import parse_donor  # noqa: E402


def default_cli(repo: str = REPO) -> str:
    """filmcraft-cli: the release build when there is one that is not older than the debug build (it
    exports several times faster), else the debug build."""
    rel = f"{repo}/target/agent-main/release/filmcraft-cli.exe"
    dbg = f"{repo}/target/agent-main/debug/filmcraft-cli.exe"
    try:
        if os.path.isfile(rel) and (not os.path.isfile(dbg) or os.path.getmtime(rel) >= os.path.getmtime(dbg)):
            return rel
    except OSError:
        pass
    return dbg


DEFAULTS = {
    "root": f"{EVE}/PS2_EVE_Bursterror解析データ",
    "out": f"{EVE}/eve_out",
    "python": f"{REPO}/tools/qwen3-tts/.venv/Scripts/python.exe",
    "cli": default_cli(),
    "whisper": f"{EVE}/models/kotoba-whisper-v2.0-faster",
    "ctc": f"{EVE}/models/japanese-hubert-base-phoneme-ctc-v4",
}
STAGES = ("fetch", "voices", "sprites", "sounds", "variants", "labels", "readings", "styles", "speak", "screen", "build", "result")
PATH_KEYS = {"voiceFile", "refAudio", "image", "openImage", "blink", "background", "bgm", "sfx", "video", "subCg"}
# 241 %: the game's 448 lines on 1080. The insets get the same scale as the sprites and the backgrounds.
BUILD = {"spriteAlign": "bottom", "spriteScale": 241, "subCgScale": 241, "check": True, "synthesize": True}
GPU_BUSY_PERCENT = 50
GPU_BUSY_MIB = 8000
MAX_ROUNDS = 3
REVIEW_TEXT = re.compile(r"ほぉ|へぇ|ふーん|ふぅん|ほう、|やれやれ|ったく|なんだと|いい性格|大したもん|結構なこと|ご苦労")


# ---------------------------------------------------------------- pure helpers (tested)

def source_for(episode: int, out: str) -> str | None:
    """The adapted script of an episode: voiced (adapted, then plain), adapted, series."""
    for c in (f"voiced/eve.ep{episode:02d}.adapted.voiced.fcscript.json", f"voiced/eve.ep{episode:02d}.voiced.fcscript.json",
              f"adapted/eve.ep{episode:02d}.fcscript.json", f"series/eve.ep{episode:02d}.fcscript.json"):
        p = os.path.join(out, c)
        if os.path.isfile(p):
            return p
    return None


def line_ids(script: dict) -> list[str]:
    """The line ids of a script in order (empty for anything that is not a script)."""
    scenes = script.get("scenes") if isinstance(script, dict) else None
    out: list[str] = []
    for s in scenes if isinstance(scenes, list) else []:
        for l in (s.get("lines") if isinstance(s, dict) and isinstance(s.get("lines"), list) else []):
            if isinstance(l, dict) and isinstance(l.get("id"), str):
                out.append(l["id"])
    return out


def check_source(episode: int, src: str, out: str) -> str | None:
    """A reason to refuse `src` as the script of `episode`, or None.

    A stale copy under voiced/ once survived the renumbering of the episodes and was rendered as another
    episode without any check noticing, so a source is refused when its title names another episode, or
    when it is not the adapted script itself and its line ids differ from adapted/'s for this episode."""
    try:
        with io.open(src, encoding="utf-8") as f:
            s = json.load(f)
    except (OSError, ValueError) as e:
        return f"cannot read {src}: {e}"
    m = re.search(r"第\s*(\d+)\s*話", str(s.get("title", "")) if isinstance(s, dict) else "")
    if m and int(m.group(1)) != episode:
        return f"{src} is titled episode {m.group(1)}, not {episode}"
    ref = os.path.join(out, f"adapted/eve.ep{episode:02d}.fcscript.json")
    if os.path.isfile(ref) and os.path.abspath(ref) != os.path.abspath(src):
        try:
            with io.open(ref, encoding="utf-8") as f:
                want = line_ids(json.load(f))
        except (OSError, ValueError):
            return None
        got = line_ids(s)
        if want and got != want:
            return (f"{src} has {len(got)} lines ({got[0] if got else '-'} .. {got[-1] if got else '-'}) but adapted/ has "
                    f"{len(want)} ({want[0]} .. {want[-1]}): a stale copy; rebuild it from adapted/ (extras_voices.py --apply)")
    return None


def rebase(node, src_dir: str, dst_dir: str, key: str | None = None):
    """Relative file paths of a script (voiceFile, refAudio, image, ...) rewritten from src_dir to dst_dir.
    Only paths that exist from src_dir are rewritten; anything else is left as it is."""
    if isinstance(node, dict):
        return {k: rebase(v, src_dir, dst_dir, k) for k, v in node.items()}
    if isinstance(node, list):
        return [rebase(v, src_dir, dst_dir, key) for v in node]
    if isinstance(node, str) and key in PATH_KEYS | {"vowelImages", "a", "i", "u", "e", "o"} and node and not os.path.isabs(node) and not re.match(r"^[A-Za-z]:", node):
        p = os.path.normpath(os.path.join(src_dir, node))
        if os.path.exists(p):
            try:
                return os.path.relpath(p, dst_dir).replace("\\", "/")
            except ValueError:  # the work folder is on another drive: the full path is all there is
                return os.path.abspath(p).replace("\\", "/")
    return node


def duplicate_ids(ids: list[str]) -> list[str]:
    """The line ids that come more than once (each named once, in order)."""
    seen, twice = set(), []
    for i in ids:
        if i in seen and i not in twice:
            twice.append(i)
        seen.add(i)
    return twice


def sha256_of(path: str) -> str | None:
    try:
        h = hashlib.sha256()
        with open(path, "rb") as f:
            for block in iter(lambda: f.read(1 << 20), b""):
                h.update(block)
        return h.hexdigest()
    except OSError:
        return None


def gpu_reason(text: str, percent: int = GPU_BUSY_PERCENT, mib: int = GPU_BUSY_MIB) -> str:
    """Why a GPU is not free, from `nvidia-smi --query-gpu=utilization.gpu,memory.used
    --format=csv,noheader,nounits` (one line a GPU, the first is used); "" when it is free or the
    text cannot be read."""
    line = (text or "").strip().splitlines()[0] if (text or "").strip() else ""
    parts = [x.strip() for x in line.split(",")]
    try:
        util, used = int(float(parts[0])), int(float(parts[1]))
    except (ValueError, IndexError):
        return ""
    if util >= percent or used >= mib:
        return f"the GPU is in use by something else ({util} %, {used} MiB)"
    return ""


def gpu_busy() -> str:
    try:
        r = subprocess.run(["nvidia-smi", "--query-gpu=utilization.gpu,memory.used", "--format=csv,noheader,nounits"],
                           capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.SubprocessError):
        return ""
    return gpu_reason(r.stdout) if r.returncode == 0 else ""


# what a script means is in these files: a filmcraft-cli built before one of them changed reads a script
# differently (it happened: a build from before `voice.styles` spoke 29 episodes without the styles)
CLI_SOURCES = ("crates/project/src/script.rs", "crates/engine/src/script_tools.rs", "crates/engine/src/anime.rs", "crates/speech/src/synth.rs")


def stale_cli(cli: str, repo: str = REPO) -> str:
    """Why `cli` is too old to run a script with, or "" (also when it cannot be told)."""
    try:
        built = os.path.getmtime(cli)
    except OSError:
        return ""
    newer = []
    for rel in CLI_SOURCES:
        try:
            if os.path.getmtime(os.path.join(repo, rel)) > built:
                newer.append(rel)
        except OSError:
            continue
    if not newer:
        return ""
    when = time.strftime("%Y-%m-%d %H:%M", time.localtime(built))
    return f"{cli} was built {when}, before {', '.join(newer)} changed"


def default_donors(out: str) -> list[str]:
    """The conversions made with --screen that screen_fields.py copies from, when they are there."""
    return [f"{prefix}={out}/routes_screen/{name}.fcscript.json" for prefix, name in (("k", "kojirou"), ("m", "marina"))]


def fresh(out: str, inputs: list[str]) -> bool:
    """True when `out` exists and is newer than every existing input."""
    if not os.path.isfile(out):
        return False
    t = os.path.getmtime(out)
    return all(not os.path.isfile(i) or os.path.getmtime(i) <= t for i in inputs)


def all_voiced(script: dict) -> bool:
    """Every character who speaks a line without a recording has a voice."""
    chars = script.get("characters") if isinstance(script.get("characters"), dict) else {}
    for sc in script.get("scenes") or []:
        for l in (sc.get("lines") or []) if isinstance(sc, dict) else []:
            if not isinstance(l, dict) or l.get("kind") == "direction" or l.get("voiceFile") or l.get("spoken") is False:
                continue
            sp = l.get("speaker")
            if sp and not (isinstance(chars.get(sp), dict) and chars[sp].get("voice")):
                return False
    return True


def labels_to_review(rows: list[list[str]]) -> list[dict]:
    """Label lines worth a second look (labels.tsv rows: id, speaker, label, strength, source, text): a
    label taken from punctuation where the words suggest sarcasm, teasing or a muttered aside."""
    out = []
    for r in rows:
        if len(r) < 6 or r[4] != "text":
            continue
        lid, sp, lab, st, _, text = r[:6]
        if (lab in ("問い", "静か", "普通") and REVIEW_TEXT.search(text)) or (lab == "怒り" and st == "中" and text.count("！") == 1 and len(text) <= 8):
            out.append({"id": lid, "speaker": sp, "label": f"{lab}（{st}）", "text": text})
    return out


def missing_variants(report) -> dict[str, list[str]]:
    """Sprites shown in the episode that lack pictures, from sprite_variants.py's JSON report:
    `noMouth` (no open mouth and no vowel mouths: the face does not move) and `noBlink`."""
    out = {"noMouth": [], "noBlink": []}
    items = report.get("missing") if isinstance(report, dict) and isinstance(report.get("missing"), list) else []
    for m in items[:10_000]:
        if not isinstance(m, dict) or not isinstance(m.get("base"), str):
            continue
        kinds = set(m.get("kinds") or [])
        if kinds & {"open", "a", "i", "u", "e", "o"} and m["base"] not in out["noMouth"]:
            out["noMouth"].append(m["base"])
        if "blink" in kinds and m["base"] not in out["noBlink"]:
            out["noBlink"].append(m["base"])
    return out


def summarize(build: dict, check: dict, review: list, missing: list, mp4: str) -> dict:
    steps = {s.get("step"): s for s in build.get("steps", []) if isinstance(s, dict)} if isinstance(build, dict) else {}
    chk = steps.get("check", {})
    exp = steps.get("export", {})
    return {
        "ok": bool(build.get("ok")) and bool(exp.get("ok")),
        "checkErrors": chk.get("errors"), "checkWarnings": chk.get("warnings"), "problems": chk.get("problems"),
        "seconds": steps.get("build", {}).get("seconds"),
        "misreadsLeft": {k: v.get("words") for k, v in (check.get("lines") or {}).items() if isinstance(v, dict) and v.get("words")},
        "labelsToReview": review, "spritesWithoutMouth": missing.get("noMouth", []), "spritesWithoutBlink": missing.get("noBlink", []),
        "mp4": exp.get("path") or (mp4 if os.path.isfile(mp4) else None),
        "failedStep": build.get("failedStep"),
    }


# ---------------------------------------------------------------- the run

class Run:
    def __init__(self, a):
        self.a = a
        self.ep = a.episode
        self.tag = f"ep{self.ep:02d}" if self.ep else os.path.basename(a.script).split(".")[0]
        self.work = a.work_dir or os.path.join(a.out, self.tag)
        self.f = lambda stage, ext="fcscript.json": os.path.join(self.work, f"{self.tag}.{stage}.{ext}")  # noqa: E731
        self.tts = os.path.join(self.work, "tts")
        self.state_path = os.path.join(self.work, f"{self.tag}.state.json")
        self.done = []

    def log(self, msg: str):
        line = f"{time.strftime('%H:%M:%S')} {self.tag} {msg}"
        print(line, flush=True)
        if not self.a.dry_run:
            os.makedirs(self.work, exist_ok=True)
            with io.open(os.path.join(self.work, f"{self.tag}.log"), "a", encoding="utf-8") as f:
                f.write(line + "\n")

    def call(self, args: list[str], out: str | None = None) -> str:
        if self.a.dry_run:
            self.log("would run: " + " ".join(os.path.basename(x) if i == 0 else x for i, x in enumerate(args))[:300])
            return "{}"
        r = subprocess.run(args, capture_output=True, text=True, encoding="utf-8", errors="replace")
        if out:
            with io.open(out, "w", encoding="utf-8") as f:
                f.write(r.stdout)
            with io.open(out + ".err", "w", encoding="utf-8") as f:
                f.write(r.stderr[-50_000:])
        if r.returncode != 0:
            raise RuntimeError(f"{os.path.basename(args[1] if args[0].endswith('python.exe') else args[0])}: exit {r.returncode}: {r.stderr[-600:]}")
        return r.stdout

    def py(self, tool: str, *args: str, out: str | None = None) -> str:
        return self.call([self.a.python, "-X", "utf8", os.path.join(HERE, tool), *args], out)

    def cli(self, cmd: str, params: dict, out: str | None = None, extra: list[str] | None = None) -> dict:
        o = self.call([self.a.cli, *(extra or []), "exec", cmd, json.dumps(params, ensure_ascii=False)], out)
        try:
            r = json.loads(o)
        except ValueError:
            return {}
        return r.get("result", r) if isinstance(r, dict) else {}

    def stage(self, name: str, out: str, inputs: list[str], fn) -> str:
        if self.a.force and name in self.a.force:
            pass
        elif fresh(out, inputs):
            self.log(f"{name}: up to date, skipped")
            return out
        self.log(f"{name}: running")
        fn(out)
        if not self.a.dry_run and not os.path.isfile(out):
            raise RuntimeError(f"{name}: produced no {out}")
        self.done.append(name)
        return out

    def run(self) -> dict:
        a = self.a
        src = a.script or source_for(self.ep, a.out)
        if not src:
            raise FileNotFoundError(f"no adapted script for episode {self.ep} under {a.out}")
        if not a.script:
            why = check_source(self.ep, src, a.out)
            if why:
                raise ValueError(why)
        try:
            with io.open(src, encoding="utf-8") as f:
                src_ids = line_ids(json.load(f))
        except (OSError, ValueError) as e:
            raise ValueError(f"cannot read {src}: {e}") from e
        twice = duplicate_ids(src_ids)
        if twice:
            raise ValueError(f"{src}: {len(twice)} line ids come more than once ({', '.join(twice[:5])}): two lines would share one audio file")
        old = "" if (a.allow_stale_cli or a.dry_run) else stale_cli(a.cli)
        if old:
            raise RuntimeError(f"{old}: it would read the script by the old rules. Build it again "
                               "(CARGO_TARGET_DIR=target/agent-main cargo build --release -p filmcraft-cli) or pass --allow-stale-cli")
        os.makedirs(self.work, exist_ok=True)
        self.log(f"source {src} ({len(src_ids)} lines); cli {a.cli}")

        def fetch(out):
            with io.open(src, encoding="utf-8") as f:
                s = json.load(f)
            s = rebase(s, os.path.dirname(os.path.abspath(src)), os.path.abspath(self.work))
            if not a.dry_run:
                with io.open(out, "w", encoding="utf-8", newline="\n") as f:
                    f.write(json.dumps(s, ensure_ascii=False, indent=1))
        s1 = self.stage("fetch", self.f("1-fetch"), [src], fetch)

        def voices(out):
            if a.dry_run:
                self.log("would check voices and maybe run extras_voices.py --apply")
                return
            with io.open(s1, encoding="utf-8") as f:
                s = json.load(f)
            if all_voiced(s):
                self.log("voices: every speaker has a voice, copied")
                if not a.dry_run:
                    shutil.copyfile(s1, out)
                return
            self.py("extras_voices.py", "--scripts", s1, "--voices-dir", f"{a.out}/voices", "--apply", "--refs", os.path.join(HERE, "voice_refs.json"),
                    "--root", a.root, "--out", out, "--report", self.f("voices", "report.txt"))
        s2 = self.stage("voices", self.f("2-voices"), [s1], voices)
        s3 = self.stage("sprites", self.f("3-sprites"), [s2], lambda out: self.py("protagonist_sprites.py", "--scripts", s2, "--root", a.root, "--out", out))
        s4 = self.stage("sounds", self.f("4-sounds"), [s3], lambda out: self.py("onomatopoeia.py", s3, out, "--json", out=self.f("sounds", "report.json")))

        def variants(out):
            # the cut-out set (mouth_cutouts.py) in eve_sprites/ is the one set in use (user's decision,
            # 2026-11-06); the Gemini pictures in eve_sprites_gemini/ are not used
            self.py("sprite_variants.py", "--script", s4, "--root", a.root, "--variants", a.sprites, "--out", out, "--report", self.f("variants", "report.json"))
        s5 = self.stage("variants", self.f("5-variants"), [s4], variants)
        s6 = self.stage("labels", self.f("6-labels"), [s5], lambda out: self.py(
            "emotion_labels.py", "--script", s5, "--expressions", a.expressions, "--skip-neutral", "--report", self.f("labels", "tsv"), "--out", out))
        s7 = self.stage("readings", self.f("7-readings"), [s6], lambda out: self.py("tts_readings.py", "apply", "--script", s6, "--names", a.names, "--out", out))
        s8 = self.stage("styles", self.f("8-styles"), [s7, a.styles], lambda out: self.py(
            "voice_styles.py", "apply", "--styles", a.styles, "--script", s7, "--alias", "koj=koj", "--alias", "narrator=koj", "--alias", "mar=mar", "--out", out))

        # speak: synthesize, listen back, put misread words in kana, again (at most MAX_ROUNDS)
        if a.seed_cache and os.path.isdir(a.seed_cache) and not os.path.isdir(self.tts) and not a.dry_run:
            shutil.copytree(a.seed_cache, self.tts)
            self.log(f"speak: cache seeded from {a.seed_cache}")
        st = self.load_state()
        stamp = os.path.getmtime(s8) if os.path.isfile(s8) else 0.0
        done_for = st.get("speakDoneFor") == stamp and os.path.isfile(st.get("final") or "") and not (a.force and "speak" in a.force)
        script = st["final"] if done_for else s8
        if done_for:
            self.log("speak: up to date, skipped")
        else:
            busy = "" if (a.ignore_gpu or a.dry_run) else gpu_busy()
            if busy:
                raise RuntimeError(f"{busy}: synthesis would be many times slower. Stop the other job (ask its owner) or pass --ignore-gpu")
            for rnd in range(MAX_ROUNDS):
                r = self.cli("script.synthesize", {"path": script, "out": self.tts, "python": a.python}, self.f(f"speak-r{rnd}", "synth.json"))
                self.log(f"speak round {rnd + 1}: synthesized {len(r.get('synthesized') or []) if isinstance(r.get('synthesized'), list) else r.get('synthesized')}, failed {len(r.get('failed') or []) if isinstance(r.get('failed'), list) else r.get('failed')}")
                chk = self.f("speak", "check.json")
                o = self.py("tts_readings.py", "check", "--script", script, "--tts-dir", self.tts, "--names", a.names, "--overrides", a.overrides,
                            "--whisper", a.whisper, "--ctc", a.ctc, "--report", chk)
                flagged = (json.loads(o.strip().splitlines()[-1]) if o.strip() and not a.dry_run else {}).get("flagged", 0)
                self.log(f"speak round {rnd + 1}: misread lines {flagged}")
                if not flagged or rnd == MAX_ROUNDS - 1:
                    break
                nxt = self.f(f"9-read-r{rnd + 1}")
                self.py("tts_readings.py", "apply", "--script", script, "--report", chk, "--names", a.names, "--out", nxt)
                script = nxt
            self.save_state({"final": script, "speakDoneFor": stamp})

        # screen: what the game shows, copied by id from the conversions made with --screen
        donors = [d for d in (a.screen_donor if a.screen_donor else default_donors(a.out)) if os.path.isfile(parse_donor(d)[1])]
        if a.no_screen:
            self.log("screen: --no-screen, skipped")
        elif not donors:
            self.log("screen: no conversion made with --screen (eve_out/routes_screen/), skipped")
        else:
            spoken = script

            def screen(out):
                args = ["--script", spoken]
                for d in donors:
                    args += ["--donor", d]
                args += ["--siblings", a.siblings or f"{a.out}/adapted/eve.ep*.fcscript.json", "--out", out, "--report", self.f("screen", "report.json")]
                try:
                    self.py("screen_fields.py", *args)
                except RuntimeError as e:
                    if "exit 1" not in str(e):
                        raise
                    self.log("screen: no line of the script is in the conversions (other ids): built without the screen fields")
                    shutil.copyfile(spoken, out)
            script = self.stage("screen", self.f("10-screen"), [spoken] + [parse_donor(d)[1] for d in donors], screen)

        # build and export
        mp4 = os.path.join(self.work, f"{self.tag}.mp4")
        build_out = self.f("build", "json")
        if fresh(build_out, [script]) and os.path.isfile(mp4) and not (a.force and "build" in a.force):
            self.log("build: up to date, skipped")
        else:
            params = dict(BUILD, path=script, out=self.tts, name=f"EVE {self.tag}", python=a.python, device="cuda:0", export=mp4)
            self.cli("anime.run", params, build_out, ["--save-as", os.path.join(self.work, f"{self.tag}.fcproj")])
            self.done.append("build")
        build = self.read_json(build_out)
        check = self.read_json(self.f("speak", "check.json"))
        rows = []
        if os.path.isfile(self.f("labels", "tsv")):
            with io.open(self.f("labels", "tsv"), encoding="utf-8") as f:
                rows = [line.rstrip("\n").split("\t") for line in f]
        rep = self.read_json(self.f("variants", "report.json"))
        scr = self.read_json(self.f("screen", "report.json"))
        result = dict(summarize(build, check, labels_to_review(rows), missing_variants(rep), mp4), episode=self.ep, script=script, tts=self.tts,
                      stagesRun=self.done, source=src, sourceSha256=sha256_of(src), sourceLines=len(src_ids), cli=a.cli,
                      screen={k: scr.get(k) for k in ("videos", "videosLeftOut", "fades", "hidden", "slots", "freeSide", "insetsOn")} if scr else None)
        if not a.dry_run:
            with io.open(self.f("result", "json"), "w", encoding="utf-8", newline="\n") as f:
                f.write(json.dumps(result, ensure_ascii=False, indent=1))
        self.log(f"result: ok={result['ok']} misreads={len(result['misreadsLeft'])} review={len(result['labelsToReview'])} mp4={result['mp4']}")
        return result

    def read_json(self, p: str) -> dict:
        try:
            with io.open(p, encoding="utf-8") as f:
                v = json.load(f)
            return v.get("result", v) if isinstance(v, dict) else {}
        except (OSError, ValueError):
            return {}

    def load_state(self) -> dict:
        return self.read_json(self.state_path)

    def save_state(self, d: dict):
        if self.a.dry_run:
            return
        st = self.load_state()
        st.update(d)
        tmp = self.state_path + ".tmp"
        with io.open(tmp, "w", encoding="utf-8") as f:
            f.write(json.dumps(st, ensure_ascii=False, indent=1))
        os.replace(tmp, self.state_path)


def main(argv=None) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except Exception:  # noqa: BLE001
            pass
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--episode", type=int, help="episode number (names the work folder and files)")
    ap.add_argument("--script", help="the adapted script to start from (else found from --episode)")
    ap.add_argument("--seed-cache", help="a synthesis cache folder copied into <work>/tts before the first run (keeps audio of unchanged lines)")
    ap.add_argument("--work-dir")
    ap.add_argument("--out", default=DEFAULTS["out"], help="eve_out folder")
    ap.add_argument("--root", default=DEFAULTS["root"], help="the dump (read only)")
    ap.add_argument("--python", default=DEFAULTS["python"], help="the Qwen3-TTS venv's python")
    ap.add_argument("--cli", default=DEFAULTS["cli"])
    ap.add_argument("--whisper", default=DEFAULTS["whisper"])
    ap.add_argument("--ctc", default=DEFAULTS["ctc"])
    ap.add_argument("--names", default=os.path.join(HERE, "eve_names.json"))
    ap.add_argument("--sprites", default=f"{EVE}/eve_sprites", help="mouth/blink variants (the cut-out set)")
    ap.add_argument("--expressions")
    ap.add_argument("--styles")
    ap.add_argument("--overrides")
    ap.add_argument("--screen-donor", action="append", help="a conversion made with eve_to_script.py --screen, PREFIX=path (default: k= and m= under <out>/routes_screen/)")
    ap.add_argument("--siblings", help="the other episodes of the series, a glob (default: <out>/adapted/eve.ep*.fcscript.json)")
    ap.add_argument("--no-screen", action="store_true", help="leave the screen stage out")
    ap.add_argument("--allow-stale-cli", action="store_true", help="run even when filmcraft-cli is older than the sources that define a script")
    ap.add_argument("--ignore-gpu", action="store_true", help="synthesize even when something else is using the GPU")
    ap.add_argument("--force", action="append", choices=STAGES, help="redo a stage even if its file is up to date")
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args(argv)
    if a.episode is None and not a.script:
        print("give --episode or --script", file=sys.stderr)
        return 2
    if a.episode is not None and not 1 <= a.episode <= 99:
        print("episode must be 1..99", file=sys.stderr)
        return 2
    a.expressions = a.expressions or f"{a.out}/tts_quality/eve_expressions.json"
    a.styles = a.styles or f"{a.out}/voices/styles/styles.json"
    a.overrides = a.overrides or f"{a.out}/tts_quality/overrides.json"
    try:
        r = Run(a).run()
    except (OSError, RuntimeError, ValueError) as e:
        print(f"episode_run: {e}", file=sys.stderr)
        return 1
    print(json.dumps({k: r[k] for k in ("ok", "mp4", "seconds", "checkErrors", "checkWarnings")} | {"misreadsLeft": len(r["misreadsLeft"]), "labelsToReview": len(r["labelsToReview"])}, ensure_ascii=False))
    return 0 if r["ok"] or a.dry_run else 1


if __name__ == "__main__":
    sys.exit(main())
