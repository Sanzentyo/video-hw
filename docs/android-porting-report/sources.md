# 参照元

調査日: 2026-06-04（Asia/Tokyo）

## Sanzentyo/video-hw

- Repository: https://github.com/Sanzentyo/video-hw
- README: https://github.com/Sanzentyo/video-hw/blob/main/README.md
- `crates/video-hw/Cargo.toml`: https://github.com/Sanzentyo/video-hw/blob/main/crates/video-hw/Cargo.toml
- `crates/video-hw-core/Cargo.toml`: https://github.com/Sanzentyo/video-hw/blob/main/crates/video-hw-core/Cargo.toml
- `crates/video-hw/src/lib.rs`: https://github.com/Sanzentyo/video-hw/blob/main/crates/video-hw/src/lib.rs
- `crates/video-hw-core/src/lib.rs`: https://github.com/Sanzentyo/video-hw/blob/main/crates/video-hw-core/src/lib.rs
- `crates/video-hw-backend-vulkan/Cargo.toml`: https://github.com/Sanzentyo/video-hw/blob/main/crates/video-hw-backend-vulkan/Cargo.toml
- `crates/video-hw-backend-vulkan/src/lib.rs`: https://github.com/Sanzentyo/video-hw/blob/main/crates/video-hw-backend-vulkan/src/lib.rs
- `crates/video-hw-backend-vulkan/src/vulkan_backend.rs`: https://github.com/Sanzentyo/video-hw/blob/main/crates/video-hw-backend-vulkan/src/vulkan_backend.rs

## Android official docs

- Android NDK Media: https://developer.android.com/ndk/reference/group/media
  - `AMediaCodec_configure`: API 21
  - `AMediaCodec_createDecoderByType`: API 21
  - `AMediaCodec_createEncoderByType`: API 21
  - `AMediaCodec_createInputSurface`: API 26
  - `AMediaCodec_setAsyncNotifyCallback`: API 28
- Android NDK Native Hardware Buffer: https://developer.android.com/ndk/reference/group/a-hardware-buffer
  - `AHardwareBuffer_lockPlanes`: API 29
- Android Java MediaCodec: https://developer.android.com/reference/android/media/MediaCodec
- Android Java MediaCodecInfo: https://developer.android.com/reference/android/media/MediaCodecInfo
  - `isHardwareAccelerated`: API 29
  - `isSoftwareOnly`: API 29
  - `isVendor`: API 29
- Android Java MediaCodecList: https://developer.android.com/reference/android/media/MediaCodecList

## 注意

GitHub の main branch は更新される可能性があります。このレポートは 2026-06-04 時点で取得できた公開情報と主要ファイル内容に基づきます。最終実装では、実装開始時点の commit SHA を固定して差分を作成してください。
