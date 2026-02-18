# video-hw

`video-hw` は、複数のハードウェア backend（現状: VideoToolbox / NVIDIA）を同一契約で扱う Rust workspace です。

## Workspace 構成

```text
crates/
  backend-contract/   # 共通 trait / type / error
  bitstream-core/     # Annex-B 増分パースと AU 組み立て
  vt-backend/         # VideoToolbox 実装
  nvidia-backend/     # NVIDIA 実装（SDK bridge は今後接続）
root `src/` provides the unified facade API; backends remain in `crates/` as optional features.
```

## 統一 API（video-hw crate）

`video-hw` crate から `BackendKind` を選んで `Decoder` / `Encoder` を生成すると、同じ呼び出しコードで backend を差し替えられます。

```rust
use backend_contract::{Codec, DecoderConfig};
use video_hw::{BackendKind, Decoder};

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
cargo check --workspace
cargo test --workspace -- --nocapture

# facade crate の example 実行例
cargo run --example decode_vt
cargo run --example encode_vt
```

## 現在の実装状況

- `vt-backend`: decode/encode の実装あり（E2E テストあり）
- `nvidia-backend`: contract と packer 接続済み（SDK 連携部分は未実装）
- `bitstream-core`: chunk 増分処理と parameter set 抽出の unit test あり
