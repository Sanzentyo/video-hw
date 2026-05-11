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
cargo +nightly -Zscript scripts/benchmark_ffmpeg_vt_precise.rs --codec av1 --release --warmup 1 --repeat 3 --verify
```

- 生成レポート: `output/benchmark-vt-precise-<codec>-<epoch>.md`
- `--verify` で `ffprobe` + `ffmpeg -v error` 検証を実行する。
- `--equal-raw-input` で `video-hw` / `ffmpeg` encode に同一 raw ARGB 入力を供給する。
- `--include-internal-metrics` で `--vt-report-metrics true` を example CLI へ渡し、
  `Internal Metrics (video-hw)` セクションを NV 精密レポートと同形式で出力する。
- VT backend 固有パラメータを変える場合は
  `--vt-enable-pipeline-scheduler <true|false>` / `--vt-pipeline-queue-capacity <N>` を使用する。
- AV1 は `libaom-av1` で生成した fMP4 `av01` input を使う decode-only 比較。`video-hw decode_to_yuv --input-format mp4 --backend vt --codec av1` と FFmpeg `-hwaccel videotoolbox` decode を同じ入力で測る。`--verify` では FFmpeg software NV12 reference との PSNR-Y も記録し、`--min-psnr-y` で閾値を変更できる。AV1 encode は未実装のため測定対象外。

### 5) VideoToolbox 精密ベンチ定常運用（直列実行）

```bash
cargo +nightly -Zscript scripts/run_vt_precise_suite.rs
cargo +nightly -Zscript scripts/run_vt_precise_suite.rs --include-av1
```

- 既定は `warmup=1`, `repeat=3`, `verify=true`, `equal-raw-input=true`, `include-internal-metrics=true`
- H264 と HEVC を同時ではなく順番に実行する
- `--vt-enable-pipeline-scheduler` / `--vt-pipeline-queue-capacity` は各 codec の精密ベンチへ引き渡す
- H264 と HEVC を同時ではなく順番に実行する。`--include-av1` で AV1 fMP4 decode-only 比較も追加する。

### 6) Intel 精密ベンチ（反復 + 統計）

```bash
cargo +nightly -Zscript scripts/benchmark_ffmpeg_intel_precise.rs --codec h264 --release --warmup 2 --repeat 9
cargo +nightly -Zscript scripts/benchmark_ffmpeg_intel_precise.rs --codec hevc --release --warmup 2 --repeat 9
cargo +nightly -Zscript scripts/benchmark_ffmpeg_intel_precise.rs --codec av1 --release --warmup 1 --repeat 3
```

- 生成レポート: `output/benchmark-intel-precise-<codec>-<epoch>.md`
- `video-hw` は `backend=intel` で実行し、`--require-hardware <true|false>` で HW 固定 / fallback 許可を切り替え可能
- `ffmpeg` は `h264_qsv` / `hevc_qsv` / `av1_qsv` を使用
- レポートに encode/decode の平均スループット差分（ffmpeg 比、±10% 判定）を出力
- encode の厳密比較は `--equal-raw-input` を推奨（`video-hw` / `ffmpeg` に同一 raw ARGB 入力を供給）
- HEVC で parity を詰める場合は `--equal-raw-input --raw-input-pix-fmt nv12` を推奨（直近 report 1773584282 で decode/encode とも ±10% 内）
- AV1 decode input は生成済み OBU elementary stream を使い、H.264/HEVC のような byte-repeat は行わない
- 既定で `--grouped-cases=true`（caseごとに warmup/measure をまとめて実行）になっており、round-robin より計測ばらつきを抑えやすい
- 既定値は `--warmup 2` / `--repeat 7` / `--decode-loops 10`（短窓ノイズを抑えて parity 判定を安定化）
- `--settle-ms <N>` で各計測の間に待機を入れて、熱/スケジューリング揺れを緩和できる
- decode 側チューニングは `--intel-decode-async-depth <N>`（1..=16）で `VIDEO_HW_INTEL_DECODE_ASYNC_DEPTH` を video-hw decode ケースへ注入できる（backend 側 default は 16）
- encode 側チューニングは環境変数 `VIDEO_HW_INTEL_RATE_CONTROL` / `VIDEO_HW_INTEL_CQP` / `VIDEO_HW_INTEL_ASYNC_DEPTH` / `VIDEO_HW_INTEL_HEVC_USE_VPP` / `VIDEO_HW_INTEL_HEVC_LOW_POWER` で調整できる（未指定時は H.264=CBR、HEVC=CQP、CQP=24、async_depth=10、HEVCはCPU NV12投入を優先、low-power は有効）
- runtime 依存で一部ケースが失敗する環境では `--allow-case-failures` を付けると失敗ケースを記録したままレポート生成を継続
- `--allow-case-failures --verify` 併用時、失敗ケースで出力が欠けた検証対象は `skipped` としてレポートに記録する

### 6.1) AV1 MSE/PSNR smoke

```bash
cargo +nightly -Zscript scripts/check_av1_psnr.rs --backends nvidia,intel --release true
```

- 生成レポート: `output/av1-psnr/av1-psnr-<epoch>.md`
- FFmpeg `testsrc2` から raw ARGB 入力を生成し、`video-hw` AV1 encode 出力を FFmpeg reference と MSE/PSNR 比較する
- decode pixel PSNR は `video-hw decode_to_yuv --output-mode rgb24` と FFmpeg RGB24 reference を比較する
- 320x180/30 frames の確認では NVIDIA / Intel とも encode PSNR-Y avg 55 dB 以上、decode PSNR-Y min 50 dB 以上で PASS

### 6.2) AV1 fMP4 roundtrip smoke

```bash
cargo +nightly -Zscript scripts/check_av1_fmp4_roundtrip.rs --backends nvidia,intel --release true --require-hardware true --min-decode-psnr 40
```

- 生成レポート: `output/av1-fmp4-roundtrip/av1-fmp4-roundtrip-<epoch>.md`
- `video-hw-fmp4` の `write_synthetic_fmp4` で backend AV1 encode 出力を `av01` fMP4 に書き込む。
- `read_fmp4_file` で reader sample数を確認する。
- `ffprobe` で `codec_name=av1` / `codec_tag_string=av01` / duration を確認する。
- FFmpeg software decode と `decode_to_yuv --input-format mp4 --output-mode metadata` の両方が通ることを確認する。
- `decode_to_yuv --input-format mp4 --output-mode rgb24` と FFmpeg RGB24 reference の PSNR を比較し、既定では decode PSNR min 40 dB 以上を要求する。旧名 `--min-decode-psnr-y` も互換aliasとして受け付ける。

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
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends vulkan --codec av1 --warmup 0 --repeat 1 --allow-failures true
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends vulkan --codec av1 --vulkan-decode-input-format fmp4 --warmup 0 --repeat 1 --allow-failures true
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends vulkan --codec av1 --vulkan-av1-gop-size 30 --warmup 1 --repeat 3 --allow-failures true
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends vulkan --codec av1 --vulkan-av1-gop-size 30 --vulkan-av1-lag-in-frames 25 --verify --allow-failures true
```

- 生成レポート: `output/benchmark-backends-<codec>-<epoch>.md`
- `--codecs h264,hevc` を指定すると、codecごとに同じbackend集合を測り、codec別の統合レポートを生成する。
- Windows/Linux では `nv,intel,vulkan`、macOS では `vt` が既定。
- NV/Intel/VT は子レポートの parity / verification 結果も統合statusへ反映する。
- Vulkan は `vulkaninfo --summary` と `list_vulkan_adapters` の結果を名前/IDで対応付け、adapter ごとに `video-hw` decode/encode と FFmpeg Vulkan decode/encode を記録する。
- Vulkan decode は `--width` / `--height` / `--frame-count` に合わせた Annex-B / OBU 入力を FFmpeg software encoder（H.264=`libx264`, HEVC=`libx265`, AV1=`libaom-av1`）で `output/benchmark-vulkan-decode-input-...` に生成し、`video-hw` と FFmpeg Vulkan の両方へ同じ入力を渡す。decode throughput の分母はこの `--frame-count` と一致する。
- Vulkan AV1 decode は既定で generated long-GOP OBU 入力（生成時 `-g 30 -lag-in-frames 0`）を測定対象にする。`--vulkan-av1-gop-size <N>` で GOP サイズ、`--vulkan-av1-lag-in-frames <N>` で libaom lookahead を変更できる。keyframe-only 比較が必要な場合は `--vulkan-av1-gop-size 1` を指定する。NVIDIA では `video-hw decode` と FFmpeg Vulkan decode を同じ入力で比較し、unsupported adapter の失敗理由も同じ report に残す。Vulkan AV1 encode は現行 `ash` binding が `VK_KHR_video_encode_av1` を公開していないため `unavailable` として記録する。
- `--vulkan-av1-lag-in-frames 25` のような alt-ref/show-existing を含む入力は、Vulkan AV1 の DPB replay と表示フレーム選択を測る負荷として使う。2026-05-06 の NVIDIA 実測では DPB slot replay は bounded slot に収まり、metadata decode は readback なし command submit として PASS し、表示フレーム数を報告する。PSNR/NV12 readback は論理DPB slotと出力image layerを分離し、show-existingを含む表示フレームを正しいreadback layerへmapする。OBU/fMP4とも `psnr_y_min=inf` でPASS済み。
- より長い alt-ref/show-existing 入力では、1つの decode が同じ Vulkan DPB slot に畳まれた複数の異なる参照image layerを同時に必要とする場合がある。この場合は誤った画素を返さず、`aliases Vulkan DPB slot ... with multiple image layers` として明示的に unsupported にする。
- Vulkan AV1 decode で `--verify` を付けると、各 `video-hw` Vulkan adapter に対して同じ生成入力と測定に使った `decode_to_yuv` binary を `scripts/check_vulkan_av1_psnr.rs` に渡し、FFmpeg software decode reference との `psnr_y_min >= 60 dB` を統合レポートの `video-hw PSNR verify` 行として記録する。
- `--vulkan-decode-input-format fmp4` を指定すると、Vulkan AV1 decode比較用入力を fragmented MP4 (`av01`) として生成し、`decode_to_yuv --input-format mp4` と FFmpeg MP4 demuxerで測定する。現状この指定は AV1 専用。
- VideoToolbox AV1 decode は fMP4 `av01` sample entry 由来の `av1C` と track dimensions を `VtDecoderOptions` に渡す bootstrap まで実装済み。実decode/FFmpeg parity/PSNR は macOS AV1 hardware での検証待ち。AV1 encode は未実装のまま。
- FFmpeg Vulkan の adapter 指定は `-init_hw_device vulkan:<index>` を使う。Windows hybrid GPU 環境では `vulkan=vk:<index>` が指定した物理デバイスを選ばず、NVIDIA encode ケースが Intel 側へ流れることがあった。
- `cargo run -p video-hw --features backend-vulkan --example list_vulkan_adapters` で `video-hw` / `vk-video` 側に見えている Vulkan adapter を確認できる。
- `ffmpeg-only` Vulkan adapter のHEVC decodeでは、runnerが `VIDEO_HW_VULKAN_HEVC_DECODE_PHYSICAL_DEVICE_INDEX=<index>` を設定して direct ash HEVC decode bootstrap も試す。これは `--vulkan-adapter-index` の既存意味を変えず、Intel decode-only capability の切り分けに使う。
- adapter ごとに失敗理由も report に残すため、複数 GPU 環境では `--allow-failures true` で全候補を走査する。
- FFmpeg Vulkan encode 比較は encoded bitstream muxer ではなく null muxer に流す。AV1 OBU muxer の DTS 制約で encode自体とは無関係に失敗するケースを避け、encoder throughput を測るため。
- Vulkan HEVC encode は NVIDIA adapter 上で実験的な IDR-only production path を測定できる。runner は FFmpeg `hevc_vulkan` で同サイズの parameter/header sample を生成し、名前/vendor/device idで対応したFFmpeg Vulkan adapter番号を使って `VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_SAMPLE_PATH` を自動設定する。現状は1回の `flush` 内で Vulkan video session / session parameters を再利用し、warmup後の統合benchmarkではFFmpeg Vulkan HEVC encode以上のthroughputを確認済み。長寿命encoder sessionと参照フレーム/GOP encodeは未実装。失敗adapterや非公開adapterも report に残す。診断用には `cargo +nightly -Zscript scripts/check_vulkan_hevc_encode_probe.rs --adapter-index <n>`、または `cargo test -p video-hw-backend-vulkan --features backend-vulkan live_hevc_encode_session_bootstrap_reports_submit_feedback -- --ignored --nocapture` を明示実行する。

### 11.1) Vulkan AV1 command record probe

```bash
cargo +nightly -Zscript scripts/check_vulkan_av1_record_probe.rs
cargo +nightly -Zscript scripts/check_vulkan_av1_record_probe.rs --no-record-command-buffer
cargo +nightly -Zscript scripts/check_vulkan_av1_record_probe.rs --record-mode barrier_only
cargo +nightly -Zscript scripts/check_vulkan_av1_record_probe.rs --submit-command-buffer
cargo +nightly -Zscript scripts/check_vulkan_av1_record_probe.rs --readback
cargo +nightly -Zscript scripts/check_vulkan_av1_record_probe.rs --readback --generate-ffmpeg-obu --width 320 --height 180 --frames 1
```

- 生成レポート: `output/vulkan-av1-record-probe/vulkan-av1-record-probe-<epoch>.md`
- 既定で `VIDEO_HW_VULKAN_AV1_RECORD_COMMAND_BUFFER=1` を付け、ignored live test を通じて AV1 decode command buffer の begin/reset/decode/end record probe を実行する。
- `--input <obu>` で AV1 low-overhead OBU elementary stream を live probe に渡せる。
- `--generate-ffmpeg-obu` は FFmpeg `libaom-av1` で1フレームの OBU elementary stream を生成して、その入力で live probe を走らせる。`--ffmpeg` または `FFMPEG_PATH` で ffmpeg 実行ファイルを指定できる。
- `--record-mode barrier_only|begin_end|reset_end|first_decode|full` で command-buffer record の停止位置を切り分けられる。
- `--submit-command-buffer` で record 済み command buffer を queue submit し、5秒 fence wait まで進める。
- `--readback` は `--record-mode full` / submit を強制し、decode output image から NV12 plane を readback buffer へ copy して map する。
- `--no-record-command-buffer` は通常diagnosticsと同じく driver command path を発行せず、session/source/image/barrier/command sequence の構築だけを確認する。
- 2026-05-06 の Windows 環境では `--no-record-command-buffer` / `--record-mode barrier_only` / `begin_end` / `reset_end` / `first_decode` / `full` が PASS。`--record-mode full --submit-command-buffer` も PASS し、queue submit と fence wait まで到達する。readback は `VIDEO_DECODE|TRANSFER` を同じ queue family に持つ adapter を選ぶ。Intel は decode-only queue のため、この単一 queue readback probe では避け、NVIDIA 側で FFmpeg生成OBUの readback を確認する。
- これは isolated probe の submit/readback であり、PSNR や実AV1 file decode parity ではない。Vulkan AV1 backend はまだ実装完了扱いにしない。

### 11.2) Vulkan AV1 PSNR check

```bash
cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --min-psnr-y 60
cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --frames 8 --skip-build --min-psnr-y 60
cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --input-format fmp4 --frames 8 --skip-build --min-psnr-y 60
cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --frames 8 --skip-build --gop-size 30 --min-psnr-y 60
cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --input-format fmp4 --frames 8 --skip-build --gop-size 30 --min-psnr-y 60
cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --frames 16 --gop-size 30 --lag-in-frames 25 --vulkan-adapter-index 0 --min-psnr-y 60
cargo +nightly -Zscript scripts/check_vulkan_av1_corpus_matrix.rs --skip-build --vulkan-adapter-index 0 --decode-bin target\release\examples\decode_to_yuv.exe
```

- 生成レポート: `output/vulkan-av1-psnr/vulkan-av1-psnr-<epoch>.md`
- 既定では FFmpeg `libaom-av1` で AV1 low-overhead OBU を生成し、`decode_to_yuv --backend vulkan --codec av1 --output-mode nv12` の出力を FFmpeg software decode の NV12 と raw-vs-raw で比較する。
- `--decode-bin <PATH>` を指定すると、その `decode_to_yuv` binary をPSNR確認に使う。未指定かつ `--skip-build` なしの場合は debug example をbuildして使う。
- `--input-format fmp4` は FFmpeg fragmented MP4 (`av01`) を生成し、`decode_to_yuv --input-format mp4` 経由で同じ PSNR 比較を行う。FFmpeg 生成時は `delay_moov` を使い、`av1C` に sequence header OBU が入った fMP4 を作る。
- `--gop-size <N>` は生成入力の `libaom-av1 -g` を切り替える。既定は現在のサポート範囲に合わせて `1`。`N>1` は inter-frame/GOP replay の負荷として使う。
- `--lag-in-frames <N>` は生成入力の libaom lookahead を切り替える。既定は `0`。`N>0` は alt-ref/show-existing を含む入力を作り、DPB replay、readback、表示フレーム選択の負荷として使う。
- AV1 metadata decode は PSNR 用の NV12 readback とは分離されており、readback なしで Vulkan decode command の record/submit を測る。PSNR check は従来どおり NV12 readback 必須で、display frame to readback layer mapping も含めて検証する。
- decode や PSNR setup で失敗した場合も FAIL markdown report を残す。
- PASS/FAIL どちらの report にも、同じ入力を
  `scripts/inspect_av1_frame_types.rs --expect-inter-frame ...` で検査する
  `frame_type_gate` コマンドを記録する。
- `scripts/inspect_av1_frame_types.rs` は `show_existing_frame` に加えて `show_frame` と `frame_to_show_map_idx` も出力するため、alt-ref/show-existing の表示順とDPB参照を追跡できる。
- `scripts/check_vulkan_av1_corpus_matrix.rs` は OBU/fMP4、keyframe-only、generated GOP、alt-ref/show-existing、expected unsupported alias case をまとめて実行し、PASSすべきケースと明示的unsupportedにすべきケースを1つのmatrix reportに記録する。
- 2026-05-06 の Windows/NVIDIA 環境では OBU と fMP4 のどちらも 8-frame keyframe-only 入力で `--min-psnr-y 60` に PASS し、`psnr_y_min=inf`。

### 11.3) Vulkan AV1 encode binding check

```bash
cargo +nightly -Zscript scripts/check_vulkan_av1_encode_bindings.rs
```

- local Cargo registry にある `ash` source を走査し、
  `VK_KHR_video_decode_av1` / `VideoDecodeAV1*` と
  `VK_KHR_video_encode_av1` / `VideoEncodeAV1*` の有無を markdown report に
  記録する。
- `--fail-on-missing` を付けると、AV1 encode binding がない場合に非0終了する。

### 11.4) AV1 frame type inspection

```bash
cargo +nightly -Zscript scripts/inspect_av1_frame_types.rs --frames 8 --gop-size 1 --expect-inter-frame false
cargo +nightly -Zscript scripts/inspect_av1_frame_types.rs --frames 8 --gop-size 30 --expect-inter-frame true
cargo +nightly -Zscript scripts/inspect_av1_frame_types.rs --input-format fmp4 --frames 8 --gop-size 30 --expect-inter-frame true
```

- FFmpeg `libaom-av1` で生成した low-overhead OBU または fragmented MP4
  を検査し、frame/header OBU の `show_existing_frame` と `frame_type` を
  markdown report に記録する。
- Vulkan AV1 GOP replay 実装前の診断では、`--gop-size 1` が全
  `frame_type=0`、`--gop-size 30` が2フレーム目以降 `frame_type=1` に
  なることを確認できる。
- `--expect-inter-frame true|false` を指定すると、期待と異なる入力で
  非0終了する。失敗時も markdown report は残る。

### 12) fMP4 decode access pattern benchmark

```bash
cargo +nightly -Zscript scripts/benchmark_fmp4_decode_access.rs -- --input sample-videos/foreman_cif.mp4 --backend auto --frame-count 90
```

- `decode_range_iter`、単発 `decode_sample`、`CachedFrameDecoder` を同じ入力で比較する。
- 逆順 cold access と prefetch 方向差（`reverse_before` / `reverse_after`）も同じレポートで比較する。
- FFmpeg RGB24 reference に対する max MSE / min PSNR も同じレポートへ記録する。
- `--generate-codec av1 --generate-backend <backend>` を付けると、先に
  `write_synthetic_fmp4` で synthetic AV1 fMP4 を生成し、その入力を同じ
  benchmark に渡す。
- Windows/Linux の既定 features は `backend-nvidia backend-intel backend-vulkan`、macOS は `backend-vt`。
- 詳細は `docs/benchmark/FMP4_DECODE_ACCESS_BENCHMARKS.md` を参照。

```bash
cargo +nightly -Zscript scripts/benchmark_fmp4_decode_access.rs --features "backend-nvidia backend-intel backend-vulkan" --generate-codec av1 --generate-backend nvidia --generate-width 320 --generate-height 180 --generate-frames 90 --generate-fragment-frames 30 --generate-require-hardware -- --backend nvidia --require-hardware --frame-count 90
```

## 前提

- `nightly` ツールチェーンが利用可能であること
- `cargo -Zscript` が有効な Cargo であること
- ベンチ用途では `ffmpeg` が必要（Intel 精密ベンチは QSV 有効環境が必要）
- NVIDIA ベンチ用途では NVIDIA ドライバ / CUDA 実行環境が必要
