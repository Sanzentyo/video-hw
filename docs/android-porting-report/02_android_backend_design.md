# 02. Android backend の具体設計

## 1. 設計方針

新規 crate `crates/video-hw-backend-android` を追加し、Android NDK `AMediaCodec` を直接呼び出す backend として実装します。facade の既存設計に合わせ、次の2つの adapter を公開します。

```rust
pub struct AndroidDecoderAdapter { ... }
pub struct AndroidEncoderAdapter { ... }
```

facade 側では次を追加します。

```rust
#[cfg(all(target_os = "android", feature = "backend-android"))]
pub use video_hw_backend_android::{AndroidDecoderAdapter, AndroidEncoderAdapter};
```

そして `BackendKind::Android` / `Backend::Android` を追加し、`DecodeSession::<AndroidDecoderAdapter>::new(config)` と `EncodeSession::<AndroidEncoderAdapter>::new(config)` を既存 backend と同じ形で利用できるようにします。

## 2. API階層

```text
User code
  |
  v
video-hw facade
  - Backend::Android / BackendKind::Android
  - DecodeSession<AndroidDecoderAdapter>
  - EncodeSession<AndroidEncoderAdapter>
  |
  v
video-hw-backend-android
  - AndroidDecoderAdapter: VideoDecoder
  - AndroidEncoderAdapter: VideoEncoder
  - capability detection
  - MediaCodec RAII wrappers
  |
  v
Android NDK
  - media/NdkMediaCodec.h
  - media/NdkMediaFormat.h
  - android/native_window.h
  - android/hardware_buffer.h (Phase 2)
```

## 3. Codec mapping

| `video-hw::Codec` | Android MIME | 備考 |
|---|---|---|
| `Codec::H264` | `video/avc` | MVPの最優先対象 |
| `Codec::Hevc` | `video/hevc` | 端末差あり。decodeは比較的多いがencodeは要capability確認 |
| `Codec::Av1` | `video/av01` | 新しめの端末依存。必ずruntime queryで判断 |

## 4. Decoder設計

### 4.1 構造体案

```rust
pub struct AndroidDecoderAdapter {
    config: DecoderConfig,
    options: AndroidDecoderOptions,
    codec: Option<MediaCodec>,
    output_format: Option<AndroidOutputFormat>,
    pending_eos: bool,
    ready: Vec<Frame>,
    summary: DecodeSummary,
}
```

### 4.2 decode flow（MVP: sync ByteBuffer）

1. `DecoderConfig` から MIME を決める。
2. `AMediaCodec_createDecoderByType(mime)` で decoder を作成。
3. `AMediaFormat` に `mime`, `width`, `height`, 必要に応じて `csd-0` / `csd-1` / `csd-2` を設定する。
4. `AMediaCodec_configure(codec, format, surface = null, crypto = null, flags = 0)`。
5. `AMediaCodec_start(codec)`。
6. `push_bitstream_chunk()` で input buffer を dequeue し、Annex B access unit または変換済み sample をコピーして `queueInputBuffer()`。
7. output buffer を drain し、`INFO_OUTPUT_FORMAT_CHANGED` 相当では stride / slice height / crop / color format を更新。
8. `DecodeOutputMode::Metadata` なら dimensions / pts のみ返す。
9. `DecodeOutputMode::Nv12` なら CPU output buffer を NV12 に正規化して `Frame.nv12` に入れる。
10. `DecodeOutputMode::Rgb24` なら NV12 から RGB24 変換して返す。

### 4.3 入力 bitstream の扱い

facade は `BitstreamInput` を `normalize_bitstream_input()` で Annex B に寄せています。Android decoder は端末実装により Annex B と codec-specific-data の期待が違うため、Android backend では以下を明確に分けます。

- H.264:
  - Annex B から SPS/PPS を抽出し、configure 時に `csd-0` / `csd-1` を設定する。
  - access unit は start code 付き Annex B のまま投入するか、端末ごとに必要なら length-prefixed に変換する fallback を用意する。
- HEVC:
  - VPS/SPS/PPS を抽出し、`csd-0` へ HVCC 相当または Annex B CSD として設定する。
  - 端末差が大きいため最初は H.264 を優先し、HEVC は capability / device matrix で拡張する。
- AV1:
  - AV1C / sequence header OBU の扱いが必要。`video-hw` 側には AV1 fMP4 / OBU の処理があるため、既存 helper を再利用する。

## 5. Encoder設計

### 5.1 構造体案

```rust
pub struct AndroidEncoderAdapter {
    config: EncoderConfig,
    options: AndroidEncoderOptions,
    codec: Option<MediaCodec>,
    input_surface: Option<NativeWindow>,
    output_csd: AndroidCodecSpecificData,
    next_pts_us: i64,
    ready: Vec<EncodedPacket>,
}
```

### 5.2 encode flow（MVP: CPU input buffer）

1. MIME / bitrate / fps / width / height / i-frame interval を `AMediaFormat` に設定。
2. color format は原則 `YUV420Flexible` 相当を要求する。
3. `AMediaCodec_createEncoderByType(mime)`。
4. `AMediaCodec_configure(..., flags = AMEDIACODEC_CONFIGURE_FLAG_ENCODE)`。
5. `AMediaCodec_start()`。
6. `RawFrameBuffer::Nv12` は stride を考慮して encoder input buffer にコピー。
7. `RawFrameBuffer::Argb8888` は backend 内で NV12 に変換して投入。現行 `RawFrameBuffer` の契約では input_format と一致必須なので、`EncoderConfig.input_format = Argb8888` の時だけ変換を行う。
8. output buffer を drain し、CSD を保持。
9. H.264 / HEVC は CSD + sample を Annex B に正規化して `EncodedLayout::AnnexB` として返す。
10. AV1 は OBU payload を `EncodedLayout::Av1` として返す。

### 5.3 Surface input（Phase 2）

NDK の `AMediaCodec_createInputSurface` は API 26 から使えます。Surface input にすると GPU / camera / renderer から encoder への copy を減らせますが、現行 `RawFrameBuffer` は CPU buffer のみです。したがって Phase 2 で以下のどちらかが必要です。

- `RawFrameBuffer::AndroidHardwareBuffer(...)` / `RawFrameBuffer::NativeSurfaceFrame(...)` を追加する。
- Android 専用の `SurfaceEncodeSession` を facade に追加する。

汎用APIを壊したくない場合は後者、zero-copy を全 backend で共通化したい場合は前者が向いています。

## 6. Android options 型

`video-hw-core` に Android options を追加します。

```rust
#[derive(Debug, Clone, Default)]
pub struct AndroidDecoderOptions {
    pub codec_name: Option<String>,
    pub use_async: bool,
    pub allow_surface_output: bool,
    pub require_flexible_yuv: bool,
    pub timeout_us: i64,
}

#[derive(Debug, Clone)]
pub struct AndroidEncoderOptions {
    pub codec_name: Option<String>,
    pub bitrate: Option<u32>,
    pub i_frame_interval_sec: Option<f32>,
    pub use_input_surface: bool,
    pub require_hardware: Option<bool>,
    pub timeout_us: i64,
}
```

そして既存 enum に variant を追加します。

```rust
pub enum BackendDecoderOptions {
    Default,
    VideoToolbox(VtDecoderOptions),
    Nvidia(NvidiaDecoderOptions),
    Intel(IntelDecoderOptions),
    Vulkan(VulkanDecoderOptions),
    Android(AndroidDecoderOptions),
}

pub enum BackendEncoderOptions {
    Default,
    VideoToolbox(VtEncoderOptions),
    Nvidia(NvidiaEncoderOptions),
    Intel(IntelEncoderOptions),
    Vulkan(VulkanEncoderOptions),
    Android(AndroidEncoderOptions),
}
```

## 7. RAII / FFI安全設計

FFI pointer は直接露出させず、以下のような RAII wrapper に閉じます。

```rust
struct MediaCodec(NonNull<AMediaCodec>);
struct MediaFormat(NonNull<AMediaFormat>);
struct NativeWindow(NonNull<ANativeWindow>);

impl Drop for MediaCodec {
    fn drop(&mut self) {
        unsafe { AMediaCodec_delete(self.0.as_ptr()); }
    }
}
```

設計上の注意点:

- `AMediaCodec_*` の status は必ず `BackendError` に変換する。
- callback mode では `dequeueInputBuffer` / `dequeueOutputBuffer` を呼ばない。
- callback thread では重い処理をせず、lock-free queue または channel に event を積むだけにする。
- `ANativeWindow` / `AHardwareBuffer` は参照カウント規約に従い、取得と release を必ず対にする。

## 8. CapabilityReport の設計

MVPでは `query_capability()` を「静的契約 + runtime probe」の合成にします。

| 項目 | MVP設定案 |
|---|---|
| Decode codecs | H.264 は優先対応。HEVC / AV1 は codec create + configure probe 成功時のみ Available |
| Decode output modes | `Metadata` は基本対応、`Nv12` は CPU YUV output が得られる場合、`Rgb24` は NV12 変換可能な場合 |
| Encode codecs | H.264 優先。HEVC / AV1 encode は端末 capability 依存 |
| Encode input formats | `Nv12`、`Argb8888`（backend内変換あり） |
| Encoded layouts | H.264/HEVC は `AnnexB` に正規化、AV1 は `Av1` |
| Streaming mode | sync queue/dequeue 実装なら `Streaming`; MVPを簡略化するなら `FlushOnly` |
| Fallback policy | Android側に software codec があるため、`require_hardware` と `MediaCodecInfo` 結果で制御 |

JNI を使える構成なら、Java `MediaCodecList` / `MediaCodecInfo` で codec 名、encoder/decoder、hardware/software/vendor 属性、profile/level、color format を取得します。NDKのみで完結したい場合は、まず `AMediaCodec_create*ByType` + configure probe で実利用可能性を判定します。
