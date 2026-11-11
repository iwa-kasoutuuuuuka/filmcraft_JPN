# EVE burst error PLUS: 1 話を回す手順（episode_run.py）

個人の技術検証のメモ（利用者本人の解析ダンプが対象。**作品のテキストや音声はこのリポジトリに入れない**、公開しない）。
脚色済みの台本 1 話分から、声（感情別の参照による B1 合成）・立ち絵・効果音・読みの検品までを通して mp4 を作る。
Sonnet のセッションがこれを読んで、第 2 話以降を 1 話ずつ順に回すための手順書。背景は
[eve-burst-tts-quality.md](eve-burst-tts-quality.md) §10〜11、台本の形式は [script.md](script.md)。

## 1. 前提（毎回、始める前に確かめる）

| 項目 | 値・確かめ方 |
|---|---|
| GPU | RTX 5080 1 台（16 GB）。話は 1 本ずつが基本。急ぐときは**合成を 2 本まで**同時にできる（§7、どちらも `--ignore-gpu`）。3 本は VRAM に入らない。`nvidia-smi` で使用率と VRAM を見る |
| マイナー | GPU を使う別のプロセス（マイナーなど）が動いていたら、**利用者に止めてもらう**（自分では止めない）。LM Studio がモデルを載せていたら、外してよいか利用者に聞く |
| Python | Qwen3-TTS の venv: `E:/Zfilmcraft_JPN/tools/qwen3-tts/.venv/Scripts/python.exe`（`episode_run.py` 自体は標準ライブラリだけなので、どの Python でも動く） |
| FilmCraft | `E:/Zfilmcraft_JPN/target/agent-main/release/filmcraft-cli.exe`（release 版。debug 版より新しいか同じ時だけ既定で使われ、古ければ debug 版 `…/debug/filmcraft-cli.exe` になる。どちらを使ったかはログの 1 行目と `result.json` の `cli` に出る。作り直しは `CARGO_TARGET_DIR=target/agent-main cargo build --release -p filmcraft-cli`） |
| 画面の指定の元 | `E:/filmcraft_JPN/eve_out/routes_screen/{kojirou,marina}.fcscript.json`（`eve_to_script.py --screen` の出力。無ければ screen の段は飛ばす） |
| 認識器 | `E:/filmcraft_JPN/models/kotoba-whisper-v2.0-faster`、`E:/filmcraft_JPN/models/japanese-hubert-base-phoneme-ctc-v4` |
| 感情別の参照 | `E:/filmcraft_JPN/eve_out/voices/styles/styles.json`（`voice_styles.py pick` で作成済み、18 人分） |
| 表情番号の対応 | `E:/filmcraft_JPN/eve_out/tts_quality/eve_expressions.json`（茜: 06 怒り など） |
| ダンプ | `E:/filmcraft_JPN/PS2_EVE_Bursterror解析データ`（**読み取り専用**。書き込まない） |
| コミット | しない（別のセッションが品質ゲートの後に行う） |

### GPU の空き確認（始める前に毎回）

```bash
nvidia-smi --query-gpu=utilization.gpu,memory.used --format=csv,noheader   # 使用率と VRAM。空きなら数 % / 2 GB 前後
nvidia-smi pmon -c 1                                                       # GPU を使っているプロセス（type C = 計算）
```

`pmon` に `python` の C（計算）があり、使用率が高ければ、何のプロセスかを確かめる
（PowerShell: `Get-CimInstance Win32_Process -Filter "ProcessId=<pid>" | Select CommandLine`）。

`episode_run.py` も合成を始める直前に同じ確認をする: 使用率が 50 % 以上か VRAM の使用が 8 000 MiB 以上なら、
何もせずに止まる（「the GPU is in use by something else」）。確かめた上でそのまま進めるときだけ `--ignore-gpu` を付ける。
合成が要らない再実行（キャッシュが揃っていて組み立てと書き出しだけ）ではこの確認は行わない。

- `main.py --solo`（マイナー）や LM Studio（`llama-server.exe`）なら、利用者に止めてもらう。
- 別の `episode_run.py` / `filmcraft-cli`（前の話がまだ動いている）なら、それが終わるまで待つ。
- 自分では止めない。

## 2. コマンド

```bash
cd E:/Zfilmcraft_JPN/tools/eve_burst
python episode_run.py --episode 3 --dry-run      # 何が走るかを見る（何もしない）
python episode_run.py --episode 3                 # 本番。E:/filmcraft_JPN/eve_out/ep03/ にすべて置く
```

時間がかかる（1 話 45〜60 分、ほとんどが合成）ので、バックグラウンドで実行し、終わりの通知を待つ。進み具合は
`E:/filmcraft_JPN/eve_out/epNN/epNN.log` の末尾で分かる（`speak round 1: synthesized 220` → `misread lines 33` →
… → `result: ok=True`）。

**再開の仕方**: 途中で止まった（PC の再起動、プロセスの終了、失敗）ら、**同じコマンドをもう一度**実行する。成果物が入力より新しい
段階は飛ばし（ログに `up to date, skipped`）、合成は FilmCraft のキャッシュ（`epNN/tts/manifest.json`）で続きから始まる。
最初からやり直したいときだけ作業フォルダ `epNN/` ごと別名に退避してから実行する（消さない）。
ある段階だけやり直すときは `--force labels` のように指定する（その後の段階も、入力が新しくなるので自動でやり直しになる）。

手で直したい場合（感情ラベルの見直しなど）は、作業フォルダの段階ファイル（例 `ep02.6-labels.fcscript.json`）を直接直して保存し、
同じコマンドを再実行する。直したファイルより後の段階だけがやり直しになる。

## 3. 何をするか

作業フォルダ `E:/filmcraft_JPN/eve_out/epNN/` に、段階ごとのファイル `epNN.<n>-<段階>.fcscript.json` を残す。

| 段階 | 道具 | 内容 |
|---|---|---|
| fetch | – | 脚色済み台本を `eve_out/voiced/eve.epNN.adapted.voiced` → `voiced/eve.epNN.voiced` → `adapted/` → `series/` の順で探し、相対パスを作業フォルダ基準に付け直して写す |
| voices | `extras_voices.py --apply` | 声のないキャラに声を割り当てる（全員に声があれば写すだけ） |
| sprites | `protagonist_sprites.py` | 立ち絵のない主人公にゲームの立ち絵を入れる（あれば何もしない） |
| sounds | `onomatopoeia.py` | 擬音の行を「読まない・効果音にする」に印を付ける |
| variants | `sprite_variants.py` | 口パク・瞬きの差分を、切り貼り版 `E:/filmcraft_JPN/eve_sprites/` から入れる（2026-11-06 の利用者の決定でこの組だけを使う。Gemini 版 `eve_sprites_gemini/` は使わない） |
| labels | `emotion_labels.py --skip-neutral` | 感情ラベルを `instruct` に書く（`怒り（強）` など。「普通」は書かない） |
| readings | `tts_readings.py apply` | 名前とゲームの読み仮名をルビにする |
| styles | `voice_styles.py apply` | キャラの声に感情別の参照（`voice.styles`）を入れる |
| speak | `script.synthesize` ＋ `tts_readings.py check/apply` | 合成 → 誤読の検品 → 誤読語をルビ → 再合成、最大 3 周 |
| screen | `screen_fields.py` | ゲームが画面に出すものを、`--screen` 付きの変換結果から行 id で写す: ムービー（`video` の行）、サブ CG の小窓（`subCg`）、2 ショットの立ち位置（`slot`）、フェード（`transition`）、イベント CG の場面で立ち絵を出さない（`hideCharacters`）。音声は変わらない。出力は `epNN.10-screen.fcscript.json`、内訳は `epNN.screen.report.json`。台本の id が変換結果と合わない（別の変換から作った台本）ときは、写さずにそのまま進む |
| build | `anime.run` | 組み立て・検査・書き出し（`spriteAlign: bottom`, `spriteScale: 241`, `subCgScale: 241`）。プロジェクトを `epNN.fcproj` に保存 |
| result | – | `epNN.result.json` を書く |

**古い CLI では動かない。** filmcraft-cli が、台本の意味を決めるソース（`crates/project/src/script.rs`、`crates/engine/src/script_tools.rs`、
`crates/engine/src/anime.rs`、`crates/speech/src/synth.rs`）より古いと、何も作らずに止まる（「was built …, before … changed」）。
2026-11-09〜10 に第 2〜30 話を回したとき、既定の CLI が `voice.styles` の実装より前（11-06）のビルドのままで、
感情別の参照が無視され、全行が基本の参照 1 本で合成されていた（エラーは出ない。合成のジョブの `refAudio` が話者ごとに 1 種類しか
無いことで分かる）。それを繰り返さないための確認。作り直してから実行する。確かめた上で古い CLI を使うときだけ `--allow-stale-cli`。

始める前に、入力の台本に同じ行 id が 2 回出てこないかを確かめる（出てくれば、2 行が 1 つの音声ファイルを取り合うので止まる）。
`result.json` には入力の台本のパス（`source`）、その SHA-256（`sourceSha256`）、行数（`sourceLines`）を残すので、
後から「どの台本で作った mp4 か」を突き合わせられる。

## 4. 結果の見方（`epNN.result.json`）

| 項目 | 意味 | 正常 | 対応 |
|---|---|---|---|
| `ok` | 組み立てと書き出しが成功したか | `true` | `false` なら `failedStep` と `epNN.build.json.err` を見る |
| `mp4` | 書き出した動画 | パスがある | – |
| `seconds` | 尺（秒） | 1 話 10〜30 分程度 | 極端に短い・長いなら台本の取り違えを疑う |
| `checkErrors` / `checkWarnings` | `anime.check` のエラー・警告の数 | エラー 0 | 警告の中身は `epNN.build.json` の `check` 段（`problems`）。音量の警告は通常無視してよい |
| `misreadsLeft` | 3 周しても誤読と判定された行と語 | 空か数行 | 数字（助数詞つき）・人名なら台本の `reading` を手で直して再実行。多ければ Opus に回す |
| `labelsToReview` | 記号だけで付けた感情ラベルのうち、皮肉・からかいの言葉（ほぉ、へぇ、ったく…）がある行 | 十数行 | 下の「Opus に回す例外」 |
| `spritesWithoutMouth` | 口の差分がない立ち絵（その表情の間、口が動かない） | 既知の欠けだけ（下の一覧） | 一覧にないものは「Opus に回す例外」 |
| `spritesWithoutBlink` | 瞬きの画像がない立ち絵 | ほぼ全部（瞬きは今どの表情にもない） | 対応不要 |
| `screen` | screen の段が入れたもの: `videos`（置いたムービー）、`videosLeftOut`（前後の行が削られていて置かなかったもの）、`fades`、`hidden`（立ち絵を出さない場面）、`slots` / `freeSide`（立ち位置）、`insetsOn`（小窓） | 話によって 0 もある | `videosLeftOut` に話の山場のムービーがあれば、台本の該当箇所を見て手で `video` の行を足す |
| `source` / `sourceSha256` / `sourceLines` | 入力にした台本、その SHA-256、行数 | – | mp4 と台本の突き合わせに使う |
| `cli` | 使った filmcraft-cli | release 版 | debug 版なら書き出しが数倍遅い（release 版を作り直す） |
| `stagesRun` | 今回実行した段階 | – | 再実行で空なら、すべて最新 |

ログは `epNN.log`、各道具の出力は `epNN.*.json` / `*.txt`（標準エラーは同名の `.err`）。

## 5. Opus に回す例外

次のときは、自分で直さずに、話数と該当ファイルを添えて Opus のセッションに回す（その話はそこで止め、次の話に進んでよい）。

1. **感情ラベルの要確認行**（`labelsToReview` が 20 行を超える、または重要な場面。第 1 話は 17 行で、多くは小次郎の「ほぉ」）。記号では分からない演技（詰問・皮肉・
   からかい）の判断は LLM の読解が要る。Opus は `epNN.6-labels.fcscript.json` の `instruct` を直して再実行する。
2. **口の差分がない立ち絵が、既知の欠けの一覧にない**。既知の欠け（決定済みで、そのままにする）: 瞬きは全表情、
   小次郎（`koj*`）とまりな（`mar*`）の全表情、茜の 1 枚絵 `aka05ad`・`aka07ad`、第 2 話で出た `nik02ad`・`pur00dd`
   （同じ姿勢の別表情が無く、切り貼りで作れない）。それ以外が `spritesWithoutMouth` に出たら、差分画像の作成
   （`mouth_cutouts.py`、`docs/eve-burst-assets.md`）を Opus に回す。動画はそのままでも作れる（口が動かないだけ）ので、
   次の話に進んでよい。
3. **声のないキャラ**（`voices` 段で `extras_voices.py` が役名を知らないと言った、`checkWarnings` に「no voice」）。
4. **道具の想定外の失敗**（`episode_run: ...` で終わった、`ok: false`、Python の Traceback、CLI の exit が 0 以外）。
   同じコマンドを 1 回だけ再実行してよい（一時的な失敗のことがある）。2 回目も同じなら回す。
5. 誤読が 3 周しても 5 行以上残る、または合成の `failed` が 0 でない。

## 6. 第 2 話以降の順番

第 2 話は確認のために作成済み（`eve_out/ep02/ep02.mp4`、28.7 分、誤読 0、要確認ラベル 7 行）。第 3 話から話数順に 1 本ずつ。1 話終わるごとに、`result.json` の要点（ok、尺、誤読の残り、要確認の行数）を短く報告する。
第 30 話は `voiced/` に台本がなく `adapted/` から始まるので、`voices` 段で声の割り当てが走る（時間が少し延びる）。

## 7. 時間の目安

1 話の合成は約 280 行で、1 行 6 秒前後 → 約 30 分。検品 1 周 3 分、組み立てと書き出し約 15 分。合わせて 1 話 45〜60 分。
感情ラベルの付いた行は参照ごとに準備が要るが、キャラ×ラベルごとに 1 回だけ。

全 30 話の作り直し（2026-11-10〜11、release 版の CLI）で測った値: 1 話 26〜41 分（合成 17〜22 分、検品と読み直し 6〜20 分、
組み立てと書き出し 15 分）。ほかの作業が CPU を使っていると 57〜77 分に延びた。

**2 本同時に回すとき。** 合成 1 本は VRAM 約 5.3 GB、GPU 使用率 25 % 前後なので、2 本は入る（合わせて約 11 GB、使用率 45〜75 %）。
3 本は 16 GB を超える。書き出しは 1 本で CPU の全スレッドを使うので、書き出し同士を並べても合計の時間は変わらない。
得をするのは「片方が書き出している間に、もう片方が合成する」重なりだけで、残り 4 話で約 1 時間縮んだ。2 本目からは GPU の
空き確認に掛かるので `--ignore-gpu` を付け、同じ話を 2 つのプロセスが同時に回さないようにする（作業フォルダが同じになる）。
