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
| H264 | ffmpeg cuvid decode | 0.509 | 595.3 | 1.680 |
| H264 | video-hw nv decode | 2.678 | 113.1 | 8.838 |
| HEVC | ffmpeg cuvid decode | 0.502 | 603.6 | 1.657 |
| HEVC | video-hw nv decode | 2.587 | 117.1 | 8.538 |

## 5. encode 計測

### 実行コマンド

- ffmpeg encode (NVENC)
  - `ffmpeg -y -hide_banner -benchmark -f lavfi -i testsrc2=size=640x360:rate=30 -frames:v 300 -c:v <h264_nvenc|hevc_nvenc> -preset p1 -f <h264|hevc> <output>`
- video-hw encode (NVENC)
  - `cargo run --release --features backend-nvidia --example encode_synthetic -- --backend nv --codec <h264|hevc> --fps 30 --frame-count 300 --require-hardware --output <output>`

### 結果（最新）

| Codec | Path | real(s) | fps (300/real) | ms/frame | 備考 |
|---|---|---:|---:|---:|---|
| H264 | ffmpeg nvenc encode | 0.212 | 1415.1 | 0.707 | 正常完了 |
| H264 | video-hw nv encode | 0.279 | 1075.3 | 0.930 | 正常完了 |
| HEVC | ffmpeg nvenc encode | 0.209 | 1435.4 | 0.697 | 正常完了 |
| HEVC | video-hw nv encode | 0.260 | 1153.8 | 0.867 | 正常完了 |

## 6. 解釈

- decode は H264/HEVC ともに、現状の `video-hw` NVDEC 実装より `ffmpeg cuvid` が高速。
- encode は H264/HEVC ともに `video-hw` が完走し、`ffmpeg nvenc` がより高速。
- HEVC encode の異常終了は解消し、比較計測が可能になった。

## 7. 実装メモ（今回の修正）

- NVIDIA encode の出力回収を in-flight 化（submit/reap 分離）し、bitstream を再利用。
- `max_in_flight_outputs` は NVIDIA backend 固有パラメータとして公開し、default は `4`。
- この変更で `lock_ms` が大幅に低下し、H264/HEVC encode の wall time が改善した。

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
  - `output/benchmark-nv-precise-h264-1771493200.md`
  - `output/benchmark-nv-precise-hevc-1771493244.md`
  - 詳細分析: `docs/status/NV_PRECISE_ANALYSIS_2026-02-19.md`
