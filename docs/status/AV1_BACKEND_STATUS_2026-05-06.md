# AV1 Backend Status - 2026-05-06

## Objective

AV1-capable backends should support encode/decode, AV1 fMP4 should write/read
`av01` / `av1C`, and measured video-hw performance should target FFmpeg parity
or better.

## Completion Checklist

| Requirement | Evidence | Status |
|---|---|---|
| Public codec/layout contract has AV1 | `Codec::Av1`, `EncodedLayout::Av1` in `video-hw-core`; layout inference test in `video-hw` | Done |
| NVIDIA AV1 decode | `NvDecoderAdapter` maps AV1 to NVDEC; benchmark `output/benchmark-nv-precise-av1-1778049346.md`; PSNR `output/av1-psnr/av1-psnr-1778051481.md` | Done |
| NVIDIA AV1 encode | `NvEncoderAdapter` maps AV1 to NVENC and repeats sequence header; FFmpeg parity report shows PASS | Done |
| Intel oneVPL AV1 decode | `OneVplCodec::AV1`; Intel AV1 RGB decode fixed by `874bc71`; PSNR report has Intel decode PASS | Done |
| Intel oneVPL AV1 encode | `OneVplCodec::AV1`; CQP default for AV1; integrated benchmark `output/benchmark-intel-precise-av1-1778050647.md` | Done |
| AV1 fMP4 writer | `av01` sample entry / `av1C` box; writer helper tests cover OBU passthrough and `config_obus` | Done |
| AV1 fMP4 reader | reader tests cover AV1 codec/layout detection, `av1C` parameter access, and OBU passthrough | Done |
| FFmpeg comparison | `scripts/benchmark_ffmpeg_backends.rs`, NV/Intel precise scripts, reports in `output/*av1*.md` | Done for NVIDIA/Intel |
| PSNR/MSE verification | `scripts/check_av1_psnr.rs`; latest report `output/av1-psnr/av1-psnr-1778051481.md` | Done for NVIDIA/Intel |
| Vulkan AV1 decode/encode | `vulkan_backend.rs` returns explicit `UnsupportedConfig`; regression tests prevent false capability claims | Not implemented |
| VideoToolbox AV1 decode/encode | `vt_backend.rs` returns explicit `UnsupportedConfig`; macOS target check and unsupported tests cover the current contract | Not implemented |

## Latest Verified Results

- NVIDIA integrated AV1 benchmark:
  - `output/benchmark-backends-av1-1778049346.md`
  - detail: `output/benchmark-nv-precise-av1-1778049346.md`
  - video-hw decode around 252 fps vs FFmpeg around 227 fps
  - video-hw encode around 194 fps vs FFmpeg around 176 fps
- Intel integrated AV1 benchmark:
  - `output/benchmark-backends-av1-1778050647.md`
  - detail: `output/benchmark-intel-precise-av1-1778050647.md`
  - video-hw decode 0.406s vs FFmpeg 0.392s
  - video-hw encode 0.425s vs FFmpeg 0.429s
- AV1 PSNR smoke:
  - `output/av1-psnr/av1-psnr-1778051481.md`
  - NVIDIA encode PSNR-Y avg 60.43 dB; decode PSNR-Y min 50.54 dB
  - Intel encode PSNR-Y avg 55.62 dB; decode PSNR-Y min 50.48 dB
- AV1 fMP4 smoke files:
  - `output/synthetic-av1-fmp4.mp4`
  - `output/synthetic-intel-av1-fmp4.mp4`

## Vulkan AV1 Status

Vulkan AV1 is intentionally not reported as supported by video-hw yet. The
integrated benchmark records explicit unsupported errors:

- `output/benchmark-vulkan-av1-1778051619.md`
- `output/benchmark-backends-av1-1778051619.md`

Observed FFmpeg behavior on this Windows host:

- NVIDIA adapter: FFmpeg `av1_vulkan` encode/decode can run.
- Intel adapter: FFmpeg `av1_vulkan` encode fails because the device does not
  expose the encode queue / AV1 encode path. Intel advertises decode-related
  Vulkan video capability, but this is not a passing video-hw AV1 implementation.

Remaining work is a real Vulkan AV1 path using `VK_KHR_video_decode_av1` and, for
encode, `VK_KHR_video_encode_av1` where the driver exposes it. The current HEVC
Vulkan implementation is not a drop-in AV1 implementation because AV1 requires
different codec profile/session parameters, picture info, reference handling,
OBU packetization, and validation/PSNR gates.
The detailed implementation plan is
`docs/plan/VULKAN_AV1_IMPLEMENTATION_PLAN_2026-05-06.md`.

## VideoToolbox AV1 Status

VideoToolbox AV1 is intentionally not reported as supported by video-hw yet.
Current safeguards:

- capability reports return unsupported for AV1;
- decode/encode runtime paths return `VideoToolbox AV1 ... is not implemented in video-hw yet`;
- `scripts/benchmark_ffmpeg_vt_precise.rs --codec av1` produces a FAIL report
  on macOS instead of treating AV1 as a passing parity case;
- `cargo check -p video-hw-backend-vt --target x86_64-apple-darwin --features backend-vt --tests`
  passes on the cross target.

Remaining work requires a macOS host with AV1 VideoToolbox support to verify
format description creation, encoded packet layout, fMP4 integration, FFmpeg
`av1_videotoolbox` comparison, and PSNR.

## Next Concrete Work

1. Implement Vulkan AV1 decode first, gated behind adapter capability detection.
2. Add Vulkan AV1 OBU parser/session parameter extraction and PSNR check against
   FFmpeg software decode.
3. Add Vulkan AV1 encode only for adapters exposing encode queue and
   `VK_KHR_video_encode_av1`; report Intel encode as unavailable when FFmpeg also
   cannot encode.
4. On macOS AV1 hardware, prototype VideoToolbox AV1 encode/decode and update
   the VT unsupported contract only after fMP4, FFmpeg benchmark, and PSNR pass.
