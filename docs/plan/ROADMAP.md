# Roadmap

更新日: 2026-03-08

## 現在の到達点（実装ベース）

- workspace 構成（`crates/video-hw-core` + `crates/video-hw`）へ移行済み
- `BackendKind` + feature による VT/NV backend 切替
- submit/reap/flush モデルの公開 API を提供
- NV backend で decode/encode の E2E が成立（環境依存 skip を含む）
- VT/NV とも session switch API を公開
- ライセンス文書を追加（`LICENSE-MIT` / `LICENSE-APACHE` / `NOTICE`）
- NVIDIA backend はラッパー依存（`nvidia-video-codec-sdk`）+ SDK外部配置前提

## 現在の優先課題

1. docs drift
- 実装と docs の同期を継続運用にする

2. API 契約の明確化
- decode 出力（Metadata中心）と encode 入力（ARGB中心）の現行制約を明示する

3. 配布/運用基盤
- ライセンスファイルと依存ポリシーを整備
- GitHub CI は導入しない。backend 別 test/bench は実機で手動実行し、結果を docs に記録する

## 直近4週間（短期計画）

詳細: `docs/plan/NEXT_ACTION_PLAN_2026-02-23.md`

- Week 1: docs baseline fix
- Week 2: API contract clarification
- Week 3: licensing and distribution hygiene
- Week 4: bench/テスト再測定と docs 反映（GitHub CI は対象外）

## 中期（2026 Q2）

- core/facade/backend の責務分離（crate 分割を含む設計検討）
- decode 出力モード（Metadata/NV12/RGB）の正式契約化
- encode 経路の copy/lock ボトルネックの再測定と改善

## 長期（2026 H2 以降）

- マルチベンダ拡張（Intel/AMD/OS標準API経路の実装検討）
- zero-copy 共有メモリ契約（DMABUF / IOSurface 等）の整理
- cross-platform benchmark / quality comparison 自動化

## 関連文書

- `docs/status/STATUS.md`
- `docs/plan/NEXT_ACTION_PLAN_2026-02-23.md`
- `docs/plan/API_REDESIGN_BLUEPRINT_2026-02-21.md`
- `docs/spec/TEST_SPEC_INVENTORY.md`
- `docs/research/deepresearch.md`
