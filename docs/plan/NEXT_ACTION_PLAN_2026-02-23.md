# Next Action Plan (2026-02-23)

更新日: 2026-03-08
対象期間: 2026-02-23 〜 2026-03-22（4週間）

## 0. 方針（deepresearch 反映）

この計画は `docs/research/deepresearch.md` の提案を、現行 workspace 実装へ接続するための実行台帳として扱う。

- docs drift を先に止血し、実装と資料を一致させる
- API 契約の「型と実挙動の差」を明文化する
- NVIDIA 依存はラッパー + SDK 外部配置前提を維持する
- CI は後段導入とし、当面は手動検証を継続する

## 1. 進捗（2026-02-23）

完了:
- docs baseline の一次是正（`STATUS` / `ROADMAP` / `IO_FORMAT_CONTRACT` / `TEST_SPEC_INVENTORY`）
- API 契約の明文化（decode は Metadata 中心、encode は ARGB 系のみ受理）
- ライセンス/配布整備（`LICENSE-*` / `NOTICE` / `THIRD_PARTY_NOTICES` / `NVIDIA_SDK_DISTRIBUTION_POLICY`）
- `cargo deny check licenses advisories bans sources` 実行

継続:
- `deepresearch.md` の提案内容と現行 workspace 実装のマッピング
- docs 間の参照整合（path・用語・運用前提）
- encode 入力契約の feature 隔離運用（`unstable-raw-inputs`）の定着

保留:
- CI 導入（runner 設計と運用者確定後）

## 2. 目的

`docs/research/deepresearch.md` で特定された課題を、短期で実行可能なタスクへ落とし込む。

- docs drift の解消
- API 契約の不一致（decode 出力 / encode 入力）の明確化
- ライセンス・配布面の最低限整備
- CI 設計の準備（導入自体は後段）

## 3. 現在地（2026-02-23時点）

- workspace 実装とテストは稼働している（`backend-nvidia` 付きもこの環境で pass）
- ただし docs は過去時点の記述が混在し、実装との乖離がある
- 公開APIの型と実挙動に差がある
  - decode は実質 `DecodedFrame::Metadata` 中心
  - encode 入力は ARGB 限定（`Nv12` / `Rgb24` は reject）
- ライセンス関連ファイルは整備済み（NOTICE/THIRD_PARTY含む）
- CI は後回しのため未整備

## 4. 実行計画

### Week 1: Docs Baseline Fix（完了）

1. `STATUS.md` / `TEST_SPEC_INVENTORY.md` / `ROADMAP.md` を同期
2. docs の「実装済み」と「計画中」を明確に分離
3. `docs/README.md` の索引を更新

完了条件:
- 実在しないファイル・機能が status/spec から消える
- workspace `cargo test` 結果と docs 記載が一致する

### Week 2: API Contract Clarification（完了）

1. `RawFrameBuffer` のサポート範囲を仕様化（ARGB only を明記）
2. `DecodedFrame` の現行出力モード（Metadata中心）を明記
3. 未サポート型の将来方針（`unstable` または削除）を決定

完了条件:
- `docs/spec/IO_FORMAT_CONTRACT.md` と `USAGE_STRICT.md` に実装準拠の記述が入る
- エラー契約（`InvalidInput` / `UnsupportedConfig`）の期待が E2E と一致する

### Week 3: Licensing and Distribution Hygiene（完了）

1. ルートに `LICENSE-MIT` / `LICENSE-APACHE` / `NOTICE` / `THIRD_PARTY_NOTICES` を追加
2. NVIDIA SDK 依存の取り扱い（同梱しない前提）を README と docs へ明記
3. 依存ライセンス確認フロー（`cargo deny` 等）を導入

完了条件:
- 配布時に参照するライセンスファイルが揃う
- NVIDIA 依存の配布ポリシーが明文化される

### Week 4: Docs/Bench Follow-up（進行中）

1. `deepresearch.md` と現行 workspace の差分を追記で可視化（本文破壊なし）
2. bench/テストの直近実測値を `STATUS.md` に継続反映
3. 計画文書の古い前提（root 単一crate、root canonical など）を除去

完了条件:
- deepresearch と現行資料を読み比べたとき、前提差分が追跡可能
- 現行運用の一次資料が `STATUS` / `ROADMAP` / `NEXT_ACTION_PLAN` で一致する

### CI Plan (Deferred)

1. CI 導入の前提条件（運用者・runner・保守コスト）を確定
2. NVIDIA 経路は optional job（self-hosted runner 前提）で設計のみ先行
3. benchmark は nightly/manual job 方針を設計し、導入時期を別決定

完了条件:
- CI 導入判断に必要な要件と運用コストが文書化される
- 導入時に即着手できるジョブ設計が残る

## 5. リスクと対策

- リスク: NVIDIA 環境依存で再現性が揺れる
  - 対策: skip 条件と理由を統一し、GPU job を分離
- リスク: API 契約変更で利用側が混乱
  - 対策: まず docs で明示し、コード変更は段階的に行う
- リスク: deepresearch 本文と現行実装の時点差で誤読が起きる
  - 対策: deepresearch は履歴調査として保持し、現行差分は追記節で管理する

## 6. マイルストーン

1. M1（2026-03-01）: docs drift 解消
2. M2（2026-03-08）: API 契約の現行仕様化
3. M3（2026-03-15）: ライセンス/配布の最低限整備
4. M4（2026-03-22）: docs/bench 運用を定常化（CI は別マイルストーン）

## 7. 参照

- `docs/research/deepresearch.md`
- `docs/status/STATUS.md`
- `docs/spec/TEST_SPEC_INVENTORY.md`
- `docs/plan/ROADMAP.md`
- `docs/plan/API_REDESIGN_BLUEPRINT_2026-02-21.md`
