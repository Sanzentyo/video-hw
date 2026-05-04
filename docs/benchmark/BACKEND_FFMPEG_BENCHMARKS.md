# Backend / FFmpeg Benchmark Scripts

## Integrated Runner

Use the integrated runner when comparing all available `video-hw` decode/encode
backends against FFmpeg from one command:

```sh
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --codec h264 --warmup 1 --repeat 5 --frame-count 300 --verify
```

By default, the runner selects host-appropriate backends:

- Windows/Linux: `nv,intel,vulkan`
- macOS: `vt`

You can select backends explicitly:

```sh
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends nv,intel,vulkan --codec h264 --warmup 1 --repeat 5 --verify
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends vt --codec h264 --warmup 1 --repeat 5 --verify
```

The integrated runner writes an aggregate report to:

```text
output/benchmark-backends-<codec>-<epoch>.md
```

For NV, Intel, and VT, it also calls the existing backend-specific precise
scripts and links their generated reports. Vulkan currently has a decode-only
path in the integrated runner because there is no separate Vulkan encode
benchmark script.

## Backend-Specific Scripts

These scripts remain useful when tuning one backend in detail:

```sh
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec h264 --warmup 1 --repeat 7 --frame-count 300 --verify
cargo +nightly -Zscript scripts/benchmark_ffmpeg_intel_precise.rs --codec h264 --warmup 1 --repeat 7 --frame-count 300 --verify
cargo +nightly -Zscript scripts/benchmark_ffmpeg_vt_precise.rs --codec h264 --warmup 1 --repeat 7 --frame-count 300 --verify
```

## Requirements

- Rust nightly is required for `cargo +nightly -Zscript`.
- FFmpeg and FFprobe must be available on `PATH`.
- NVIDIA benchmarking requires NVDEC/NVENC support and the NVIDIA Video Codec
  SDK link libraries configured as expected by the workspace.
- Intel benchmarking requires oneVPL/QSV support. On Linux, the required Intel
  media driver stack must be installed and visible to oneVPL and FFmpeg.
- Vulkan benchmarking requires a Vulkan video decode capable driver/device.
- VT benchmarking is macOS-only.

## Notes

- The integrated runner executes backend benchmarks sequentially. This avoids
  Cargo target directory races and feature-set collisions between backend builds.
- FFmpeg comparison modes are backend-specific. NV uses CUDA/NVENC paths in the
  NV script; Intel uses QSV paths in the Intel script; VT uses VideoToolbox on
  macOS. Vulkan is currently compared against FFmpeg decode into a null sink.
- Use `--allow-failures false` when CI should fail immediately on the first
  backend benchmark error.
- Reports use wall-clock timings and should be run on an otherwise quiet system
  for stable numbers. Prefer `--warmup 1` or higher to exclude first-run driver
  initialization costs.
