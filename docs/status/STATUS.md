# video-hw Status

更新日: 2026-02-21

## 1. 現在の構成

- root の `src/` に実装を集約した単一 crate 構成
- `BackendKind`（VideoToolbox / NVIDIA）で実行時切替
- feature で backend 実装を有効化
  - default: なし（`default = []`）
  - macOS: `backend-vt` を明示有効化
  - Linux/Windows: `backend-nvidia` を明示有効化

## 2. 実装済み

- VideoToolbox decode/encode 実装
- NVIDIA decode/encode 実装（`src/nv_backend.rs`）
  - decode: NVDEC メタデータ専用経路を接続（NV12->RGB 変換を回避）
  - decode: call ごとの reaper thread 生成を除去し、metadata 集計をインライン化
  - encode: `nvidia-video-codec-sdk` safe `Encoder/Session` を接続
  - encode: input/output buffer を in-flight 数に応じてプール再利用
  - encode: `Frame.argb` を優先して入力 upload（未指定時のみ synthetic fallback）
  - encode: CUDA context を flush 跨ぎで再利用
  - encode: `NvidiaEncoderOptions` で `gop_length` / `frame_interval_p` 指定をサポート
  - encode: `Frame.force_keyframe` を `NV_ENC_PIC_FLAG_FORCEIDR` にマップ
  - encode: `Encoder::request_session_switch(SessionSwitchRequest)` を追加（NVIDIA最小実装）
  - encode: `NvEncodeSession`（`Pin<Box<Session>>` + reusable buffer pool）を導入し、flush 跨ぎで再利用
  - encode: session switch は `Session::reconfigure` を優先し、失敗時のみ再作成
  - encode: `pending_switch` 状態を導入し、`OnNextKeyframe` 切替を保留適用
  - encode: session generation（`active/config/next`）を導入し、切替適用世代を明示管理
  - encode: `VIDEO_HW_NV_SAFE_LIFETIME=1` で safe lifetime 経路（per-frame buffer）を選択可能化
  - encode: safe lifetime 経路を flush 内ローカルプール再利用に最適化（per-frame buffer 作成を回避）
  - encode: `VIDEO_HW_NV_PIPELINE=1` で `PipelineScheduler` を encode 本線前処理に接続（generation 同期つき）
  - encode tuning: backend 固有パラメータ `max_in_flight_outputs`（default: 6 に更新）
  - metrics: decode/encode stage 時間 + queue/jitter + p95/p99 出力に対応
  - metrics: encode copy 計測（`input_copy_bytes`, `output_copy_bytes`）を追加
  - 設計追補: RGB 変換を非同期 worker へ切り出す分散パイプライン設計を追加
- 分散パイプライン基盤（実装）
  - `src/pipeline.rs`: bounded queue（depth/peak 統計付き）
  - `src/pipeline.rs`: in-flight credit 制御（スロット制）
  - `src/transform.rs`: `TransformDispatcher`（NV12->RGB を CPU worker で非同期実行）
  - `src/transform.rs`: `ColorRequest::KeepNative` fast-path 判定を追加
  - `src/backend_transform_adapter.rs`: backend 差分 adapter（NV-P1-006）
    - `BackendTransformAdapter` trait / `DecodedUnit` 抽象
    - `NvidiaTransformAdapter`: KeepNative fast-path + CUDA NV12->RGB 優先（失敗時 CPU worker fallback）
    - `VtTransformAdapter`: Metal NV12->RGB 優先 + CPU worker fallback
  - `src/cuda_transform.rs`: CUDA kernel（NVRTC）による NV12->RGB 変換実体
  - `src/vt_metal_transform.rs`: Metal compute shader による NV12->RGB 変換実体
  - `src/pipeline_scheduler.rs`: `BackendTransformAdapter` を使う submit/reap スケジューラ
  - `src/pipeline_scheduler.rs`: generation 制御（`submit_with_generation` / `set_generation` / stale drop）を追加
  - `src/nv_backend.rs`: `PipelineScheduler` 連携前処理（KeepNative fast-path）を接続
  - `src/vt_backend.rs`: `PipelineScheduler` 連携前処理（decode/encode）を接続
  - `src/vt_backend.rs`: `VIDEO_HW_VT_PIPELINE` / `VIDEO_HW_VT_PIPELINE_QUEUE` トグルを追加
  - `src/backend_transform_adapter.rs`: `VIDEO_HW_VT_GPU_TRANSFORM` トグルを追加（既定有効）
  - `src/vt_backend.rs`: `VIDEO_HW_VT_METRICS` で decode/encode 計測ログを追加
  - `src/vt_backend.rs`: VT decode/encode 計測に queue/jitter/copy 指標を追加
  - `src/vt_backend.rs`: VT decode metadata に callback 由来の `pts/decode_info_flags/color` を追加
  - `src/vt_backend.rs`: VT encode session の flush 跨ぎ再利用を実装（解像度変更時のみ再生成）
  - `src/vt_backend.rs`: VT encode の session switch + generation 制御を実装
  - `src/vt_backend.rs`: `configured_generation` / `pending_switch_generation` / `sync_pipeline_generation` を追加（NV と同契約）
  - `src/vt_backend.rs`: VT encode packet の keyframe 判定を payload 解析ベースへ更新
  - `src/vt_backend.rs`: VT encode packet 出力順を frame index 基準で安定化
  - `examples/transform_nv12_rgb.rs`: worker 分散動作の実行例
  - `examples/encode_raw_argb.rs`: raw ARGB 入力で encode する実行例
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
- `cargo test -- --nocapture`: pass
- `cargo test --features backend-vt -- --nocapture`: pass
- `cargo test --features backend-vt --test e2e_video_hw --no-run`: pass
- `cargo bench --features backend-vt --bench decode_bench --no-run`: pass
  - `decode_bench` は VT/NV の両 backend マトリクス実行に対応（有効 feature/OS に応じて自動選択）
- `cargo check --all-targets --features backend-nvidia`: fail（実行環境依存）
  - `nvcc --version` が見つからない
  - `NVIDIA_VIDEO_CODEC_SDK_PATH` 未設定で `libnvidia-encode` / `libnvcuvid` 未検出

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
  - encode は in-flight reap + buffer 再利用を中心に改善を継続中
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
    - `output/benchmark-nv-precise-h264-1771513976.md`
    - `output/benchmark-nv-precise-hevc-1771513976.md`
    - `output/benchmark-nv-precise-h264-1771514429.md`（外れ値軽試行）
    - `output/benchmark-nv-precise-h264-1771514448.md`（repeat=5）
    - `output/benchmark-nv-precise-hevc-1771514450.md`（repeat=5）
    - `output/benchmark-nv-precise-h264-1771514780.md`（repeat=3, verify）
    - `output/benchmark-nv-precise-hevc-1771514780.md`（repeat=3, verify）
    - `output/benchmark-nv-precise-h264-1771515974.md`（repeat=3, verify）
    - `output/benchmark-nv-precise-hevc-1771515974.md`（repeat=3, verify）
    - `output/benchmark-nv-precise-h264-1771517285.md`（repeat=1, verify）
    - `output/benchmark-nv-precise-h264-1771517463.md`（repeat=1, verify）
    - `output/benchmark-nv-precise-h264-1771518104.md`（repeat=1, verify）
    - `output/benchmark-nv-precise-h264-1771519379.md`（repeat=1, verify）
    - `output/benchmark-nv-precise-h264-1771519756.md`（repeat=1, verify）
    - `output/benchmark-nv-precise-h264-1771520277.md`（repeat=1, verify）
    - `output/benchmark-nv-precise-hevc-1771520285.md`（repeat=1, verify）
    - `output/benchmark-nv-precise-h264-1771520908.md`（repeat=1, verify, safe-lifetime）
    - `output/benchmark-nv-precise-h264-1771520915.md`（repeat=1, verify）
    - `output/benchmark-nv-precise-h264-1771521536.md`（repeat=1, verify, safe-lifetime）
    - `output/benchmark-nv-precise-h264-1771521543.md`（repeat=1, verify）
    - `output/benchmark-nv-precise-h264-1771521720.md`（repeat=3, verify, equal-raw-input）
    - `output/benchmark-nv-precise-h264-1771521734.md`（repeat=3, verify, equal-raw-input, safe-lifetime）
    - `output/benchmark-nv-precise-hevc-1771521747.md`（repeat=3, verify, equal-raw-input）
    - `output/benchmark-nv-precise-hevc-1771521759.md`（repeat=3, verify, equal-raw-input, safe-lifetime）
    - `output/benchmark-nv-precise-h264-1771522777.md`（repeat=3, verify, equal-raw-input）
    - `output/benchmark-nv-precise-hevc-1771522791.md`（repeat=3, verify, equal-raw-input）
    - `output/benchmark-nv-precise-h264-1771522805.md`（repeat=3, verify, equal-raw-input, safe-lifetime）
    - `output/benchmark-nv-precise-hevc-1771522818.md`（repeat=3, verify, equal-raw-input, safe-lifetime）
    - `output/benchmark-nv-precise-h264-1771522938.md`（repeat=3, verify, equal-raw-input, 最新）
    - `output/benchmark-nv-precise-h264-1771523551.md`（repeat=1, verify, equal-raw-input, pipeline-on）
    - `output/benchmark-nv-precise-h264-1771523753.md`（repeat=1, verify, equal-raw-input, pipeline-on, 最新）
    - `output/benchmark-nv-precise-h264-1771515386.md`（repeat=3, verify, equal-raw-input）
    - `output/benchmark-nv-precise-hevc-1771515398.md`（repeat=3, verify, equal-raw-input）
  - 最新 mean（warmup 1 / repeat 3 / verify）
    - h264: video-hw decode 0.365s, encode 0.324s / ffmpeg decode 0.546s, encode 0.265s
    - hevc: video-hw decode 0.394s, encode 0.317s / ffmpeg decode 0.517s, encode 0.266s
  - repeat=5（include-internal-metrics）
    - h264: video-hw decode 0.289s, encode 0.271s / ffmpeg decode 0.588s, encode 0.229s
    - hevc: video-hw decode 0.305s, encode 0.258s / ffmpeg decode 0.543s, encode 0.208s
  - 外れ値軽試行（h264, warmup 0 / repeat 1 / verify）:
    - `output/benchmark-nv-precise-h264-1771514429.md` では 24.677s ケースは非再現
  - 最新（warmup 1 / repeat 3 / verify）:
    - h264: video-hw decode 0.291s, encode 0.324s / ffmpeg decode 0.547s, encode 0.217s
    - hevc: video-hw decode 0.312s, encode 0.316s / ffmpeg decode 0.536s, encode 0.254s
  - 同一 raw 入力（warmup 1 / repeat 3 / verify / equal-raw-input）:
    - h264: video-hw decode 0.286s, encode 0.467s / ffmpeg decode 0.493s, encode 0.228s
    - hevc: video-hw decode 0.326s, encode 0.435s / ffmpeg decode 0.495s, encode 0.218s
  - 直近軽試行（warmup 1 / repeat 1 / verify）:
    - h264: video-hw decode 0.294s, encode 0.310s / ffmpeg decode 0.523s, encode 0.203s
    - hevc: video-hw decode 0.303s, encode 0.290s / ffmpeg decode 0.467s, encode 0.202s
  - safe lifetime 軽試行（warmup 0 / repeat 1 / verify）:
    - h264: video-hw decode 0.364s, encode 1.011s / ffmpeg decode 0.535s, encode 0.218s
  - safe lifetime 軽試行（再計測, warmup 0 / repeat 1 / verify）:
    - h264: video-hw decode 0.448s, encode 0.391s / ffmpeg decode 0.571s, encode 0.237s
  - 実運用寄り条件（warmup 1 / repeat 3 / verify / equal-raw-input）:
    - h264（通常）: video-hw decode 0.300s, encode 0.487s / ffmpeg decode 0.509s, encode 0.229s
    - h264（safe-lifetime）: video-hw decode 0.293s, encode 0.479s / ffmpeg decode 0.527s, encode 0.235s
    - hevc（通常）: video-hw decode 0.304s, encode 0.455s / ffmpeg decode 0.495s, encode 0.227s
    - hevc（safe-lifetime）: video-hw decode 0.292s, encode 0.458s / ffmpeg decode 0.505s, encode 0.233s
  - 実運用寄り条件（直列再計測, warmup 1 / repeat 3 / verify / equal-raw-input）:
    - h264（通常）: video-hw decode 0.288s, encode 0.475s / ffmpeg decode 0.494s, encode 0.229s
    - h264（safe-lifetime）: video-hw decode 0.282s, encode 0.475s / ffmpeg decode 0.506s, encode 0.231s
    - hevc（通常）: video-hw decode 0.315s, encode 0.446s / ffmpeg decode 0.498s, encode 0.230s
    - hevc（safe-lifetime）: video-hw decode 0.325s, encode 0.444s / ffmpeg decode 0.484s, encode 0.234s
    - h264（最新確認）: video-hw decode 0.286s, encode 0.457s / ffmpeg decode 0.480s, encode 0.224s
  - verify: h264/hevc とも `ffprobe` + `ffmpeg -v error` で decode=ok

- VT 精密計測スクリプト: `scripts/benchmark_ffmpeg_vt_precise.rs`（cargo script）
  - `warmup/repeat/verify/equal-raw-input` をサポート
  - `--include-internal-metrics` で `Internal Metrics (video-hw)` を NV と同形式で出力
  - 定常運用ランナー: `scripts/run_vt_precise_suite.rs`（H264/HEVC を直列実行）
  - 直近再計測（warmup 1 / repeat 3 / verify / equal-raw-input）:
    - h264: video-hw decode 0.172s, encode 0.328s / ffmpeg decode 0.895s, encode 0.307s
    - hevc: video-hw decode 0.162s, encode 0.382s / ffmpeg decode 0.757s, encode 0.352s
  - 再計測（2026-02-21, warmup 1 / repeat 3 / verify / equal-raw-input）:
    - h264: video-hw decode 0.176s, encode 0.334s / ffmpeg decode 0.853s, encode 0.304s
    - hevc: video-hw decode 0.168s, encode 0.381s / ffmpeg decode 0.825s, encode 0.356s
  - レポート:
    - `output/benchmark-vt-precise-h264-1771530053.md`
    - `output/benchmark-vt-precise-hevc-1771530065.md`
    - `output/benchmark-vt-precise-h264-1771651558.md`
    - `output/benchmark-vt-precise-hevc-1771651567.md`
  - 注意:
    - `video-hw` 出力は raw payload 形式のため、`ffprobe` が直接解釈できない場合がある
    - スクリプトでは fallback として `output_bytes > 0` の検証を併用

## 6. 残課題

1. NV 運用タスク
   - `NV-P1-002` safe lifetime 経路の追加最適化（保留中の再開）
   - `VIDEO_HW_NV_PIPELINE=1` 経路の soak test（長時間・複数条件）
2. 共通運用基盤
   - CI での GPU ランナー常設（Windows/Linux + NVIDIA）
   - encode 品質比較（PSNR/SSIM）と bitrate 比較の自動化

## 6.1 将来タスク（保留）

- `NV-P1-002` の safe lifetime 追加最適化は保留（現状は運用可能、最適化は次回）
- 品質比較（PSNR/SSIM）と bitrate 比較の自動化は保留
- GPU ランナー常設 CI は保留（運用基盤タスク）
- マルチストリーム backpressure 最適化（`NV-P2-001`）は保留
- canary/rollback 手順の整備（`NV-P2-002`）は保留

## 7. 次セッションで着手すること（優先順）

1. NV 保留タスクを再開
   - `NV-P1-002` safe lifetime の追加最適化 + pipeline-on soak test

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
- `docs/plan/NV_SESSION_ARCHITECTURE_REDESIGN_2026-02-19.md`
- `docs/plan/NV_RAW_INPUT_ZERO_COPY_CONTRACT_2026-02-19.md`
- `docs/plan/VT_PARITY_EXECUTION_PLAN_2026-02-19.md`
- `docs/plan/TEST_PLAN_MULTIBACKEND.md`
- `docs/research/RESEARCH.md`
