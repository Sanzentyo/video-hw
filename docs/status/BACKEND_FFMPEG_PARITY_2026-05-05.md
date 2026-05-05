# Backend / FFmpeg Parity Check - 2026-05-05

## Environment

- OS: Windows
- GPUs from `vulkaninfo --summary`:
  - GPU0: Intel(R) Graphics, driver 101.6629
  - GPU1: NVIDIA GeForce RTX 5070 Ti Laptop GPU, driver 591.44
- FFmpeg hwaccels observed: `cuda`, `qsv`, `d3d11va`, `vulkan`, `d3d12va`, `amf`

## Commands

```sh
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends nv,intel,vulkan --codec h264 --warmup 1 --repeat 3 --frame-count 30 --width 320 --height 180 --vulkan-adapter-indexes 0,1 --allow-failures true
```

## Reports

- Integrated: `output/benchmark-backends-h264-1777949002.md`
- NVIDIA detail: `output/benchmark-nv-precise-h264-1777948949.md`
- Intel detail: `output/benchmark-intel-precise-h264-1777948989.md`
- Vulkan detail: `output/benchmark-vulkan-h264-1777949002.md`

## Result Summary

| Backend | Case | Result |
|---|---|---|
| NVIDIA | H.264 decode | PASS: video-hw 1053.93 fps vs FFmpeg CUDA/CUVID 615.37 fps |
| NVIDIA | H.264 encode | PASS: video-hw 198.82 fps vs FFmpeg NVENC 190.04 fps (+4.62%) |
| Intel oneVPL | H.264 decode | PASS: video-hw 1078.85 fps vs FFmpeg QSV 1125.58 fps (-4.15%) |
| Intel oneVPL | H.264 encode | PASS: video-hw 74.66 fps vs FFmpeg QSV 72.65 fps (+2.77%) |
| Vulkan NVIDIA | H.264 decode | PASS: video-hw 603.68 fps vs FFmpeg Vulkan 512.49 fps |
| Vulkan NVIDIA | H.264 encode | PASS: video-hw 112.89 fps vs FFmpeg Vulkan 89.06 fps |
| Vulkan Intel | H.264 decode/encode | Not usable here: not exposed by `vk-video` / `video-hw`; FFmpeg Vulkan decode exits 0xc0000005 and encode fails with unsupported encode queue |

## Notes

- NV and Intel decode throughput comparisons use `--decode-output-mode metadata`
  so they match FFmpeg null-sink decode instead of measuring pixel readback.
- NV encode comparisons now default to equal raw ARGB input. The older synthetic
  vs lavfi comparison could report a false encode parity failure.
- Vulkan is now adapter-addressable in `video-hw` options and examples. The
  integrated runner matches adapters by name/device id instead of assuming
  `video-hw` and FFmpeg use the same numeric Vulkan index.
- `video-hw` / `vk-video` exposes the NVIDIA Vulkan video adapter on this
  machine. Intel Vulkan is listed by `vulkaninfo`, but is not usable by either
  `video-hw` or FFmpeg Vulkan in this environment; Intel hardware parity is
  covered through the oneVPL/QSV backend above.
- Vulkan metadata decode now uses GPU texture decode instead of the byte decoder
  path, avoiding CPU NV12 readback for metadata-only benchmarks.
