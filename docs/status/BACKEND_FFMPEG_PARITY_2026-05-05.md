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

### H.264

| Backend | Case | Result |
|---|---|---|
| NVIDIA | H.264 decode | PASS: video-hw 1053.93 fps vs FFmpeg CUDA/CUVID 615.37 fps |
| NVIDIA | H.264 encode | PASS: video-hw 198.82 fps vs FFmpeg NVENC 190.04 fps (+4.62%) |
| Intel oneVPL | H.264 decode | PASS: video-hw 1078.85 fps vs FFmpeg QSV 1125.58 fps (-4.15%) |
| Intel oneVPL | H.264 encode | PASS: video-hw 74.66 fps vs FFmpeg QSV 72.65 fps (+2.77%) |
| Vulkan NVIDIA | H.264 decode | PASS: video-hw 603.68 fps vs FFmpeg Vulkan 512.49 fps |
| Vulkan NVIDIA | H.264 encode | PASS: video-hw 112.89 fps vs FFmpeg Vulkan 89.06 fps |
| Vulkan Intel | H.264 decode/encode | Not usable here: not exposed by `vk-video` / `video-hw`; FFmpeg Vulkan decode exits 0xc0000005 and encode fails with unsupported encode queue |

### HEVC audit

- Integrated: `output/benchmark-backends-hevc-1777949168.md`
- NVIDIA detail: `output/benchmark-nv-precise-hevc-1777949113.md`
- Intel detail: `output/benchmark-intel-precise-hevc-1777949151.md`
- Vulkan detail: `output/benchmark-vulkan-hevc-1777949168.md`

| Backend | Case | Result |
|---|---|---|
| NVIDIA | HEVC decode | PASS: video-hw 1003.27 fps vs FFmpeg CUDA/CUVID 604.55 fps |
| NVIDIA | HEVC encode | PASS: video-hw 201.17 fps vs FFmpeg NVENC 208.88 fps (-3.69%) |
| Intel oneVPL | HEVC decode | PASS: video-hw 1358.28 fps vs FFmpeg QSV 1403.59 fps (-3.23%) |
| Intel oneVPL | HEVC encode | PASS: video-hw 71.27 fps vs FFmpeg QSV 71.77 fps (-0.71%) |
| Vulkan NVIDIA | HEVC decode | PASS: video-hw 690.15 fps vs FFmpeg Vulkan 511.34 fps |
| Vulkan NVIDIA | HEVC encode | Not complete: `video-hw` reports `Vulkan HEVC encode initialization failed; runtime prerequisites are present, but the direct ash-level HEVC encode submit path is not wired yet`; FFmpeg Vulkan HEVC encode on the NVIDIA adapter succeeds at 87.55 fps |
| Vulkan Intel | HEVC decode/encode | Decode works in FFmpeg Vulkan at 196.72 fps but is not exposed by `vk-video` / `video-hw`; FFmpeg Vulkan encode fails with unsupported encode queue |

### Vulkan HEVC encode live probe

The direct ash-level Vulkan HEVC encode path remains blocked after the latest
probe work. The live ignored test can be run explicitly:

```sh
cargo test -p video-hw-backend-vulkan --features backend-vulkan live_hevc_encode_session_bootstrap_reports_submit_feedback -- --ignored --nocapture
```

Optional environment variables:

- `VIDEO_HW_VULKAN_HEVC_ENCODE_LIVE_WIDTH`
- `VIDEO_HW_VULKAN_HEVC_ENCODE_LIVE_HEIGHT`
- `VIDEO_HW_VULKAN_HEVC_ENCODE_LIVE_FPS`
- `VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_MODE`

On the RTX 5070 Ti Laptop GPU, the HEVC encode feedback query pool now requires
an explicit HEVC video profile in the query pool `pNext` chain; without it,
`vkCreateQueryPool` fails. With that fixed, session and parameter creation
complete, but the submit probe still fails at `vkEndCommandBuffer` whenever
`cmdEncodeVideoKHR` is included. Both the 320x180 `empty-template` smoke case
and the 1920x1080 sample-parameter case fail, so the blocker is not only the
repository sample parameter set's coded size. The probe now carries SPS-derived
SAO and temporal-MVP flags into H.265 slice/picture info, matching FFmpeg's
Vulkan HEVC encode setup more closely, but that does not unblock submit on this
driver. VPS parsing now also feeds VPS sub-layer ordering, timing flags, timing
values, and a VPS-owned `StdVideoH265DecPicBufMgr` into the StdVideo parameter
storage instead of reusing only SPS-derived DPB data. The 1920x1080 live probe
still fails at `vkEndCommandBuffer`, and session-parameter feedback still fails
with `ERROR_OUT_OF_HOST_MEMORY`, so the blocker is not limited to missing VPS
timing/DPB fields. The encode slice-header flags now also use an FFmpeg-like
baseline, including an explicit `collocated_from_l0_flag=1`; the same 1920x1080
probe still fails, so that flag shape is not sufficient either. The
`control_mode=ffmpeg` path now also attaches an H.265-specific rate-control
pNext with FFmpeg-style flat regular GOP hints; that path still fails at
`vkEndCommandBuffer` too. StdVideo SPS VUI construction no longer advertises
HRD parameters or scaling lists while their payload flags are off, but now keeps
zeroed HRD payload pointers wired like FFmpeg's H.265 Vulkan conversion. VPS
StdVideo construction now also uses the VPS's own profile-tier-level instead of
reusing the SPS profile-tier-level pointer. With `rate_control_mode=cbr` and
`control_mode=ffmpeg`, slice `constant_qp=0` and `slice_qp_delta=-26` now match
FFmpeg's rate-control shape, but the 1920x1080 sample live probe still reports
session-parameter feedback `ERROR_OUT_OF_HOST_MEMORY` and fails at
`vkEndCommandBuffer`. The aligned `1920x1088` `empty-template` probe also still
fails, so coded-size granularity is not the remaining blocker. More complete
HEVC session parameter generation and picture info wiring are still required
before enabling `Codec::Hevc` in `VulkanEncoderAdapter`.

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
