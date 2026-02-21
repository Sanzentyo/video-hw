# FFmpeg(VideoToolbox) vs video-hw 比較レポート (2026-02-19)

## 1. 目的

`ffmpeg` のハードウェアデコード/エンコード（VideoToolbox）が利用可能な環境で、
同一素材を使って `video-hw` と実測比較する。

環境: M5 MacBook Pro

## 2. 事前確認（ffmpeg 機能）

- `ffmpeg -hide_banner -hwaccels`
  - `videotoolbox` を確認
- `ffmpeg -hide_banner -encoders`
  - `h264_videotoolbox`, `hevc_videotoolbox` を確認

## 3. 入力素材

- `sample-videos/sample-10s.h264`（1920x1080, yuv420p, 25fps）
- `sample-videos/sample-10s.h265`（1920x1080, yuv420p, 25fps）
- 比較時のフレーム数基準: 303 frames

## 4. decode 比較（同一素材, null sink）

### 実行コマンド

- ffmpeg SW decode
  - `/usr/bin/time -lp ffmpeg -benchmark -v error -stats -i <input> -f null -`
- ffmpeg HW decode
  - `/usr/bin/time -lp ffmpeg -benchmark -v error -stats -hwaccel videotoolbox -i <input> -f null -`
- video-hw decode
  - `/usr/bin/time -lp cargo run --release --example decode_annexb -- --codec <codec> --input <input> --chunk-bytes 4096 --require-hardware`

### 結果

| Codec | Path | real(s) | fps (303/real) | ms/frame |
|---|---|---:|---:|---:|
| H264 | ffmpeg SW decode | 0.22 | 1377.3 | 0.726 |
| H264 | ffmpeg VT decode | 0.94 | 322.3 | 3.102 |
| H264 | video-hw VT decode | 0.38 | 797.4 | 1.254 |
| HEVC | ffmpeg SW decode | 0.30 | 1010.0 | 0.990 |
| HEVC | ffmpeg VT decode | 0.74 | 409.5 | 2.442 |
| HEVC | video-hw VT decode | 0.37 | 818.9 | 1.221 |

## 5. encode 計測（ffmpeg VT）

### 実行コマンド

- `/usr/bin/time -lp ffmpeg -benchmark -v error -stats -i <input> -an -c:v h264_videotoolbox -f null -`
- `/usr/bin/time -lp ffmpeg -benchmark -v error -stats -i <input> -an -c:v hevc_videotoolbox -f null -`

### 結果

| Input | Encoder | real(s) | fps (303/real) | ms/frame |
|---|---|---:|---:|---:|
| H264 sample | h264_videotoolbox | 1.36 | 222.8 | 4.488 |
| H264 sample | hevc_videotoolbox | 1.33 | 227.8 | 4.389 |
| HEVC sample | h264_videotoolbox | 1.32 | 229.5 | 4.356 |
| HEVC sample | hevc_videotoolbox | 1.31 | 231.3 | 4.323 |

## 6. 解釈

- この素材・条件では ffmpeg の decode は SW が最速で、VT decode は相対的に遅い。
- `video-hw` の VT decode は ffmpeg VT decode より高速だが、ffmpeg SW decodeには届かない。
- 短尺素材（約10秒）では、初期化/同期/転送オーバーヘッドの比率が高くなりやすい。

## 7. 重要な注意（公平比較）

- decode 比較は「同一素材 + null sink」で比較可能性が高い。
- encode は現状、`video-hw` が `examples/encode_synthetic.rs`（合成フレーム入力）であり、
  ffmpeg の「同一素材入力 encode」と厳密に同条件ではない。
- encode の完全公平比較には、`video-hw` 側へ「同一素材入力で encode」する経路追加が必要。

## 8. 精密再計測（warmup/repeat/verify/equal-raw-input）

実行コマンド:

```bash
cargo +nightly -Zscript scripts/benchmark_ffmpeg_vt_precise.rs --codec h264 --release --warmup 1 --repeat 3 --verify --equal-raw-input --include-internal-metrics
cargo +nightly -Zscript scripts/benchmark_ffmpeg_vt_precise.rs --codec hevc --release --warmup 1 --repeat 3 --verify --equal-raw-input --include-internal-metrics
```

定常運用（直列実行）:

```bash
cargo +nightly -Zscript scripts/run_vt_precise_suite.rs
```

生成レポート:
- `output/benchmark-vt-precise-h264-1771530053.md`
- `output/benchmark-vt-precise-hevc-1771530065.md`

結果（mean, 秒）:

| Codec | video-hw decode | video-hw encode | ffmpeg decode | ffmpeg encode |
|---|---:|---:|---:|---:|
| H264 | 0.172 | 0.328 | 0.895 | 0.307 |
| HEVC | 0.162 | 0.382 | 0.757 | 0.352 |

所見:
- decode は H264/HEVC とも `video-hw` が `ffmpeg videotoolbox` より高速（約4.7〜5.2x）。
- encode は現条件で `video-hw` が `ffmpeg videotoolbox` より遅い（約1.07x）。
- 反復のばらつきは小さく、`video-hw` は CV 2% 未満。

検証メモ:
- `ffmpeg` 出力は `ffprobe` + `ffmpeg -v error` で decode=ok。
- `video-hw` 出力は現状 raw payload 形式のため `ffprobe` が直接解釈できない場合がある。
  このためスクリプト側で「出力バイト数 > 0」を fallback 検証として扱っている。
- `--include-internal-metrics` 有効時は `Internal Metrics (video-hw)` を出力し、
  NV 精密レポートと同一セクション構成（`decode` / `encode`）で比較可能。

## 9. 再計測ログ（2026-02-21）

実行コマンド:

```bash
cargo +nightly -Zscript scripts/run_vt_precise_suite.rs --warmup 1 --repeat 3 --verify --equal-raw-input --include-internal-metrics
```

生成レポート:
- `output/benchmark-vt-precise-h264-1771651558.md`
- `output/benchmark-vt-precise-hevc-1771651567.md`

結果（mean, 秒）:

| Codec | video-hw decode | video-hw encode | ffmpeg decode | ffmpeg encode |
|---|---:|---:|---:|---:|
| H264 | 0.176 | 0.334 | 0.853 | 0.304 |
| HEVC | 0.168 | 0.381 | 0.825 | 0.356 |

比較（2026-02-19 の直近値との関係）:
- decode は今回も `video-hw` が `ffmpeg videotoolbox` より大幅に速い（約 4.9x〜5.0x）。
- encode は今回も `video-hw` が `ffmpeg videotoolbox` より遅い（約 1.09x〜1.13x）。
- 傾向は 2026-02-19 の計測と同じで、decode 優位 / encode 劣位の構図は不変。
