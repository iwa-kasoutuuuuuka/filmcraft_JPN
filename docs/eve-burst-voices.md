# EVE burst error PLUS: voice inventory, gaps and cloning references

Personal technical study notes for the extracted data of the PS2 game *EVE burst error PLUS*
(the user's own extraction; **nothing of the game is in this repository** and none of it may be
published). This page records how many dialogue lines the game has against how many have a
recorded voice, which characters can be cloned from the game's own clips, and how the reference
clips were chosen by [`tools/eve_burst/voice_refs.py`](../tools/eve_burst/voice_refs.py). The
pipeline it feeds is [script.md](script.md) + [text-to-speech.md](text-to-speech.md): a script
character's `voice` is `{"refAudio", "refText"}` (cloning) or `{"speaker", "instruct"}` (preset),
and `script.synthesize` speaks every line that has no `voiceFile`.

Numbers below were measured on the complete copy (15,505 voice WAVs, all present and readable;
the catalog `00_目録/分類別/03_音声_ファイル一覧.tsv` lists exactly those). Re-run the tool to
refresh them.

```
python tools/eve_burst/voice_refs.py --root "E:/filmcraft_JPN/PS2_EVE_Bursterror解析データ" \
    --inventory-md inventory.md --inventory-json inventory.json
```

## 1. The clips

- WAV, 16-bit PCM, **mono**. Person voices are **48 kHz** (15,365 of the 15,505 clips); the
  non-person effects (`j_擬音`, `m_擬音`, `se_効果音`, `eve_ジングル`, 140 clips) are **24 kHz**.
  Nothing is 8, 16, 22.05 or 44.1 kHz. Qwen3-TTS takes a reference at any rate, so no resampling
  is needed.
- Level is uniform and clean: picked clips have peak -1 to -7 dBFS, RMS -17 to -24 dBFS and a noise
  floor 45 to 60 dB below speech (no BGM, the voices were recorded dry). The median clip is
  2 to 3.6 s long (short interjections dominate), so only about 20 to 30 % of a character's clips
  fall in the 4 to 12 s window a reference needs.
- Clip to line: `00_目録/ボイスと台詞の対応表.tsv` (taken from the scenario's own message
  instruction, not a guess). A clip may serve several lines; the picker uses only clips whose lines
  all carry one transcript.
- Text is written with `‥` (two dots) for pauses; the picker hands the cloner `…` (one per `‥`).
- Nine voices are stored as ADX (`03_音声/その他`: `may3000`-`may3005`, `kat0995`, `koj9999`,
  `mar9999`); they are not in the voice folders and are ignored.

## 2. Inventory: lines versus recorded voices

The whole game: **87 scenarios, 52,033 dialogue lines, 16,159 with a voice (31.1 %)**, 35,874
without. 787 of the voiced lines play only a sound effect (`j`, `m`, `se`, `eve_` clips); the
rest are spoken by a person. A speaker's code is the code of the voice clips that speak their
lines; an unvoiced line is credited to a speaker's code when most of that speaker's voiced lines
use it (so 小次郎 is `koj`, 孔 is `dep`). Generic names that borrow many actors' voices (男, 女,
声) stay unattributed.

By character code (`to synthesize` = lines of that character without a recording):

| code | character | lines | voiced | to synthesize | voiced % | clips on disk | referenced | 4-12 s clips | median s |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| koj | 天城小次郎 | 18383 | 438 | 17945 | 2.4 | 440 | 439 | 65 | 1.79 |
| mar | 法条まりな | 15241 | 496 | 14745 | 3.3 | 499 | 498 | 95 | 2.29 |
| may | 御堂真弥子 | 2279 | 2266 | 13 | 99.4 | 2272 | 2272 | 474 | 2.25 |
| kyo | 氷室恭子 | 2001 | 1974 | 27 | 98.7 | 1985 | 1985 | 312 | 2.27 |
| yay | 桂木弥生 | 1680 | 1678 | 2 | 99.9 | 1679 | 1679 | 507 | 2.79 |
| hon | 本部長 | 1418 | 1418 | 0 | 100.0 | 1419 | 1419 | 328 | 2.66 |
| pul | プリン／プリシア＝レム | 1407 | 1399 | 8 | 99.4 | 1354 | 1354 | 284 | 2.38 |
| kat | 桂木源三郎／鈴木源三郎 | 1054 | 1053 | 1 | 99.9 | 1056 | 1056 | 353 | 2.97 |
| aka | 茜 | 781 | 781 | 0 | 100.0 | 782 | 782 | 178 | 2.45 |
| mid | 御堂大使 | 773 | 771 | 2 | 99.7 | 776 | 776 | 348 | 3.62 |
| mat | 松乃 | 652 | 651 | 1 | 99.8 | 654 | 654 | 214 | 3.14 |
| dep | ディーブ（偽の孔） | 555 | 548 | 7 | 98.7 | 551 | 551 | 221 | 3.35 |
| sir | シリア | 506 | 505 | 1 | 99.8 | 505 | 505 | 65 | 1.92 |
| nik | 二階堂 | 450 | 449 | 1 | 99.8 | 449 | 449 | 137 | 2.86 |
| aqu | アクア＝スティル | 402 | 397 | 5 | 98.8 | 395 | 395 | 119 | 2.77 |
| gul | グレン＝ストールマン | 351 | 351 | 0 | 100.0 | 352 | 352 | 134 | 3.33 |
| kag | 香川美純 | 155 | 155 | 0 | 100.0 | 155 | 155 | 58 | 3.19 |
| kou | 孔 | 42 | 42 | 0 | 100.0 | 42 | 42 | 11 | 2.64 |
| pur | プリシア（立ち絵のみ） | 0 | 0 | 0 | - | 0 | 0 | 0 | - |
| uwe | ウエイトレス | 0 | 0 | 0 | - | 0 | 0 | 0 | - |

(`referenced` counts distinct clip names used by scenario lines; it can differ from `clips on
disk` by a few unreferenced or extra clips. `pul` has 1,354 clips for 1,403 voiced uses because
some clips are reused.)

**The gap is almost entirely the two protagonists.** In a protagonist's own route the player
character's lines are text only: 小次郎編 has 17,939 unvoiced 小次郎 lines (5 voiced), まりな編
has 14,745 unvoiced まりな lines (9 voiced). Their voice clips come from the common scenes and
from the other route, where they are supporting characters (小次郎 230 voiced lines in まりな編,
まりな 199 in 小次郎編). Every other named character is 98.7 to 100 % voiced: together they have
only 70 unvoiced lines.

By speaker name (largest 30; `code` is where the unvoiced lines are credited):

| speaker | lines | voiced | unvoiced | code |
|---|---:|---:|---:|---|
| 小次郎 | 18376 | 432 | 17944 | koj |
| まりな | 15161 | 416 | 14745 | mar |
| （地の文）narration | 2416 | 1387 | 1029 | - |
| 真弥子 | 1856 | 1845 | 11 | may |
| 恭子 | 1790 | 1789 | 1 | kyo |
| 弥生 | 1624 | 1622 | 2 | yay |
| 本部長 | 1379 | 1379 | 0 | hon |
| 茜 | 769 | 769 | 0 | aka |
| プリン | 702 | 702 | 0 | pul |
| 鈴木 | 650 | 650 | 0 | kat |
| 御堂 | 646 | 646 | 0 | mid |
| 松乃 | 629 | 628 | 1 | mat |
| プリシア | 560 | 560 | 0 | pul |
| 二階堂 | 418 | 418 | 0 | nik |
| シリア | 385 | 385 | 0 | sir |
| 孔 | 361 | 361 | 0 | dep |
| グレン | 349 | 349 | 0 | gul |
| 桂木 | 328 | 327 | 1 | kat |
| 声 | 325 | 197 | 128 | - |
| アクア | 291 | 290 | 1 | aqu |
| 守衛 | 228 | 0 | 228 | - |
| 男 | 208 | 48 | 160 | - |
| 警備員 | 173 | 0 | 173 | - |
| 女 | 166 | 28 | 138 | - |
| フロント | 156 | 0 | 156 | - |
| 香川 | 151 | 151 | 0 | kag |
| ディーブ | 142 | 142 | 0 | dep |
| 警官 | 108 | 0 | 108 | - |
| 制服の女の子 | 103 | 97 | 6 | kyo |
| 目の前の女の子 | 99 | 85 | 14 | kyo |

Unvoiced lines that belong to no character code (3,116 in all):

| speaker | unvoiced lines |
|---|---:|
| （地の文）narration | 1029 (小次郎編 563, まりな編 402, 共通 64) |
| 守衛 | 228 |
| 警備員 | 173 |
| 男 | 160 |
| フロント | 156 |
| 女 | 138 |
| 声 | 128 |
| 警官 | 108 |
| 受付の人 | 85 |
| サラリーマン | 76 |
| 机に向かう男 | 73 |
| 店員 | 57 |
| 本を読んでる人 | 56 |
| ウエイトレス | 41 |
| シルディ | 36 |
| 70 more names | 572 |

### Which characters have voice data

- **Plenty (cloneable, with room to choose):** every one of the 18 codes above except `kou`:
  `koj` and `mar` have 440 and 499 clips (65 and 95 of them 4 to 12 s); `may`, `kyo`, `yay`,
  `hon`, `pul`, `kat`, `mid` have 1,000 to 2,300 clips each.
- **Little:** `kou` (42 clips, 11 usable) and `kag` (155 clips, 58 usable) can still be cloned
  (kag, kou references were found under the strict limits), but a long synthesis run for them
  would sound more repetitive; both are fully voiced already, so nothing needs synthesizing.
- **None:** `pur` (portraits only, same person as `pul`), `uwe` / ウエイトレス and every extra in the
  table above (守衛, 警備員, フロント, 警官, 受付の人, narration, the generic 男, 女, 声, ...). They
  need a preset speaker or a borrowed reference (section 5).

## 3. How the references are chosen

`voice_refs.py` builds a pool per character code, scores it and keeps the best one to three clips.
Everything is deterministic: ties are broken by the clip's file name, there is no random sampling,
and the same tree gives the same file.

**Pool (hard filters, text side).** A clip is a candidate when its voice name has the character's
code; all rows of the correspondence table agree on one transcript; and the speaker of every row
is the character itself (a speaker that has at least 10 % of the code's lines, and is not "声",
"目の前の...", a duet with `＆`, or a bare 男/女). The transcript is rejected when it has fewer than
6 letters or no words (ellipsis-only), contains `《》` or `（）` (inner voice or aside), a laugh or
scream pattern (ははは, きゃ, ぎゃ, うわあ, ぐあ, あああ, ...), Latin letters or digits (the reading
is ambiguous), or is more than a quarter ellipsis.

**Pre-ranking.** Candidates of 4 to 12 s (read from the WAV header) are ordered by
`text score - |seconds - 7|`, and the first 60 are measured (decoded and analysed).

**Hard filters, audio side.** Duration 4 to 12 s; clipping (samples at or above 32700) at most 0.1 %
of the samples; speech level (90th percentile of 20 ms frames) above -35 dBFS; no silent run longer
than 0.8 s inside the speech (leading and trailing silence do not count); at most 35 % of the frames
silent (more than 35 dB under the speech level); speech to noise floor (90th over 10th percentile
frame level) at least 12 dB, which catches BGM or hiss; speaking rate between 3 and 14 morae per
second (estimated from the text), which catches a transcript that does not match the clip.

**Score, 0 to 100** (higher is better):

| part | points | rule |
|---|---:|---|
| text tone | 40 | starts at 40; -8 per `！` (max -16); -4 per interjection (あっ, えっ, くそ, はあ, ...; max -12); up to -12 for ellipsis share; -8 if it starts with an ellipsis; -4 if it trails off in one; -6 for `～` or `―`; -3 without a closing `。` or `？`. A plain calm sentence scores 40. |
| duration | 12 | full marks from 5.5 to 9.5 s, falling to 3 at 4 s or 12 s |
| peak | 8 | full marks from -12 to -1 dBFS; -0.6 per dB too quiet, -3 per dB above -1 dBFS |
| speech to noise | 16 | 0 at 12 dB, full from 52 dB |
| silence ratio | 12 | full at 0 %, none at 35 % |
| speaking rate | 8 | full from 5.5 to 10 morae/s, -2 per mora/s outside |
| level | 4 | full when the speech level is -30 to -12 dBFS |
| inner gap | 4 | full without a gap, none at 0.8 s |

A neutral tone is judged from the text only (the audio has no tone label): `！`, interjection
openers, ellipsis and screams/laughs push a clip down or out; the calm full sentence that is a
statement or a plain question wins. Whispers are not detectable in the text and are only caught
when they come with a long pause, low level or a bad speaking rate.

**Choosing the one to three.** Best score first; a further clip is added only when its transcript
is different and its score is within 15 points of the best. The first becomes `refAudio` /
`refText` / `seconds` / `why`; the others are listed under `alternatives`.

**Relaxed pass.** A character with no clip under the strict limits would be retried with 2.5 to 14 s,
a gap up to 1.4 s, 50 % silence, SNR 8 dB and digits allowed, and the `why` says `RELAXED limits`.
No character needed it in the real data.

Because the game's voices are all clean, the audio score of most clips is near its maximum and the
text part and the duration decide; the picks are therefore calm, full-sentence, 5 to 9 s statements.

## 4. The picked references

`tools/eve_burst/voice_refs.json` (written by `voice_refs.py`; not tracked, because it holds lines of
the game) holds, per code,
`refAudio` (relative to the data root; use `--abs` for absolute paths), `refText`, `seconds`, `why`
and up to two `alternatives`. Copy the three fields into a script character:

```json
"koj": {"name": "天城小次郎", "voice": {
  "refAudio": "C:/.../03_音声/ボイス/koj_天城小次郎/koj0003.wav",
  "refText": "<the line the clip speaks, from the catalog>"}}
```

(A script's `refAudio` is relative to the script's folder, so either put the script next to a copy
or junction of the data root, or write absolute paths; do not copy the clips into this repository.)

| code | first choice | s |
|---|---|---:|
| koj | `koj_天城小次郎/koj0003.wav` | 4.54 |
| mar | `mar_法条まりな/mar0131.wav` | 6.24 |
| kyo | `kyo_氷室恭子/kyo1577.wav` | 5.52 |
| may | `may_御堂真弥子/may0374.wav` | 6.18 |
| yay | `yay_桂木弥生/yay1398.wav` | 6.82 |
| hon | `hon_本部長/hon1338.wav` | 5.36 |
| pul | `pul_プリシア/pul0260.wav` | 6.11 |
| kat | `kat_鈴木源三郎/kat0674.wav` | 6.03 |
| aka | `aka_茜/aka0140.wav` | 7.29 |
| mid | `mid_御堂大使/mid0350.wav` | 6.76 |
| mat | `mat_松乃/mat0103.wav` | 6.68 |
| dep | `dep_ディーブ/dep0228.wav` | 5.88 |
| sir | `sir_シリア/sir0420.wav` | 6.05 |
| nik | `nik_二階堂/nik0123.wav` | 7.33 |
| aqu | `aqu_アクア/aqu0165.wav` | 4.84 |
| gul | `gul_グレン/gul0083.wav` | 7.44 |
| kag | `kag_香川美純/kag0050.wav` | 8.87 |
| kou | `kou_孔/kou0016.wav` | 4.49 |

(Folder prefix `03_音声/ボイス/` omitted. `aqu0165` opens with "フフ", a small laugh; its two
alternatives `aqu0266` and `aqu0280` are plainer if the cloned voice should sound serious.)

### Sanity check by measurement (not by listening)

Each picked clip was measured again with an independent calculation (20 ms / 50 ms frames, RMS over
the whole clip, absolute -50 dBFS silence): length 4.5 to 8.9 s; RMS -16.9 to -24.4 dBFS; peak -1.0
to -6.8 dBFS; **0 clipped samples** in all 30 clips checked; 9 to 26 % of the 50 ms frames under
-50 dBFS (the pauses between phrases; the protagonists' clips 19 to 26 %, matching their pool medians
of 25 to 29 % for 4 to 12 s clips). The pool medians of koj and mar (5.3 and 5.2 s, RMS -22.4 dBFS,
peak -2 to -3 dBFS) are what the picks look like, so the picks are typical clean clips, not
outliers. One alternative, `mar0102`, has only 2 % silent frames and a lower speech to noise ratio
(28 dB against 45 to 60 dB for the rest): prefer the first choice or `mar0046` for `mar`.

## 5. Recommended synthesis plan

Clone every named character from the game's own clips (the Japanese preset `Ono_Anna` is a young
woman's voice and would not match anyone, and the game has the actors' real clips); use a preset
only where no recording exists.

| who | lines to synthesize | voice | note |
|---|---:|---|---|
| koj | 17,945 | **clone**, `koj0003` (alternatives `koj0268`, `koj0157`) | the protagonist of 小次郎編: essentially the whole route |
| mar | 14,745 | **clone**, `mar0131` (alternatives `mar0046`, `mar0102`) | the protagonist of まりな編 |
| kyo, may, yay, mat, dep, aqu, pul, mid, nik, sir, kat | 27, 13, 2, 1, 7, 5, 8, 2, 1, 1, 1 | clone, first choice | fully voiced in the game; a handful of lines (mostly where the game's voice is a sound-effect only) |
| hon, aka, gul, kag, kou | 0 | clone (for new lines only) | nothing to synthesize |
| narration, 小次郎編 | 563 | clone `koj` | the first-person narrator of the route |
| narration, まりな編 | 402 | clone `mar` | the game itself voices 63 narration lines with Marina's actor |
| narration, 共通 | 64 | preset `Ono_Anna`, `instruct` "落ち着いた声で、淡々と" | or whichever protagonist the scene follows |
| male extras (守衛, 警備員, フロント, 警官, 受付の人, サラリーマン, 机に向かう男, 店員, 男, ...) | about 1,100 | **choice for you** | no male Japanese preset exists; options: clone a minor male actor (`hon` or `kat` for older men, `dep` for smooth ones) with a different reference, or `Ono_Anna` with a low `instruct` (weak) |
| female extras (女, ウエイトレス, ...) | about 300 | clone `mat` or `aka`, or `Ono_Anna` | `Ono_Anna` fits best here |
| 声 (a voice off-screen) | 128 | by the scene | speak it as the character the scene implies, or `Ono_Anna` |

Totals: **32,758** unvoiced lines belong to a named character (of which 32,690 are the two
protagonists), **3,116** to narration and extras. All 35,874 can be spoken; the clone route covers
33,000 or more. A full run is about 36,000 lines, which at the 1 s a line of an RTX 5080 is about
10 hours; run per scenario (`script.synthesize` is incremental).

Open points for you: the voice actors' recordings are used here only as a style reference for a
personal study; keep the synthesized audio and the data private. The protagonists' lines are
inner-monologue style, which a clone of the *supporting-role* clips (spoken aloud to others) will
read in a more outgoing tone than the original text-only narration; try `instruct` ("静かな独白で")
only with a preset, since a clone follows its recording.

## 6. Using and testing the tool

```
python tools/eve_burst/voice_refs.py --root DATA [--out voice_refs.json] [--per-char 3]
        [--pool 60] [--abs] [--no-numpy] [--inventory-md F] [--inventory-json F]
python -m unittest discover tools/eve_burst/tests
```

Standard library only. `numpy` is used if installed (here it was: the whole run, which reads
15,505 WAV headers and decodes about 1,000 clips, took a few minutes, most of it disk time);
`--no-numpy` forces the stdlib decoder, which gives the same picks on the test tree (not measured
on the real data). `soundfile` is optional and only needed for a WAV that is not 8/16-bit PCM; it
was not installed and not needed. The data folder is only read. The tests build a tiny fake tree
with generated tones (noisy, clipped, gapped, too short, laughing and shouting clips) and skip the
real-data tests when the data folder is absent (set `EVE_BURST_DATA` for another location).

## 7. Fixing the extras' voices across episodes

The converter numbers unnamed speakers per script (`n001`, `m-n003`, ...), so the same 守衛 has
another id in every episode and a voice set by id would change from episode to episode.
[`tools/eve_burst/extras_voices.py`](../tools/eve_burst/extras_voices.py) keys the extras by their
**display name** instead: name -> voice class -> one designed reference, the same in every script.
Standard library only (the pitch check uses `wave` + `array`); the table and the voices live
outside the repository, under `eve_out/voices/`.

```
python tools/eve_burst/extras_voices.py --scripts "eve_out/series/eve.ep*.fcscript.json" \
    --scripts "eve_out/adapted/eve.ep*.fcscript.json" --voices-dir eve_out/voices --init --list
python tools/eve_burst/extras_voices.py --scripts ... --voices-dir eve_out/voices --design [--dry-run]
python tools/eve_burst/extras_voices.py --scripts ... --voices-dir eve_out/voices --apply \
    --root DATA --out-dir eve_out/voiced --tag voiced [--report report.json]
```

- `--list` prints every character without a voice by display name, with its lines, the lines
  that would be synthesized, the scripts it is in, how it will be voiced (`ref:<code>` for a named
  character in `voice_refs.json`, `narrator`, `skip`, `role`) and, for a role, its class and voice.
- `--init` writes `extras_roles.json` (or adds new names to it; edits are kept): `voices` (id ->
  `gender`, `design`, `refText`), `classes` (class -> gender and an ordered voice list), `names`
  (display name -> `class`, optional pinned `voice`, `variant`), `aliases` (`pur` -> `pul`: the
  same actress) and `skipIds` (`sfx`, whose lines are all sound files). A new name's class is
  guessed from generic role words, in this order: child (子供, 男の子, 少年), female-old (おばちゃん),
  female-young (女の子, 娘, ウエイトレス, 生徒), female-mid (女, 婦, 受付), gatekeeper (守衛), security
  (警備員), police (警官, 刑事), front (フロント), thug (野郎, チンピラ), male-old (親父, 老人),
  male-young (若い, ボーイ), announcer (TV, 放送), male-mid (男, 員, 人, 手, 客, 店...), else unknown
  (声, 声１...). A trailing １/２ or Ａ/Ｂ, or 左/中央/右, picks the first, second or third voice of the
  class (`variant`), so 男１ and 男２ differ. Proper names (シルディ, サラ, 景子) need an edit.
- `--design` designs the voices the names can use and that `extras/extras_refs.json` does not
  have yet (the six designed before, front, police, man-a, man-b, man-c, woman-a, are reused):
  three candidates each, written as one `filmcraft_tts.py --jobs` file and spoken by one process
  (the GPU is used serially; a warning is printed when `nvidia-smi` shows it busy). Of the
  candidates in `extras/cand/<voice>-<k>.wav` the pick is the one whose median F0 (autocorrelation
  of 40 ms frames every 20 ms at about 8 kHz) fits the gender (male 60-160 Hz, female 165-300,
  child 200-450, any 90-300) and whose length is 3-15 s; then the steadiest pitch (narrowest 10-90 %
  range); then the longest. It is copied to `extras/<voice>.wav` and recorded in
  `extras_refs.json` with `f0`, `why` and the other candidates. The F0 matches the earlier numpy
  measurement (`pick_f0.py`) to the hertz on the six existing voices. An existing `<voice>.wav`
  that is not in `extras_refs.json` is never overwritten (that is why the old `guard.wav`, whose
  transcript was not kept, stays and 守衛 got a new `gatekeeper`).
- `--apply` writes a copy of each script (`--out-dir` + `--tag`, `--out` for one, `--in-place` to
  overwrite). A character that has a voice is never touched. Named characters get their clone from
  `voice_refs.json` (via `apply_voices.apply`, no preset fallback); `narrator` the clone of the
  scenes' point of view (`koj` for 小次郎編, `mar` for まりな編; a scene without `pov` takes the one
  before it in the route). A script whose narration is in both routes gives the narrator the
  majority's voice and moves the other route's narration lines to `narrator-<pov>` (same name
  語り). Extras get their name's voice; two extras that speak in the same scene never share one:
  the name with fewer lines **over all the scripts given** (`--scripts` plus `--rank-scripts`, which
  are only counted) takes the next voice of its class, then any voice of the same gender, and the
  report lists it under `moved` (`clash` if nothing was free). So the big roles never move; pin a
  `voice` in the table for a small role that must not move either (a pinned voice is never moved,
  only reported as a clash). Paths are written relative to the output script and the script's
  other paths are rebased to the output folder.

### Result on the 29 episodes (2026-11-06)

The voiceless characters of `eve_out/series` and `eve_out/adapted` (29 scripts each): 18 named
codes (all in `voice_refs.json`; `pur` through its alias `pul`), the narrator, `sfx` (every line
a recording) and **78 extra names**. 13 voices were taken by the names: the six designed before
and ten designed now (30 candidates, one GPU process, 11 minutes with the model load; every pick
inside its window):

| voice | used by | pick | F0 Hz (10-90 %) | other candidates |
|---|---|---|---:|---|
| gatekeeper | 守衛 | `gatekeeper-1` | 127 (104-145) | 154, 101 |
| security | 警備員 | `security-2` | 143 (116-154) | 157, 99 |
| male-mid-2 | second male (男２, clashes) | `male-mid-2-2` | 98 (78-110) | 118, 205 |
| male-young-2 | 本を読んでる人, clashes | `male-young-2-3` | 108 (89-143) | 143, 229 |
| thug | 中央の野郎 | `thug-2` | 157 (113-242) | 242, 216 |
| female-young | ウエイトレス, 景子, 生徒 | `female-young-1` | 258 (205-400) | 364, 381 |
| female-mid-2 | サラ, 女２ | `female-mid-2-2` | 178 (148-211) | 211, 195 |
| female-old | おばちゃん | `female-old-2` | 205 (163-250) | 229, 93 |
| unknown | 声, 声１..., 野次馬の声 | `unknown-1` | 138 (108-167) | 111, 136 |
| vamp | シルディ (table only) | `vamp-1` | 235 (205-267) | 242, 267 |

(`child` was not designed: no name uses it.) Two thug candidates came out at 216-242 Hz and the
pick is at the edge of the male window (157 Hz, with a wide 113-242 Hz spread). Pitch says
nothing about whether a voice sounds right, so the ten picks were listened to on 2026-11-06 and
kept as they are (`--redo <voice>` designs one again).

`--apply` on all 58 scripts (into `eve_out/voiced/`, `eve.epNN.voiced.fcscript.json` from
series and `eve.epNN.adapted.voiced.fcscript.json` from adapted; the inputs are unchanged):
**every spoken line without a recording has a voice** (34,339 lines to synthesize), and
`script.validate` reports **0 errors and 0 warnings** in all 58. The narrator is `mar` or `koj` by
the episode's route; ep29 (both routes) has `narrator` = mar and `narrator-koj` for 小次郎's scenes.
68 of the 78 names have one voice in every episode; the other ten are small roles that sometimes
share a scene with a bigger one of the same class (サラリーマン with 男, コピー機の男 with 机に向かう男,
男１/男２, 警官の声 with 警官, ...): 27 moves in all. Two clashes are left: 本を呼んでる人 with
本を読んでる人 in ep11 (one person, the name misspelt in the game, so one voice is right) and 声４
in ep29 (five unknown voices in one scene, more than the class has).

Tests: `tools/eve_burst/tests/test_extras_voices.py` (classification, the table and its repair,
F0 of generated tones and broken files, the pick, the design jobs, the apply with a fake script and
WAVs, the same name with another id in another script, hostile input, the command line).
