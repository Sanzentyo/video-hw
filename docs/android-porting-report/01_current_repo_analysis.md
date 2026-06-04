# 01. 現行 `Sanzentyo/video-hw` の調査結果

## 1. リポジトリの性格

`video-hw` は、複数の hardware video backend を共通 API で扱う Rust workspace です。README では backend として VideoToolbox / NVIDIA / Intel oneVPL / Vulkan が挙げられ、主要 crate は次の構成です。

| crate / directory | 役割 |
|---|---|
| `crates/video-hw-core` | 共通型、エラー、codec契約、`VideoDecoder` / `VideoEncoder` trait |
| `crates/video-hw` | facade、backend選択、`DecodeSession` / `EncodeSession` API |
| `crates/video-hw-backend-nvidia` | NVIDIA NVENC/NVDEC backend |
| `crates/video-hw-backend-intel` | Intel oneVPL backend |
| `crates/video-hw-backend-vulkan` | Vulkan Video backend |
| `crates/video-hw-backend-vt` | macOS VideoToolbox backend |
| `sample-videos` / `scripts` | E2E / benchmark 用素材と補助スクリプト |

参照: `https://github.com/Sanzentyo/video-hw`、`README.md`。

## 2. 現行 feature / platform 切替

README 上の利用例では、次のような platform 前提になっています。

```toml
[target.'cfg(target_os = "macos")'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "...", default-features = false, features = ["backend-vt"] }

[target.'cfg(any(target_os = "linux", target_os = "windows"))'.dependencies]
video-hw = { git = "https://github.com/Sanzentyo/video-hw", rev = "...", default-features = false, features = ["backend-nvidia"] }
```

現行の `crates/video-hw/Cargo.toml` には以下の feature が存在します。

```toml
[features]
default = []
backend-vt = [
  "video-hw-core/backend-vt",
  "dep:video-hw-backend-vt",
  "video-hw-backend-vt/backend-vt",
]
backend-nvidia = [
  "video-hw-core/backend-nvidia",
  "dep:video-hw-backend-nvidia",
  "video-hw-backend-nvidia/backend-nvidia",
]
backend-intel = [
  "video-hw-core/backend-intel",
  "dep:video-hw-backend-intel",
  "video-hw-backend-intel/backend-intel",
]
backend-vulkan = [
  "video-hw-core/backend-vulkan",
  "dep:video-hw-backend-vulkan",
  "video-hw-backend-vulkan/backend-vulkan",
]
```

Android 用 feature はありません。

## 3. facade API の重要点

`crates/video-hw/src/lib.rs` では、backend adapter を cfg 付きで re-export し、`BackendKind` / `Backend` enum で backend を表現しています。現行コードでは概ね次のような条件です。

- `VideoToolbox`: `#[cfg(all(target_os = "macos", feature = "backend-vt"))]`
- `Nvidia`: `#[cfg(all(feature = "backend-nvidia", any(target_os = "linux", target_os = "windows")))]`
- `Intel`: `#[cfg(all(feature = "backend-intel", any(target_os = "linux", target_os = "windows")))]`
- `Vulkan`: `#[cfg(all(feature = "backend-vulkan", any(target_os = "linux", target_os = "windows")))]`

したがって、Android target では既存 backend enum variant が基本的に有効になりません。Android 対応では `BackendKind::Android` / `Backend::Android` を追加し、`target_os = "android"` を含む cfg 経路を追加する必要があります。

## 4. 共通 trait と session 構造

`video-hw-core` は backend 実装に必要な最低限の trait を持っています。

```rust
pub trait VideoDecoder {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError>;
    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError>;
    fn flush(&mut self) -> Result<Vec<Frame>, BackendError>;
    fn try_reap(&mut self) -> Result<Vec<Frame>, BackendError> { Ok(Vec::new()) }
    fn decode_summary(&self) -> DecodeSummary;
}

pub trait VideoEncoder {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError>;
    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError>;
    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError>;
    fn try_reap(&mut self) -> Result<Vec<EncodedPacket>, BackendError> { Ok(Vec::new()) }
}
```

facade 側の `DecodeSession<D>` / `EncodeSession<E>` は generic 型 `D: VideoDecoder` / `E: VideoEncoder` を持ち、backend crate は adapter 型を実装するだけで統合できます。この点は Android backend 追加に有利です。

## 5. 現行入出力型

README と `video-hw-core` から、現行公開 API の主な入出力型は次です。

- Decode output: `DecodedFrame::{Metadata, Nv12, Rgb24}`
- Encode input: `RawFrameBuffer::{Argb8888, Argb8888Shared, Nv12 { pitch, data }}`
- Encode input format: `EncodeInputFormat::{Argb8888, Nv12}`
- Encoded layout: `EncodedLayout::{AnnexB, Avcc, Hvcc, Av1, Opaque}`

Android でゼロコピー Surface / HardwareBuffer を扱うには、この型体系に native frame を追加するか、別セッション API を追加する必要があります。MVP では CPU ByteBuffer / NV12 経路を優先し、Surface / AHardwareBuffer は Phase 2 とするのが安全です。

## 6. Vulkan backend のAndroid流用可否

現行 `crates/video-hw-backend-vulkan/Cargo.toml` は、Vulkan 依存を Linux/Windows に限定しています。

```toml
[target.'cfg(any(target_os = "linux", target_os = "windows"))'.dependencies]
vk-video = { version = "0.2.1" }
ash = { version = "0.38.0" }
scuffle-h265 = { version = "0.2.2" }
oxideav-av1 = "0.1.5"
```

`src/lib.rs` でも module や helper が `#[cfg(any(target_os = "linux", target_os = "windows"))]` で囲まれています。したがって、単に `backend-vulkan` を Android で有効化しても現行設計では使えません。

さらに `vulkan_capability_report(codec)` は、現行コード上では H.264 のみを capability として返す形になっており、HEVC / AV1 は周辺実装が存在しても公開 contract としては限定的です。Android で安定利用したい場合、まずは Android 標準 codec API の MediaCodec backend を新設する方が合理的です。

## 7. Android対応の方向性まとめ

| 選択肢 | 評価 | 理由 |
|---|---:|---|
| 既存 Vulkan backend を Android に拡張 | △ / Phase 3 | Vulkan Video のAndroid端末対応が不均一。現行実装も Linux/Windows cfg。H.264中心のcontract。 |
| VideoToolbox / NVIDIA / Intel backend の流用 | × | Android runtime とAPIが異なる。 |
| 新規 `video-hw-backend-android` を追加 | ◎ / 推奨 | 既存 static adapter 構造に合う。Android公式 MediaCodec を使える。device capability を runtime に反映しやすい。 |
