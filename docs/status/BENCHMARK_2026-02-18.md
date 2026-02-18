# Decode Benchmark Report (2026-02-18)

## 1. 実行条件

- 実行コマンド
  - `/usr/bin/time -lp cargo bench --bench decode_bench -- --noplot`
- ベンチ対象
  - `benches/decode_bench.rs`（Criterion）
  - ケース: `h264/hevc × chunk_4096/chunk_1048576`
- 入力サンプル
  - `sample-videos/sample-10s.h264`: 7,262,620 bytes
  - `sample-videos/sample-10s.h265`: 4,061,877 bytes
- デコードフレーム基準
  - いずれも 303 frames（E2E 検証値）

## 2. 生値（Criterion median）

| Case | Time (ms) | Throughput (MiB/s) |
|---|---:|---:|
| H264 / chunk=4096 | 125.13 | 55.35 |
| H264 / chunk=1048576 | 123.52 | 56.07 |
| HEVC / chunk=4096 | 120.27 | 32.21 |
| HEVC / chunk=1048576 | 112.16 | 34.54 |

補足:
- 実行時に `Unable to complete 100 samples in 5.0s` 警告あり（統計は取得済み）
- `Gnuplot not found` のため plotters backend で描画

## 3. 導出指標（1フレーム当たり）

算出式:
- `ms_per_frame = median_ms / 303`
- `effective_fps = 303 / (median_ms / 1000)`
- `bytes_per_frame = input_size / 303`

| Case | ms/frame | effective fps | bytes/frame |
|---|---:|---:|---:|
| H264 / chunk=4096 | 0.41297 | 2421.48 | 23969.04 |
| H264 / chunk=1048576 | 0.40766 | 2453.04 | 23969.04 |
| HEVC / chunk=4096 | 0.39693 | 2519.33 | 13405.53 |
| HEVC / chunk=1048576 | 0.37017 | 2701.50 | 13405.53 |

## 4. chunk サイズ感度

- H264: `4096 -> 1048576` で **1.29% 改善**
  - `125.13ms -> 123.52ms`
  - ほぼ頭打ちで、chunk 依存は小さい
- HEVC: `4096 -> 1048576` で **6.74% 改善**
  - `120.27ms -> 112.16ms`
  - chunk 境界由来のオーバーヘッド影響がまだ見える

## 5. 実行プロセス指標（/usr/bin/time）

- `real`: 63.43s
- `user`: 11.93s
- `sys`: 5.10s
- `maximum resident set size`: 64,864,256 bytes
- `peak memory footprint`: 33,538,576 bytes

## 6. まとめ

- 1フレーム当たり処理時間は約 `0.37ms ~ 0.41ms`。
- H264 は chunk の影響が小さく、現在の実装で安定。
- HEVC は大chunkで改善余地を示し、入力分割オーバーヘッド最適化の効果が残る。
- 継続計測では Criterion 設定（sample/time）を調整し、警告を減らした比較運用が望ましい。