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

- Integrated: `output/benchmark-backends-h264-1777947507.md`
- NVIDIA detail: `output/benchmark-nv-precise-h264-1777947453.md`
- Intel detail: `output/benchmark-intel-precise-h264-1777947494.md`
- Vulkan detail: `output/benchmark-vulkan-h264-1777947507.md`

## Result Summary

| Backend | Case | Result |
|---|---|---|
| NVIDIA | H.264 decode | PASS: video-hw 1017.61 fps vs FFmpeg CUDA/CUVID 622.39 fps |
| NVIDIA | H.264 encode | PASS: video-hw 188.85 fps vs FFmpeg NVENC 198.10 fps (-4.67%) |
| Intel oneVPL | H.264 decode | PASS: video-hw 1072.69 fps vs FFmpeg QSV 1123.66 fps (-4.54%) |
| Intel oneVPL | H.264 encode | PASS: video-hw 74.64 fps vs FFmpeg QSV 71.63 fps (+4.21%) |
| Vulkan adapter 0 | video-hw H.264 decode/encode | PASS in the short adapter sweep |
| Vulkan adapter 0 | FFmpeg Vulkan H.264 | FAIL: FFmpeg Vulkan decode exits 0xc0000005 and encode lacks `VK_KHR_video_encode_queue` |
| Vulkan adapter 1 | FFmpeg Vulkan H.264 decode/encode | PASS in the short adapter sweep |
| Vulkan adapter 1 | video-hw H.264 decode/encode | BLOCKED: vk-video reports `Vulkan adapter index 1 is not available` |

## Notes

- NV and Intel decode throughput comparisons use `--decode-output-mode metadata`
  so they match FFmpeg null-sink decode instead of measuring pixel readback.
- NV encode comparisons now default to equal raw ARGB input. The older synthetic
  vs lavfi comparison could report a false encode parity failure.
- Vulkan is now adapter-addressable in `video-hw` options and examples, but this
  environment exposes two adapters via `vulkaninfo` while vk-video exposes only
  adapter 0 to `video-hw`. The integrated runner records this as a concrete
  per-adapter failure instead of hiding it.
