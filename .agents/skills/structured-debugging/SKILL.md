---
name: structured-debugging
description: >-
  Systematic root-cause analysis and structured debugging methodology for Antigravity,
  FilmCraft, WebGPU, and Rust codebases. Use when investigating bugs, runtime crashes,
  rendering artifacts, shader errors, state regressions, or logical inconsistencies.
---

# Structured Debugging Guide (構造化デバッグガイド)

Anti-Gravity 環境および FilmCraft におけるバグ調査・デバッグを当たる際、当てずっぽうな場当たり修正（trial-and-error guessing）を厳禁とし、確実な論理検証と根本原因特定（Root Cause Analysis）を行うための体系的ワークフローです。

---

## 4段階の構造化デバッグプロセス

### フェーズ 1: 根本原因の特定 (Root Cause Analysis)
単に「動かない」「期待と違う」という表面的な事象を追うのではなく、システムの本質的な原因を突き止めます。

1. **事象の明確化 (Symptom Clarification)**:
   - 期待値（Expected Behavior）と実測値（Actual Behavior）の差分を正確に言語化する。
   - エラーメッセージ、ログ、スタックトレース、または視覚的グリッチの境界条件を記録する。
2. **エラーの局所化 (Error Localization)**:
   - パイプライン全体のどこ（データモデル、プラン生成、GPUアップロード、シェーダー、UIバインド）で不整合が発生しているかを特定。
   - 影響を受けるモジュール・関数・コードブロックを最小単位に切り離す。
3. **再現手順の確立 (Minimal Reproduction)**:
   - バグを100%確実に再現できる最小限の入力パラメータ、データセット、または単体テスト（Synthetic Regression Test）を作成する。

---

### フェーズ 2: 推論と仮説検証 (Hypothesis & Verification)
コードを修正する前に、必ず「なぜそのエラーが発生しているのか」の仮説を立てて検証します。

1. **複数仮説の構築 (Minimum 3 Hypotheses)**:
   - 仮説A: 「ライフサイクルや実行タイミングの不整合（初期化前アクセス、フレーム順序）」
   - 仮説B: 「データ型や座標系・スケール変換の不整合（行列乗算の単位、テクスチャ境界）」
   - 仮説C: 「境界値・例外系のハンドリング漏れ（空スライス、None/ゼロ除算、リソース枯渇）」
2. **自己反証・悪魔の代弁者 (Devil's Advocate)**:
   - 各仮説に対して「反例がないか」「論理的矛盾がないか」「他の正常系を壊さないか」を批判的に自己検証する。
3. **検証の実行 (Verification via Inspection & Mocks)**:
   - ログ出力、アサーション、テストケース、または状態のモックにより仮説を1つずつ検証し、偽の仮説を棄却する。
4. **推論プロセスの記録 (Reasoning Trail)**:
   - 「何を試し、何が違ったか」の思考ログを整理し、後続の調査での重複を防ぐ。

---

### フェーズ 3: 修正の適用と退行（デグレード）防止
1. **最小限の修正 (Minimal & Surgical Fix)**:
   - バグの根本原因を断ち切る必要最小限のコード変更のみを行う。
   - バグ修正のついでに関係のないリファクタリングを同時に行わない。
2. **副作用と不変条件の確認 (Invariants & Side Effects)**:
   - 他のモジュール、依存関係、上位レイヤーに悪影響を及ぼさないかを精査する。
   - 内部ID、JSONスキーマ、コマンド互換性を破壊していないか検証する。
3. **回帰テストの追加 (Regression Prevention)**:
   - 修正前のコードで失敗し、修正後のコードでパスする回帰テスト（Unit / Integration Test）を追加する。

---

### フェーズ 4: ドキュメント化と完了報告
- 根本原因、採用した解決策、検証結果、および追加したテストを明確に記録する。

---

## FilmCraft / Anti-Gravity 特有のデバッグチェックリスト

### 1. 「Never Crash」原則の遵守 (Rule 0)
- [ ] `unwrap()`, `expect()`, `panic!`, `unimplemented!`, `todo!` が本番コードに含まれていないか？
- [ ] スライスアクセスや配列インデックスは `get()` や境界チェックを行っているか？
- [ ] ゼロ除算の可能性（フレームレート、アスペクト比、テクスチャ解像度、タイムスケール）が排除されているか？
- [ ] 浮動小数点数（NaN、Infinity）が整数キャスト前にクランプされているか？
- [ ] ミューテックスのロックポイズニング（`lock().unwrap_or_else(PoisonError::into_inner)`）が考慮されているか？

### 2. WebGPU / グラフィックスパイプライン
- [ ] テクスチャのUsage（`RENDER_ATTACHMENT`, `TEXTURE_BINDING`, `COPY_SRC`, `COPY_DST`）は適正か？
- [ ] 描画パス（RenderPass）とコピー（CopyTexture）が同一エンコーダ内で競合・オーバーラップしていないか？
- [ ] テクスチャフォーマット（`Rgba16Float` vs `Rgba8UnormSrgb` 等）とカラー空間（Linear vs sRGB）が一致しているか？
- [ ] UniformバッファのアライメントおよびデータパディングがWGSLの型定義（`vec4` 等の16バイト境界）と一致しているか？
- [ ] テクスチャプールおよびバッファプールからの再利用時に古いデータが残留していないか（初期化・クリアの保証）？

### 3. egui / UI レンダリング
- [ ] `egui::Id` の衝突がないか（ループ内のウィジェットには一意なIDキーが付与されているか）？
- [ ] テキスト表示のみを日本語化し、内部の `app.auto` キーやコマンド識別文字列を変更していないか？
- [ ] コンテキストメニューやダイアログのポップアップがUI階層の外側で正しく閉じられているか？

---

## AI (Gemini) へのデバッグ依頼プロンプト作法
AIにデバッグ協力を求める際は、以下の3要素を明示します:
1. **コンテキスト**: 目的、関連するコード、期待される設計。
2. **エラーの詳細**: エラー全文、スタックトレース、期待値と実際の挙動の差異。
3. **試したこと**: 検証済みの仮説、試して効果のなかったアプローチ。
