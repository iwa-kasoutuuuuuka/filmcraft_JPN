<p align="center">
  <a href="https://getartcraft.com/">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="docs/brand/artcraft-logo-white.svg">
      <img alt="ArtCraft" src="docs/brand/artcraft-logo.svg" width="200">
    </picture>
  </a>
</p>

<h1 align="center">FilmCraft (日本語版 / filmcraft_JPN)</h1>

<p align="center">
  <b>Adobe Premiere Pro の操作体系をピュア Rust で再実装したオープンソース・クリーンルーム設計のノンリニア動画編集ソフトウェア（完全日本語対応・GPU 最適化版）</b><br>
  <sub>An open-source, clean-room non-linear video editor that reimplements Adobe Premiere Pro's workflow in pure Rust (fully Japanese-localized, GPU-optimized edition)</sub>
</p>

<p align="center">
  macOS、Windows、Linux、WebAssembly (ブラウザ) にネイティブ対応。<br>
  <sub>Runs natively on macOS, Windows and Linux, and in the browser via WebAssembly.</sub><br>
  開発: ArtCraft チーム · 日本語化と GPU 最適化: filmcraft_JPN<br>
  <sub>Developed by the ArtCraft team · Japanese localization and GPU optimization by filmcraft_JPN</sub>
</p>

<p align="center">
  <a href="#ライセンスとクレジット"><img alt="ライセンス: MIT または Apache-2.0 / License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20%7C%20Apache--2.0-8b5cf6"></a>
  <img alt="ピュア Rust で実装 / Written in pure Rust" src="https://img.shields.io/badge/pure-Rust-6a3fd6?logo=rust&logoColor=white">
  <img alt="macOS・Windows・Linux で動作 / Runs on macOS, Windows and Linux" src="https://img.shields.io/badge/runs%20on-macOS%20%7C%20Windows%20%7C%20Linux-8b5cf6">
  <img alt="UI: 完全日本語対応 / UI: fully Japanese" src="https://img.shields.io/badge/UI-完全日本語対応-22c55e">
  <img alt="GPU アクセラレーション / GPU acceleration" src="https://img.shields.io/badge/GPU-WebGPU%20%2F%20wgpu-f59e0b">
</p>

<p align="center">
  <a href="https://discord.gg/artcraft"><img alt="ArtCraft の Discord コミュニティに参加 / Join the ArtCraft community on Discord" src="https://img.shields.io/badge/Discord%20%E3%81%AB%E5%8F%82%E5%8A%A0%20%2F%20Join%20us-5865F2?style=for-the-badge&logo=discord&logoColor=white" height="40"></a>
</p>

<p align="center">
  <a href="#概要と特長">概要と特長 / Overview</a> ·
  <a href="#セットアップと起動方法">セットアップと起動 / Setup</a> ·
  <a href="#実践チュートリアル">実践チュートリアル / Tutorial</a> ·
  <a href="#主要パネルとワークスペース">パネル解説 / Panels</a> ·
  <a href="#高度な便利機能と操作ガイド">高度機能ガイド / Advanced features</a> ·
  <a href="#便利機能">追加便利機能 / Added features</a> ·
  <a href="#gpu最適化とハードウェアアクセラレーション">GPU 最適化 / GPU</a> ·
  <a href="#キーボードショートカット完全早見表">ショートカット早見表 / Shortcuts</a> ·
  <a href="#aiエージェント--mcp自動化連携">AI / MCP 連携 / AI &amp; MCP</a> ·
  <a href="#トラブルシューティング--faq">FAQ</a> ·
  <a href="#アーキテクチャと設計思想">設計・構造 / Architecture</a> ·
  <a href="#ライセンスとクレジット">ライセンス / License</a>
</p>

<p align="center">
  <sub>この README は日本語が本文です。各節の「English」を開くと英語版を読めます。<br>
  This README is written in Japanese. Expand "English" under each section for the English version.</sub>
</p>

<br>

<p align="center">
  <img src="docs/images/filmcraft-hero.png" alt="FilmCraft カラーワークスペース / FilmCraft Color workspace" width="100%">
</p>

---

## <a id="概要と特長"></a>📖 概要と特長 / Overview and features

**FilmCraft** は、Adobe Premiere Pro を使い慣れた映像クリエイターやプロフェッショナルのために設計された、オープンソースの次世代ノンリニア動画編集ソフトウェア（NLE）です。

従来の動画編集ソフトウェアが抱える「動作の重さ」「頻繁なクラッシュ」「高額なサブスクリプション」「ブラックボックス化された設計」を打破するため、ArtCraft チームによってゼロからピュア Rust で開発されました。

本リポジトリ（**`filmcraft_JPN`**）は、日本のクリエイター環境に完璧に適合するよう、**全UIの自然な日本語化（英語との切り替え対応）**、**日本語フォントの同梱**、および **WebGPU による超高速 GPU レンダリング・テクスチャプーリング最適化** を施した特別エディションです。

<details>
<summary>English</summary>

**FilmCraft** is an open-source, next-generation non-linear video editor (NLE) designed for video creators and professionals who are used to Adobe Premiere Pro.

It was built from scratch in pure Rust by the ArtCraft team to get rid of the problems of traditional video editors: sluggish performance, frequent crashes, expensive subscriptions and black-box design.

This repository (**`filmcraft_JPN`**) is a special edition tailored to Japanese creators, with **natural Japanese localization of the entire UI (switchable to English)**, **bundled Japanese fonts**, and **very fast WebGPU GPU rendering with texture-pooling optimizations**.

</details>

### 🌟 7つの革新的コア技術 / Seven innovative core technologies

1. **完全日本語ローカライズ (100% Japanese UI)**
   - メニューバー（ファイル、編集、クリップ、シーケンス、マーカー、表示、ウィンドウ等）、ツールボックス、タイムライン、デュアルモニター、エフェクトライブラリ、Lumetri カラー、環境設定、各種モーダルダイアログに至るまで、Premiere Pro 準拠の自然で分かりやすい日本語で統一されています。
   - 日本語フォント **BIZ UDPゴシック**（モリサワ、SIL OFL 1.1）を同梱。OS に日本語フォントが無くても、UI・タイトル・キャプションの日本語が文字化け（□ / トーフ）せず、どの環境でも同じ見た目で表示されます。
   - 表示言語は **設定 ➔ アピアランス ➔ 言語** で「自動 / 日本語 / English」から選べます。「自動」は OS の表示言語が日本語なら日本語、それ以外は英語になります。コマンド ID・オートメーション ID とラベル・プロジェクトファイルは、どの言語でも共通（英語）です。

2. **馴染みのある操作体系とレイアウト**
   - ソース/プログラムの 2 画面モニター、複数トラックタイムライン、ビンによる素材管理、エフェクトコントロールなど、業界標準のレイアウトを完全踏襲。
   - 「編集」「アセンブリ」「カラー」「エフェクト」「オーディオ」「キャプションとグラフィック」のワークスペースをワンクリックで切り替え可能です。

3. **厳格な「Never Crash」設計（AGENTS.md Rule 0 準拠）**
   - 編集作業中のクラッシュやデータ喪失を防止するため、パニックを誘発する記述（`unwrap()` や `expect()`）を本番コードから徹底排除し、安全な `Result<T, E>` 伝播を採用。
   - アトミックなファイル保存、定期バックアップ、および毎秒記録されるクラッシュリカバリージャーナルにより、万一の電源断でも作業状態を確実に復元します。

4. **WebGPU ハードウェアアクセラレーション & ゼロアロケーション再生**
   - WebGPU (`wgpu`) を活用したグラフィックスパイプラインを構築。
   - レイヤー合成・27 種のブレンドモード・Lumetri カラーグレーディング・各種ビデオエフェクトを GPU コンピュートシェーダー（WGSL）で並列処理。
   - テクスチャプールおよびユニフォームバッファプールにより、再生ループ中のメモリ確保コストを実質ゼロに抑え、4K/60fps の滑らかなリアルタイム再生を実現しています。

5. **ピュア Rust による高信頼性コーデック & 整数時間計算**
   - H.264、HEVC、ProRes、VP9、AAC などのコーデックを公開仕様書に基づいて独自実装（GPL/LGPL コード非依存のクリーンルーム設計）。
   - 1秒を 254,016,000,000 分割した超高精度な整数時間（Tick）ベースでタイムラインを管理。フレームレート変換やドロップフレーム環境でも音声同期（リップシンク）のズレが原理的に発生しません。

6. **先進的な次世代編集機能の標準搭載**
   - **テキストベース編集**: 音声文字起こし結果から不要な文や無音区間を一括削除。
   - **マルチカメラ編集**: 音声波形やタイムコードによる自動同期と、リアルタイムプレビュースイッチング。
   - **プロキシワークフロー**: 4K/8K 素材を軽量プロキシへワンクリック切り替え。
   - **ビジュアルショートカットエディタ**: グラフィカルキーボードによる直感的なキー割り当て。

7. **AI エージェント（MCP / 自動化）ネイティブ対応**
   - すべての UI 操作や内部コマンドは JSON-RPC / MCP (Model Context Protocol) 経由で外部の AI エージェントやスクリプトから非同期かつ安全にコントロール可能です。

<details>
<summary>English</summary>

1. **Full Japanese localization (100% Japanese UI)**
   - Everything, from the menu bar (File, Edit, Clip, Sequence, Markers, View, Window, etc.), the toolbox, the timeline, the dual monitors, the effects library, Lumetri Color and Preferences to the various modal dialogs, uses natural, easy-to-understand Japanese that follows Premiere Pro's terminology.
   - Bundles the Japanese font **BIZ UDPGothic** (Morisawa, SIL OFL 1.1). Even if the OS has no Japanese font, Japanese text in the UI, titles and captions never turns into garbage (□ / tofu) and looks the same in every environment.
   - Choose the display language under **Settings ➔ Appearance ➔ Language**: Auto / 日本語 / English. "Auto" uses Japanese if the OS display language is Japanese and English otherwise. Command IDs, automation IDs and labels, and project files are the same (English) in every language.

2. **Familiar controls and layout**
   - Faithfully follows the industry-standard layout: dual Source/Program monitors, a multi-track timeline, bins for media management, Effect Controls and more.
   - Switch between the Editing, Assembly, Color, Effects, Audio and Captions and Graphics workspaces with one click.

3. **Strict "Never Crash" design (per AGENTS.md Rule 0)**
   - To prevent crashes and data loss while editing, panic-prone code (`unwrap()` and `expect()`) is removed entirely from production code in favor of safe `Result<T, E>` propagation.
   - Atomic file saves, periodic backups and a crash-recovery journal written every second reliably restore your work even after a sudden power loss.

4. **WebGPU hardware acceleration & zero-allocation playback**
   - A graphics pipeline built on WebGPU (`wgpu`).
   - Layer compositing, 27 blend modes, Lumetri color grading and various video effects run in parallel as GPU compute shaders (WGSL).
   - Texture pools and uniform-buffer pools keep memory allocation in the playback loop at practically zero, giving smooth real-time 4K/60fps playback.

5. **Reliable pure-Rust codecs & integer time math**
   - Codecs such as H.264, HEVC, ProRes, VP9 and AAC are implemented in-house from public specifications (clean-room design with no dependency on GPL/LGPL code).
   - The timeline is managed in ultra-precise integer time (Tick), dividing one second into 254,016,000,000 units. Audio sync (lip sync) drift cannot happen by design, even with frame-rate conversion or drop-frame timecode.

6. **Advanced next-generation editing features built in**
   - **Text-based editing**: delete unwanted sentences and silent sections from a speech transcript in one go.
   - **Multi-camera editing**: automatic sync by audio waveform or timecode, with real-time preview switching.
   - **Proxy workflow**: switch 4K/8K media to lightweight proxies with one click.
   - **Visual shortcut editor**: intuitive key assignment on a graphical keyboard.

7. **Native support for AI agents (MCP / automation)**
   - Every UI action and internal command can be controlled asynchronously and safely by external AI agents and scripts via JSON-RPC / MCP (Model Context Protocol).

</details>

---

## <a id="セットアップと起動方法"></a>🛠 セットアップと起動方法 / Setup and launch

### 動作要件 / System requirements
- **OS**: Windows 10 / 11, macOS 12 Monterey 以降, Linux (Ubuntu 22.04+ 等)
- **CPU**: x86_64 または ARM64 (Apple Silicon M1/M2/M3/M4 完全対応)
- **GPU**: Vulkan, Metal, DirectX 12, または WebGPU 対応のグラフィックスカード
- **ビルドツール**: [Rust](https://www.rust-lang.org/tools/install) (1.80 以上推奨 / `rustup`)

<details>
<summary>English</summary>

- **OS**: Windows 10 / 11, macOS 12 Monterey or later, Linux (Ubuntu 22.04+, etc.)
- **CPU**: x86_64 or ARM64 (full Apple Silicon M1/M2/M3/M4 support)
- **GPU**: A graphics card supporting Vulkan, Metal, DirectX 12 or WebGPU
- **Build tools**: [Rust](https://www.rust-lang.org/tools/install) (1.80 or later recommended / `rustup`)

</details>

### リポジトリのクローンと起動手順 / Cloning the repository and launching

```powershell
# 1. リポジトリをクローン
git clone https://github.com/iwa-kasoutuuuuuka/filmcraft_JPN.git
cd filmcraft_JPN

# 2. アプリケーションのビルドと起動（GUIモード）
cargo run --release -p filmcraft

# （オプション）AI 連携用 MCP サーバーをビルドする場合（Claude Code / Antigravity から使う。詳細は「AI / MCP 連携」）
cargo build --release -p filmcraft-cli
```

> [!TIP]
> 初回ビルド時は依存クレートのダウンロードとコンパイルに数分かかりますが、2回目以降はビルドキャッシュにより瞬時に起動します。

<details>
<summary>English</summary>

```powershell
# 1. Clone the repository
git clone https://github.com/iwa-kasoutuuuuuka/filmcraft_JPN.git
cd filmcraft_JPN

# 2. Build and launch the application (GUI mode)
cargo run --release -p filmcraft

# (Optional) Build the MCP server for AI integration (used from Claude Code / Antigravity; see "AI / MCP integration")
cargo build --release -p filmcraft-cli
```

> [!TIP]
> The first build takes a few minutes to download and compile the dependency crates; after that, the build cache makes it start instantly.

</details>

---

## <a id="実践チュートリアル"></a>🎬 実践チュートリアル：最初の動画を完成させるまで / Hands-on tutorial: finishing your first video

FilmCraft を初めて使う方向けに、素材の読み込みからカット編集、カラー調整、動画の書き出しまでの一連の流れをステップ・バイ・ステップで解説します。

```text
[素材インポート] ➔ [ソースモニター(I/O)] ➔ [タイムライン配置] ➔ [カット・トリミング] ➔ [カラー・音声調整] ➔ [書き出し]
```

<details>
<summary>English</summary>

For first-time FilmCraft users, this walks step by step through the whole flow: importing media, cutting, color adjustment and exporting the video.

```text
[Import media] ➔ [Source Monitor (I/O)] ➔ [Place on timeline] ➔ [Cut & trim] ➔ [Color & audio adjustment] ➔ [Export]
```

</details>

### ステップ 1: 素材の読み込みと整理 / Step 1: Importing and organizing media
1. アプリを起動すると、上部タブの **「読み込み」** モードが開きます。
2. 画面上の **「ファイルを参照…」** ボタンをクリックするか、エクスプローラー（Finder）から動画・音声・画像をウィンドウ内へ直接ドラッグ＆ドロップします。
3. （※手元に動画がない場合は、画面内の **「デモ映像」** または **「デモプロジェクト」** を押すだけで、テスト用メディアが即座に生成されます。）
4. 画面上部の **「編集」** タブをクリックしてメイン編集画面へ遷移します。
5. プロジェクトパネルで右クリック ➔ **「新規ビン」**（または `Ctrl + B`）を作成すると、フォルダー形式で素材をすっきり整理できます。

<details>
<summary>English</summary>

1. When you launch the app, the **Import** mode in the top tabs opens.
2. Click the **Browse files…** button on screen, or drag and drop video, audio and images straight into the window from Explorer (Finder).
3. (If you have no video at hand, just press **Demo footage** or **Demo project** on screen and test media is generated instantly.)
4. Click the **Edit** tab at the top of the screen to go to the main editing screen.
5. Right-click in the Project panel ➔ **New Bin** (or `Ctrl + B`) to keep your media neatly organized in folders.

</details>

### ステップ 2: ソースモニターでの 3 点編集 (In / Out) / Step 2: Three-point editing in the Source Monitor (In / Out)
1. プロジェクトパネルの動画をダブルクリックすると、左上の **ソースモニター** に読み込まれます。
2. キーボードで映像を確認します：
   - `Space`: 再生 / 停止
   - `J` / `K` / `L`: シャトル再生（J: 逆再生, K: 停止, L: 早送り。連打すると 2x, 4x, 8x に加速）
   - `←` / `→`: 1フレームずつ前後にコマ送り
3. クリップの使用したい範囲を指定します：
   - `I` キー: **イン点**（開始位置）をマーク
   - `O` キー: **アウト点**（終了位置）をマーク
4. タイムラインへ配置します：
   - `,` (カンマ): **インサート**（再生ヘッド位置に割り込み挿入。後続クリップは自動で後ろへスライド）
   - `.` (ピリオド): **上書き**（再生ヘッド位置の既存クリップを上書き配置）

<details>
<summary>English</summary>

1. Double-click a video in the Project panel to load it into the **Source Monitor** at the top left.
2. Review the footage with the keyboard:
   - `Space`: Play / Stop
   - `J` / `K` / `L`: Shuttle playback (J: reverse, K: stop, L: fast forward; press repeatedly to speed up to 2x, 4x, 8x)
   - `←` / `→`: Step back or forward one frame
3. Mark the range of the clip you want to use:
   - `I` key: mark the **In point** (start)
   - `O` key: mark the **Out point** (end)
4. Place it on the timeline:
   - `,` (comma): **Insert** (inserts at the playhead; following clips slide later automatically)
   - `.` (period): **Overwrite** (places it over the existing clip at the playhead)

</details>

### ステップ 3: タイムライン編集テクニック / Step 3: Timeline editing techniques
- **クリップの選択と移動**:
  - `V` キーで **選択ツール** に切り替え、クリップをドラッグして自由なトラック・位置へ配置します。
- **無駄な部分のカット（レーザー & リップル）**:
  - `Ctrl + K`: 現在の再生ヘッド位置でクリップを一発分割。
  - `C` キー: **レーザーツール** に切り替え、クリックした場所を分割。
  - `Q` キー: 直前の編集点から再生ヘッド位置までを一瞬でリップル削除（間を自動で詰める）。
  - `W` キー: 再生ヘッド位置から次の編集点までを一瞬でリップル削除。
  - `Shift + Delete`: 選択したクリップを削除し、できた隙間を自動で詰める（リップル削除）。
- **スナップ機能**:
  - `S` キーを押すことで、クリップの端や再生ヘッドへの吸着（スナップ）の ON/OFF を切り替えられます。

<details>
<summary>English</summary>

- **Selecting and moving clips**:
  - Press `V` to switch to the **Selection Tool**, then drag clips to any track and position.
- **Cutting out unwanted parts (Razor & Ripple)**:
  - `Ctrl + K`: split the clip at the current playhead position in one step.
  - `C` key: switch to the **Razor Tool** and split where you click.
  - `Q` key: instantly ripple delete from the previous edit point to the playhead (closing the gap automatically).
  - `W` key: instantly ripple delete from the playhead to the next edit point.
  - `Shift + Delete`: delete the selected clips and automatically close the resulting gap (ripple delete).
- **Snapping**:
  - Press `S` to toggle snapping to clip edges and the playhead ON/OFF.

</details>

### ステップ 4: カラーグレーディング (Lumetri カラー) / Step 4: Color grading (Lumetri Color)
1. 画面上部のワークスペースバーから **「カラー」** を選択します。
2. タイムライン上のクリップを選択すると、右側に **Lumetri カラー** パネルが表示されます。
3. **基本補正**:
   - **露光量 (Exposure)**: 明るさ全体の引き上げ・引き下げ
   - **コントラスト (Contrast)**: 明暗のメリハリ
   - **ハイライト / シャドウ**: 白飛び・黒つぶれの微調整
   - **色温度 (Temperature) / 色合い (Tint)**: ホワイトバランスの補正
   - **彩度 (Saturation)**: 鮮やかさの調整
4. **クリエイティブ**:
   - **Look (LUT)** の選択、**フィルム調フェード**、**自然な彩度 (Vibrance)**。
5. **ビネット**:
   - 周辺光量を落として映画のようなフォーカス効果を与える演出。

> [!NOTE]
> FilmCraft では、これらの Lumetri パラメータが WebGPU コンピュートシェーダーによって毎フレーム並列計算されるため、再生中もコマ落ちなくリアルタイムにプレビュー可能です。

<details>
<summary>English</summary>

1. Select **Color** from the workspace bar at the top of the screen.
2. Select a clip on the timeline and the **Lumetri Color** panel appears on the right.
3. **Basic Correction**:
   - **Exposure**: raise or lower overall brightness
   - **Contrast**: the difference between lights and darks
   - **Highlights / Shadows**: fine-tune blown-out highlights and crushed blacks
   - **Temperature / Tint**: white balance correction
   - **Saturation**: adjust color intensity
4. **Creative**:
   - Choose a **Look (LUT)**, **Faded Film**, **Vibrance**.
5. **Vignette**:
   - Darken the edges of the frame for a cinematic, focused look.

> [!NOTE]
> In FilmCraft these Lumetri parameters are computed in parallel every frame by WebGPU compute shaders, so you can preview in real time without dropped frames, even during playback.

</details>

### ステップ 5: 音声の調整とノーマライズ / Step 5: Adjusting and normalizing audio
1. タイムラインの音声クリップを選択し、キーボードの `G` キーを押します。
2. **「オーディオゲイン」** ダイアログが開きます：
   - **「最大ピークをノーマライズ」**: クリップ内の最大音量を自動検出し、指定した dB（例: `-1.0 dB`）に揃えます。
   - **「ゲインの調整」**: 手動で音量を `+3 dB` や `-6 dB` など増減。
3. 「オーディオ」ワークスペースに切り替えると、リアルタイムの **ラウドネスメーター (LUFS / True Peak)** を確認しながらミックスが可能です。

<details>
<summary>English</summary>

1. Select an audio clip on the timeline and press `G` on the keyboard.
2. The **Audio Gain** dialog opens:
   - **Normalize Max Peak to**: automatically detects the loudest point in the clip and sets it to the given dB (e.g. `-1.0 dB`).
   - **Adjust Gain by**: manually raise or lower the volume, e.g. `+3 dB` or `-6 dB`.
3. Switch to the Audio workspace to mix while watching the real-time **loudness meter (LUFS / True Peak)**.

</details>

### ステップ 6: 高速動画書き出し (エクスポート) / Step 6: Fast video export
1. 画面右上の **「書き出し」** タブをクリック（またはショートカット `Ctrl + M`）。
2. 書き出し設定を指定：
   - **ファイル名と保存先**: 出力先フォルダーを選択。
   - **プリセット**: 「YouTube 1080p フル HD」「ソースの一致 - 高ビットレート」などから選択。
   - **形式**: H.264, Apple ProRes（422 / 4444 / 4444 XQ、4444 はアルファチャンネル付きも可）, DNxHR, 静止画連番, 音声のみ（WAV / AIFF / FLAC 16・24 bit ロスレス）など。
   - **書き出し範囲**: 「ソース全体」または「シーケンスのイン/アウト」。
3. 右下の **「書き出し」** ボタンを押すと、ピュア Rust エンコーダーによる高速バックグラウンドレンダリングが開始されます。

<details>
<summary>English</summary>

1. Click the **Export** tab at the top right of the screen (or use the shortcut `Ctrl + M`).
2. Choose the export settings:
   - **File name and location**: choose the output folder.
   - **Preset**: choose from "YouTube 1080p Full HD", "Match Source - High bitrate" and others.
   - **Format**: H.264, Apple ProRes (422 / 4444 / 4444 XQ; 4444 can also include an alpha channel), DNxHR, image sequence, audio only (WAV / AIFF / FLAC 16/24-bit lossless), and more.
   - **Range**: "Entire Source" or "Sequence In/Out".
3. Press the **Export** button at the bottom right to start fast background rendering with the pure-Rust encoders.

</details>

---

## <a id="主要パネルとワークスペース"></a>🎛 主要パネルとワークスペース / Main panels and workspaces

FilmCraft のパネルは、自由にドラッグ＆ドロップしてドッキング、分割、タブグループ化が可能です。

| パネル名 | 英語表記 | 主な役割 |
|---|---|---|
| **プロジェクト** | Project | 動画・音声・静止画・シーケンス・ビンの整理、検索、メタデータ確認 |
| **ソースモニター** | Source | 素材クリップの個別プレビュー、イン点・アウト点の設定 |
| **プログラムモニター** | Program | タイムラインで編集中のシーケンス映像のリアルタイム確認 |
| **タイムライン** | Timeline | 複数トラック（映像 V1〜 / 音声 A1〜）でのクリップ配置、トリミング、カット |
| **ツール** | Tools | 選択、リップル、レーザー、スリップ、ペン、文字などの編集ツール |
| **エフェクト** | Effects | ビデオ/オーディオエフェクト、トランジションのライブラリ |
| **エフェクトコントロール** | Effect Controls | モーション（位置・スケール・回転・不透明度）やキーフレームアニメーション |
| **Lumetri カラー** | Lumetri Color | 基本補正、LUT、カーブ、ホイール、ビネットなどの色調補正 |
| **テキスト** | Text | 自動文字起こし、無音カット、キャプション（字幕）生成と編集 |
| **オーディオメーター** | Audio Meters | マスター出力のリアルタイム音量、LUFS、ピークレベルの監視 |
| **ヒストリー** | History | 編集操作履歴の確認と、任意ステップへの無制限 Undo / Redo |

<details>
<summary>English</summary>

FilmCraft's panels can be freely dragged and dropped to dock, split and group into tabs.

| Panel name | English name | Main role |
|---|---|---|
| **Project** | Project | Organize, search and inspect metadata of video, audio, stills, sequences and bins |
| **Source Monitor** | Source | Preview individual source clips, set In and Out points |
| **Program Monitor** | Program | Real-time view of the sequence being edited on the timeline |
| **Timeline** | Timeline | Place, trim and cut clips on multiple tracks (video V1… / audio A1…) |
| **Tools** | Tools | Editing tools such as Selection, Ripple, Razor, Slip, Pen and Type |
| **Effects** | Effects | Library of video/audio effects and transitions |
| **Effect Controls** | Effect Controls | Motion (Position, Scale, Rotation, Opacity) and keyframe animation |
| **Lumetri Color** | Lumetri Color | Color correction: Basic Correction, LUTs, curves, wheels, vignette and more |
| **Text** | Text | Automatic transcription, silence removal, caption (subtitle) generation and editing |
| **Audio Meters** | Audio Meters | Monitor real-time master output level, LUFS and peak levels |
| **History** | History | Review the edit history, with unlimited Undo / Redo to any step |

</details>

### ワークスペース一覧 (`Alt + Shift + 1..6`) / Workspaces (`Alt + Shift + 1..6`)
- **編集 (Editing)** (`Alt + Shift + 1`): 最もバランスの良い標準カット編集レイアウト。
- **アセンブリ (Assembly)** (`Alt + Shift + 2`): 素材の確認と粗編集（並べ替え）に特化した大画面レイアウト。
- **カラー (Color)** (`Alt + Shift + 3`): Lumetri カラーとスコープを中心に配置したグレーディング環境。
- **エフェクト (Effects)** (`Alt + Shift + 4`): エフェクトライブラリとエフェクトコントロールを広く配置。
- **オーディオ (Audio)** (`Alt + Shift + 5`): トラックミキサーとメーターを大きく表示した音響調整環境。
- **キャプションとグラフィック** (`Alt + Shift + 6`): テロップ入れ、テキスト編集、字幕同期に最適な環境。

<details>
<summary>English</summary>

- **Editing** (`Alt + Shift + 1`): the most balanced, standard cutting layout.
- **Assembly** (`Alt + Shift + 2`): a large-screen layout focused on reviewing media and rough cutting (reordering).
- **Color** (`Alt + Shift + 3`): a grading environment centered on Lumetri Color and the scopes.
- **Effects** (`Alt + Shift + 4`): gives plenty of room to the effects library and Effect Controls.
- **Audio** (`Alt + Shift + 5`): an audio environment with a large Track Mixer and meters.
- **Captions and Graphics** (`Alt + Shift + 6`): ideal for adding on-screen text, editing text and syncing subtitles.

</details>

---

## <a id="高度な便利機能と操作ガイド"></a>🚀 高度な便利機能と操作ガイド / Advanced features and how-to guide

### 1. テキストベース編集 ＆ AI自動文字起こし（無音・フィラー自動削除） / 1. Text-based editing & AI transcription (automatic silence and filler removal)
インタビュー動画やYouTubeのトーク動画、ポッドキャスト編集の作業時間を最大 80% 削減できる強力な機能です。

```text
[音声認識] ➔ [テキスト段落化] ➔ [無音区間を削除(0.5s)] ➔ [フィラー除去] ➔ [字幕一括生成]
```

- **文字起こしの実行**:
  - ウィンドウ ➔ **「テキスト」** パネルを開き、**「文字起こし」** タブを選択。
  - シーケンス内の音声を自動認識し、話者ごとのテキスト段落としてタイムラインと完全同期して表示されます。
- **無音区間のワンクリック自動カット**:
  - ツールバーの **「無音区間を削除」**（時計アイコン）をクリック。
  - 設定されたしきい値（デフォルト: 0.5秒以上の休止）を超える無音部分を一括検出し、タイムライン上の全トラックを連動させて自動リップル削除（ギャップを詰める）します。
- **フィラーワード（つなぎ言葉）の削除**:
  - **「フィラーワードを削除」**（リンクアイコン）をクリックすることで、「あー」「えー」「そのー」などの不要なつなぎ言葉を一発で検出・カット可能。
- **言い直し（同じフレーズの撮り直し）の削除**:
  - **「言い直しを削除」**（ループアイコン）をクリックすると、「今日は、今日はですね」「the the」のように言いかけて言い直した部分を検出し、最後に言い直したテイクだけを残して前のテイクを詰めます。
- **テキストを消して動画をカット**:
  - テキスト上で不要な単語や文章をドラッグ選択し、ツールバーの **「抽出（ギャップを詰める）」** または **「リフト（空白を残す）」** をクリックするだけで、タイムラインの該当箇所が瞬時にカットされます。
- **ワンクリック字幕（キャプション）生成**:
  - ツールバーの **「キャプションを作成」** をクリックすると、文字起こし結果から自動的にキャプショントラック（C1）へ字幕ブロックが生成されます。
  - 下部のスタイルバーから、フォント・サイズ・文字色・境界線・背景ボックス・画面配置（下部中央など）を一括適用できます。

<details>
<summary>English</summary>

A powerful feature that cuts editing time for interview videos, YouTube talk videos and podcasts by up to 80%.

```text
[Speech recognition] ➔ [Text paragraphs] ➔ [Remove silences (0.5s)] ➔ [Remove fillers] ➔ [Generate captions in bulk]
```

- **Transcribing**:
  - Open Window ➔ the **Text** panel and select the **Transcript** tab.
  - The audio in the sequence is recognized automatically and shown as text paragraphs per speaker, fully synced with the timeline.
- **One-click automatic silence cutting**:
  - Click **Remove Silences** (clock icon) in the toolbar.
  - Detects all silent parts beyond the configured threshold (default: pauses of 0.5 s or longer) and ripple-deletes them automatically (closing the gaps) across all tracks on the timeline in sync.
- **Removing filler words**:
  - Click **Remove Filler Words** (link icon) to detect and cut unneeded fillers such as "uh", "um" and "well" in one go.
- **Removing repeated takes (re-said phrases)**:
  - Click **Remove Repeats** (loop icon) to detect places where the speaker started a phrase and said it again, such as 「今日は、今日はですね」 or "the the"; only the last take is kept and the earlier takes are closed up.
- **Cut the video by deleting text**:
  - Drag-select unwanted words or sentences in the text and just click **Extract (close gap)** or **Lift (leave gap)** in the toolbar; the matching part of the timeline is cut instantly.
- **One-click caption generation**:
  - Click **Create Captions** in the toolbar to generate caption blocks automatically on the caption track (C1) from the transcript.
  - From the style bar at the bottom, apply font, size, text color, stroke, background box and on-screen position (e.g. bottom center) all at once.

</details>

---

### 2. マルチカメラ編集ワークフロー / 2. Multi-camera editing workflow
複数台のカメラで同時収録されたライブイベント、インタビュー、対談動画のスイッチング編集を直感的に行えます。

- **複数素材の自動同期**:
  - プロジェクトパネルで各カメラのクリップを複数選択し、右クリック ➔ **「クリップを同期」** または **「マルチカメラソースシーケンスを作成」** を選択。
  - 同期基準として以下から選択できます：
    - **オーディオ**: 各カメラの音声波形の一致点を自動解析して瞬時にズレなく同期（おすすめ）。
    - **タイムコード**: 同期タイムコード（LTC）をもとに整列。
    - **イン点 / アウト点 / クリップマーカー**: 手動で打った目印で同期。
- **マルチカメラプレビューとリアルタイム切り替え**:
  - プログラムモニターのレンチメニューから **「マルチカメラ表示」** をオンにすると、全カメラアングルが並んだグリッド表示（2×2、3×3 等）に切り替わります。
  - スペースキーで再生しながら、目的のカメラ画面をクリック（またはテンキーの `1`〜`9`）するだけで、再生に合わせてリアルタイムにカットが切り替わり、タイムラインに自動的に編集点が記録されます（**マルチカメラ録画モード**）。
- **カメラアングルの編集**:
  - **「カメラを編集」** ダイアログから、カメラの表示順序の入れ替え、不要アングルの非表示、カメラ名（「Aカメ（寄り）」「Bカメ（引き）」など）の設定が可能です。

<details>
<summary>English</summary>

Switch between angles intuitively when editing live events, interviews and conversation videos recorded simultaneously on multiple cameras.

- **Automatic sync of multiple clips**:
  - Select the clips from each camera in the Project panel, right-click ➔ choose **Synchronize** or **Create Multi-Camera Source Sequence**.
  - You can choose the sync point from:
    - **Audio**: automatically analyzes where the cameras' audio waveforms match and syncs them instantly with no drift (recommended).
    - **Timecode**: aligns using the sync timecode (LTC).
    - **In point / Out point / Clip marker**: syncs on marks you set manually.
- **Multi-camera preview and real-time switching**:
  - Turn on **Multi-Camera** view from the Program Monitor's wrench menu to switch to a grid showing all camera angles (2×2, 3×3, etc.).
  - While playing with the space bar, just click the camera you want (or press `1`–`9` on the numeric keypad); the cut switches in real time with playback and edit points are recorded on the timeline automatically (**multi-camera record mode**).
- **Editing camera angles**:
  - In the **Edit Cameras** dialog you can reorder cameras, hide unneeded angles and set camera names (such as "Cam A (close)" or "Cam B (wide)").

</details>

---

### 3. プロキシ＆オフラインメディア管理 / 3. Proxies & offline media management
高ビットレートな 4K/8K RAW 素材や重いコーデックでも、軽量プロキシを活用することでノート PC でも軽快に編集作業が行えます。

- **プロキシの作成**:
  - プロジェクトパネルで対象素材を右クリック ➔ **「プロキシ」** ➔ **「プロキシを作成…」**。
  - 形式（Apple ProRes Proxy または H.264）と解像度（1/2 または 1/4）を選択すると、バックグラウンドで安全に生成され、自動的にオリジナルクリップへ紐付けられます。
- **プロキシのワンクリック切り替え**:
  - プログラムモニター下の **「プロキシの切り替え」** ボタン（またはプロジェクト一覧の「P」バッジ）を押すだけで、編集用の軽量表示とオリジナル高画質表示を一瞬で切り替えられます。
  - 最終書き出し（エクスポート）時は、自動的に最高画質のオリジナルファイルが参照されます。
- **メディアの再リンクとオフライン管理**:
  - ドライブレターの変更やフォルダー移動でリンクが切れた素材も、右クリック ➔ **「メディアにリンク…」** でファイル名・拡張子・デュレーションをもとに自動検索し、一括で再接続できます。
- **プロジェクトマネージャー**:
  - ファイル ➔ **「プロジェクトマネージャー」** を開くと、使用している素材だけを別フォルダーへ収集・コピーしたり、不要な部分をトリミング（ハンドル余白付き）して軽量な保管用アーカイブを作成できます。

<details>
<summary>English</summary>

Even with high-bitrate 4K/8K RAW footage or heavy codecs, lightweight proxies let you edit smoothly on a laptop.

- **Creating proxies**:
  - Right-click the media in the Project panel ➔ **Proxy** ➔ **Create Proxies…**.
  - Choose a format (Apple ProRes Proxy or H.264) and a resolution (1/2 or 1/4); proxies are generated safely in the background and attached to the original clips automatically.
- **One-click proxy toggle**:
  - Just press the **Toggle Proxies** button below the Program Monitor (or the "P" badge in the project list) to switch instantly between the lightweight editing view and the original full-quality view.
  - The final export automatically uses the full-quality original files.
- **Relinking media and managing offline media**:
  - Media whose links broke because of a drive letter change or a moved folder can be found automatically by file name, extension and duration via right-click ➔ **Link Media…**, and reconnected in bulk.
- **Project Manager**:
  - Open File ➔ **Project Manager** to collect and copy only the media in use into another folder, or trim away unused parts (with handles) to create a lightweight archive.

</details>

---

### 4. ビジュアル・キーボードショートカット編集 / 4. Visual keyboard shortcut editor
- `Ctrl + Alt + K`（macOS: `Option + Command + K`）で **「キーボードショートカット」** ダイアログが起動します。
- **ビジュアルキーボード**:
  - 画面上にフルキーボードが描画され、アプリケーション全体で割り当てられているキー（紫色）、特定のパネルに特化したキー（緑色）、空いているキー（灰色）が色分けでひと目でわかります。
- **キーの割り当て**:
  - コマンドリストから割り当てたい機能を検索し、ショートカット列をクリックしてキーボードのキーを押すだけで即座に登録。
  - 既存のキーと重複した場合は、競合警告と再割り当て先が分かりやすく案内されます。
- **プリセットの管理**:
  - 独自のキー設定をプリセット名をつけて保存可能。JSON ファイルとしてのエクスポート／インポートにも対応しており、チーム内でのショートカット共有も容易です。

<details>
<summary>English</summary>

- `Ctrl + Alt + K` (macOS: `Option + Command + K`) opens the **Keyboard Shortcuts** dialog.
- **Visual keyboard**:
  - A full keyboard is drawn on screen, color-coded so you can see at a glance which keys are assigned application-wide (purple), which are specific to a panel (green) and which are free (gray).
- **Assigning keys**:
  - Search the command list for the function you want, click its shortcut column and press a key on the keyboard to register it immediately.
  - If it clashes with an existing key, a conflict warning clearly shows you where it is assigned and lets you reassign it.
- **Managing presets**:
  - Save your own key settings under a preset name. Export/import as a JSON file is supported too, making it easy to share shortcuts within a team.

</details>

---

## <a id="gpu最適化とハードウェアアクセラレーション"></a>⚡ GPU 最適化とハードウェアアクセラレーション / GPU optimization and hardware acceleration

本リポジトリでは、フレームレートの安定化と GPU メモリ効率を極限まで高めるため、以下の最適化を実装しています。

```text
[CPU: デコード & Tick 管理] ➔ [GPU テクスチャプール] ➔ [WGSL Lumetri Compute] ➔ [27種ブレンド合成] ➔ [sRGB ディスプレイ出力]
                                       ↑ (回収・再利用)
```

<details>
<summary>English</summary>

This repository implements the following optimizations to keep the frame rate stable and push GPU memory efficiency as far as possible.

```text
[CPU: decode & Tick management] ➔ [GPU texture pool] ➔ [WGSL Lumetri Compute] ➔ [27 blend modes compositing] ➔ [sRGB display output]
                                       ↑ (reclaim & reuse)
```

</details>

### 1. テクスチャプーリング・リサイクル（ゼロアロケーション再生ループ） / 1. Texture pooling and recycling (zero-allocation playback loop)
- **課題**: 従来の再生ループでは、毎フレームのデコード画像に対して新規に `wgpu::Texture` を確保・破棄しており、ドライバ呼び出しオーバーヘッドと GPU メモリ断片化によるフレームドロップが発生していました。
- **実装内容**: `GpuCompositor` に解像度・フォーマット別の LRU テクスチャプールを導入。LRU キャッシュから退避されたテクスチャおよびビューを自動的にプールへ回収し、次フレームで再利用（`write_texture` のみ発行）することで、再生中の GPU リソース確保コストを実質ゼロに抑えています。
- **アクティブフレーム保護**: 多重レイヤーやトランジション合成時、現在フレームで利用中のテクスチャが LRU 回収されて上書きされるのを防ぐフレーム保護インバリアント（`frame_start`）を導入。

<details>
<summary>English</summary>

- **Problem**: The old playback loop allocated and freed a new `wgpu::Texture` for every decoded frame, causing dropped frames from driver-call overhead and GPU memory fragmentation.
- **Implementation**: Added an LRU texture pool per resolution and format to `GpuCompositor`. Textures and views evicted from the LRU cache are returned to the pool automatically and reused on the next frame (issuing only `write_texture`), bringing the cost of GPU resource allocation during playback down to effectively zero.
- **Active-frame protection**: Added a frame-protection invariant (`frame_start`) that prevents textures in use by the current frame from being reclaimed by the LRU and overwritten when compositing multiple layers or transitions.

</details>

### 2. レイヤー・エフェクトユニフォームバッファのプーリングとヒープ確保ゼロ化 / 2. Pooling layer and effect uniform buffers with zero heap allocation
- **課題**: レイヤー合成ごとに 112 バイトのユニフォームバッファ、およびエフェクトコンピュートパスごとに 96 バイトのユニフォームバッファが毎フレーム新規作成（`device.create_buffer`）され、60fps 再生時に秒間数百回の GPU メモリ確保が発生していました。
- **実装内容**: `GpuCompositor` および `FxStage` にユニフォームバッファプール（`uniform_pool`）を導入。スタック上の固定長バイト配列（`[u8; 112]` / `[u8; 96]`）によりヒープアロケーションをゼロにし、フレーム完了後にバッファをプールへ即座に回収・再利用。再生中のユニフォームバッファ確保を完全ゼロに最適化しました。

<details>
<summary>English</summary>

- **Problem**: A 112-byte uniform buffer per layer composite and a 96-byte uniform buffer per effect compute pass were created anew every frame (`device.create_buffer`), causing hundreds of GPU memory allocations per second during 60fps playback.
- **Implementation**: Added a uniform buffer pool (`uniform_pool`) to `GpuCompositor` and `FxStage`. Fixed-size byte arrays on the stack (`[u8; 112]` / `[u8; 96]`) eliminate heap allocation, and buffers are returned to the pool and reused as soon as the frame completes. Uniform buffer allocation during playback is now fully zero.

</details>

### 3. Lumetri Color の WGSL コンピュートパイプライン統合 / 3. Lumetri Color in the WGSL compute pipeline
- **課題**: これまで Lumetri Color はクリップに適用されると CPU レンダリング（マルチスレッド rayon）へフォールバックし、1 フレームあたり数十〜数百ミリ秒の負荷がかかってプレビュー再生がコマ落ちしていました。
- **実装内容**: `gpufx` および `fx.wgsl` に `OP_LUMETRI` コンピュートパスを統合。
  - **基本補正**: 露光量、コントラスト、ハイライト、シャドウ、白レベル、黒レベル、色温度、色合い、彩度
  - **クリエイティブ**: 自然な彩度 (Vibrance)、色褪せたフィルム (Faded Film)
  - **ビネット**: 適用量、中間点、真円度、ぼかし
  - 厳格な 16 バイト境界（96 バイト構造体 `struct U`）で Uniform データを WGSL シェーダーへ転送し、16×16 ワークグループでピクセル並列計算。CPU リファレンスと完全一致するカラー精度を維持したまま、数ミリ秒以内の超高速 GPU プレビューを実現しています。

<details>
<summary>English</summary>

- **Problem**: Previously, applying Lumetri Color to a clip fell back to CPU rendering (multithreaded rayon), costing tens to hundreds of milliseconds per frame and making preview playback drop frames.
- **Implementation**: Integrated an `OP_LUMETRI` compute pass into `gpufx` and `fx.wgsl`.
  - **Basic Correction**: Exposure, Contrast, Highlights, Shadows, Whites, Blacks, Temperature, Tint, Saturation
  - **Creative**: Vibrance, Faded Film
  - **Vignette**: Amount, Midpoint, Roundness, Feather
  - Uniform data is passed to the WGSL shader with strict 16-byte alignment (the 96-byte struct `struct U`), and pixels are computed in parallel in 16×16 workgroups. This delivers very fast GPU preview within a few milliseconds while keeping color accuracy identical to the CPU reference.

</details>

### 4. 27 種類の全ブレンドモードとエフェクトの GPU コンポジット / 4. GPU compositing of all 27 blend modes and effects
- Premiere Pro 互換の全 27 種類のブレンドモード（通常、ディゾルブ、乗算、スクリーン、オーバーレイ、ソフトライト、ハードライト、差の絶対値等）を GPU フラグメントシェーダーで高速合成。
- ガウスブラー、方向ブラー、アンシャープマスク、変形、クロップ、モザイク等の主要ビデオエフェクトおよびマスク処理も GPU コンピュートパスで実行されます。

<details>
<summary>English</summary>

- All 27 Premiere Pro-compatible blend modes (Normal, Dissolve, Multiply, Screen, Overlay, Soft Light, Hard Light, Difference, etc.) are composited quickly in GPU fragment shaders.
- Major video effects such as Gaussian Blur, Directional Blur, Unsharp Mask, Transform, Crop and Mosaic, as well as mask processing, also run in GPU compute passes.

</details>

### 5. ハードウェアデコード / エンコード / 5. Hardware decoding / encoding
- **設定 ➔ 再生 ➔ ハードウェアデコード**（`playback.hardwareDecoding`）で **自動 / オフ** を選べます。
- 対応する OS ネイティブのデコーダー:
  - **macOS**: VideoToolbox（H.264 / HEVC、8 / 10 bit、4:2:0 / 4:2:2）
  - **Windows**: Media Foundation（Direct3D 11 / DXVA。H.264 8 bit、HEVC 8 / 10 bit。HEVC は「HEVC ビデオ拡張機能」が必要）
  - **Linux**: 未対応（独自のソフトウェアデコーダーで再生します）
- ハードウェアで扱えないストリームや、途中で失敗したストリームは、自動で独自のソフトウェアデコーダーに切り替わります（画が変わることはありません）。
- **Windows の書き出し**: 書き出し ➔ パフォーマンスで「ハードウェアエンコーディング」を選ぶと、GPU のメディアエンジンで H.264 / HEVC（HDR シーケンスは HEVC Main 10）を書き出します。失敗した場合は独自のエンコーダーで書き出しを続けます。

<details>
<summary>English</summary>

- Choose **Auto / Off** under **Settings ➔ Playback ➔ Hardware decoding** (`playback.hardwareDecoding`).
- Supported OS-native decoders:
  - **macOS**: VideoToolbox (H.264 / HEVC, 8 / 10 bit, 4:2:0 / 4:2:2)
  - **Windows**: Media Foundation (Direct3D 11 / DXVA. H.264 8 bit, HEVC 8 / 10 bit. HEVC requires the "HEVC Video Extensions")
  - **Linux**: not supported (playback uses our own software decoders)
- Streams the hardware can't handle, or that fail partway, switch automatically to our own software decoders (the picture doesn't change).
- **Exporting on Windows**: choose "Hardware Encoding" under Export ➔ Performance to export H.264 / HEVC (HEVC Main 10 for HDR sequences) with the GPU's media engine. If that fails, the export continues with our own encoder.

</details>

---

## <a id="キーボードショートカット完全早見表"></a>⌨️ キーボードショートカット完全早見表 / Complete keyboard shortcut reference

Premiere Pro 標準キーバインドに完全準拠しています。`Ctrl + Alt + K` でキーボードショートカット設定画面を開き、自由にカスタマイズも可能です。

<details>
<summary>English</summary>

Fully follows Premiere Pro's default key bindings. Press `Ctrl + Alt + K` to open the keyboard shortcut settings and customize them freely.

</details>

### 1. 再生・ナビゲーション / 1. Playback and navigation
| ショートカット | 機能説明 |
|---|---|
| `Space` | 再生 / 一時停止 |
| `L` | 早送り（シャトル右。連打で 2x, 4x, 8x に加速） |
| `K` | 停止（シャトルストップ） |
| `J` | 逆再生（シャトル左。連打で逆倍速） |
| `Home` / `End` | シーケンスの先頭 / 末尾へジャンプ |
| `PageUp` / `PageDown` | 前 / 次の編集点（カット位置）へスナップ移動 |
| `←` / `→` | 1フレーム前 / 次へコマ送り |
| `Shift + ←` / `Shift + →` | 5フレーム前 / 次へ移動 |
| `\` (バックスラッシュ) | タイムライン全体を画面幅にフィット表示 |
| `=` / `-` | タイムラインの水平ズームイン / ズームアウト |

<details>
<summary>English</summary>

| Shortcut | Function |
|---|---|
| `Space` | Play / Pause |
| `L` | Play forward (shuttle right; press repeatedly to speed up to 2x, 4x, 8x) |
| `K` | Stop (shuttle stop) |
| `J` | Play in reverse (shuttle left; press repeatedly for faster reverse) |
| `Home` / `End` | Jump to the start / end of the sequence |
| `PageUp` / `PageDown` | Snap to the previous / next edit point (cut) |
| `←` / `→` | Step back / forward 1 frame |
| `Shift + ←` / `Shift + →` | Move back / forward 5 frames |
| `\` (backslash) | Zoom the whole timeline to fit the window width |
| `=` / `-` | Zoom the timeline in / out horizontally |

</details>

### 2. 編集・トリミング / 2. Editing and trimming
| ショートカット | 機能説明 |
|---|---|
| `I` / `O` | イン点をマーク / アウト点をマーク |
| `Shift + I` / `Shift + O` | イン点へ移動 / アウト点へ移動 |
| `Ctrl + Shift + I` / `Ctrl + Shift + O` | イン点を消去 / アウト点を消去 |
| `,` (カンマ) | **インサート**（再生ヘッド位置に割り込み挿入） |
| `.` (ピリオド) | **上書き**（再生ヘッド位置の既存クリップを上書き配置） |
| `;` (セミコロン) | **リフト**（選択範囲を削除し、隙間を残す） |
| `'` (シングルクォート) | **抽出**（選択範囲を削除し、隙間を詰める） |
| `Ctrl + K` | **編集点を追加**（現在位置でクリップをカット） |
| `Ctrl + Shift + K` | 全トラックに編集点を追加 |
| `Q` | 直前の編集点から再生ヘッド位置までリップル削除 |
| `W` | 再生ヘッド位置から次の編集点までリップル削除 |
| `Delete` / `Backspace` | 選択クリップを消去（隙間はそのまま） |
| `Shift + Delete` | **リップル削除**（選択クリップを消去し、隙間を自動で詰める） |
| `S` | タイムラインのスナップ（吸着）有効/無効切り替え |

<details>
<summary>English</summary>

| Shortcut | Function |
|---|---|
| `I` / `O` | Mark In / Mark Out |
| `Shift + I` / `Shift + O` | Go to In / Go to Out |
| `Ctrl + Shift + I` / `Ctrl + Shift + O` | Clear In / Clear Out |
| `,` (comma) | **Insert** (insert at the playhead, pushing clips later) |
| `.` (period) | **Overwrite** (place over the existing clips at the playhead) |
| `;` (semicolon) | **Lift** (remove the selected range, leaving a gap) |
| `'` (single quote) | **Extract** (remove the selected range and close the gap) |
| `Ctrl + K` | **Add Edit** (cut the clip at the current position) |
| `Ctrl + Shift + K` | Add Edit to all tracks |
| `Q` | Ripple delete from the previous edit point to the playhead |
| `W` | Ripple delete from the playhead to the next edit point |
| `Delete` / `Backspace` | Clear the selected clips (the gap stays) |
| `Shift + Delete` | **Ripple Delete** (clear the selected clips and close the gap automatically) |
| `S` | Toggle timeline snapping on/off |

</details>

### 3. ツール切り替え / 3. Switching tools
| ショートカット | ツール名 |
|---|---|
| `V` | **選択ツール**（標準の選択・ドラッグ移動） |
| `A` | **トラックの前方選択ツール**（クリック位置以降の全クリップを選択） |
| `Shift + A` | **トラックの後方選択ツール**（クリック位置以前の全クリップを選択） |
| `B` | **リップル編集ツール**（後続を詰めながらクリップ長を調整） |
| `N` | **ローリング編集ツール**（隣接クリップの境界点のみを移動） |
| `R` | **レート調整ツール**（クリップ長を伸縮して再生速度を変更） |
| `C` | **レーザーツール**（クリックした位置をカット分割） |
| `Y` | **スリップツール**（クリップ長を保ったまま素材のイン/アウト位置を変更） |
| `U` | **スライドツール**（クリップ長を保ったまま前後の隣接クリップを伸縮） |
| `P` | **ペンツール**（キーフレームの作成やマスクの描画） |
| `H` | **手のひらツール**（タイムラインやモニターのスクロール移動） |
| `Z` | **ズームツール**（拡大・縮小表示） |
| `T` | **文字ツール**（画面上へのテキスト入力） |

<details>
<summary>English</summary>

| Shortcut | Tool |
|---|---|
| `V` | **Selection Tool** (standard selection and drag-to-move) |
| `A` | **Track Select Forward Tool** (selects all clips from the click point onward) |
| `Shift + A` | **Track Select Backward Tool** (selects all clips up to the click point) |
| `B` | **Ripple Edit Tool** (adjusts clip length while closing up the following clips) |
| `N` | **Rolling Edit Tool** (moves only the boundary between adjacent clips) |
| `R` | **Rate Stretch Tool** (stretches clip length to change playback speed) |
| `C` | **Razor Tool** (splits the clip at the clicked point) |
| `Y` | **Slip Tool** (changes the source In/Out while keeping clip length) |
| `U` | **Slide Tool** (stretches the adjacent clips before and after while keeping clip length) |
| `P` | **Pen Tool** (creates keyframes and draws masks) |
| `H` | **Hand Tool** (scrolls the timeline or monitor) |
| `Z` | **Zoom Tool** (zooms in and out) |
| `T` | **Type Tool** (enters text on screen) |

</details>

### 4. ファイル・ウィンドウ・システム操作 / 4. File, window and system operations
| ショートカット | 機能説明 |
|---|---|
| `Ctrl + N` | 新規シーケンスの作成 |
| `Ctrl + O` | プロジェクトを開く |
| `Ctrl + S` | プロジェクトを上書き保存 |
| `Ctrl + Shift + S` | プロジェクトを別名で保存 |
| `Ctrl + I` | メディアのインポート（読み込み） |
| `Ctrl + M` | メディアの書き出し（エクスポート） |
| `Ctrl + Z` | 元に戻す (Undo) |
| `Ctrl + Shift + Z` | やり直す (Redo) |
| `Ctrl + ,` | 環境設定ダイアログを開く |
| `Ctrl + Alt + K` | キーボードショートカット設定画面を開く |
| `Shift + 1` | プロジェクトパネルにフォーカス |
| `Shift + 2` | ソースモニターにフォーカス |
| `Shift + 3` | タイムラインにフォーカス |
| `Shift + 4` | プログラムモニターにフォーカス |

*(※ macOS 環境では `Ctrl` を `Command (⌘)`、`Alt` を `Option (⌥)` に置き換えて操作してください)*

<details>
<summary>English</summary>

| Shortcut | Function |
|---|---|
| `Ctrl + N` | Create a new sequence |
| `Ctrl + O` | Open project |
| `Ctrl + S` | Save project |
| `Ctrl + Shift + S` | Save project as |
| `Ctrl + I` | Import media |
| `Ctrl + M` | Export media |
| `Ctrl + Z` | Undo |
| `Ctrl + Shift + Z` | Redo |
| `Ctrl + ,` | Open the Preferences dialog |
| `Ctrl + Alt + K` | Open the keyboard shortcut settings |
| `Shift + 1` | Focus the Project panel |
| `Shift + 2` | Focus the Source Monitor |
| `Shift + 3` | Focus the Timeline |
| `Shift + 4` | Focus the Program Monitor |

*(On macOS, use `Command (⌘)` instead of `Ctrl` and `Option (⌥)` instead of `Alt`.)*

</details>

---

## <a id="aiエージェント--mcp自動化連携"></a>🤖 AI エージェント / MCP 自動化連携 / AI agents and MCP automation

FilmCraft は、AI エージェントや外部スクリプトから自動操作できるように設計されています。すべての編集操作、タイムライン走査、エフェクト適用、書き出しは **MCP (Model Context Protocol)** 経由で呼び出すことができます。MCP に対応したクライアントなら何でも接続できます。**Claude Code** と **Antigravity** は設定ファイルをリポジトリに同梱済みで、フォルダを開くだけで使えます。

<details>
<summary>English</summary>

FilmCraft is designed to be driven by AI agents and external scripts. Every editing operation, timeline traversal, effect application and export can be invoked through **MCP (Model Context Protocol)**. Any MCP-capable client can connect. Config files for **Claude Code** and **Antigravity** ship with the repository, so they work as soon as you open the folder.

</details>

### MCP サーバーの 2 つのモード / Two MCP server modes

`filmcraft-cli mcp` が stdio で MCP サーバーになります。

| モード | コマンド | 操作対象 |
|---|---|---|
| **ヘッドレス** | `filmcraft-cli mcp --demo` または `--project 作品.fcproj`（どちらも無ければ空のプロジェクト） | ウィンドウを開かずにエンジンを直接操作。編集・レンダリングの自動化向け |
| **ブリッジ** | `filmcraft-cli mcp --bridge 127.0.0.1:9876` | `filmcraft --control 9876` で起動中のデスクトップアプリ。`ui_click` / `ui_screenshot` で画面そのものを操作 |

どちらのモードでも、先に CLI をビルドしておきます（設定ファイルは `target/release/filmcraft-cli` を指しています）。

```powershell
cargo build --release -p filmcraft-cli -p filmcraft

# ブリッジモードを使うときだけ、アプリを制御ポート付きで起動しておく
cargo run --release -p filmcraft -- --control 9876
```

<details>
<summary>English</summary>

`filmcraft-cli mcp` runs as an MCP server over stdio.

| Mode | Command | Target |
|---|---|---|
| **Headless** | `filmcraft-cli mcp --demo` or `--project 作品.fcproj` (an empty project if neither is given) | Drives the engine directly without opening a window. For automating editing and rendering |
| **Bridge** | `filmcraft-cli mcp --bridge 127.0.0.1:9876` | The desktop app running with `filmcraft --control 9876`. Drives the actual UI with `ui_click` / `ui_screenshot` |

For either mode, build the CLI first (the config files point at `target/release/filmcraft-cli`).

```powershell
cargo build --release -p filmcraft-cli -p filmcraft

# Only for bridge mode: start the app with a control port
cargo run --release -p filmcraft -- --control 9876
```

</details>

### Claude Code で使う / Using with Claude Code

リポジトリ直下の [`.mcp.json`](.mcp.json) に `filmcraft`（ブリッジ）と `filmcraft-headless`（ヘッドレス / デモ）の 2 つが登録されています。このフォルダで `claude` を起動し、プロジェクトの MCP サーバーを承認すれば完了です。手動で登録する場合や別のフォルダから使う場合は、絶対パスで追加します。

```powershell
claude mcp add filmcraft-headless -- E:\path\to\filmcraft_JPN\target\release\filmcraft-cli.exe mcp --demo
```

<details>
<summary>English</summary>

[`.mcp.json`](.mcp.json) at the repository root registers two servers: `filmcraft` (bridge) and `filmcraft-headless` (headless / demo). Start `claude` in this folder and approve the project's MCP servers, and you're done. To register them manually or use them from another folder, add them with an absolute path.

```powershell
claude mcp add filmcraft-headless -- E:\path\to\filmcraft_JPN\target\release\filmcraft-cli.exe mcp --demo
```

</details>

### Antigravity で使う / Using with Antigravity

Antigravity はワークスペース直下の [`.agents/mcp_config.json`](.agents/mcp_config.json) を読み込みます。このファイルも同梱済みなので、Antigravity でこのフォルダを開き、エージェントパネル右上の **…** ➔ **MCP Servers** ➔ **Manage MCP Servers** で `filmcraft` / `filmcraft-headless` が有効になっていることを確認してください。

全ワークスペース共通で使いたい場合、または相対パスで接続できない場合は、**View raw config** で開く `~/.gemini/config/mcp_config.json` に絶対パスで追加します（Windows では `C:\Users\<ユーザー名>\.gemini\config\mcp_config.json`）。

```json
{
  "mcpServers": {
    "filmcraft-headless": {
      "command": "E:\path\to\filmcraft_JPN\target\release\filmcraft-cli.exe",
      "args": ["mcp", "--demo"]
    },
    "filmcraft": {
      "command": "E:\path\to\filmcraft_JPN\target\release\filmcraft-cli.exe",
      "args": ["mcp", "--bridge", "127.0.0.1:9876"]
    }
  }
}
```

<details>
<summary>English</summary>

Antigravity reads [`.agents/mcp_config.json`](.agents/mcp_config.json) at the workspace root. This file also ships with the repository, so open this folder in Antigravity and check that `filmcraft` / `filmcraft-headless` are enabled under **…** ➔ **MCP Servers** ➔ **Manage MCP Servers** at the top right of the agent panel.

To use them across all workspaces, or if connecting with a relative path doesn't work, add them with absolute paths to `~/.gemini/config/mcp_config.json`, which opens via **View raw config** (on Windows: `C:\Users\<ユーザー名>\.gemini\config\mcp_config.json`).

```json
{
  "mcpServers": {
    "filmcraft-headless": {
      "command": "E:\path\to\filmcraft_JPN\target\release\filmcraft-cli.exe",
      "args": ["mcp", "--demo"]
    },
    "filmcraft": {
      "command": "E:\path\to\filmcraft_JPN\target\release\filmcraft-cli.exe",
      "args": ["mcp", "--bridge", "127.0.0.1:9876"]
    }
  }
}
```

</details>

### その他の MCP クライアント (Cursor / Gemini CLI / Codex など) / Other MCP clients (Cursor, Gemini CLI, Codex, etc.)

上と同じ `command` と `args` を、各クライアントの MCP 設定（stdio サーバー）に登録してください。設定ファイルの形式はどれも `{"mcpServers": {"名前": {"command": …, "args": […]}}}` が基本です。

<details>
<summary>English</summary>

Register the same `command` and `args` as above in each client's MCP settings (as a stdio server). The config format is basically `{"mcpServers": {"名前": {"command": …, "args": […]}}}` in all of them (where `名前` is the server name).

</details>

### MCP ツール一覧 / MCP tools

| ツール | 内容 |
|---|---|
| `command_list` | エンジンコマンド（約 600 個）の ID とパラメータを一覧 |
| `command_run` | コマンドを実行（すべて「元に戻す」可能） |
| `project_inspect` / `sequence_inspect` | プロジェクトツリー / アクティブなシーケンスを JSON で取得。返ってくる ID をコマンドに渡せます |
| `media_import` | 素材の読み込み |
| `render_frame` | 指定時刻のフレームを画像で取得 |
| `ui_inspect` / `ui_elements` / `ui_click` / `ui_drag` / `ui_key` / `ui_type` / `ui_screenshot` / `ui_control` | ブリッジモードで起動中のアプリを操作（画面要素の列挙、クリック、キー入力、スクリーンショット） |

時間は Tick（1 秒 = 254,016,000,000）ですが、コマンドは `seconds` / `frame` / `timecode` でも受け付けます。

<details>
<summary>English</summary>

| Tool | Description |
|---|---|
| `command_list` | Lists the ids and parameters of the engine commands (about 600) |
| `command_run` | Runs a command (all are undoable) |
| `project_inspect` / `sequence_inspect` | Returns the project tree / active sequence as JSON. The returned ids can be passed to commands |
| `media_import` | Imports media |
| `render_frame` | Returns the frame at a given time as an image |
| `ui_inspect` / `ui_elements` / `ui_click` / `ui_drag` / `ui_key` / `ui_type` / `ui_screenshot` / `ui_control` | Drive the app running in bridge mode (list UI elements, click, type keys, take screenshots) |

Time is in ticks (1 second = 254,016,000,000), but commands also accept `seconds` / `frame` / `timecode`.

</details>

### `command_run` の例 / `command_run` examples

エージェントからは「`sequence.removeSilence` を実行して」のように頼むだけで済みますが、内部では次のような呼び出しになります。

- **無音区間の自動削除**:
  ```json
  { "command": "sequence.removeSilence", "params": { "minSeconds": 0.5, "keepSeconds": 0.1 } }
  ```
- **再生ヘッド位置でカット**:
  ```json
  { "command": "sequence.addEdit", "params": { "seconds": 3 } }
  ```
- **マルチカメラシーケンスの生成**:
  ```json
  { "command": "clip.createMulticam", "params": { "name": "対談_マルチカム", "method": "audio", "audio": "switch" } }
  ```
- **プロキシの生成**:
  ```json
  { "command": "media.createProxies", "params": { "preset": "prores_proxy_half" } }
  ```

MCP を介さずスクリプトから直接叩く場合は、制御チャネル（`filmcraft --control 9876`、1 行 1 JSON）に `{"id": 1, "method": "engine.execute", "params": {"command": "sequence.addEdit", "params": {"seconds": 3}}}` を送ります。詳細は [docs/control-protocol.md](docs/control-protocol.md) と [docs/agents.md](docs/agents.md) を参照してください。

<details>
<summary>English</summary>

From an agent you just ask something like "run `sequence.removeSilence`", but under the hood it becomes a call like these:

- **Remove silent sections automatically**:
  ```json
  { "command": "sequence.removeSilence", "params": { "minSeconds": 0.5, "keepSeconds": 0.1 } }
  ```
- **Cut at the playhead**:
  ```json
  { "command": "sequence.addEdit", "params": { "seconds": 3 } }
  ```
- **Create a multi-camera sequence**:
  ```json
  { "command": "clip.createMulticam", "params": { "name": "対談_マルチカム", "method": "audio", "audio": "switch" } }
  ```
- **Create proxies**:
  ```json
  { "command": "media.createProxies", "params": { "preset": "prores_proxy_half" } }
  ```

To call it directly from a script without MCP, send `{"id": 1, "method": "engine.execute", "params": {"command": "sequence.addEdit", "params": {"seconds": 3}}}` to the control channel (`filmcraft --control 9876`, one JSON object per line). See [docs/control-protocol.md](docs/control-protocol.md) and [docs/agents.md](docs/agents.md) for details.

</details>

---

## <a id="便利機能"></a>✨ この版で追加した便利機能 / Convenience features added in this edition

どの機能もメニュー、コマンド ID(CLI / エージェント)の両方から使えます。すべて 1 回の「元に戻す」で取り消せます。無音カット・範囲を隠す・縦型バージョン・話者に合わせて自動切り替えは、メニューから選ぶと設定ダイアログが開きます(コマンドに引数を付けて呼ぶと、ダイアログなしで実行されます)。

| 機能 | メニュー | コマンド | 内容 |
|---|---|---|---|
| **無音カット** | シーケンス ➔ 無音部分を削除 | `sequence.removeSilence` | ミックスの音量が小さい区間を検出して一括で詰めます。文字起こしは不要です。 |
| **範囲を隠す** | クリップ ➔ 範囲を隠す (モザイク / ぼかし) | `clip.censor` | モザイクかぼかしをマスク付きで適用し、対象に追従させられます。 |
| **縦型バージョン** | シーケンス ➔ 縦型バージョンを作成 | `sequence.makeVertical` | 9:16 / 1:1 / 4:5 のシーケンスを元のシーケンスから作ります。 |
| **テキストスタイル** | プロパティ ➔ テキストスタイル | `graphics.textStyle` | 字幕・袋文字・帯・シャドウ・グローの 8 種を 1 クリックで適用します。 |
| **ルビ(ふりがな)** | プロパティ ➔ ルビ | `graphics.ruby` | `漢字《かんじ》` や `｜東京タワー《とうきょうタワー》` と書くとルビになります。 |
| **日本語の文字起こし** | テキスト ➔ 文字起こし | `transcript.generate` | 音声から言語を自動検出して文字起こしし、字幕を作れます。 |
| **日本語のフィラー除去** | シーケンス ➔ 文字起こし ➔ フィラーワードを削除 | `transcript.removeFillers` | 「えー」「えーと」「えっと」「あのー」「うーん」などを一括で詰めます。 |
| **言い直しの削除** | シーケンス ➔ 文字起こし ➔ 言い直しを削除 | `transcript.removeRepeats` | 「今日は、今日はですね」のような言い直しで、前のテイクを消して最後のテイクを残します。`transcript.findRepeats` で削除せずに確認できます。 |
| **英語字幕** | シーケンス ➔ 文字起こし ➔ 英語字幕を作成 | `transcript.createTranslatedCaptions` | 日本語などの音声を英語に翻訳した字幕トラックを作ります。 |
| **話者で色分け** | テキスト ➔ キャプション ➔ 話者で色分け | `captions.setStyle` | 話者ごとに字幕の文字色を変えます(2 人目以降は黄・水色・緑…)。 |
| **YouTube 用チャプター** | マーカー ➔ YouTube 用チャプターをコピー | `markers.exportChapters` | チャプターマーカーを「0:00 オープニング」形式でクリップボードにコピーします。 |
| **ビートマーカー** | マーカー ➔ ビートにマーカーを追加 | `markers.addBeats` | 選択した音楽クリップのビートにマーカーを打ちます。 |
| **ビートに合わせてカット** | シーケンス ➔ カットをビートに合わせる | `sequence.cutToBeats` | V1 のカット位置を音楽の最寄りのビートへ移動します(リンクした音声も一緒に)。 |
| **ショート動画を作成** | シーケンス ➔ ショート動画を作成 | `sequence.makeShort` | 縦型化・無音カット・字幕付けを 1 回で行います。 |
| **Filmora のプロジェクトを読み込む** | ファイル ➔ 読み込み (`.wfp`) | `file.import` | Wondershare Filmora のプロジェクトを、トラック・クリップ・字幕ごと FilmCraft のシーケンスにします。 |
| **話者に合わせて自動切り替え** | クリップ ➔ マルチカメラ ➔ 話者に合わせて自動切り替え | `multicam.autoSwitch` | 対談・ポッドキャストのマルチカメラで、話している人のカメラへ自動でカットします。 |

- **無音カット**: 既定は −40 dB 未満が 0.5 秒以上続く区間が対象で、音の前後に 0.1 秒を残します。`{"action": "detect"}` で削除せずに検出だけできます。
- **範囲を隠す**: 実行すると画面中央に範囲が出るので、プログラムモニター上で動かしてから「選択したマスクをトラック」で追従させます。コマンドでは `{"center": [x, y], "size": [w, h], "track": "both"}` で位置指定と追従を一度に行えます。マスク付きのモザイク・ぼかしも GPU で処理されます。
- **縦型バージョン**: `mode` は `follow`(被写体を追う)、`fill`(中央を切り出す)、`blur`(全体を表示し、余白をぼかした映像で埋める)の 3 種です。元のシーケンスを入れ子にしているので、元を編集すると縦型にも反映されます。`follow` は画面内の目立つ部分を追う近似で、人物の認識ではありません。
- **ルビ**: 親文字が漢字だけなら `漢字《かんじ》`、それ以外は先頭に `｜` を付けます。縦書きでは右側に付きます。
- **文字起こし**: 初回に音声認識モデル(Whisper base、約 290 MB、MIT ライセンス)のダウンロードが必要です。アプリには同梱していません。字幕は句の切れ目で改行します(助詞や送り仮名は前の語に付け、漢字同士・カタカナ同士は切りません)。1 行に収まらない句は、最後の助詞のあとで改行します。形態素解析は行っていないので、まれに不自然な位置で切れることがあります。
- **フィラー除去**: 「あの」「まあ」「なんか」は普通の言葉としても使われるので、既定では消しません。消したい場合はコマンドで `{"fillers": ["まあ", "なんか"]}` のように指定します。
- **言い直しの削除**: 同じ話者が 2 秒未満の間に同じ言葉を言い直した場合だけが対象です。1〜2 語の言い直しは直後の繰り返し(「the the」)のみ、長いフレーズは間に言いかけの語が少しあっても検出します。1 文字だけの語(「の」「は」など)や「いろいろ」のような繰り返し語は対象外です。「very very」のような意図的な繰り返しも消えるので、`transcript.findRepeats` で先に確認できます。`{"minWords": 2}` で 2 語以上の言い直しだけにできます。
- **英語字幕**: Whisper の翻訳機能を使います。base モデルでは大意は合いますが、細部の訳は不正確なことがあります(例: 「午後から雨」が "from the next day" になる)。元の文字起こしは変更しません。
- **YouTube 用チャプター**: チャプターマーカーがなければ名前付きのマーカーを使います。0:00 のチャプターがなければ「Intro」を自動で追加します(`firstTitle` で変更可)。YouTube の条件(3 件以上、各 10 秒以上)を満たさない場合はステータスバーに表示します。`{"path": "chapters.txt"}` でファイルにも保存できます。
- **ビート**: 一定のテンポがある音楽向けです。話し声やテンポが揺れる曲ではビートが見つからないことがあります。`{"every": 4}` で 4 拍ごと(小節ごとの目安)にできます。カット位置の移動は素材の余り(ハンドル)の範囲内で行い、届かないカットや音声とずれるカットはそのままにします。
- **ショート動画**: 縦型バージョン → 字幕 → 無音カットの順に実行します。字幕は元のシーケンスの文字起こしから作るので、先に文字起こしをしておいてください(なければ字幕だけ省略し、理由を返します)。元のシーケンスは変更しません。
- **Filmora の読み込み**: `.wfp` を読み込みダイアログで選ぶか、プロジェクトパネルにドロップします。クリップの位置・長さ・速度・逆再生、画面に合わせる表示と位置・拡大・回転、入れ子のシーケンス(複合クリップ)、字幕(文字・フォント・大きさ・色・背景・位置)、手描きの図形(四角・矢印・ペンの線(曲線を含む)と矢じり。線の一部だけを描く設定(トリム)、線の色と太さ、塗り、位置・拡大・回転を含めて図形レイヤーにします)、同じ素材の映像と音声のリンク、音量(dB)、音声のフェードイン/アウト(ゲイン一定のトランジション)、切り抜き(クロップして画面いっぱいに表示)を読み込みます。スピードランプは同じ長さになる一定速度で再生します。EQ などその他のエフェクトと音量のキーフレームは、読み込み結果に一覧で表示します。素材は保存時の場所で探し、見つからなければ `.wfp` を開いたフォルダからの相対位置で探します(フォルダごと移動したプロジェクトも開けます)。プロジェクトファイルは読むだけで、変更も削除もしません。Filmora 15〜16 で保存したプロジェクトで確認しています。
- **話者に合わせて自動切り替え**: 再生ヘッド位置のマルチカメラクリップについて、各カメラ自身のマイクの音量(50 ms ごと、0.3 秒で平滑化)を比べ、一番大きい人のカメラを映します。カットが落ち着くように、今のカメラより 6 dB 以上大きく、0.4 秒以上続いたときだけ切り替え、1 カットは 2 秒以上にします(`minShotSeconds`、`marginDb` で変更可)。引きのカメラを指定すると(`wideCamera`、1 から数える)、全員が黙っている間や 2 人が同時に話している間はそのカメラにします。引きのカメラのマイクは全員の声を拾うので、判定には使いません。各カメラが自分の音声トラックを持っている必要があります。「マルチカメラのオーディオをビデオに追従」がオンなら音声も一緒に切り替わります。解析はバックグラウンドで行うので、長い収録でも画面は止まりません(進み具合はジョブ一覧に表示され、途中で中止すると何も変更しません。解析中にそのクリップを編集した場合は、古い解析結果で上書きせずにお知らせします)。`{"action": "detect"}` で変更せずにカット位置だけを返し、`{"wait": true}` で終わるまで待って結果を返します。文字起こしや AI モデルは使いません。

<details>
<summary>English</summary>

Every feature is available both from the menus and by command ID (CLI / agents), and each one can be undone with a single Undo. Remove Silence, Censor Region, Vertical Version and Auto-Switch to Speaker open a settings dialog when chosen from the menu (calling the command with arguments runs it without the dialog).

| Feature | Menu | Command | Description |
|---|---|---|---|
| **Remove silence** | Sequence ➔ Remove Silent Parts | `sequence.removeSilence` | Detects stretches where the mix is quiet and closes them up in one go. No transcript needed. |
| **Censor region** | Clip ➔ Censor Region (Mosaic / Blur) | `clip.censor` | Applies a masked mosaic or blur that can track the subject. |
| **Vertical version** | Sequence ➔ Create Vertical Version | `sequence.makeVertical` | Creates a 9:16 / 1:1 / 4:5 sequence from the original sequence. |
| **Text styles** | Properties ➔ Text Styles | `graphics.textStyle` | Applies one of 8 styles (subtitle, outlined text, banner, shadow, glow) in one click. |
| **Ruby (furigana)** | Properties ➔ Ruby | `graphics.ruby` | Writing `漢字《かんじ》` or `｜東京タワー《とうきょうタワー》` produces ruby text. |
| **Japanese transcription** | Text ➔ Transcribe | `transcript.generate` | Transcribes audio with automatic language detection and can create captions from it. |
| **Japanese filler removal** | Sequence ➔ Transcript ➔ Remove Filler Words | `transcript.removeFillers` | Closes up fillers such as 「えー」「えーと」「えっと」「あのー」「うーん」 in one go. |
| **Remove repeated takes** | Sequence ➔ Transcript ➔ Remove Repeats | `transcript.removeRepeats` | For restarts like 「今日は、今日はですね」, removes the earlier take and keeps the last one. `transcript.findRepeats` lets you review them without deleting. |
| **English captions** | Sequence ➔ Transcript ➔ Create English Captions | `transcript.createTranslatedCaptions` | Creates a caption track translating Japanese (or other) speech into English. |
| **Color by speaker** | Text ➔ Captions ➔ Color by Speaker | `captions.setStyle` | Gives each speaker a different caption text color (yellow, cyan, green… from the second speaker on). |
| **YouTube chapters** | Markers ➔ Copy YouTube Chapters | `markers.exportChapters` | Copies chapter markers to the clipboard in the "0:00 Opening" format. |
| **Beat markers** | Markers ➔ Add Markers at Beats | `markers.addBeats` | Adds markers at the beats of the selected music clip. |
| **Cut to the beat** | Sequence ➔ Snap Cuts to Beats | `sequence.cutToBeats` | Moves V1 cut points to the nearest beat of the music (linked audio moves with them). |
| **Create short video** | Sequence ➔ Create Short Video | `sequence.makeShort` | Makes it vertical, removes silence and adds captions in one step. |
| **Import Filmora projects** | File ➔ Import (`.wfp`) | `file.import` | Turns a Wondershare Filmora project into a FilmCraft sequence, including tracks, clips and captions. |
| **Auto-switch to speaker** | Clip ➔ Multi-Camera ➔ Auto-Switch to Speaker | `multicam.autoSwitch` | In multi-camera interviews and podcasts, automatically cuts to the camera of whoever is speaking. |

- **Remove silence**: By default it targets stretches below −40 dB lasting 0.5 s or more, and keeps 0.1 s before and after the sound. `{"action": "detect"}` only detects without deleting.
- **Censor region**: Running it places a region in the center of the frame; move it in the Program Monitor, then use "Track Selected Mask" to make it follow the subject. From the command, `{"center": [x, y], "size": [w, h], "track": "both"}` sets the position and tracks in one go. Masked mosaic and blur are also processed on the GPU.
- **Vertical version**: `mode` is one of `follow` (follows the subject), `fill` (crops the center) or `blur` (shows the whole frame and fills the margins with a blurred copy). The original sequence is nested, so edits to the original carry over to the vertical version. `follow` approximates by tracking the salient part of the frame; it doesn't recognize people.
- **Ruby**: If the base text is all kanji, write `漢字《かんじ》`; otherwise prefix it with `｜`. In vertical text, ruby goes on the right side.
- **Transcription**: The first run requires downloading the speech recognition model (Whisper base, about 290 MB, MIT license); it isn't bundled with the app. Captions break lines at phrase boundaries (particles and okurigana stay with the preceding word, and runs of kanji or katakana aren't split). A phrase that doesn't fit on one line breaks after its last particle. There's no morphological analysis, so a line occasionally breaks in an unnatural place.
- **Filler removal**: 「あの」「まあ」「なんか」 are also used as ordinary words, so they aren't removed by default. To remove them, specify them in the command, e.g. `{"fillers": ["まあ", "なんか"]}`.
- **Remove repeated takes**: Only targets cases where the same speaker restates the same words within 2 seconds. Restarts of 1–2 words are caught only as immediate repeats ("the the"); longer phrases are detected even with a few false-start words in between. Single-character words (「の」「は」 etc.) and reduplicated words like 「いろいろ」 are excluded. Intentional repetition like "very very" is also removed, so you can check first with `transcript.findRepeats`. `{"minWords": 2}` limits it to restarts of 2 or more words.
- **English captions**: Uses Whisper's translation feature. With the base model the gist is right but details may be mistranslated (e.g. 「午後から雨」 becomes "from the next day"). The original transcript is left unchanged.
- **YouTube chapters**: Without chapter markers, named markers are used. If there's no chapter at 0:00, "Intro" is added automatically (changeable with `firstTitle`). If YouTube's requirements (at least 3 chapters, each at least 10 seconds) aren't met, the status bar says so. `{"path": "chapters.txt"}` also saves to a file.
- **Beats**: Meant for music with a steady tempo. Beats may not be found in speech or in songs with a drifting tempo. `{"every": 4}` uses every 4th beat (roughly once per bar). Cuts are only moved within the available media handles; cuts that can't reach a beat, or that would drift out of sync with audio, are left alone.
- **Short video**: Runs Vertical Version → captions → Remove Silence, in that order. Captions come from the original sequence's transcript, so transcribe first (otherwise only the captions are skipped and the reason is returned). The original sequence is left unchanged.
- **Filmora import**: Choose the `.wfp` in the Import dialog or drop it on the Project panel. It imports clip position, duration, speed and reverse; fit-to-frame plus position, scale and rotation; nested sequences (compound clips); captions (text, font, size, color, background, position); hand-drawn shapes (rectangles, arrows, pen strokes (including curves) and arrowheads, made into shape layers including partial-stroke (trim) settings, stroke color and width, fill, and position, scale and rotation); video/audio links for the same media; volume (dB); audio fade in/out (constant-gain transitions); and crops (cropped and shown full-frame). Speed ramps play at a constant speed giving the same duration. EQ and other effects, and volume keyframes, are listed in the import result. Media is looked up at its saved location and, if not found, relative to the folder the `.wfp` was opened from (so projects moved as a whole folder still open). The project file is only read, never modified or deleted. Tested with projects saved in Filmora 15–16.
- **Auto-switch to speaker**: For the multi-camera clip at the playhead, compares the level of each camera's own microphone (every 50 ms, smoothed over 0.3 s) and shows the camera of the loudest person. To keep cuts steady, it only switches when the new camera is at least 6 dB louder for at least 0.4 s, and each shot lasts at least 2 seconds (changeable with `minShotSeconds` and `marginDb`). If you specify a wide camera (`wideCamera`, counted from 1), that camera is used while everyone is silent or two people talk at once. The wide camera's mic picks up everyone, so it isn't used for detection. Each camera must have its own audio track. With "Multi-Camera Audio Follows Video" on, the audio switches too. Analysis runs in the background, so the UI doesn't freeze even on long recordings (progress shows in the job list; cancelling midway changes nothing; if you edit the clip during analysis, you're notified instead of having it overwritten with stale results). `{"action": "detect"}` returns only the cut points without changing anything, and `{"wait": true}` waits until it finishes and returns the result. No transcription or AI model is used.

</details>

### 操作性と安定性の改善 / Usability and stability improvements

**作業を失わないために**
- 未保存の変更があるときは、ウィンドウを閉じる・新規プロジェクトを作成する・別のプロジェクト(デモを含む)を開く前に「保存 / 保存しない / キャンセル」を確認します。「保存しない」を選ぶと復元用の記録も破棄します。
- 1 つの操作は必ず 1 回の「元に戻す」で取り消せます(内部で複数の処理をする操作も 1 ステップ)。途中で失敗した操作は何も変更を残しません。
- 複数の書き出しを同時に実行したときに、まれに処理が止まる問題を解消しました。

**ダイアログ**(Premiere Pro と同様に、メニューやショートカットから選ぶと設定画面が開きます)
- 新規シーケンス(プリセット 10 種・名前・サイズ・フレームレート・トラック数)、速度・デュレーション(Ctrl+R。速度・逆再生・リップル・時間補間)、ネスト、トラックを追加、シーケンス設定、チャプターマーカー、新規キャプショントラック。
- 新規カラーマット(カラーピッカー)、調整レイヤー、カラーバー＆トーン(既定 10 秒)、ブラックビデオ、ユニバーサルカウントリーダー、透明ビデオ(名前・サイズ・長さ)。
- ファイル ➔ 書き出し ➔ EDL / OpenTimelineIO / Final Cut Pro XML / FCPXML は保存先を尋ね、メディア… は書き出しモードを開きます。

**ソースモニター**
- ソースモニターでクリップを再生できます(Space / J / K / L / 再生ボタン。音声付き)。
- ソースモニターを選んでいるときの ← → / Home / End / M は、ソースクリップの再生位置・クリップマーカーに働きます。

**タイムライン**
- クリップの端をすばやくドラッグしてもトリミングになります(以前はクリップの移動になることがありました)。
- 空のシーケンスは最初の 60 秒が見える倍率で表示し、シーケンスの先頭にもスナップします。
- 右クリックしたクリップが選択され、メニューの操作はそのクリップに働きます。

**その他**
- ダイアログを開いている間は、背後のパネルにショートカットが届きません(Space で再生が始まる、Delete でクリップが消える、を防ぎます)。
- エフェクトコントロールの長いラベルは値と重ならないよう「…」で省略し、ポインタを重ねると全文を表示します。
- 未保存のプロジェクトの書き出し先と「フレームを書き出し」の保存先は、Windows では「ビデオ」フォルダです。
- Windows でも CPU 使用時間(`perf.stats` の `process.cpuS`)を計測できます。
- AI エージェント向け: すべての操作部品に自動化 ID があり(監査テストで検査)、ウィンドウが最小化されているときは理由を返します。全コマンドは極端な引数でも落ちず、取り消し・やり直しが正確に働くことをテストで検査しています。

<details>
<summary>English</summary>

**Never losing work**
- With unsaved changes, FilmCraft asks "Save / Don't Save / Cancel" before closing the window, creating a new project, or opening another project (including the demo). Choosing "Don't Save" also discards the recovery record.
- Every operation can be undone with exactly one Undo (operations that do several things internally are still one step). An operation that fails midway leaves no changes behind.
- Fixed a rare hang when running several exports at the same time.

**Dialogs** (as in Premiere Pro, choosing these from a menu or shortcut opens a settings dialog)
- New Sequence (10 presets, name, size, frame rate, track count), Speed/Duration (Ctrl+R; speed, reverse, ripple, time interpolation), Nest, Add Tracks, Sequence Settings, Chapter Marker, New Caption Track.
- New Color Matte (color picker), Adjustment Layer, Bars and Tone (10 s by default), Black Video, Universal Counting Leader, Transparent Video (name, size, duration).
- File ➔ Export ➔ EDL / OpenTimelineIO / Final Cut Pro XML / FCPXML ask for a destination; Media… opens Export mode.

**Source Monitor**
- Clips can be played in the Source Monitor (Space / J / K / L / Play button, with audio).
- When the Source Monitor is selected, ← → / Home / End / M act on the source clip's playhead and clip markers.

**Timeline**
- Quickly dragging a clip edge now trims (it used to sometimes move the clip).
- An empty sequence is shown at a zoom level where the first 60 seconds are visible, and snapping to the start of the sequence works.
- Right-clicking a clip selects it, and menu commands act on that clip.

**Other**
- While a dialog is open, shortcuts don't reach the panels behind it (preventing Space from starting playback or Delete from removing clips).
- Long labels in Effect Controls are truncated with "…" so they don't overlap values; hovering shows the full text.
- On Windows, the export destination for unsaved projects and the save location for "Export Frame" are the Videos folder.
- CPU time (`process.cpuS` in `perf.stats`) can now be measured on Windows too.
- For AI agents: every control has an automation ID (checked by an audit test), and a reason is returned when the window is minimized. Tests check that every command survives extreme arguments and that undo/redo works exactly.

</details>

## <a id="トラブルシューティング--faq"></a>❓ トラブルシューティング / FAQ / Troubleshooting / FAQ

### Q. 日本語フォントが正しく表示されない / トーフ（□）になる / Q. Japanese fonts don't display correctly / show up as tofu (□)
**A.** 日本語フォント（BIZ UDPゴシック）はアプリに同梱されているため、OS 側のフォントは不要です。タイトルやキャプションで別の日本語フォントを使いたい場合は、OS にインストールすればフォント一覧に表示されます。

<details>
<summary>English</summary>

**A.** A Japanese font (BIZ UDPGothic) is bundled with the app, so no OS font is needed. To use a different Japanese font in titles or captions, install it in the OS and it will appear in the font list.

</details>

### Q. 英語表示に切り替えたい / 日本語表示に戻したい / Q. I want to switch to English / back to Japanese
**A.** **設定 ➔ アピアランス ➔ 言語**（Settings ➔ Appearance ➔ Language）で切り替えます。変更はすぐに反映され、次回起動時も保持されます。

<details>
<summary>English</summary>

**A.** Switch it under **Settings ➔ Appearance ➔ Language**. The change takes effect immediately and persists across restarts.

</details>

### Q. ビルド時にエラーが出る / Q. I get errors when building
**A.** Rust のバージョンが古い可能性があります。ターミナルで以下を実行して最新の安定版（stable）にアップデートしてください。
```powershell
rustup update stable
```

<details>
<summary>English</summary>

**A.** Your Rust version may be out of date. Run the following in a terminal to update to the latest stable release.
```powershell
rustup update stable
```

</details>

### Q. 編集途中で不意に PC が再起動・電源断してしまった / Q. My PC restarted or lost power unexpectedly while editing
**A.** FilmCraft は編集のたびに少し遅れてリカバリージャーナルを記録しています。再度アプリを起動すると、**「未保存の変更を復元」** ダイアログが自動表示され、直前の状態へ復帰できます。なお、未保存のままウィンドウを閉じたり、新規プロジェクトを作成・別のプロジェクトを開いたりするときは、先に「保存 / 保存しない / キャンセル」を確認します。

<details>
<summary>English</summary>

**A.** FilmCraft writes a recovery journal shortly after every edit. When you start the app again, a **"Restore Unsaved Changes"** dialog appears automatically and lets you return to the last state. Also, closing the window, creating a new project or opening another project with unsaved changes first asks "Save / Don't Save / Cancel".

</details>

### Q. GPU アクセラレーションが効いているか確認したい / Q. How do I check that GPU acceleration is working?
**A.** ヘルプ ➔ **「システム互換性レポート」** またはコンソールログから、検出された WebGPU アダプター名（例: NVIDIA GeForce RTX / Apple M シリーズ / AMD Radeon）を確認できます。対応 GPU が検出された場合、Lumetri カラーやブラーエフェクトは自動的に WGSL コンピュートパイプラインで実行されます。

<details>
<summary>English</summary>

**A.** Check the detected WebGPU adapter name (e.g. NVIDIA GeForce RTX / Apple M series / AMD Radeon) in Help ➔ **"System Compatibility Report"** or in the console log. When a supported GPU is detected, Lumetri Color and blur effects run automatically on WGSL compute pipelines.

</details>

---

## <a id="アーキテクチャと設計思想"></a>🏗 アーキテクチャと設計思想 / Architecture and design philosophy

FilmCraft は、関心事の分離（Separation of Concerns）と厳格なレイヤリング（`L0`〜`L6`、下位の層は上位の層に依存しない）に基づき設計されています。`crates/` の 45 クレートと `apps/` の 3 つの実行体からなり、層の決まりは `cargo xtask layers` で検査されます：

```text
crates/ (45 クレート) + apps/ (3 実行体)
├── [L0] time / geom / color / bitstream / testkit         # 整数 Tick・幾何・色空間・ビット読み取り・テスト補助
│        isobmff / matroska / mxf / cfb / mpegts / ogg / riff  # コンテナ (MP4/MOV, MKV/WebM, MXF, AAF/OMF, TS, Ogg, AVI)
│        h264 / h264enc / hevc / vp9 / av1 / av1enc / prores / dnx / mpeg2v / aac / ac3 / opus / flac  # 独自コーデック
├── [L1] frame / media / project / audio-dsp / text         # フレーム・メディア抽象・データモデル・音声DSP・文字組版(ルビ)
├── [L2] edit / codecs / captions / speech / interchange    # 編集コア・デコード統合・字幕・Whisper 文字起こし・EDL/XML/AAF
├── [L3] render / gpu / scopes / export / format / golden   # レンダリング・WebGPU 合成・スコープ・書き出し・保存形式
├── [L4] engine                                             # セッション・約 550 のコマンド・Undo 履歴・自動保存
├── [L5] ui-egui / platform / automation                    # egui 日本語 UI・OS のハードウェアデコード/エンコード・MCP
└── [L6] apps/filmcraft / filmcraft-cli / filmcraft-web      # デスクトップ / CLI / WebAssembly
```

<details>
<summary>English</summary>

FilmCraft is designed around separation of concerns and strict layering (`L0`–`L6`; lower layers never depend on higher ones). It consists of 45 crates in `crates/` and 3 executables in `apps/`, and the layering rules are checked by `cargo xtask layers`:

```text
crates/ (45 crates) + apps/ (3 executables)
├── [L0] time / geom / color / bitstream / testkit         # integer Tick, geometry, color spaces, bit reading, test helpers
│        isobmff / matroska / mxf / cfb / mpegts / ogg / riff  # containers (MP4/MOV, MKV/WebM, MXF, AAF/OMF, TS, Ogg, AVI)
│        h264 / h264enc / hevc / vp9 / av1 / av1enc / prores / dnx / mpeg2v / aac / ac3 / opus / flac  # in-house codecs
├── [L1] frame / media / project / audio-dsp / text         # frames, media abstraction, data model, audio DSP, text layout (ruby)
├── [L2] edit / codecs / captions / speech / interchange    # editing core, decode integration, captions, Whisper transcription, EDL/XML/AAF
├── [L3] render / gpu / scopes / export / format / golden   # rendering, WebGPU compositing, scopes, export, save format
├── [L4] engine                                             # session, ~550 commands, undo history, autosave
├── [L5] ui-egui / platform / automation                    # egui Japanese UI, OS hardware decode/encode, MCP
└── [L6] apps/filmcraft / filmcraft-cli / filmcraft-web      # desktop / CLI / WebAssembly
```

</details>

### クリーンルーム設計ポリシー / Clean-room design policy
- 本プロジェクトは Adobe 製品のバイナリの解析、リバースエンジニアリング、逆アセンブル、または Adobe アプリケーション内リソースのコピーを一切行っていません。
- すべてのコーデックやコンテナフォーマットは、ITU-T、ISO/IEC、IETF RFC、SMPTE 等の公開された公的仕様書に基づいて独自に実装されています。
- GPL/LGPL ライセンスの外部ライブラリ（FFmpeg, x264 等）はリンク・バンドルされておらず、商用環境でも安心して利用可能なパーミッシブライセンス（MIT / Apache-2.0）のみで構成されています。

<details>
<summary>English</summary>

- This project never analyzes, reverse-engineers or disassembles Adobe product binaries, and never copies resources from inside Adobe applications.
- All codecs and container formats are implemented independently from published public specifications such as ITU-T, ISO/IEC, IETF RFCs and SMPTE.
- No GPL/LGPL external libraries (FFmpeg, x264, etc.) are linked or bundled; the project uses only permissive licenses (MIT / Apache-2.0) that are safe to use in commercial settings.

</details>

---

## <a id="ライセンスとクレジット"></a>📄 ライセンスとクレジット / License and credits

FilmCraft は **MIT License** または **Apache License 2.0** のデュアルライセンスで提供されています。

- **著作権**: (c) 2026 ArtCraft Team and FilmCraft contributors（ArtCraft チームおよび FilmCraft の貢献者）
- **商標について**: Adobe, Premiere Pro は Adobe Inc. の登録商標です。互換性およびワークフロー説明の目的でのみ参照しています。
- **日本語ローカライズ & GPU 最適化**: 本リポジトリ (`filmcraft_JPN`) は、日本のクリエイターコミュニティ向けに UI・ドキュメントを完全日本語化し、WebGPU 最適化を施した特別エディションです。

<details>
<summary>English</summary>

FilmCraft is dual-licensed under the **MIT License** or the **Apache License 2.0**.

- **Copyright**: (c) 2026 ArtCraft Team and FilmCraft contributors
- **Trademarks**: Adobe and Premiere Pro are registered trademarks of Adobe Inc. They are referenced only to describe compatibility and workflow.
- **Japanese localization & GPU optimization**: This repository (`filmcraft_JPN`) is a special edition with a fully Japanese UI and documentation for the Japanese creator community, plus WebGPU optimizations.

</details>

---
<p align="center">
  <sub>ArtCraft チームと日本語化プロジェクトが ❤️ を込めて制作</sub>
</p>

<details>
<summary>English</summary>

Made with ❤️ by the ArtCraft team and the Japanese localization project

</details>
