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

## 前提

- `nightly` ツールチェーンが利用可能であること
- `cargo -Zscript` が有効な Cargo であること
- ベンチ用途では `ffmpeg` / NVIDIA ドライバ / CUDA 実行環境が必要
