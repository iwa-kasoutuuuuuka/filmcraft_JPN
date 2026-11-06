# EVE burst error PLUS: converted assets and how scenario names map to them

Personal technical study only. The converted PS2 data lives outside the repository
(`PS2_EVE_Bursterror解析データ`, read-only); nothing from it is copied, committed or published.
`tools/eve_burst/assets.py` is the resolver: `Assets(root)` turns the names used inside the scenario
JSON files into paths relative to that root.

```python
from assets import Assets
a = Assets(r"E:\filmcraft_JPN\PS2_EVE_Bursterror解析データ")
a.background("01______")      # '02_画像/背景/01______.png'
a.sprite("kyo", 1, "b")       # '02_画像/立ち絵/kyo_氷室恭子/kyo01bd.png'  (same as a.sprite("kyo01bd"))
a.bgm("eve039")               # '03_音声/BGM/eve039_<title>.wav'
a.voice("kyo0123")            # '03_音声/ボイス/kyo_氷室恭子/kyo0123.wav'
print(Assets.format_coverage(a.coverage()))      # or: python tools/eve_burst/assets.py <root>
```

Every method returns a path relative to the root (forward slashes) or `None`, ignores case and a
trailing `.bmp/.pbd/.adx/.ahx/.sfd/.png/.wav/.mp4`, and never raises on a missing file or folder.

## Where things are

| Scenario asset type (op) | Resolver | Folder under root | File |
| --- | --- | --- | --- |
| 背景 (0x8014, 0x801a) | `background` | `02_画像/背景`, then `イベントCG`, `日付画面`, `システム画面` | `<name>.png` |
| 背景（端末画面） (0x8041) | `background` | `02_画像/ハッキング画面` | `hack_k.png`, `hack_m.png` |
| (full-screen picture) | `event_cg` | `02_画像/イベントCG`, then `背景` | `<name>.png` |
| サブCG (0x8036) | `sub_cg` | `02_画像/サブCG` | `<name>.png` |
| 立ち絵 (0x8048, 0x804b) | `sprite` | `02_画像/立ち絵/<code>_<name>/` | `<code><NN><outfit>d.png` |
| BGM (0x8026) | `bgm` | `03_音声/BGM` | `eveNNN_<title>.wav` |
| 効果音, 音声 (0x800b, 0x8039, 0x8047) | `se` | `03_音声/効果音/<group>/`, then the voice folders | `<name>.wav` |
| ボイス (the `voices` list of a 0x8035 line) | `voice` | `03_音声/ボイス/<code>_<name>/`, then `その他/ADX形式のボイス`, then `効果音` | `<name>.wav` |
| ムービー (0x8008), 演出ムービー (0x8046) | `movie` | `04_動画/イベントムービー`, `日付表示`, `オープニング` | `<name>.mp4` |

Points that are not obvious from the names:

- The scenario's "background" command (0x8014) also shows full-screen event CGs: in the 87 scenarios
  1,969 background references are real backgrounds, 644 are event CGs (`s0010`, `k07fb`, `m1210`, `ev011`...)
  and 6 are date screens (`1203md`...). `background()` therefore searches all of them.
- A background name is `NN` + time-of-day letter + variant letters padded with `_` to 8 characters
  (`01______`, `02_n____`, `14hn____`). Case differs between scenario and disk in a few places
  (10 references); lookups are case-insensitive.
- Sub CGs are small windows (e.g. 272x247, 296x194) shown over the scene, not full frames.
- Folders are named `<code>_<人物名>` (`kyo_氷室恭子`); the resolver finds them by the code part, so
  renaming the Japanese half does not matter.
- `kat` is one voice/sprite set for both 桂木 (Kojiro route) and 鈴木 (Marina route); `pul` (voice) and
  `pur` (sprite) are the same person (プリン / プリシア). See `00_目録/キャラクターコード表.tsv`.

## Sprites (立ち絵)

Files are `<code><NN><outfit>d.png`, e.g. `kyo02bd`:

- `code`: 3 letters (`aka aqu gul hon kag kat koj kyo mar mat may mid nik pur sir uwe yay`).
- `NN`: two digits, an expression/pose number of that character (`00`, `01`, ...), not a mouth state.
- `outfit`: `a`, `b`, `c`, `d` = costume/figure (`kyo..a` school uniform, `kyo..b` field uniform;
  `may` and `pur` also have `c`; `pur` has `d`). Estimated by the data's author.
- trailing `d`: constant on every file (meaning unknown).
- 94 files, transparent RGBA PNG, canvas 320x448 (some 432x448), bottom-aligned on a 640x448 screen.
  KOJIROU.PAK and MARINA.PAK hold identical copies.
- Slots: `0x8048` (show sprite) takes a slot number: slot 2 (centre) 424 times, slot 3 23 times,
  slot 1 12 times. Slot 2 is the usual single speaker; 1 and 3 are the left and right of a two-shot
  (which side is which is inferred from the numbering, not verified). `0x804b` swaps the picture in
  the slot (expression change, 1,607 uses); `0x8049` clears all sprites; `0x8012` clears the screen.
- `sprite(code, pose, outfit)` builds the name from parts; `sprite_names(code)` lists what exists.

Mouth-open and blink images: **there are no separate open-mouth or blink variants of a picture.**
Each `NN` is a whole different pose/expression. Looking at the images (kyo, aka, yay, may): `00` is the
neutral picture with a closed mouth; most other numbers have a mouth drawn open, but together with a
different eyebrow/eye/blush (e.g. kyo01 angry and open, kyo02 embarrassed and open, kyo03 a bigger
pose change). There is no closed/open pair for one expression, so a talking mouth cannot be made by
switching two stills; the usable choices are alternating `00` with another number of the same outfit
(face changes too), or drawing the mouth ourselves. Short face-change clips exist as video:
`04_動画/表情アニメ/<code>_*/<from><to NN>.mp4` (e.g. `kyo00ad01.mp4` = kyo00ad -> kyo01ad, 8 frames
at 30 fps for "only the eyes and mouth change", 20-40 frames when the whole body moves; always
back to `00` from the others: `kyo01ad00.mp4`). `sprite_transition(from, to)` resolves them. These are
transitions between stills, not lip-sync loops; they are not blink loops either (not verified
frame by frame, the movies were not yet on disk when this was written).

Six sprites are never referenced by a scenario: `kyo02ad kyo03ad pur01bd pur02bd pur04bd sir02ad`
(probably used only as intermediate states of the face clips).

## Mouth and blink variants (drawn outside, named by rule)

Because the game has no open/closed pairs, the mouth and blink pictures are drawn outside FilmCraft
(the plan uses Antigravity with Gemini) and joined to the script by file name with
`tools/eve_burst/sprite_variants.py`. Nothing is written into the dump.

**Where:** a work folder outside the dump, by default `<root>/../eve_sprites/` (for the dump at
`E:/filmcraft_JPN/PS2_EVE_Bursterror解析データ` that is `E:/filmcraft_JPN/eve_sprites/`), one
subfolder per sprite code: `eve_sprites/kyo/`, `eve_sprites/aka/`... (`--variants DIR` picks
another folder; files directly in it, without the code subfolder, are found too). Keep it on the
same drive as the script so the paths in the script stay relative.

**Names:** `<base>` is the file name of the original without `.png` (`kyo01bd`). Every variant is a
PNG of **the same canvas size as the original, with the figure at exactly the same place**
(`anime.build` stacks them over the original without moving them):

| File | Picture | Goes into the script as | Needed |
|---|---|---|---|
| `<base>_a.png` `_i` `_u` `_e` `_o` | mouth open for that vowel (あ wide, い flat, う small round, え half, お round) | `expressions.<name>.vowelImages.{a,i,u,e,o}` | yes |
| `<base>_blink.png` | eyes closed, mouth closed | `expressions.<name>.blink` (V4) | yes |
| `<base>_open.png` | a plain open mouth (for kanji without a reading, letters) | `expressions.<name>.openImage` | optional: without it `_a` is used |
| `<base>_closed.png` | the same face with the mouth **closed** | replaces `expressions.<name>.image` | only for an original whose mouth is drawn open (most numbers other than `00`) |

So a face needs 6 files (`_a _i _u _e _o _blink`), 7 with `_open`, 8 when the original is drawn
mouth-open and needs `_closed` as its resting face. Case does not matter.

```bash
# what to draw for this script (TSV: character, expression, lines shown, spoken lines, file, kind, status)
python tools/eve_burst/sprite_variants.py --script ep01.fcscript.json --root "E:/filmcraft_JPN/PS2_EVE_Bursterror解析データ" --list > need.tsv
# write what exists into the script (or --out another file); missing files are listed on stderr
python tools/eve_burst/sprite_variants.py --script ep01.fcscript.json --root "E:/filmcraft_JPN/PS2_EVE_Bursterror解析データ" --report variants.json
```

Like `apply_voices.py` it changes the script in place unless `--out` is given, and keeps
`openImage` / `vowelImages` / blink pictures that are already set (`--overwrite` replaces them; a
blink given the older way, as an expression `<name>_blink`, also counts as set).
Running it again after more pictures are drawn adds only the new ones. The expressions it looks at
are those the script shows (a non-direction line's `expression`, else `defaultExpression`, else
`normal`, else the first by name, the same rule as `anime.build`); expressions only used by
directions are not listed.

**Instruction for the image model** (one picture per request; attach the original PNG):

> 添付の立ち絵 `kyo01bd.png`（320×448、背景透過 PNG）を元に、**口だけ**を「あ」の形に大きく開いた絵を描いてください。
> キャンバスの大きさ、人物の位置・大きさ・輪郭、髪、目、眉、服、色、線の太さ、塗りの調子、背景の透過は**一切変えない**こと。
> 拡大・縮小・切り抜き・余白の追加・位置の移動をしない。出力は元と同じ 320×448 の透過 PNG で、ファイル名は `kyo01bd_a.png`。

Variants of the same request: 「い」= 横に引いて歯が少し見える、「う」= すぼめて小さく丸く、「え」= 半開きで横長、
「お」= 縦長の丸、`_open` = 自然な半開き、`_blink` = 「**目だけ**を閉じた絵（まつげの線で閉じた目、口は閉じたまま）」、
`_closed` = 「**口だけ**を閉じた絵（表情は元のまま）」. Check each result by laying it over the original at 50 %
opacity: anything other than the mouth (or the eyes) that moves shows up as a double line. Image models
tend to re-render the whole picture slightly (line weight, colours, a shifted chin); if that happens, cut
the mouth region out of the result and paste it onto the original in an image editor instead.

How they are used: V2 shows `image` for the whole line, V3 shows the vowel's picture (or `openImage`)
while the voice is open, V4 shows the blink picture for about 0.12 s every 3–4 s. Each is a whole
sprite laid over the one below, so the variant must match the original everywhere except the mouth or
the eyes. Because the blink picture has the mouth closed, a blink waits for a moment when the mouth
is closed (a pause between words) instead of shutting it mid-word.

To place them as on the game screen (the sprite standing on the bottom edge, in proportion to the
background), build with `spriteAlign: "bottom"` and `spriteScale: 241` (1080 / 448) for the fitted
640x448 background with bars at the sides, or `backgroundFit: "fill"` and `spriteScale: 300`
(1920 / 640) to fill the 16:9 frame and cut 44 source pixels from the top and bottom of the
background (the sprite then stands about 130 px higher on it than in the game).

## Audio

- BGM: `03_音声/BGM/eveNNN_<title>.wav` (47 tracks `eve001`..`eve047`; the title is part of the file
  name, so `bgm("eve039")` matches by the `eve039_` prefix). The same names exist in SYSTEM, KOJIROU
  and MARINA PAKs with identical content; one converted file each.
- Voices: `03_音声/ボイス/<code>_<name>/<code><4 digits>[letter].wav`, 48 kHz mono, one folder per
  speaker code (`aka aqu dep gul hon kag kat koj kou kyo mar mat may mid nik pul sir yay`).
  Numbers are per code (`kyo0001`..`kyo1985`-ish), not per scenario. A trailing letter (`kyo0219b`,
  `may1512C`, `dep0503a/b/c`) is a continuation part of one line; the scenario writes it with any case
  (`yay1128B`). A line can carry several voice names in its `voices` list.
- Non-person voices in the same folder tree (24 kHz): `j_擬音` (`j301`..`j7xx`, Kojiro route onomatopoeia),
  `m_擬音` (`m2xx`..`m6xx`, Marina route), `se_効果音` (`se25`...), `eve_ジングル` (`eve_51b`...).
  These are dialogue-line sounds (`.ahx`).
- Effect commands play the `.adx` versions: `03_音声/効果音/{se_一般効果音, j_番号付き効果音,
  m_番号付き効果音, eve_ジングル, sysse_システム音}/<name>.wav`. The same `j`/`m`/`se` name often
  exists both as `.ahx` (ボイス) and `.adx` (効果音); the game uses `.ahx` for a message line and
  `.adx` for an effect command, so `voice()` prefers `ボイス` and `se()` prefers `効果音`, each with the
  other as fallback. Scenario type `音声` (j/m numbers) goes through `se()`.
- Nine `.adx` voices sit in `03_音声/その他/ADX形式のボイス` (`may3000`..`may3005`, `kat0995`,
  `koj9999`, `mar9999`); `voice()` and `se()` find them.
- Effect names with underscores or letters: `se04_a`, `se04_b`, `se001a`, `se018b`, `eve_51a`..`eve_51d`.

## Same name in several PAKs

SYSTEM.PAK, KOJIROU.PAK and MARINA.PAK repeat files under the same name. The converter compared MD5
and wrote one file per name; no same-name pair differed. So a name resolves to exactly one file and the
PAK (the scenario JSON's `asset.pak`) does not matter for lookup. Sprites (94) and day-movies (5) are in
KOJIROU and MARINA; BGM (47) in all three; 156 + 144 event CGs in SYSTEM+KOJIROU / SYSTEM+MARINA
(SYSTEM holds them for the gallery); backgrounds are 40 Kojiro-only, 65 Marina-only, 50 both. Origins
of each file are in `00_目録/全ファイル一覧.tsv` (columns 元PAK, PAK内番号).

## Coverage

Run `python tools/eve_burst/assets.py <root>` (or `Assets(root).coverage()`). It scans all 87 scenario
JSONs (114,906 instructions) and reports, per type, references, distinct names and unresolved names
with count, the first scenarios using them, and whether `00_目録/全ファイル一覧.tsv` lists the file
(`in_catalog`: true means "should exist but is not on disk", false means a real gap).

Final check on the completely copied data folder (every file in the catalog is on disk): every
asset reference in all 87 scenarios resolves, 0 unresolved in every type.

| Type | References | Distinct names | Unresolved |
| --- | ---: | ---: | ---: |
| 背景 | 2,619 | 490 | 0 |
| 背景（端末画面） | 23 | 1 | 0 |
| 立ち絵 | 2,066 | 88 | 0 |
| サブCG | 146 | 26 | 0 |
| BGM | 1,019 | 43 | 0 |
| ボイス (lines' `voices`) | 16,229 | 15,503 | 0 |
| 効果音 | 884 | 79 | 0 |
| 音声 (j/m numbers by effect commands) | 229 | 84 | 0 |
| ムービー | 9 | 9 | 0 |
| 演出ムービー | 12 | 7 | 0 |

Notes:

- Voices: the catalog has 15,505 voice files; `koj9999` and `mar9999` exist but no scenario uses them.
- Movies: 9 event movies (`scene01`..`scene10`, no `scene04`) and 7 day movies (`1202`..`1207`, `1209`).
- 立ち絵: 88 of the 94 sprites are referenced; the 6 others are listed above.

Gaps worth knowing (not asset-resolution failures):

- `c01` (the "???" route of `a01`) does not exist on disk, so no scenario or asset for it.
- No mouth-open or blink still images (see Sprites); drawn ones are joined by name, see
  "Mouth and blink variants".
- `hack_k`/`hack_m` are the terminal screens: they are images only, the scenario passes a flag.
- Unverified: whether the `表情アニメ` clips contain a blink; whether sprite slot 1 is left and 3 is right.
