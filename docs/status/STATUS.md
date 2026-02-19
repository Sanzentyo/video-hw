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
  - decode: NVDEC メタデータ専用経路を接続（NV12->RGB 変換を回避）
  - decode: call ごとの reaper thread 生成を除去し、metadata 集計をインライン化
  - encode: `nvidia-video-codec-sdk` safe `Encoder/Session` を接続
  - encode: input/output buffer を in-flight 数に応じてプール再利用
  - encode tuning: backend 固有パラメータ `max_in_flight_outputs`（default: 6 に更新）
  - metrics: decode/encode stage 時間 + queue/jitter + p95/p99 出力に対応
  - 設計追補: RGB 変換を非同期 worker へ切り出す分散パイプライン設計を追加
- 分散パイプライン基盤（実装）
  - `src/pipeline.rs`: bounded queue（depth/peak 統計付き）
  - `src/pipeline.rs`: in-flight credit 制御（スロット制）
  - `src/transform.rs`: `TransformDispatcher`（NV12->RGB を CPU worker で非同期実行）
  - `src/transform.rs`: `ColorRequest::KeepNative` fast-path 判定を追加
  - `src/backend_transform_adapter.rs`: backend 差分 adapter（NV-P1-006 Phase 1）
    - `BackendTransformAdapter` trait / `DecodedUnit` 抽象
    - `NvidiaTransformAdapter`: fast-path + NV12->RGB 非同期 dispatch
    - `VtTransformAdapter`: passthrough stub
  - `examples/transform_nv12_rgb.rs`: worker 分散動作の実行例
  - `src/nv_backend.rs`: decode/encode の submit/reap 分離（worker thread）を導入
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
  - `--verify` で `ffprobe` + `ffmpeg -v error` 検証を自動実行
- 生成レポート: `output/benchmark-nv-<codec>-<timestamp>.txt`
- 手順詳細: `docs/status/FFMPEG_NV_COMPARISON_2026-02-19.md`
- 精密分析: `docs/status/NV_PRECISE_ANALYSIS_2026-02-19.md`
- 現状結果:
  - H264 decode/encode は `video-hw` / `ffmpeg` ともに比較可能
  - HEVC decode/encode も比較可能（異常終了問題は解消済み）
  - `NV-P0-004` 反映で decode が大幅改善（H264/HEVC ともに 0.3s 台）
  - encode は in-flight reap + bitstream 再利用で大幅改善
  - synthetic 入力再利用化で encode が追加改善（H264/HEVC ともに 0.24s 台）
  - decode ベンチ default chunk を `65536` に更新（HEVC decode は改善確認）
  - lock 回収最適化後の精密レポート:
    - `output/benchmark-nv-precise-h264-1771493200.md`
    - `output/benchmark-nv-precise-hevc-1771493244.md`
    - `output/benchmark-nv-precise-h264-1771493302.md`
    - `output/benchmark-nv-precise-hevc-1771493327.md`
    - `output/benchmark-nv-precise-h264-1771498123.md`
    - `output/benchmark-nv-precise-hevc-1771498128.md`
    - `output/benchmark-nv-precise-h264-1771499558.md`
    - `output/benchmark-nv-precise-hevc-1771499564.md`
    - `output/benchmark-nv-precise-h264-1771500342.md`
    - `output/benchmark-nv-precise-hevc-1771500342.md`
    - `output/benchmark-nv-precise-h264-1771500639.md`
    - `output/benchmark-nv-precise-hevc-1771500639.md`
    - `output/benchmark-nv-precise-h264-1771501008.md`
    - `output/benchmark-nv-precise-hevc-1771501008.md`
    - `output/benchmark-nv-precise-h264-1771505433.md`
    - `output/benchmark-nv-precise-hevc-1771505433.md`
  - 最新 mean（warmup 1 / repeat 3 / verify）
    - h264: video-hw decode 0.338s, encode 0.320s / ffmpeg decode 0.538s, encode 0.259s
    - hevc: video-hw decode 0.371s, encode 0.321s / ffmpeg decode 0.536s, encode 0.232s
  - verify: h264/hevc とも `ffprobe` + `ffmpeg -v error` で decode=ok

## 6. 残課題

- CI での GPU ランナー常設（Windows + NVIDIA）
- encode の品質比較（PSNR/SSIM）とビットレート比較の自動化
- encode 公平比較のための raw frame 入力 API の整理

## 7. 次セッションで着手すること（優先順）

1. 外れ値（24.677s）再現条件を固定化
   - `NV-P0-005` の再現スクリプトを作り、再現有無と条件を記録
   - 成果物: `docs/status/` に外れ値切り分けメモを追加
2. 分散パイプライン実装に着手
   - `NV-P1-004/005` の本統合と `NV-P1-006` Phase 2（NVIDIA CUDA / VT Metal-CoreImage）を実装
   - 成果物: adapter 経由で backend 実装差分を吸収しつつ decode/encode 本線を非ブロッキング化
3. 公平比較のための raw frame 入力 API 設計に着手
   - `NV-P1-001` の API 案を先に固め、`NV-P1-003` ベンチ設計へ接続

## 8. 関連文書

- `README.md`
- `docs/README.md`
- `docs/status/BENCHMARK_2026-02-18.md`
- `docs/status/FFMPEG_VT_COMPARISON_2026-02-19.md`
- `docs/status/FFMPEG_NV_COMPARISON_2026-02-19.md`
- `docs/status/NV_PRECISE_ANALYSIS_2026-02-19.md`
- `docs/plan/MASTER_INTEGRATION_STEPS_2026-02-19.md`
- `docs/plan/ROADMAP.md`
- `docs/plan/PIPELINE_TASK_DISTRIBUTION_DESIGN_2026-02-19.md`
- `docs/plan/TEST_PLAN_MULTIBACKEND.md`
- `docs/research/RESEARCH.md`
