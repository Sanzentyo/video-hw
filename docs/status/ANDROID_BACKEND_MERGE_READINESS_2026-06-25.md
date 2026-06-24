# Android Backend Merge Readiness - 2026-06-25

## Summary

The Android backend branch is ready to be reviewed as an experimental Android
MediaCodec backend with a device smoke record. It is not yet a broad device
compatibility claim.

## Host Validation

- `cargo fmt --all -- --check`: pass
- `cargo test --workspace --all-targets`: pass
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- `cargo deny check licenses bans sources`: pass
  - The deny run still reports warnings for the Sanzentyo `onevpl-rs` fork
    missing license metadata/clarification files, plus duplicate transitive
    crates. These are warnings under the current policy, not license failures.

## Android Device Smoke

- Device connection: `192.168.0.244:42133`
- Battery state at install/run: 100%, AC powered
- APK build: `.\scripts\android-camera-apk\build.ps1`
- APK installed: `output\android-camera-apk\video-hw-camera-smoke.apk`
- Activity: `com.example.videohwcamera/.CameraSmokeActivity`
- Log artifact: `output\android-camera-apk\camera-smoke-20260625.log`

Smoke result:

- Camera candidate selected: `4080x3060@30-30fps`
- Surface input path: enabled
- Rust native recorder path: used
- MP4 write status: `PASS`
- MediaCodec decode status: `PASS`
- Encoded frames: 84
- Decoded frames: 84
- Packets: 87
- Keyframes: 6
- Raw H.264 bytes: 28,727,778
- MP4 bytes: 28,729,649
- Duration: 252,000 ticks at 90 kHz

## Changes Made During Readiness Pass

- Updated the NVIDIA integrated pipeline scheduler unit test to exercise the
  scheduler through explicit NVIDIA encoder options while preserving the safe
  pending-frame assertion.
- Moved a feature-gated `Dimensions` import into the matching cfg-gated import
  group to keep test builds warning-free.
- Rewrote an Intel ARGB-to-I420 chroma averaging guard using `NonZeroU32`, which
  keeps the same behavior while satisfying current clippy.
- Replaced a Vulkan HEVC encode candidate sort with `sort_by_key(Reverse(...))`
  to satisfy current clippy without changing adapter priority.

## Remaining Merge Caveats

- Android validation currently covers one connected Samsung device and its
  MediaCodec implementation. It does not cover vendor differences across Pixel,
  Qualcomm-only, MediaTek, Exynos variants, or older API levels.
- The smoke APK builds a debug native recorder. That is enough for functional
  encode/decode readiness; performance claims should use release builds and
  separate measurements.
- The APK bundles Kotlin stdlib under Apache-2.0. Repository Rust code remains
  under `MIT OR Apache-2.0`.
