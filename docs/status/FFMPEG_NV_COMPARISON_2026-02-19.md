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
  - `cargo run --release --features backend-nvidia --example decode_annexb -- --backend nv --codec <h264|hevc> --input <input> --chunk-bytes 65536 --require-hardware`

### 結果（最新）

| Codec | Path | real(s) | fps (303/real) | ms/frame |
|---|---|---:|---:|---:|
| H264 | ffmpeg cuvid decode | 0.507 | 597.6 | 1.673 |
| H264 | video-hw nv decode | 0.300 | 1010.0 | 0.990 |
| HEVC | ffmpeg cuvid decode | 0.469 | 646.1 | 1.548 |
| HEVC | video-hw nv decode | 0.329 | 920.7 | 1.086 |

## 5. encode 計測

### 実行コマンド

- ffmpeg encode (NVENC)
  - `ffmpeg -y -hide_banner -benchmark -f lavfi -i testsrc2=size=640x360:rate=30 -frames:v 300 -c:v <h264_nvenc|hevc_nvenc> -preset p1 -f <h264|hevc> <output>`
- video-hw encode (NVENC)
  - `cargo run --release --features backend-nvidia --example encode_synthetic -- --backend nv --codec <h264|hevc> --fps 30 --frame-count 300 --require-hardware --output <output>`

### 結果（最新）

| Codec | Path | real(s) | fps (300/real) | ms/frame | 備考 |
|---|---|---:|---:|---:|---|
| H264 | ffmpeg nvenc encode | 0.200 | 1500.0 | 0.667 | 正常完了 |
| H264 | video-hw nv encode | 0.246 | 1219.5 | 0.820 | 正常完了 |
| HEVC | ffmpeg nvenc encode | 0.199 | 1507.5 | 0.663 | 正常完了 |
| HEVC | video-hw nv encode | 0.244 | 1229.5 | 0.813 | 正常完了 |

## 6. 解釈

- decode は `NV-P0-004`（NV12->RGB 変換回避）反映後、H264/HEVC ともに `video-hw` が `ffmpeg cuvid` を上回る結果となった。
- encode は H264/HEVC ともに `video-hw` が完走し、`ffmpeg nvenc` がより高速。
- HEVC encode の異常終了は解消し、比較計測が可能になった。

## 7. 実装メモ（今回の修正）

- NVIDIA encode の出力回収を in-flight 化（submit/reap 分離）し、bitstream を再利用。
- `max_in_flight_outputs` は NVIDIA backend 固有パラメータとして公開し、default は `4`。
- synthetic 入力の生成をフレームごとの全画素更新から再利用方式に変更し、encode wall time を追加改善。
- decode ベンチの既定 chunk を `65536` に更新し、HEVC decode は再計測で改善（2.587s -> 2.536s）。
- `NV-P0-004`: NVDEC のメタデータ専用経路を導入し、SDK 側の NV12->RGB 変換を回避。
- ベンチスクリプトに `--verify` を追加し、`ffprobe` と `ffmpeg -v error` による自動検証を実施可能化。

## 8. 重要な注意（公平比較）

- decode 比較は「同一素材 + null sink」で比較可能性が高い。
- encode は現状、`video-hw` が synthetic ARGB 入力で、ffmpeg は `testsrc2` 入力を使用。
- encode の完全公平比較には、`video-hw` 側へ同一入力データ経路（raw frame input API）の追加が必要。

## 9. 次段の実装方針（分散パイプライン）

- RGB 変換や resize などの CPU/GPU タスクは decode/encode 本線と別スレッドへオフロードする。
- backend 差分（NVIDIA / VT）は adapter 層で吸収し、上位は共通 queue/scheduler 契約で扱う。
- 設計詳細: `docs/plan/PIPELINE_TASK_DISTRIBUTION_DESIGN_2026-02-19.md`

## 10. 自動実行

```bash
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv.rs --codec h264 --release
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv.rs --codec hevc --release
# 検証込み
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec h264 --warmup 1 --repeat 3 --include-internal-metrics --verify
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec hevc --warmup 1 --repeat 3 --include-internal-metrics --verify
```

- 最新結果ファイル:
  - `output/benchmark-nv-precise-h264-1771498123.md`
  - `output/benchmark-nv-precise-hevc-1771498128.md`
  - 詳細分析: `docs/status/NV_PRECISE_ANALYSIS_2026-02-19.md`
