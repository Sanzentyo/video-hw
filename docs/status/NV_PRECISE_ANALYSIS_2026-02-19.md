# NVIDIA Precise Performance Analysis (2026-02-19)

## 1. 目的

`nv_backend` の encode 出力回収戦略（in-flight 出力 + 遅延回収 + bitstream 再利用）を実装し、
`lock_ms` と encode 実時間が改善するかを精密計測で検証する。

## 2. 比較対象

- 実装前（基準）:
  - 主要: `output/benchmark-nv-precise-h264-1771492187.md`
  - 主要: `output/benchmark-nv-precise-hevc-1771492232.md`
  - 内訳: `output/benchmark-nv-precise-h264-1771492317.md`
  - 内訳: `output/benchmark-nv-precise-hevc-1771492347.md`
- 実装後:
  - 主要: `output/benchmark-nv-precise-h264-1771493200.md`
  - 主要: `output/benchmark-nv-precise-hevc-1771493244.md`
  - 内訳: `output/benchmark-nv-precise-h264-1771493302.md`
  - 内訳: `output/benchmark-nv-precise-hevc-1771493327.md`

## 3. 主要結果（repeat=9）

### 3.1 H264

| Case | Before mean(s) | After mean(s) | 差分 |
|---|---:|---:|---:|
| video-hw decode | 2.657 | 2.678 | +0.8% |
| video-hw encode | 0.581 | 0.279 | -52.0% |
| ffmpeg decode | 0.497 | 0.509 | +2.4% |
| ffmpeg encode | 0.202 | 0.212 | +5.0% |

### 3.2 HEVC

| Case | Before mean(s) | After mean(s) | 差分 |
|---|---:|---:|---:|
| video-hw decode | 2.568 | 2.587 | +0.7% |
| video-hw encode | 0.550 | 0.260 | -52.7% |
| ffmpeg decode | 0.485 | 0.502 | +3.5% |
| ffmpeg encode | 0.201 | 0.209 | +4.0% |

## 4. 内部メトリクス（repeat=5, include-internal-metrics）

### 4.1 H264 encode 内訳

| Metric | Before | After | 差分 |
|---|---:|---:|---:|
| synth_ms mean | 33.065 | 34.898 | +5.5% |
| upload_ms mean | 17.831 | 17.914 | +0.5% |
| encode_ms mean | 54.652 | 44.724 | -18.2% |
| lock_ms mean | 221.052 | 22.543 | -89.8% |
| queue_peak mean | 1.000 | 4.000 | +300% |

### 4.2 HEVC encode 内訳

| Metric | Before | After | 差分 |
|---|---:|---:|---:|
| synth_ms mean | 32.678 | 31.770 | -2.8% |
| upload_ms mean | 18.553 | 18.161 | -2.1% |
| encode_ms mean | 66.077 | 50.584 | -23.4% |
| lock_ms mean | 189.317 | 5.752 | -96.9% |
| queue_peak mean | 1.000 | 4.000 | +300% |

## 5. 判定

- 目標 `lock_ms mean 20%以上削減`: 達成（H264 -89.8%、HEVC -96.9%）
- 目標 `video-hw encode mean 5%以上改善`: 達成（H264 -52.0%、HEVC -52.7%）
- 目標 `queue_peak >= 2`: 達成（H264/HEVC ともに 4.0）
- E2E 安定性: 維持（`cargo test --features backend-nvidia` pass）

## 6. 解釈

- 主ボトルネックだった `lock_ms` を大幅に圧縮できた。
- 改善要因は、submit と reap の分離で lock の待機を非同期化できた点。
- decode は今回の変更対象外で、差分は誤差範囲。
- ffmpeg との比較では encode 差は残るが、相対比は大きく縮小:
  - H264 encode: `0.279 / 0.212 ~= 1.32x`
  - HEVC encode: `0.260 / 0.209 ~= 1.24x`

## 7. max_in_flight チューニング結果（2/4/8）

`NVIDIA backend 固有パラメータ (max_in_flight_outputs)` を変化させて
`warmup=1, repeat=5, include-internal-metrics` で比較。

### 7.1 H264 encode

| max_in_flight | mean(s) | lock_ms mean | queue_peak mean |
|---|---:|---:|---:|
| 2 | 0.286 | 27.098 | 2.0 |
| 4 | 0.277 | 26.542 | 4.0 |
| 8 | 0.279 | 24.715 | 8.0 |

### 7.2 HEVC encode

| max_in_flight | mean(s) | lock_ms mean | queue_peak mean |
|---|---:|---:|---:|
| 2 | 0.275 | 17.367 | 2.0 |
| 4 | 0.261 | 7.941 | 4.0 |
| 8 | 0.264 | 6.446 | 8.0 |

結論:
- 現環境では `4` が H264/HEVC 両方で encode mean 最良（または同等最良）。
- `8` は `lock_ms` 自体は改善するが、total encode mean では `4` を超えない。
- 実装デフォルトは `4` に固定し、必要時のみ backend 固有パラメータで上書き可能。

## 8. 再現コマンド

```bash
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec h264 --release --warmup 2 --repeat 9
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec hevc --release --warmup 2 --repeat 9

cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec h264 --release --warmup 1 --repeat 5 --include-internal-metrics
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec hevc --release --warmup 1 --repeat 5 --include-internal-metrics

cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec h264 --release --warmup 1 --repeat 5 --include-internal-metrics --nv-max-in-flight 2
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec h264 --release --warmup 1 --repeat 5 --include-internal-metrics --nv-max-in-flight 4
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec h264 --release --warmup 1 --repeat 5 --include-internal-metrics --nv-max-in-flight 8

cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec hevc --release --warmup 1 --repeat 5 --include-internal-metrics --nv-max-in-flight 2
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec hevc --release --warmup 1 --repeat 5 --include-internal-metrics --nv-max-in-flight 4
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec hevc --release --warmup 1 --repeat 5 --include-internal-metrics --nv-max-in-flight 8
```
