# Docs Index

最初に `status/STATUS.md` を確認し、次に実装・仕様・計画を参照してください。

## 現行ドキュメント

- `USAGE_STRICT.md`（厳密 I/O 仕様つき利用ガイド）
- `spec/IO_FORMAT_CONTRACT.md`（I/O形式の標準化方針）
- `spec/INTEL_LINUX_TROUBLESHOOTING.md`（Intel oneVPL / libva / 権限トラブルシュート）
- `spec/NVIDIA_SDK_DISTRIBUTION_POLICY.md`（NVIDIA SDK の配布・運用ポリシー）
- `spec/TEST_SPEC_INVENTORY.md`（現存テスト仕様の棚卸し）
- `spec/FEATURE_GATING_AND_DEAD_CODE_POLICY.md`（feature有効範囲とdead code運用）
- `status/STATUS.md`
- `status/AV1_BACKEND_STATUS_2026-05-06.md`（AV1 backend / fMP4 / FFmpeg parity の完了監査）
- `status/AV1_COMPLETION_AUDIT_2026-05-06.md`（AV1 目標に対する prompt-to-artifact 監査）
- `status/BENCHMARK_2026-02-18.md`
- `status/FFMPEG_VT_COMPARISON_2026-02-19.md`
- `status/FFMPEG_NV_COMPARISON_2026-02-19.md`
- `benchmark/BACKEND_FFMPEG_BENCHMARKS.md`（NV/Intel/Vulkan/VT と FFmpeg の統合 benchmark 手順）
- `benchmark/FMP4_DECODE_ACCESS_BENCHMARKS.md`（連続/単発/ランダム decode と `CachedFrameDecoder` の benchmark 手順）
- `../scripts/README.md`（スクリプト実装方針）

## 責務境界

- `video-hw-core::bitstream` は backend 非依存の codec payload utility です。Annex-B / length-prefixed sample 変換、NALU 分割、parameter set 抽出、access unit assembly を扱います。
- `video-hw-fmp4` は fMP4/container と sample entry 管理を担当します。raw frame writer と encoded stream writer は typestate session で分岐します。
- WebRTC/RTP/signaling/GUI/relay policy はこの repository の責務外で、上位アプリが `video-hw-core::bitstream` と `video-hw-fmp4` の public API を組み合わせます。

## 計画

- `plan/ROADMAP.md`
- `plan/VULKAN_AV1_IMPLEMENTATION_PLAN_2026-05-06.md`（Vulkan AV1 decode/encode の実装計画）
- `plan/WEBRTC_VIDEO_TYPESTATE_INTEGRATION_REQUEST_2026-05-07.md`（`webrtc-video` 連携で必要な typestate/newtype API 追加依頼）
- `plan/NEXT_ACTION_PLAN_2026-02-23.md`（deepresearchベースの短期実行計画）
- `plan/API_REDESIGN_BLUEPRINT_2026-02-21.md`（互換非維持の新API設計）
- `plan/PIPELINE_TASK_DISTRIBUTION_DESIGN_2026-02-19.md`
- `plan/TEST_PLAN_MULTIBACKEND.md`

## 設計・調査メモ

- `research/deepresearch.md`（調査本文 + 現行反映メモ）
- `research/RESEARCH.md`
- `research/highlevel_layer.md`
- `research/RUST_ANALYZER_BACKEND_WORKSPACES.md`（rust-analyzer backend切替運用）

## 履歴/引き継ぎ

- `history/HANDOFF_CONTEXT_2026-02-18.md`
- `plan/MIGRATION_AND_REBUILD_GUIDE.md`（再構成検討時の履歴文書）
