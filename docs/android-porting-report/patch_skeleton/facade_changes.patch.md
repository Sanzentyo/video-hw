# facade / core 変更例

## root Cargo.toml

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

## crates/video-hw-core/Cargo.toml

```diff
 [features]
 default = []
 backend-vt = []
 backend-nvidia = []
 backend-intel = []
 backend-vulkan = []
+backend-android = []
```

## crates/video-hw/Cargo.toml

```diff
 [features]
 default = []
+backend-android = [
+  "video-hw-core/backend-android",
+  "dep:video-hw-backend-android",
+  "video-hw-backend-android/backend-android",
+]
 backend-vt = [
   "video-hw-core/backend-vt",
   "dep:video-hw-backend-vt",
   "video-hw-backend-vt/backend-vt",
 ]
```

```diff
 [dependencies]
 video-hw-core = { path = "../video-hw-core" }
+video-hw-backend-android = { path = "../video-hw-backend-android", optional = true, default-features = false }
 video-hw-backend-nvidia = { path = "../video-hw-backend-nvidia", optional = true, default-features = false }
```

## crates/video-hw/src/lib.rs

```diff
+#[cfg(all(target_os = "android", feature = "backend-android"))]
+pub use video_hw_backend_android::{AndroidDecoderAdapter, AndroidEncoderAdapter};
+
 pub enum BackendKind {
+    #[cfg(all(target_os = "android", feature = "backend-android"))]
+    Android,
     #[cfg(all(target_os = "macos", feature = "backend-vt"))]
     VideoToolbox,
 }
```

```diff
 impl fmt::Display for BackendKind {
     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
         match self {
+            #[cfg(all(target_os = "android", feature = "backend-android"))]
+            Self::Android => f.write_str("android"),
             #[cfg(all(target_os = "macos", feature = "backend-vt"))]
             Self::VideoToolbox => f.write_str("videotoolbox"),
         }
     }
 }
```

```diff
 impl FromStr for Backend {
     type Err = BackendParseError;

     fn from_str(raw: &str) -> Result<Self, Self::Err> {
         match raw.to_ascii_lowercase().as_str() {
             "auto" => Ok(Self::Auto),
+            #[cfg(all(target_os = "android", feature = "backend-android"))]
+            "android" | "mediacodec" | "mc" => Ok(Self::Android),
             _ => Err(BackendParseError::new(raw)),
         }
     }
 }
```

```rust
#[cfg(all(target_os = "android", feature = "backend-android"))]
fn preferred_backend_order() -> Vec<BackendKind> {
    vec![BackendKind::Android]
}

#[cfg(all(target_os = "android", feature = "backend-android"))]
impl DecoderBackend for AndroidDecoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Android;

    fn from_decoder_config(config: DecoderConfig) -> Self {
        Self::new(config)
    }

    fn supports_output_mode(mode: DecodeOutputMode) -> bool {
        matches!(
            mode,
            DecodeOutputMode::Metadata | DecodeOutputMode::Nv12 | DecodeOutputMode::Rgb24
        )
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

## crates/video-hw-core/src/lib.rs のoptions追加例

実際の定義位置に合わせて追加してください。

```rust
#[derive(Debug, Clone)]
pub struct AndroidDecoderOptions {
    pub codec_name: Option<String>,
    pub use_async: bool,
    pub allow_surface_output: bool,
    pub require_flexible_yuv: bool,
    pub timeout_us: i64,
}

impl Default for AndroidDecoderOptions {
    fn default() -> Self {
        Self {
            codec_name: None,
            use_async: false,
            allow_surface_output: false,
            require_flexible_yuv: true,
            timeout_us: 10_000,
        }
    }
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

impl Default for AndroidEncoderOptions {
    fn default() -> Self {
        Self {
            codec_name: None,
            bitrate: None,
            i_frame_interval_sec: Some(1.0),
            use_input_surface: false,
            require_hardware: None,
            timeout_us: 10_000,
        }
    }
}
```
