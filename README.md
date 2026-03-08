# video-hw

`video-hw` は、複数のハードウェア backend（VideoToolbox / NVIDIA）を同一 API で扱う workspace 構成のライブラリ群です。

## 主要構成

```text
crates/
  video-hw-core/      # 共通型・エラー・契約（公開core crate）
  video-hw/           # facade + backend 実装（公開crate）
sample-videos/        # E2E/bench 入力素材
scripts/              # 補助スクリプト
```

## feature / platform 切替

- デフォルト: なし（`default = []`）
- macOS は `backend-vt` を有効化
- Linux/Windows は `backend-nvidia` を有効化
- 実行時は `Backend` を選択（`Backend::Auto` で OS 既定を自動選択）

### 利用側 Cargo.toml（推奨, git rev 固定）

```toml
[target.'cfg(target_os = "macos")'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3", default-features = false, features = ["backend-vt"] }

[target.'cfg(any(target_os = "linux", target_os = "windows"))'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3", default-features = false, features = ["backend-nvidia"] }
```

## 現行APIの重要制約

- decode 出力型は `DecodedFrame::{Metadata,Nv12,Rgb24}` を持つ
  - ただし標準 decode 経路の出力は `Metadata` 中心
- encode 入力型は `RawFrameBuffer::{Argb8888,Argb8888Shared,Nv12,Rgb24}` を持つ
  - ただし現行 encode が受理するのは `Argb8888` / `Argb8888Shared` のみ
  - `Nv12` / `Rgb24` は `BackendError::InvalidInput`
- `reap_timeout` は現行実装では `try_reap` と同挙動（実質 non-blocking）

## NVIDIA backend 依存

`backend-nvidia` では次の依存を使用します。

- `nvidia-video-codec-sdk`
  - `git = "https://github.com/Sanzentyo/nvidia-video-codec-sdk"`
  - `rev = "d2d0fec631365106d26adfe462f3ce15b043b879"`
- `cudarc = 0.19.2`

`nvidia-video-codec-sdk` は Rust から NVIDIA Video Codec SDK を扱うためのラッパー層です。
SDK 本体（lib/headers）は同梱しない前提で、利用者側が別途 NVIDIA から取得して配置する必要があります。

### NVIDIA Video Codec SDK ビルド前提（Windows）

```powershell
$env:NVIDIA_VIDEO_CODEC_SDK_PATH = "C:\Path\To\Video_Codec_SDK\Lib\x64"
```

`NVIDIA_VIDEO_CODEC_SDK_PATH` は `nvEncodeAPI.lib` / `nvcuvid.lib` を含むディレクトリを指します。

## ライセンス

- このプロジェクトは `MIT OR Apache-2.0` のデュアルライセンス
- 詳細は `LICENSE-MIT` / `LICENSE-APACHE` / `NOTICE` を参照
- 依存ライセンスと注意事項は `THIRD_PARTY_NOTICES.md` を参照
- NVIDIA SDK の配布運用ルールは `docs/spec/NVIDIA_SDK_DISTRIBUTION_POLICY.md` を参照

## 検証コマンド

```bash
cargo fmt --all
cargo check
cargo test -- --nocapture
cargo check --all-targets --features backend-nvidia
cargo test --features backend-nvidia -- --nocapture
```

## 実行例

```bash
# decode
cargo run --example decode_annexb -- --backend auto --codec h264 --input sample-videos/sample-10s.h264 --chunk-bytes 4096 --require-hardware

# encode
cargo run --features backend-nvidia --example encode_synthetic -- --backend nv --codec h264 --fps 30 --frame-count 300 --require-hardware --output output/video-hw-h264.bin
```

## ドキュメント

- インデックス: `docs/README.md`
- 利用ガイド: `docs/USAGE_STRICT.md`
- I/O 契約: `docs/spec/IO_FORMAT_CONTRACT.md`
- テスト台帳: `docs/spec/TEST_SPEC_INVENTORY.md`
- 状態: `docs/status/STATUS.md`
- 計画: `docs/plan/ROADMAP.md`
- 次アクション: `docs/plan/NEXT_ACTION_PLAN_2026-02-23.md`
