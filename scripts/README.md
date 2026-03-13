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
- 既定で `--grouped-cases=true`（caseごとに warmup/measure をまとめて実行）になっており、round-robin より計測ばらつきを抑えやすい
- `--settle-ms <N>` で各計測の間に待機を入れて、熱/スケジューリング揺れを緩和できる
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

- 生成した `mfx.h` / `vpl.lib` に合わせた `LIBVPL_INCLUDE_PATH` / `LIBVPL_LIBRARY_PATH` / `PATH` の設定例を出力する
- setup 後の検証コマンド（`clippy` / `test`）も出力する

## 前提

- `nightly` ツールチェーンが利用可能であること
- `cargo -Zscript` が有効な Cargo であること
- ベンチ用途では `ffmpeg` が必要（Intel 精密ベンチは QSV 有効環境が必要）
- NVIDIA ベンチ用途では NVIDIA ドライバ / CUDA 実行環境が必要
