# video-hw Status

更新日: 2026-02-19

## 1. 現在の構成

- root の `src/` に実装を集約した単一 crate 構成
- `BackendKind`（VideoToolbox / NVIDIA）で実行時切替
- feature で backend 実装を有効化
  - default: `backend-vt`
  - optional: `backend-nvidia`

## 2. 実装済み

- VideoToolbox decode/encode 実装
- NVIDIA decode/encode 実装（`src/nv_backend.rs`）
  - decode: `nvidia-video-codec-sdk` safe `Decoder` を接続
  - encode: `nvidia-video-codec-sdk` safe `Encoder/Session` を接続
- 増分 Annex-B parser + AU 組み立て
- root examples
  - `examples/decode_annexb.rs`
  - `examples/encode_synthetic.rs`
- E2E
  - `tests/e2e_video_hw.rs`（VT + NVIDIA）
- decode benchmark（Criterion）
  - `benches/decode_bench.rs`

## 3. 検証結果（最新）

- `cargo fmt --all`: pass
- `cargo check`: pass
- `cargo check --features backend-nvidia`: pass
- `cargo test -- --nocapture`: pass
- `cargo test --features backend-nvidia -- --nocapture`: pass
  - NVIDIA E2E は CUDA/NVDEC/NVENC が使えない環境では skip 分岐あり

## 4. NVIDIA 依存固定

- `nvidia-video-codec-sdk`
  - `https://github.com/Sanzentyo/nvidia-video-codec-sdk`
  - rev: `d2d0fec631365106d26adfe462f3ce15b043b879`
- `cudarc = 0.19.2`

## 5. ffmpeg 比較

- スクリプト: `scripts/benchmark_ffmpeg_nv.rs`（cargo script）
- 精密計測スクリプト: `scripts/benchmark_ffmpeg_nv_precise.rs`（cargo script）
- 生成レポート: `output/benchmark-nv-<codec>-<timestamp>.txt`
- 手順詳細: `docs/status/FFMPEG_NV_COMPARISON_2026-02-19.md`
- 精密分析: `docs/status/NV_PRECISE_ANALYSIS_2026-02-19.md`
- 現状結果:
  - H264 decode/encode は `video-hw` / `ffmpeg` ともに比較可能
  - HEVC decode/encode も比較可能（異常終了問題は解消済み）
  - lock 回収最適化後の精密レポート:
    - `output/benchmark-nv-precise-h264-1771493200.md`
    - `output/benchmark-nv-precise-hevc-1771493244.md`
    - `output/benchmark-nv-precise-h264-1771493302.md`
    - `output/benchmark-nv-precise-hevc-1771493327.md`

## 6. 残課題

- CI での GPU ランナー常設（Windows + NVIDIA）
- encode の品質比較（PSNR/SSIM）とビットレート比較の自動化
- encode 公平比較のための raw frame 入力 API の整理

## 7. 関連文書

- `README.md`
- `docs/README.md`
- `docs/status/BENCHMARK_2026-02-18.md`
- `docs/status/FFMPEG_VT_COMPARISON_2026-02-19.md`
- `docs/status/FFMPEG_NV_COMPARISON_2026-02-19.md`
- `docs/status/NV_PRECISE_ANALYSIS_2026-02-19.md`
- `docs/plan/MASTER_INTEGRATION_STEPS_2026-02-19.md`
- `docs/plan/ROADMAP.md`
- `docs/plan/TEST_PLAN_MULTIBACKEND.md`
- `docs/research/RESEARCH.md`
