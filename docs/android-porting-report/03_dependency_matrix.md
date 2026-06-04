# 03. Android対応に必要な依存関係

## 1. Cargo依存

新規 backend crate の推奨 `Cargo.toml` は次です。

```toml
[package]
name = "video-hw-backend-android"
version.workspace = true
edition.workspace = true
license.workspace = true

[features]
default = ["backend-android"]
backend-android = ["video-hw-core/backend-android"]
jni-capabilities = ["dep:jni"]

[dependencies]
video-hw-core = { path = "../video-hw-core", default-features = false }
thiserror = "2.0.18"
bitflags = "2"
log = "0.4"
jni = { version = "0.21", optional = true }
```

### bindgen を使うかどうか

| 方針 | 推奨度 | 理由 |
|---|---:|---|
| 手書き最小FFI | ◎ | `AMediaCodec` 周辺の必要関数だけを固定でき、Android cross build が安定する。 |
| build.rs + bindgen | ○ | header追従は楽だが、CI / cargo-ndk / NDK include path / clang の扱いが増える。 |
| `ndk-sys` 等に全面依存 | △ | crate側の NDK MediaCodec coverage を確認する必要がある。足りない関数は結局手書きになる。 |

MVPでは手書き最小FFIを推奨します。必要関数は `ffi.rs` に限定し、実装が進んだ段階で bindgen へ移行可能にします。

## 2. Android NDK / link libraries

NDK側で必要な header / library は以下です。

| 用途 | Header | Link library | 最低API目安 |
|---|---|---|---:|
| MediaCodec / MediaFormat | `media/NdkMediaCodec.h`, `media/NdkMediaFormat.h` | `mediandk` | 21 |
| NativeWindow / Surface | `android/native_window.h` | `android` | 21 / input surfaceは26 |
| log | `android/log.h` | `log` | 1 |
| HardwareBuffer | `android/hardware_buffer.h` | `android` | 26、`lockPlanes` は29 |

CMake で Rust shared library を link する場合は、少なくとも次を指定します。

```cmake
target_link_libraries(video_hw_android
    mediandk
    android
    log
)
```

Rust 側で `#[link(name = "mediandk")]` を指定する方式も可能です。Gradle/CMake との二重指定で問題が起きる場合は片方に寄せます。

## 3. Android API level の判断

| 機能 | API level | 設計上の扱い |
|---|---:|---|
| `AMediaCodec_createDecoderByType` / `createEncoderByType` / `configure` / sync dequeue | 21 | MVPの最低ライン |
| Encoder input `ANativeWindow` surface | 26 | zero-copy / GPU input はPhase 2 |
| Persistent input surface | 26 | encoder再利用やrenderer連携で有用 |
| `AMediaCodec_setAsyncNotifyCallback` | 28 | 低レイテンシ async backend で利用。MVPはsyncで可 |
| `AHardwareBuffer` 基本利用 | 26 | native buffer連携の基礎 |
| `AHardwareBuffer_lockPlanes` | 29 | YUV planeをCPUで読む場合に重要 |
| `MediaCodecInfo.isHardwareAccelerated` / `isSoftwareOnly` / `isVendor` | 29 | capability判定を正確にするならJNI利用で有用 |

推奨 minSdk:

- **MVP互換優先**: `minSdk 21`。sync ByteBuffer の H.264 decode/encode から開始。
- **実用性優先**: `minSdk 28`。async callback と format query が使いやすくなる。
- **native buffer / YUV plane重視**: `minSdk 29`。`AHardwareBuffer_lockPlanes` と Java codec hardware/software 判定を活用。

## 4. Gradle / Rust build

### cargo-ndk を使う場合

```bash
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk

ANDROID_NDK_HOME=$ANDROID_HOME/ndk/<version> \
  cargo ndk -t arm64-v8a -o app/src/main/jniLibs \
  build -p your-android-wrapper --release
```

### Android Gradle Plugin + CMake 連携

Android app module で `externalNativeBuild.cmake` を使い、CMake から Rust の `.so` を imported library として取り込みます。純Rust crateを直接Gradleがbuildするより、CIでは cargo-ndk を先に走らせて `jniLibs` へ配置する方式の方が単純です。

## 5. feature指定例

利用側 `Cargo.toml` は次のようになります。

```toml
[target.'cfg(target_os = "android")'.dependencies]
video-hw = {
  git = "https://github.com/Sanzentyo/video-hw",
  rev = "<android対応commit>",
  default-features = false,
  features = ["backend-android"]
}
```

ローカルworkspace内で試す場合:

```bash
cargo check -p video-hw \
  --target aarch64-linux-android \
  --no-default-features \
  --features backend-android
```

## 6. JNIを使う場合の追加依存

JNI は必須ではありませんが、以下を実現したい場合は有用です。

- `MediaCodecList` から codec 一覧を取得する。
- `MediaCodecInfo.isHardwareAccelerated()` / `isSoftwareOnly()` / `isVendor()` で hardware/software 判定を行う。
- Java `HardwareBuffer` と NDK `AHardwareBuffer` の相互変換を行う。
- Android app 側の `Surface` / `SurfaceTexture` を Rust に渡す。

推奨方針:

- MVP: NDK-only。`AMediaCodec_create*ByType` + configure probe で利用可否を判定。
- Phase 2: `jni-capabilities` feature を追加し、Java側 codec list を runtime capability に反映。
