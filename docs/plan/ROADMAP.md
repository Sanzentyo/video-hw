# Roadmap

更新日: 2026-02-19

## 現在の到達点

- root 単一 crate 構成へ整理済み
- backend 切替は `BackendKind` + feature で運用
- VideoToolbox の decode/encode は E2E まで通過
- VideoToolbox の encode は実フレーム入力（ARGB）・PTS 反映・packet順安定化まで対応
- VideoToolbox の encode session は flush 跨ぎ再利用に対応（寸法変更時のみ再生成）
- VideoToolbox の `PipelineScheduler` は decode/encode 本線に接続済み（`VIDEO_HW_VT_PIPELINE=1`）
- VideoToolbox の transform は CPU worker fallback 実装済み（`VtTransformAdapter`）
- VideoToolbox の簡易メトリクス出力を追加（`VIDEO_HW_VT_METRICS=1`）
- NVIDIA の decode/encode は `nvidia-video-codec-sdk` safe API で接続済み
- `decode_annexb` / `encode_synthetic` の examples で実行確認済み
- Criterion ベンチで `hw_optional` / `hw_required` の比較が可能
- `ffmpeg`（VideoToolbox）との同一素材比較レポートを作成済み
- `ffmpeg`（NVDEC/NVENC）比較スクリプトを追加済み
- 重複していた旧 `crates/` と `legacy-root-backup/` は削除済み

## 直近の優先タスク

1. VT parity の残作業
   - VT transform の GPU 実経路（Metal/CoreImage）を実装
   - VT session switch + generation 制御を実装
   - VT 指標を NV 同等粒度（queue/jitter/copy）へ拡張
2. VT 比較運用の固定化
   - ffmpeg VT 比較を `warmup/repeat/verify/equal-raw-input` で定常運用
3. NV 保留項目の再開
   - `NV-P1-002` safe lifetime 経路の追加最適化
   - `VIDEO_HW_NV_PIPELINE=1` 経路の soak test
4. CI 分離・安定化
   - macOS (VT) / Linux+GPU (NVIDIA) を分離して安定運用
   - GPU ランナー常設化

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
