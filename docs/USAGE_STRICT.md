# video-hw 利用ガイド（厳密 I/O 仕様, 現行実装準拠）

この文書は `DecodeSession` / `EncodeSession` の現行APIを、実装準拠で使うためのガイドです。

## 1. 導入

- macOS: `backend-vt`
- Linux/Windows: `backend-nvidia`
- `default = []`

```toml
[target.'cfg(target_os = "macos")'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3", default-features = false, features = ["backend-vt"] }

[target.'cfg(any(target_os = "linux", target_os = "windows"))'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "b88b0d9a5e8954c8443659e0b8fb1f1c7bc120b3", default-features = false, features = ["backend-nvidia"] }
```

## 2. Backend 選択

- `Backend::Auto`
- `Backend::VideoToolbox`（macOS + `backend-vt`）
- `Backend::Nvidia`（Linux/Windows + `backend-nvidia`）

`Backend::Auto` は OS 既定 backend を選択します。

### 2.1 NVIDIA backend の前提（重要）

- `backend-nvidia` は `nvidia-video-codec-sdk`（Rust bindings / ラッパー）を通じて NVIDIA SDK を利用する
- SDK 本体（lib/headers）は同梱しない前提で、利用者が NVIDIA から別途取得して配置する
- ビルド時は環境に応じて `NVIDIA_VIDEO_CODEC_SDK_PATH` などの設定が必要になる

## 3. Decode API

- `DecodeSession::new(Backend, DecoderConfig)`
- `submit(BitstreamInput)`
- `try_reap()`
- `reap_timeout(Duration)`
- `flush()`
- `summary()`
- `query_capability(Codec)`

### 3.1 Decode 入力

- `BitstreamInput::AnnexBChunk`
- `BitstreamInput::AccessUnitRawNal`
- `BitstreamInput::LengthPrefixedSample`

### 3.2 Decode 出力（重要）

`DecoderConfig.output_mode` で decode 出力モードを指定できます。

- `DecodeOutputMode::Metadata`（既定）
- `DecodeOutputMode::Nv12`
- `DecodeOutputMode::Rgb24`

`DecodedFrame` は次の variant を持ちます。

- `Metadata`
- `Nv12`
- `Rgb24`

`DecodeOutputMode::Metadata` は常用サポートです。  
`DecodeOutputMode::Nv12` / `Rgb24` は backend が ARGB payload を返す場合のみ変換出力できます。  
ARGB payload が未提供の場合は `BackendError::UnsupportedConfig` を返します。

## 4. Encode API

- `EncodeSession::new(Backend, EncoderConfig)`
- `submit(EncodeFrame)`
- `try_reap()`
- `reap_timeout(Duration)`
- `flush()`
- `query_capability(Codec)`
- `request_session_switch(SessionSwitchRequest)`

### 4.1 Encode 入力（重要）

`RawFrameBuffer` は次を持ちます。

- `Argb8888(Vec<u8>)`
- `Argb8888Shared(Arc<[u8]>)`
- `Nv12 { .. }`（`unstable-raw-inputs` feature 有効時のみ）
- `Rgb24(Vec<u8>)`（`unstable-raw-inputs` feature 有効時のみ）

現行 encode が受理するのは `Argb8888` / `Argb8888Shared` のみです。

- `Nv12` / `Rgb24` は `BackendError::InvalidInput`
- ARGB 長さは厳密に `width * height * 4`

### 4.2 Encode 出力 layout

- VT + H264: `EncodedLayout::Avcc`
- VT + HEVC: `EncodedLayout::Hvcc`
- NV: `EncodedLayout::AnnexB`

## 5. submit / reap / flush の意味

- `submit`: 入力投入
- `try_reap`: non-blocking 回収
- `reap_timeout`: timeout 上限まで待機して回収（内部キュー + backend poll）
- `flush`: EOS/遅延分回収

推奨ループは「`submit` -> `try_reap` 回収 -> 最後に `flush`」です。

## 6. 失敗時の見方

- `UnsupportedConfig`: backend/環境依存で利用不可
- `InvalidInput`: 入力不正（未対応 buffer, payload size mismatch）
- `InvalidBitstream`: bitstream 形式不正
- `TemporaryBackpressure`: 一時飽和
- `DeviceLost`: デバイスロスト
- `Backend`: backend 内部エラー

補足:
- `BackendError::kind()` でエラー種別を取得できる
- `BackendError::is_runtime_unavailable()` は `UnsupportedConfig` / `DeviceLost` を runtime unavailable として判定する

## 7. 最小検証コマンド

```bash
cargo test -- --nocapture
cargo test --features backend-nvidia -- --nocapture
cargo check --all-targets --features backend-nvidia
```

## 8. 関連

- `docs/spec/IO_FORMAT_CONTRACT.md`
- `docs/spec/TEST_SPEC_INVENTORY.md`
- `docs/status/STATUS.md`
