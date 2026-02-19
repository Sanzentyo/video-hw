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

## 前提

- `nightly` ツールチェーンが利用可能であること
- `cargo -Zscript` が有効な Cargo であること
- ベンチ用途では `ffmpeg` / NVIDIA ドライバ / CUDA 実行環境が必要
