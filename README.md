# video-hw

`video-hw` は、複数のハードウェア backend（現状: VideoToolbox / NVIDIA）を同一 API で扱う単一 crate です。

## 主要構成

```text
src/
  lib.rs              # 公開API + backend切替
  contract.rs         # 共通 trait / type / error
  bitstream.rs        # Annex-B 増分パースと AU 組み立て
  vt_backend.rs       # VideoToolbox 実装（macOS + feature）
  nvidia_backend.rs   # NVIDIA 実装（feature、SDK bridge未接続）
examples/
  decode_annexb.rs
  encode_synthetic.rs
tests/
  e2e_video_hw.rs
```

## feature / platform 切替

- デフォルト: `backend-vt`（macOS 前提）
- NVIDIA を有効化: `--features backend-nvidia`
- 実行時は `BackendKind` で backend を選択

## 統一 API

```rust
use video_hw::{BackendKind, Codec, Decoder, DecoderConfig};

let mut decoder = Decoder::new(
    BackendKind::VideoToolbox,
    DecoderConfig {
        codec: Codec::H264,
        fps: 30,
        require_hardware: false,
    },
);
let _ = decoder.push_bitstream_chunk(&[], None);
let summary = decoder.decode_summary();
```

## 検証コマンド

```bash
cargo fmt --all
cargo check
cargo test -- --nocapture

# examples
cargo run --example decode_annexb -- --codec h264 --chunk-bytes 4096
cargo run --example encode_synthetic -- --codec h264 --output ./encoded-output.h264
```

## ドキュメント配置

- ドキュメントは `docs/` 配下に整理済み
- インデックス: `docs/README.md`

## クリーンアップ状況

- 旧 workspace の重複実装（`crates/`）は root 実装での動作確認後に削除済み
- 旧バックアップ（`legacy-root-backup/`）は機能カバレッジ確認後に削除済み

## 現在の実装状況

- VideoToolbox: decode/encode 実装あり（E2E テストあり）
- NVIDIA: contract と packer 接続済み（SDK 連携部分は未実装）
- bitstream: chunk 増分処理と parameter set 抽出の unit test あり
