# Backend / FFmpeg Benchmark Scripts

## Integrated Runner

Use the integrated runner when comparing all available `video-hw` decode/encode
backends against FFmpeg from one command:

```sh
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --codec h264 --warmup 1 --repeat 5 --frame-count 300 --verify
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --codecs h264,hevc --warmup 1 --repeat 5 --frame-count 300 --verify
```

By default, the runner selects host-appropriate backends:

- Windows/Linux: `nv,intel,vulkan`
- macOS: `vt`

You can select backends explicitly:

```sh
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends nv,intel,vulkan --codec h264 --warmup 1 --repeat 5 --verify
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends nv,intel,vulkan --codecs h264,hevc --warmup 1 --repeat 5 --verify
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends vt --codec h264 --warmup 1 --repeat 5 --verify
```

The integrated runner writes an aggregate report to:

```text
output/benchmark-backends-<codec>-<epoch>.md
```

When `--codecs` is used, the runner executes the selected backend set once per
codec and writes one aggregate report per codec.

For NV, Intel, and VT, it calls the backend-specific precise scripts and links
their generated reports. Vulkan is measured directly by the integrated runner:
it discovers Vulkan adapters with `vulkaninfo --summary` (or uses
`--vulkan-adapter-indexes`) and records `video-hw` decode/encode plus FFmpeg
Vulkan decode/encode for each adapter.
For backend-specific scripts, the integrated runner also reads the generated
report's parity and verification result so a child command that completes but
reports `Overall: FAIL` is surfaced as a failed backend in the aggregate report.
The runner matches `video-hw` Vulkan adapters to FFmpeg Vulkan adapters by
device name/vendor/device id because the two tools can use different numeric
adapter indexes on hybrid-GPU systems.
For the FFmpeg Vulkan command line, the runner uses unnamed physical-device
selection (`-init_hw_device vulkan:<index>`). On the tested Windows hybrid-GPU
machine, the named form (`vulkan=vk:<index>`) did not select the requested
physical device and could silently route the encode case to Intel instead of
NVIDIA.

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
  The `list_vulkan_adapters` example prints the adapters exposed through
  `vk-video` / `video-hw`.
- VT benchmarking is macOS-only.

## Notes

- The integrated runner executes backend benchmarks sequentially. This avoids
  Cargo target directory races and feature-set collisions between backend builds.
- FFmpeg comparison modes are backend-specific. NV uses CUDA/NVENC paths in the
  NV script; Intel uses QSV paths in the Intel script; VT uses VideoToolbox on
  macOS. Vulkan uses FFmpeg `-hwaccel vulkan` for decode and
  `h264_vulkan` / `hevc_vulkan` for encode when the selected adapter supports
  the operation. The Vulkan FFmpeg path intentionally initializes devices as
  `vulkan:<index>` rather than `vulkan=vk:<index>` so adapter selection follows
  FFmpeg's physical-device index.
- NV and Intel decode timing defaults to `--decode-output-mode metadata`, which
  matches FFmpeg null-sink decode and avoids measuring pixel readback when the
  comparison is backend throughput. NV and Intel encode timing defaults to equal
  raw input where supported.
- Vulkan metadata decode uses GPU texture decode and avoids CPU NV12 readback,
  matching FFmpeg Vulkan null-sink decode more closely.
- Vulkan HEVC encode is still reported as unavailable by `video-hw` on this
  environment. The backend contains an ignored live probe for driver/session
  diagnostics, but `VulkanEncoderAdapter` must not claim HEVC encode support
  until the direct Vulkan `cmdEncodeVideoKHR` path produces a non-empty
  bitstream and passes FFmpeg decode verification. The current probe also maps
  VPS sub-layer ordering and timing fields into StdVideo session parameters;
  it also uses an FFmpeg-like H.265 slice-header flag baseline and H.265
  rate-control pNext when the FFmpeg control probe mode is selected. It also
  wires zeroed SPS/VPS HRD payload pointers while keeping HRD-present flags off,
  uses a VPS-owned profile-tier-level pointer, and maps CBR/VBR slice
  `constant_qp` / `slice_qp_delta` to FFmpeg's shape. Session creation now also
  uses the device max coded extent like FFmpeg, and profile pNext chain ordering
  now matches FFmpeg's H.265-profile-before-usage shape. Those parity
  improvements do not yet unblock NVIDIA HEVC encode submit on this driver.
- Intel oneVPL decode uses backend default async depth 16. The Intel precise
  script still accepts `--intel-decode-async-depth <1..=16>` for tuning or
  regression checks.
- Use `--allow-failures false` when CI should fail immediately on the first
  backend benchmark error.
- Reports use wall-clock timings and should be run on an otherwise quiet system
  for stable numbers. Prefer `--warmup 1` or higher to exclude first-run driver
  initialization costs.
