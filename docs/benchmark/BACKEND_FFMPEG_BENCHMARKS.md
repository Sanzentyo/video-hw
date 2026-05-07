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

For NVIDIA work, use only H.264/HEVC until AV1 support is explicitly included
in the benchmark plan:

```sh
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec h264 --warmup 1 --repeat 5 --verify --equal-raw-input --include-internal-metrics
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs --codec hevc --warmup 1 --repeat 5 --verify --equal-raw-input --include-internal-metrics
```

The NV precise script has three decode comparison modes:

| Mode | video-hw path | FFmpeg path | Use for |
|---|---|---|---|
| `--decode-output-mode metadata` | NVDEC decode, no host pixel payload | CUVID decode to null sink | Pure decoder throughput / parser overhead |
| `--decode-output-mode nv12` | NVDEC decode plus host NV12 payload | CUVID + `hwdownload,format=nv12` to rawvideo null sink | Host readback comparison |
| `--decode-output-mode rgb24` | NVDEC decode plus host RGB24 payload | CUVID + `hwdownload,format=nv12,format=rgb24` to rawvideo null sink | Application paths that need RGB frames |

Do not compare a `metadata` video-hw decode run with an FFmpeg
`hwdownload`/RGB run, or a video-hw RGB run with FFmpeg null-sink decode. Those
measure different work.

Encode comparison should normally use `--equal-raw-input`, which feeds the same
generated ARGB raw frame stream to both video-hw and FFmpeg. Without it, the
script is useful only as a smoke test or backend trend check because video-hw
and FFmpeg generate different synthetic inputs.

Use `--ffmpeg-path` and `--ffprobe-path` when comparing against a local FFmpeg
build, for example:

```sh
cargo +nightly -Zscript scripts/benchmark_ffmpeg_nv_precise.rs \
  --codec h264 \
  --decode-output-mode rgb24 \
  --warmup 1 \
  --repeat 5 \
  --verify \
  --equal-raw-input \
  --include-internal-metrics \
  --ffmpeg-path /home/sanzentyo/git/ffmpeg-nvidia-build/bin/ffmpeg \
  --ffprobe-path /home/sanzentyo/git/ffmpeg-nvidia-build/bin/ffprobe
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
- Vulkan HEVC encode now has an experimental `VulkanEncoderAdapter` production
  path on the tested NVIDIA adapter. It uses FFmpeg `hevc_vulkan` to generate a
  same-size parameter/header sample, prepends those leading non-VCL NAL units to
  the direct Vulkan `cmdEncodeVideoKHR` slice output, and emits IDR-only Annex-B
  packets. This is decodable by FFmpeg, but it is not performance parity yet:
  the current implementation reuses the Vulkan video session and session
  parameters within one `flush`, but still emits IDR-only packets and rebuilds
  per-submit resources instead of using a long-lived production encoder. The
  2026-05-05 640x360 / 30-frame cold batch run measured video-hw Vulkan HEVC
  encode at 18.519 fps versus FFmpeg Vulkan HEVC encode at 84.270 fps on the
  NVIDIA adapter, showing that first process/driver initialization dominates
  short cold runs. With `--warmup 1 --repeat 3` at 320x180 / 30 frames, the
  same NVIDIA adapter measured video-hw Vulkan HEVC encode at 135.137 fps
  versus FFmpeg Vulkan HEVC encode at 87.344 fps.
  The current probe also maps
  VPS sub-layer ordering and timing fields into StdVideo session parameters;
  it also uses an FFmpeg-like H.265 slice-header flag baseline and H.265
  rate-control pNext when the FFmpeg control probe mode is selected. It also
  wires zeroed SPS/VPS HRD payload pointers while keeping HRD-present flags off,
  uses a VPS-owned profile-tier-level pointer, defaults disabled rate-control
  fixed QP to FFmpeg's 18, defaults encode quality level to FFmpeg's 0,
  initializes the source image by uploading NV12 planes, exposes source
  picture-resource extent selection for FFmpeg parity probes, can omit the
  H.265-specific session-create pNext to match FFmpeg, can attach rate-control
  pNext to begin-coding, can reserve the final bitstream alignment from
  `dstBufferRange` like FFmpeg, can override sample-derived SPS dimensions to
  the probe coded size, can omit encode image-view YCbCr-conversion pNext for
  A/B probing, can write parameter-sample bytes into the externally encoded
  prefix area, can preserve SPS VUI from an FFmpeg-generated parameter sample,
  and maps CBR/VBR slice `constant_qp` / `slice_qp_delta` to FFmpeg's shape.
  Session creation now also uses the device max coded extent like FFmpeg, and
  profile pNext chain ordering now matches FFmpeg's H.265-profile-before-usage
  shape. With an FFmpeg `hevc_vulkan` 320x180 parameter sample and
  `parameter_vui_safety=preserve`, the ignored NVIDIA live probe reaches
  `Ready(bytes_written=47)`. The probe now reads output at
  `dstBufferOffset + feedback_offset` and can dump that slice with
  `VIDEO_HW_VULKAN_HEVC_ENCODE_OUTPUT_PATH`; when the FFmpeg-generated
  parameter/header NAL prefix is prepended, FFmpeg decodes the one-frame stream
  and the flat NV12 probe input compares at MSE=0 / PSNR=inf. This is still a
  one-frame diagnostic path. `VulkanEncoderAdapter` now uses the same slice
  extraction and header-prefix packetization for an experimental batched
  IDR-only path. A long-lived encoder with reusable per-frame resources and
  reference-frame GOP encode is still required for production completeness and
  normal compression behavior, even though the warm steady-state throughput is
  already at parity or faster on the tested NVIDIA adapter.
- Intel oneVPL decode uses backend default async depth 16. The Intel precise
  script still accepts `--intel-decode-async-depth <1..=16>` for tuning or
  regression checks.
- Intel precise encode verification keeps the video-hw output when `--verify`
  is enabled. Intel HEVC packet collection normalizes oneVPL bitstream
  `DataOffset` before reading and uses the synchronous hardware HEVC-NV12 path
  for a decodable multi-frame Annex-B stream.
- Use `--allow-failures false` when CI should fail immediately on the first
  backend benchmark error.
- Reports use wall-clock timings and should be run on an otherwise quiet system
  for stable numbers. Prefer `--warmup 1` or higher to exclude first-run driver
  initialization costs.
