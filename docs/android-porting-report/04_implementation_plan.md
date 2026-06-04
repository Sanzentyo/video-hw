# 04. 実装計画

## Phase 0: 方針固定とビルド土台

目的: Android feature が workspace 上で解決できる状態にする。

### 0.1 workspace member追加

root `Cargo.toml` に新規 crate を追加します。

```diff
 members = [
   "crates/video-hw-core",
   "crates/video-hw",
+  "crates/video-hw-backend-android",
   "crates/video-hw-backend-nvidia",
   "crates/video-hw-backend-intel",
   "crates/video-hw-backend-vulkan",
   "crates/video-hw-backend-vt",
 ]
```

### 0.2 `video-hw-core` feature追加

```diff
 [features]
 default = []
 backend-vt = []
 backend-nvidia = []
 backend-intel = []
 backend-vulkan = []
+backend-android = []
```

### 0.3 `video-hw` feature / dependency追加

```diff
 [features]
 default = []
+backend-android = [
+  "video-hw-core/backend-android",
+  "dep:video-hw-backend-android",
+  "video-hw-backend-android/backend-android",
+]
 backend-vt = [ ... ]
```

```diff
 [dependencies]
 video-hw-core = { path = "../video-hw-core" }
+video-hw-backend-android = { path = "../video-hw-backend-android", optional = true, default-features = false }
 video-hw-backend-nvidia = { path = "../video-hw-backend-nvidia", optional = true, default-features = false }
```

## Phase 1: facade integration

目的: `Backend::Android` が compile-time に見えるようにし、static session API に載せる。

### 1.1 re-export

```diff
+#[cfg(all(target_os = "android", feature = "backend-android"))]
+pub use video_hw_backend_android::{AndroidDecoderAdapter, AndroidEncoderAdapter};
```

### 1.2 enum variant追加

```diff
 pub enum BackendKind {
+    #[cfg(all(target_os = "android", feature = "backend-android"))]
+    Android,
     #[cfg(all(target_os = "macos", feature = "backend-vt"))]
     VideoToolbox,
 }
```

```diff
 pub enum Backend {
     Auto,
+    #[cfg(all(target_os = "android", feature = "backend-android"))]
+    Android,
 }
```

### 1.3 parse / display

```diff
 match raw.to_ascii_lowercase().as_str() {
   "auto" => Ok(Self::Auto),
+  #[cfg(all(target_os = "android", feature = "backend-android"))]
+  "android" | "mediacodec" | "mc" => Ok(Self::Android),
 }
```

### 1.4 preferred backend order

```rust
#[cfg(all(target_os = "android", feature = "backend-android"))]
fn preferred_backend_order() -> Vec<BackendKind> {
    vec![BackendKind::Android]
}
```

既存の `#[cfg(any(...))]` 条件にも Android を含めます。ここを漏らすと `BackendKind` の `Display`、`Default`、`preflight_*` の compile が崩れます。

### 1.5 backend trait実装

```rust
#[cfg(all(target_os = "android", feature = "backend-android"))]
impl DecoderBackend for AndroidDecoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Android;

    fn from_decoder_config(config: DecoderConfig) -> Self {
        Self::new(config)
    }

    fn supports_output_mode(mode: DecodeOutputMode) -> bool {
        matches!(mode, DecodeOutputMode::Metadata | DecodeOutputMode::Nv12 | DecodeOutputMode::Rgb24)
    }
}

#[cfg(all(target_os = "android", feature = "backend-android"))]
impl EncoderBackend for AndroidEncoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Android;

    fn from_encoder_config(config: EncoderConfig) -> Self {
        Self::with_config(config)
    }
}
```

## Phase 2: Android backend MVP

目的: H.264 の CPU ByteBuffer decode/encode を通す。

### 2.1 FFI wrapper

実装対象:

- `AMediaCodec_createDecoderByType`
- `AMediaCodec_createEncoderByType`
- `AMediaCodec_configure`
- `AMediaCodec_start`
- `AMediaCodec_stop`
- `AMediaCodec_delete`
- `AMediaCodec_dequeueInputBuffer`
- `AMediaCodec_getInputBuffer`
- `AMediaCodec_queueInputBuffer`
- `AMediaCodec_dequeueOutputBuffer`
- `AMediaCodec_getOutputBuffer`
- `AMediaCodec_releaseOutputBuffer`
- `AMediaCodec_flush`
- `AMediaFormat_new`
- `AMediaFormat_delete`
- `AMediaFormat_setString`
- `AMediaFormat_setInt32`
- `AMediaFormat_setBuffer`

### 2.2 Decode MVP

1. H.264 Annex B から SPS/PPS を抽出。
2. `csd-0` / `csd-1` を `AMediaFormat` に入れる。
3. 320x180 / 640x360 の小さい stream を decode。
4. `DecodeOutputMode::Metadata` を必ず通す。
5. CPU YUV output が安定する端末で `Nv12` / `Rgb24` を有効化。

### 2.3 Encode MVP

1. `EncoderConfig.input_format = Nv12` から開始。
2. H.264 baseline/main profile を優先。
3. output CSD と sample を Annex B に正規化。
4. `ffprobe` / `ffmpeg -i` で出力bitstreamを検証。

## Phase 3: capability強化

目的: 端末差を `CapabilityReport.runtime` に反映する。

- JNI feature で `MediaCodecList` を読む。
- `isHardwareAccelerated()` / `isSoftwareOnly()` / `isVendor()` を反映。
- codec name allow/deny list を options に追加。
- profile / level / color format / bitrate mode を report に含める拡張を検討。

## Phase 4: Surface / HardwareBuffer対応

目的: copy削減と camera / renderer / GPU 連携。

- `AMediaCodec_createInputSurface` を使う encoder surface path。
- `ANativeWindow` を安全に受け渡す API。
- `AHardwareBuffer` を `DecodedFrame` または新しい native frame API に追加。
- `AHardwareBuffer_lockPlanes` を使い、YUV planes を CPU readback して NV12 に正規化する補助 path。

## Phase 5: Vulkan Video on Android（任意）

目的: 端末/driver が対応している場合だけ Vulkan backend をAndroidにも拡張。

現状の Vulkan backend は Linux/Windows cfg で、依存も Android では有効化されません。Android Vulkan Video 対応は端末差が大きいため、`backend-android` が安定した後の選択肢です。統合するなら次のどちらかです。

1. `backend-vulkan` を Android cfg に広げる。
2. `backend-android-vulkan` のように Android 専用 Vulkan path を分ける。

推奨は 2 です。Android では `AHardwareBuffer` / `ANativeWindow` / Vulkan external memory 連携が絡むため、desktop Vulkan backend と分岐する可能性が高いためです。
