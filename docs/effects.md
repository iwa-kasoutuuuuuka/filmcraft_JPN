# エフェクトの使い方（FilmCraft）

この文書は、FilmCraft のエフェクト（ビデオエフェクト、トランジション）の使い方と、主要なエフェクトの
パラメータ、動作の仕組み、デバッグの手順をまとめたものです。定義の正本は
`crates/project/src/effect/vfx.rs`（エフェクト）と `crates/project/src/vtransition.rs`（トランジション）です。
ここに書いたデフォルト値は、そのファイルの定義に基づきます。

## 1. 種類と場所

| 種類 | 置き場所 | 例 |
|---|---|---|
| ビデオエフェクト | クリップの「エフェクトコントロール」 | Gaussian Blur, Lumetri Color, Warp Stabilizer, Mosaic |
| トランジション | 2 つのクリップの境界 | Cross Dissolve, Morph Cut, Wipe |
| 固定プロパティ | クリップごと（削除不可） | Motion（位置・スケール・回転）, Opacity |

ビデオエフェクトは、Premiere Pro の「エフェクト」パネルと同じ 16 のカテゴリに分かれています。

- Adjust, Blur & Sharpen, Color Correction, Distort, Generate, Image Control
- Immersive Video, Keying, Lights & Glows, Noise & Grain, Perspective
- Stylize, Time, Transform, Utility, Video

## 2. GUI での基本操作

1. エフェクトパネルからクリップへエフェクトをドラッグします。
2. エフェクトコントロールで、パラメータを調整します。
3. パラメータをキーフレーム化すると、時間ごとに値を変えられます（キーフレームの操作は、エフェクトコントロールの表示に従います）。
4. よく使う組み合わせは、エフェクトプリセットとして保存できます。プリセットは JSON で書き出し・
   読み込みできます（README の「エフェクトプリセット」を参照）。
5. エフェクトは、データ上の並び順に従って適用されます（`effects.remove` / `effects.reset` は並び順の番号で指定します）。
6. エフェクトは無効化（`enabled`）でき、削除やリセットもできます。

## 3. コマンドから操作する（エージェント・スクリプト）

`filmcraft-cli` と JSON-RPC のどちらからも操作できます（詳細は [agents.md](agents.md)）。

```bash
filmcraft-cli --project p.fcproj exec effects.list
filmcraft-cli --project p.fcproj exec effects.apply '{"effect":"Gaussian Blur"}'
```

| コマンド | 引数 | 内容 |
|---|---|---|
| `effects.list` | なし | 使えるエフェクトの一覧 |
| `effects.apply` | `clips`（省略時は選択中）, `effect` | クリップにエフェクトを追加。`effect` は表示名（`Gaussian Blur`）か ID（`gaussian_blur`）のどちらでもよい |
| `effects.remove` | `clip`, `index` | `index` 番目のエフェクトを削除 |
| `effects.reset` | `clip`, `index` | `index` 番目のエフェクトを既定値に戻す |

グラフィック（テキスト・図形）は `effects.apply` では追加できません。`graphics.newText` /
`graphics.newShape` を使ってください。

## 4. 主要エフェクトの説明

### 4.1 Warp Stabilizer（手ブレ補正）

最初に使うときに、クリップ全体を解析します（時間がかかります）。解析結果はメモリにキャッシュされ、
同じクリップでは再利用されます。

| パラメータ | 既定値 | 内容 |
|---|---|---|
| Result | Smooth Motion | `Smooth Motion` は揺れを滑らかにし、`No Motion` は動きを止めます |
| Smoothness | 50 % | 大きいほど、カメラの動きを強く平滑化します |
| Method | Position, Scale, Rotation | 下の「Method の違い」を参照 |
| Preserve Scale | オフ | 拡大・縮小を補正しません。Position、Scale, Rotation のときだけ働きます |
| Framing | Stabilize, Crop, Auto-scale | 補正で生じる縁の処理（下の「Framing」） |
| Maximum Scale | 150 % | 自動拡大の上限 |
| Action-Safe Margin | 0 % | 安全余白。拡大率に加算されます |
| Additional Scale | 100 % | 最後に掛ける拡大率 |
| Detailed Analysis | オフ | オンで解析の解像度を上げます（960 px。オフは 480 px） |
| Crop Less ↔ Smooth More | 50 % | 大きいほど平滑化を優先し、切り取りを増やします |

**Method の違い**

- **Position**: 平行移動だけを補正します。
- **Position, Scale, Rotation**: 平行移動・拡大縮小・回転を補正します（既定）。
- **Perspective**: 射影変換で補正します。遠近感のある揺れに向きます。
- **Subspace Warp**: カメラがパン・チルト・ロールしたと見なして補正します。焦点距離は映像の動きから推定します。
  推定がうまくいかない場合は、Perspective と同じ処理に切り替わります。平行移動の揺れは補正しません。

**Framing**

- **Stabilize Only**: 補正だけを行います。縁は空白になります。
- **Stabilize, Crop**: 縁が見えない範囲まで切り取ります。
- **Stabilize, Crop, Auto-scale**: 縁が見えないように、自動で拡大します（既定）。
- **Stabilize, Synthesize Edges**: 縁を、映像の端の色で埋めます。

### 4.2 Morph Cut（トランジション）

同じ被写体のジャンプカットを、なめらかにつなぐトランジションです。

- 切り替え前の最後のフレームと、切り替え後の最初のフレームで、正面の顔を検出します。
- 後のショットを、顔の位置と大きさが前のショットに合うように拡大縮小・移動します。
- そのうえで、2 つのショットをディゾルブし、後のショットを元の構図へ戻していきます。
- 顔が両方で見つからない場合は、通常のクロスディゾルブになります。

制限: 検出器は正面向きの顔だけを対象とします。顔の特徴点は使わず、位置と大きさを合わせるだけです。

### 4.3 Auto Reframe（自動リフレーム）

縦横比の違うシーケンス（縦型 9:16、正方形 1:1、縦 4:5 など）へ変換するときに、被写体が中央に来るように
切り取ります。

- **Target Aspect**: 目標の縦横比。`Sequence` はシーケンスの縦横比に合わせます。
- **Reframe Zoom**: 既定 100 %。拡大率を上げると、被写体により寄ります。
- **Motion Preset**: 追従の速さ（Slower / Default / Faster Motion）。

## 5. デバッグの手順

### 5.1 エフェクトが効かない・何も変わらない

- **警告の確認**: 実装のないエフェクト ID が描画に来ると、標準エラーに
  `warning: effect '…' has no renderer implementation and is skipped` が 1 回だけ出ます。
  ID が古いプロジェクトや、定義だけ先に入れたエフェクトで起きます。
- **パラメータの確認**: `effects.list` で ID と既定値を確認してください。
- **キャッシュの確認**: Warp Stabilizer は解析結果をメモリにキャッシュします。素材を差し替えて結果が古いままに見える場合は、
  開発者向けの `clear_stabilizer_cache()`（[temporal.rs](crates/render/src/vfx/temporal.rs)）で破棄できます。アプリがいつ自動で破棄するかは、まだ確認していません。

### 5.2 Warp Stabilizer が揺れを取らない

1. 特徴点が少ない映像（空、単色の壁、ぼけた映像）は、追跡できません。Detailed Analysis をオンにして、
   結果を比べてください。
2. Method を変えてください。遠近のある揺れは Perspective、回転のある揺れは Subspace Warp が向いています。
3. 被写体が動いている映像は、背景の動きと区別できず、補正が崩れることがあります。

### 5.3 Morph Cut が顔を合わせない

- 両側のショットで、正面向きの顔が検出されているか確認してください。顔が横向きや小さすぎる場合は、
  検出されず通常のディゾルブになります。
- 検出は縮小画像（幅 320 px）で行います。極端に小さい被写体は見つからないことがあります。

### 5.4 テストの実行

```bash
cargo test -p filmcraft-render --lib            # エフェクトとトランジションの単体テスト
cargo test -p filmcraft-project                 # エフェクト定義のテスト
```

写真を使う確認テストは、既定では実行されません（`#[ignore]`）。写真のパスを環境変数で指定します。

```bash
FILMCRAFT_FACE_SAMPLE=/path/to/face.png cargo test -p filmcraft-render --lib -- detects_a_face --ignored --nocapture
FILMCRAFT_FACE_SAMPLE=/path/to/face.png FILMCRAFT_MORPH_OUT=/tmp/morph.png \
  cargo test -p filmcraft-render --lib -- aligns_a_face --ignored --nocapture
```

PNG 形式だけ読めます。写真は、リポジトリに含めないでください。

## 5.5 VST3 オーディオプラグイン（Windows）

インストール済みの VST3 オーディオエフェクトを、組み込みのオーディオエフェクトと同じように使えます
（[ADR 0002](adr/0002-audio-plugin-hosting.md)）。

1. プラグインを `C:\Program Files\Common Files\VST3`（または `%LOCALAPPDATA%\Programs\Common\VST3`、
   環境変数 `FILMCRAFT_VST3_PATH` のフォルダ）に置きます。
2. エフェクトパネル ▸ オーディオエフェクト ▸「VST3 プラグインをスキャン…」を押します
   （コマンドでは `audioPlugins.scan`）。起動時には読み込みません。
3. 見つかったプラグインは「オーディオエフェクト ▸ VST3」に並びます。クリップへドラッグするか、
   `effects.apply {"effect": "vst3.<uid>"}`、トラックには `mixer.addInsert` で追加します。
4. パラメーターはエフェクトコントロールに `p<id>`（0〜1 の正規化値）として表示され、キーフレームも使えます。
   プラグインの状態はプロジェクトに保存されます。

プラグインが見つからないプロジェクトを開いても、そのエフェクトは音をそのまま通し、設定は保存し直されます。
プラグイン自身の画面（エディター）はまだ表示できません。プラグインがネイティブにクラッシュすると
FilmCraft も終了します（自動保存から復旧できます）。

## 6. 既知の制限

- **Warp Stabilizer の 3D**: Subspace Warp は回転だけをモデル化します。平行移動を含むカメラの動きは、部分的にしか補正されません。
  平面の合成映像では、Perspective のほうがわずかに良い結果でした。実写の 3D 映像での評価はまだです。
- **Morph Cut**: 顔の特徴点を使わず、位置と大きさを合わせるだけです。後のショットの縁が、半透明の枠として見えることがあります。
  検出は毎フレーム行われ、結果はキャッシュされていません。
- **Auto Reframe**: 被写体の認識は簡易的な方法です。Premiere Pro の AI 認識とは異なります。
- **Lumetri や一部の効果**: GPU で動くのは一部だけです。残りは CPU で処理されるため、重くなることがあります。

詳しい進捗は [ROADMAP.md](../ROADMAP.md) を参照してください。
