# Roadmap

更新日: 2026-02-19

## 現在の到達点

- root 単一 crate 構成へ整理済み
- backend 切替は `BackendKind` + feature で運用
- VideoToolbox の decode/encode は E2E まで通過
- NVIDIA の decode/encode は `nvidia-video-codec-sdk` safe API で接続済み
- `decode_annexb` / `encode_synthetic` の examples で実行確認済み
- Criterion ベンチで `hw_optional` / `hw_required` の比較が可能
- `ffmpeg`（VideoToolbox）との同一素材比較レポートを作成済み
- `ffmpeg`（NVDEC/NVENC）比較スクリプトを追加済み
- 重複していた旧 `crates/` と `legacy-root-backup/` は削除済み

## 直近の優先タスク

1. パイプライン分散化（CPU/GPU）
   - `PipelineScheduler` による submit/reap/transform/egress の分離
   - RGB 変換を非同期 worker（GPU優先、CPU fallback）へ移動
2. E2E の NVIDIA 実機検証拡充
   - Linux + GPU 環境で sample video ベースの回帰テストを追加
3. CI 分離
   - macOS (VT) / Linux+GPU (NVIDIA) を分離して安定運用
4. encode 比較の公平化
   - `video-hw` に同一素材入力 encode 経路を追加し、`ffmpeg` 比較条件を統一
5. VT parity セッション
   - `docs/plan/VT_PARITY_EXECUTION_PLAN_2026-02-19.md` に沿って VT を NV 同等水準へ引き上げる

## 受け入れ条件

- VT/NVIDIA の双方で同一 trait API で decode/encode が呼べる
- sample video ベースで backend ごとの E2E が再現可能
- README と docs の実行手順が実装と一致している

## 関連文書

- `docs/status/STATUS.md`
- `docs/status/FFMPEG_VT_COMPARISON_2026-02-19.md`
- `docs/plan/PIPELINE_TASK_DISTRIBUTION_DESIGN_2026-02-19.md`
- `docs/plan/VT_PARITY_EXECUTION_PLAN_2026-02-19.md`
- `docs/plan/TEST_PLAN_MULTIBACKEND.md`
- `docs/plan/MIGRATION_AND_REBUILD_GUIDE.md`
- `docs/research/RESEARCH.md`
