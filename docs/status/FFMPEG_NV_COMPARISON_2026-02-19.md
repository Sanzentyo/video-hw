# FFmpeg(CUDA/NVENC) vs video-hw 比較レポート (2026-02-19)

## 1. 目的

`ffmpeg` の NVIDIA hardware decode/encode（`*_cuvid` / `*_nvenc`）が利用可能な環境で、
同一素材を使って `video-hw` と実測比較する。

環境: Windows + NVIDIA GPU

## 2. 事前確認（ffmpeg 機能）

- `ffmpeg -hide_banner -encoders`
  - `h264_nvenc`, `hevc_nvenc` を確認
- `ffmpeg -hide_banner -decoders`
  - `h264_cuvid`, `hevc_cuvid` を確認

## 3. 入力素材

- `sample-videos/sample-10s.h264`（1920x1080, 303 frames）
- `sample-videos/sample-10s.h265`（1920x1080, 303 frames）
- encode 比較のフレーム数基準: 300 frames

## 4. decode 比較（同一素材, null sink）

### 実行コマンド

- ffmpeg decode (CUDA+CUVID)
  - `ffmpeg -y -hide_banner -benchmark -hwaccel cuda -c:v <h264_cuvid|hevc_cuvid> -i <input> -f null NUL`
- video-hw decode (NVDEC)
  - `cargo run --release --features backend-nvidia --example decode_annexb -- --backend nv --codec <h264|hevc> --input <input> --chunk-bytes 4096 --require-hardware`

### 結果

| Codec | Path | real(s) | fps (303/real) | ms/frame |
|---|---|---:|---:|---:|
| H264 | ffmpeg cuvid decode | 0.515 | 588.3 | 1.700 |
| H264 | video-hw nv decode | 2.898 | 104.6 | 9.564 |
| HEVC | ffmpeg cuvid decode | 0.562 | 539.1 | 1.855 |
| HEVC | video-hw nv decode | 2.976 | 101.8 | 9.822 |

## 5. encode 計測

### 実行コマンド

- ffmpeg encode (NVENC)
  - `ffmpeg -y -hide_banner -benchmark -f lavfi -i testsrc2=size=640x360:rate=30 -frames:v 300 -c:v <h264_nvenc|hevc_nvenc> -preset p1 -f <h264|hevc> <output>`
- video-hw encode (NVENC)
  - `cargo run --release --features backend-nvidia --example encode_synthetic -- --backend nv --codec <h264|hevc> --fps 30 --frame-count 300 --require-hardware --output <output>`

### 結果

| Codec | Path | real(s) | fps (300/real) | ms/frame | 備考 |
|---|---|---:|---:|---:|---|
| H264 | ffmpeg nvenc encode | 0.213 | 1408.5 | 0.710 | 正常完了 |
| H264 | video-hw nv encode | 0.590 | 508.5 | 1.967 | 正常完了 |
| HEVC | ffmpeg nvenc encode | 0.275 | 1090.9 | 0.917 | 正常完了 |
| HEVC | video-hw nv encode | timeout | - | - | 30秒以内に完了せず（30 framesでもtimeout） |

## 6. 解釈

- decode は H264/HEVC ともに、現状の `video-hw` NVDEC 実装より `ffmpeg cuvid` が高速。
- H264 encode は `video-hw` でも完了し、`ffmpeg nvenc` がより高速。
- HEVC encode は `video-hw` 側でハングが再現し、比較不能。

## 7. 既知課題（要修正）

- `video-hw` の HEVC encode (`encode_synthetic --codec hevc`) が進行停止する。
- 挙動上、`src/nv_backend.rs` の NVENC flush/EOS 周辺（`NeedMoreInput` / `EncoderBusy` の扱い）に改善余地がある可能性が高い。

## 8. 重要な注意（公平比較）

- decode 比較は「同一素材 + null sink」で比較可能性が高い。
- encode は現状、`video-hw` が synthetic ARGB 入力で、ffmpeg は `testsrc2` 入力を使用。
- encode の完全公平比較には、`video-hw` 側へ同一入力データ経路（raw frame input API）の追加が必要。

## 9. 自動実行

```bash
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv.rs --codec h264 --release
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv.rs --codec hevc --release
```

- `h264` は完了し、結果は `output/benchmark-nv-h264-1771486740.txt` に保存済み。
- `hevc` は現在の既知課題により script 完走不可。
