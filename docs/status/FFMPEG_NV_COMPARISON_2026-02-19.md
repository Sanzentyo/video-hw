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

### 結果（最新）

| Codec | Path | real(s) | fps (303/real) | ms/frame |
|---|---|---:|---:|---:|
| H264 | ffmpeg cuvid decode | 0.485 | 624.7 | 1.601 |
| H264 | video-hw nv decode | 2.958 | 102.4 | 9.762 |
| HEVC | ffmpeg cuvid decode | 0.491 | 616.9 | 1.621 |
| HEVC | video-hw nv decode | 2.773 | 109.3 | 9.152 |

## 5. encode 計測

### 実行コマンド

- ffmpeg encode (NVENC)
  - `ffmpeg -y -hide_banner -benchmark -f lavfi -i testsrc2=size=640x360:rate=30 -frames:v 300 -c:v <h264_nvenc|hevc_nvenc> -preset p1 -f <h264|hevc> <output>`
- video-hw encode (NVENC)
  - `cargo run --release --features backend-nvidia --example encode_synthetic -- --backend nv --codec <h264|hevc> --fps 30 --frame-count 300 --require-hardware --output <output>`

### 結果（最新）

| Codec | Path | real(s) | fps (300/real) | ms/frame | 備考 |
|---|---|---:|---:|---:|---|
| H264 | ffmpeg nvenc encode | 0.203 | 1477.8 | 0.677 | 正常完了 |
| H264 | video-hw nv encode | 0.745 | 402.7 | 2.483 | 正常完了 |
| HEVC | ffmpeg nvenc encode | 0.201 | 1492.5 | 0.670 | 正常完了 |
| HEVC | video-hw nv encode | 0.713 | 420.8 | 2.377 | 正常完了 |

## 6. 解釈

- decode は H264/HEVC ともに、現状の `video-hw` NVDEC 実装より `ffmpeg cuvid` が高速。
- encode は H264/HEVC ともに `video-hw` が完走し、`ffmpeg nvenc` がより高速。
- HEVC encode の異常終了は解消し、比較計測が可能になった。

## 7. 実装メモ（今回の修正）

- NVIDIA encode の出力回収を「submit順に `lock()` で回収する」方式へ変更。
- `try_lock` と複雑なリトライ分岐を除去し、NVENC の同期的な取得順に合わせた。
- この変更で HEVC encode (`encode_synthetic --codec hevc`) の `STATUS_ACCESS_VIOLATION` は再現しなくなった。

## 8. 重要な注意（公平比較）

- decode 比較は「同一素材 + null sink」で比較可能性が高い。
- encode は現状、`video-hw` が synthetic ARGB 入力で、ffmpeg は `testsrc2` 入力を使用。
- encode の完全公平比較には、`video-hw` 側へ同一入力データ経路（raw frame input API）の追加が必要。

## 9. 自動実行

```bash
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv.rs --codec h264 --release
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv.rs --codec hevc --release
```

- 最新結果ファイル:
  - `output/benchmark-nv-h264-1771489556.txt`
  - `output/benchmark-nv-hevc-1771489564.txt`
