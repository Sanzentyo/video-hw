# Scripts Policy

このリポジトリの `scripts/` は、原則として **Cargo Script**（RFC 3424 / Cargo issue #12207 の `-Zscript` 形式）で実装します。

## ルール

- 新規スクリプトは `scripts/*.rs` で追加する。
- ファイル先頭は以下の形式にする。

```rust
#!/usr/bin/env -S cargo +nightly -Zscript
---cargo
[package]
edition = "2024"

[dependencies]
# 必要な依存
---
```

- `ps1` / `sh` は Cargo Script で表現しづらい場合のみ許可。

## 実行方法

### 1) 直接実行

```bash
cargo +nightly -Zscript scripts/<name>.rs <args>
```

### 2) NVIDIA ベンチマーク

```bash
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv.rs --codec h264 --release
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv.rs --codec hevc --release
```

- 生成レポート: `output/benchmark-nv-<codec>-<epoch>.txt`

### 3) NVIDIA 精密ベンチ（反復 + 統計）

```bash
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec h264 --release --warmup 2 --repeat 9
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec hevc --release --warmup 2 --repeat 9
```

- 生成レポート: `output/benchmark-nv-precise-<codec>-<epoch>.md`
- 既定で `--equal-raw-input=true`。encode 比較では `video-hw` / `ffmpeg` に同一 raw ARGB 入力を供給する。
- decode 比較は既定で `--decode-output-mode metadata`。FFmpeg null-sink decode と条件を揃える。
- レポートに encode/decode の平均スループット差分（ffmpeg 比、±10% 以内または video-hw が高速なら PASS）を出力する。
- `--include-internal-metrics` を付けると `VIDEO_HW_NV_METRICS=1` を有効化し、
  `nv_backend` の decode/encode ステージ内訳も集計する。
- NVIDIA backend 固有パラメータ（`max_in_flight_outputs`）を変える場合は
  `--nv-max-in-flight <N>` を使用する（未指定時は default `6`）。

### 4) VideoToolbox 精密ベンチ（反復 + 統計）

```bash
cargo +nightly -Zscript scripts/benchmark_ffmpeg_vt_precise.rs --codec h264 --release --warmup 2 --repeat 9
cargo +nightly -Zscript scripts/benchmark_ffmpeg_vt_precise.rs --codec hevc --release --warmup 2 --repeat 9
```

- 生成レポート: `output/benchmark-vt-precise-<codec>-<epoch>.md`
- `--verify` で `ffprobe` + `ffmpeg -v error` 検証を実行する。
- `--equal-raw-input` で `video-hw` / `ffmpeg` encode に同一 raw ARGB 入力を供給する。
- `--include-internal-metrics` で `VIDEO_HW_VT_METRICS=1` を有効化し、
  `Internal Metrics (video-hw)` セクションを NV 精密レポートと同形式で出力する。

### 5) VideoToolbox 精密ベンチ定常運用（直列実行）

```bash
cargo +nightly -Zscript scripts/run_vt_precise_suite.rs
```

- 既定は `warmup=1`, `repeat=3`, `verify=true`, `equal-raw-input=true`, `include-internal-metrics=true`
- H264 と HEVC を同時ではなく順番に実行する

### 6) Intel 精密ベンチ（反復 + 統計）

```bash
cargo +nightly -Zscript scripts/benchmark_ffmpeg_intel_precise.rs --codec h264 --release --warmup 2 --repeat 9
cargo +nightly -Zscript scripts/benchmark_ffmpeg_intel_precise.rs --codec hevc --release --warmup 2 --repeat 9
```

- 生成レポート: `output/benchmark-intel-precise-<codec>-<epoch>.md`
- `video-hw` は `backend=intel` で実行し、`--require-hardware <true|false>` で HW 固定 / fallback 許可を切り替え可能
- `ffmpeg` は `h264_qsv` / `hevc_qsv` を使用
- レポートに encode/decode の平均スループット差分（ffmpeg 比、±10% 判定）を出力
- encode の厳密比較は `--equal-raw-input` を推奨（`video-hw` / `ffmpeg` に同一 raw ARGB 入力を供給）
- HEVC で parity を詰める場合は `--equal-raw-input --raw-input-pix-fmt nv12` を推奨（直近 report 1773584282 で decode/encode とも ±10% 内）
- 既定で `--grouped-cases=true`（caseごとに warmup/measure をまとめて実行）になっており、round-robin より計測ばらつきを抑えやすい
- 既定値は `--warmup 2` / `--repeat 7` / `--decode-loops 10`（短窓ノイズを抑えて parity 判定を安定化）
- `--settle-ms <N>` で各計測の間に待機を入れて、熱/スケジューリング揺れを緩和できる
- decode 側チューニングは `--intel-decode-async-depth <N>`（1..=16）で `VIDEO_HW_INTEL_DECODE_ASYNC_DEPTH` を video-hw decode ケースへ注入できる（backend 側 default は 16）
- encode 側チューニングは環境変数 `VIDEO_HW_INTEL_RATE_CONTROL` / `VIDEO_HW_INTEL_CQP` / `VIDEO_HW_INTEL_ASYNC_DEPTH` / `VIDEO_HW_INTEL_HEVC_USE_VPP` / `VIDEO_HW_INTEL_HEVC_LOW_POWER` で調整できる（未指定時は H.264=CBR、HEVC=CQP、CQP=24、async_depth=10、HEVCはCPU NV12投入を優先、low-power は有効）
- runtime 依存で一部ケースが失敗する環境では `--allow-case-failures` を付けると失敗ケースを記録したままレポート生成を継続
- `--allow-case-failures --verify` 併用時、失敗ケースで出力が欠けた検証対象は `skipped` としてレポートに記録する

### 7) Intel oneVPL fallback セットアップ（Windows CLI）

```bash
# dry-run（実行内容確認）
cargo +nightly -Zscript scripts/setup_onevpl.rs

# 実行
cargo +nightly -Zscript scripts/setup_onevpl.rs --apply

# 既存ディレクトリを消してやり直し
cargo +nightly -Zscript scripts/setup_onevpl.rs --apply --force
```

- 生成した `mfx.h` / `libvpl.dll` に合わせた `LIBVPL_INCLUDE_PATH` / `PATH` の設定例を出力する（`LIBVPL_LIBRARY_PATH` は通常不要）
- setup 後の検証コマンド（`clippy` / `test`）も出力する

### 8) fMP4 Lazy index 検証

```bash
cargo +nightly -Zscript scripts/verify_fmp4_lazy.rs sample-videos/sample-10s.mp4
cargo +nightly -Zscript scripts/verify_fmp4_lazy.rs sample-videos/foreman_cif_fmp4.mp4
cargo +nightly -Zscript scripts/verify_fmp4_lazy.rs sample-videos/sample-10s.mp4 --decode-features "backend-nvidia backend-intel backend-vulkan" --decode-backend auto
```

- `IndexMode::Eager` が open 時に metadata index を構築することを確認する。
- `IndexMode::Lazy` が open 時に sample metadata を構築しないことを確認する。
- `next_sample` と `read_sample(SampleId)` が必要な位置までだけ index を延長することを確認する。
- `samples(track)` が完全な metadata slice を返すため EOF まで index 化することを確認する。
- first / middle / last checkpoint で `sample_at_pts_with_delta` の `Exact`、GOP lookup、`index_snapshot`、`clear_cache` を確認する。
- `--decode-features` を渡すと `read_fmp4_slider_gui --smoke-test` を子プロセスで起動し、checkpoint GOP decode と `DecodeDiagnostics` 出力を確認する。
- 通常 MP4 と fragmented MP4 の両方で使える。

### 9) Vulkan HEVC PSNR 検証

```bash
cargo +nightly -Zscript scripts/check_vulkan_hevc_psnr.rs
cargo +nightly -Zscript scripts/check_vulkan_hevc_psnr.rs --input sample-videos/foreman_cif.h265 --min-psnr-y 40
cargo +nightly -Zscript scripts/check_vulkan_hevc_encode_probe.rs --adapter-index 1 --width 320 --height 180 --min-psnr-y 60
```

- `decode_to_yuv` の `backend-vulkan` HEVC NV12 出力を FFmpeg software decode の NV12 と raw-vs-raw で比較する。
- 既定入力は `sample-videos/foreman_cif.h265`、既定しきい値は frame 単位の `psnr_y` 最小値 40 dB。
- slice offset の診断をしたい場合は `--offset-mode annexb|rbsp|nalu|global|memory` を指定する。
- `FFMPEG_PATH` または `--ffmpeg` で FFmpeg 実行ファイルを指定できる。
- `check_vulkan_hevc_encode_probe.rs` は FFmpeg `hevc_vulkan` で生成した parameter/header NAL を使って ignored live encode probe の出力sliceをFFmpeg decodeし、probe入力の平坦NV12（Y=16/UV=128）に対するMSE/PSNRを確認する。

### 10) NVIDIA HEVC decode PSNR 検証

```bash
cargo +nightly -Zscript scripts/check_nvidia_decode_psnr.rs
cargo +nightly -Zscript scripts/check_nvidia_decode_psnr.rs --input sample-videos/foreman_cif.h265 --min-psnr-y 40
```

- `decode_to_yuv` の `backend-nvidia` HEVC NV12 出力を FFmpeg software decode の NV12 と raw-vs-raw で比較する。
- 既定入力は `sample-videos/foreman_cif.h265`、既定しきい値は frame 単位の `psnr_y` 最小値 40 dB。
- `FFMPEG_PATH` または `--ffmpeg` で FFmpeg 実行ファイルを指定できる。

### 11) 統合 backend / FFmpeg ベンチ

```bash
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends nv,intel,vulkan --codec h264 --warmup 1 --repeat 5
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends nv,intel,vulkan --codecs h264,hevc --warmup 1 --repeat 5
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends vulkan --codec h264 --vulkan-adapter-indexes 0,1 --allow-failures true
```

- 生成レポート: `output/benchmark-backends-<codec>-<epoch>.md`
- `--codecs h264,hevc` を指定すると、codecごとに同じbackend集合を測り、codec別の統合レポートを生成する。
- Windows/Linux では `nv,intel,vulkan`、macOS では `vt` が既定。
- NV/Intel/VT は子レポートの parity / verification 結果も統合statusへ反映する。
- Vulkan は `vulkaninfo --summary` と `list_vulkan_adapters` の結果を名前/IDで対応付け、adapter ごとに `video-hw` decode/encode と FFmpeg Vulkan decode/encode を記録する。
- FFmpeg Vulkan の adapter 指定は `-init_hw_device vulkan:<index>` を使う。Windows hybrid GPU 環境では `vulkan=vk:<index>` が指定した物理デバイスを選ばず、NVIDIA encode ケースが Intel 側へ流れることがあった。
- `cargo run -p video-hw --features backend-vulkan --example list_vulkan_adapters` で `video-hw` / `vk-video` 側に見えている Vulkan adapter を確認できる。
- adapter ごとに失敗理由も report に残すため、複数 GPU 環境では `--allow-failures true` で全候補を走査する。
- Vulkan HEVC encode は NVIDIA adapter 上で実験的な IDR-only production path を測定できる。runner は FFmpeg `hevc_vulkan` で同サイズの parameter/header sample を生成し、名前/vendor/device idで対応したFFmpeg Vulkan adapter番号を使って `VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_SAMPLE_PATH` を自動設定する。現状は1 frameごとに Vulkan video session を作り直すため性能 parity は未達で、失敗adapterや非公開adapterも report に残す。診断用には `cargo +nightly -Zscript scripts/check_vulkan_hevc_encode_probe.rs --adapter-index <n>`、または `cargo test -p video-hw-backend-vulkan --features backend-vulkan live_hevc_encode_session_bootstrap_reports_submit_feedback -- --ignored --nocapture` を明示実行する。

### 12) fMP4 decode access pattern benchmark

```bash
cargo +nightly -Zscript scripts/benchmark_fmp4_decode_access.rs -- --input sample-videos/foreman_cif.mp4 --backend auto --frame-count 90
```

- `decode_range_iter`、単発 `decode_sample`、`CachedFrameDecoder` を同じ入力で比較する。
- 逆順 cold access と prefetch 方向差（`reverse_before` / `reverse_after`）も同じレポートで比較する。
- FFmpeg RGB24 reference に対する max MSE / min PSNR も同じレポートへ記録する。
- Windows/Linux の既定 features は `backend-nvidia backend-intel backend-vulkan`、macOS は `backend-vt`。
- 詳細は `docs/benchmark/FMP4_DECODE_ACCESS_BENCHMARKS.md` を参照。

## 前提

- `nightly` ツールチェーンが利用可能であること
- `cargo -Zscript` が有効な Cargo であること
- ベンチ用途では `ffmpeg` が必要（Intel 精密ベンチは QSV 有効環境が必要）
- NVIDIA ベンチ用途では NVIDIA ドライバ / CUDA 実行環境が必要
