# NV Lock-Time Reduction Execution Plan (2026-02-19)

対象ブランチ: `feat/nv-precise-performance-analysis-2026-02-19`

## 1. 目的

`nv_backend` の encode 経路で支配的になっている `lock_ms` を削減し、`video-hw encode` の実効時間（mean/p95）を改善する。  
優先は「低遅延を維持しつつ、CPU待機時間を減らす」こと。

## 2. 現状整理（基準値）

`docs/status/NV_PRECISE_ANALYSIS_2026-02-19.md` の内部メトリクスより:

- H264 encode: `lock_ms mean ~= 221ms`
- HEVC encode: `lock_ms mean ~= 189ms`
- `queue_peak ~= 1.0`（ほぼ逐次 submit/lock）

示唆:
- 現状は encode 出力回収が同期直列化され、submit と lock の重なりが小さい。

## 3. 仮説

1. 出力 bitstream の in-flight 数を増やし、回収を遅延/バッチ化すると `lock` 時点の待ちが減る。
2. 出力バッファをプール再利用すると、回収と再投入のオーバーヘッドが下がる。
3. 上記を backend 内部だけで成立できるが、継続運用するなら抽象層に「チューニング/メトリクス」の受け口が必要。

## 4. 実装方針

## 4.1 Phase A（抽象層変更なしで実装）

- 対象: `src/nv_backend.rs`
- 変更:
  - `reap_threshold`（例: 2/4/8）を導入し、`encode_picture` 後すぐ lock しない。
  - `pending_outputs` をリング運用し、閾値超過時に先頭から回収。
  - `create_output_bitstream` を毎回生成ではなく、可能な範囲で再利用。
  - `VIDEO_HW_NV_METRICS` に `queue_peak` と `lock_ms` を継続出力。
- 期待:
  - `queue_peak > 1` が安定して観測される。
  - `lock_ms` mean/p95 が低下する。

## 4.2 Phase B（必要時のみ抽象層を拡張）

Phase A で最適点が backend 固有パラメータ依存になる場合に実施:

- 対象:
  - `src/contract.rs`
  - `src/lib.rs`
  - `src/nv_backend.rs`
- 変更候補:
  - `Encoder` 初期化に runtime tuning 引数を追加（例: `max_in_flight_outputs`）。
  - encode メトリクス取得 API を trait に追加（環境変数依存を縮小）。
- 方針:
  - 既存 API 互換を維持（デフォルト値で従来挙動）。
  - backend 非依存の名前で契約化し、NV 固有値はオプション化。

## 5. 検証計画

## 5.1 機能検証

- `cargo fmt --all`
- `cargo check --features backend-nvidia`
- `cargo test --features backend-nvidia -- --nocapture`

## 5.2 性能検証（主要）

- 反復計測:
  - `cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec h264 --release --warmup 2 --repeat 9`
  - `cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec hevc --release --warmup 2 --repeat 9`
- 内部内訳:
  - `cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec h264 --release --warmup 1 --repeat 5 --include-internal-metrics`
  - `cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec hevc --release --warmup 1 --repeat 5 --include-internal-metrics`

## 5.3 受け入れ基準

- `video-hw encode mean(s)` が H264/HEVC でそれぞれ **5%以上改善**。
- `lock_ms mean` が H264/HEVC でそれぞれ **20%以上改善**。
- `queue_peak` が 1 固定から脱し、`>=2` を観測。
- E2E と HEVC 安定性（クラッシュ再発なし）を維持。

## 6. リスクと対策

- リスク: in-flight を増やすと tail latency が悪化する可能性。
  - 対策: `mean` だけでなく `p95/p99` と `CV` を必須評価。
- リスク: HEVC で再び不安定化。
  - 対策: codec 別に閾値を分ける設計を許容。
- リスク: backend 固有設定が散在。
  - 対策: Phase B で契約層に吸収し、設定入口を一本化。

## 7. 成果物

- コード変更（Phase A 必須、Phase B 必要時）
- 比較レポート更新:
  - `docs/status/NV_PRECISE_ANALYSIS_2026-02-19.md`
  - `docs/status/FFMPEG_NV_COMPARISON_2026-02-19.md`
  - `docs/status/STATUS.md`
- 主要ベンチ結果ファイル（`output/benchmark-nv-precise-*.md`）

## 8. 実行順序

1. Phase A 実装
2. 機能テスト
3. 精密ベンチ（before/after 比較）
4. 改善有無を判定
5. 必要なら Phase B の契約拡張へ進む
