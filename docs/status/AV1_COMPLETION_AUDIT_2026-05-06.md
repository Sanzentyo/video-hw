# AV1 Completion Audit - 2026-05-06

## Objective

Implement encode/decode for AV1-capable backends, support AV1 fMP4 read/write,
and measure video-hw performance against FFmpeg with a target of parity or
better.

## Prompt-To-Artifact Checklist

| Requirement | Evidence inspected | Coverage | Status |
|---|---|---|---|
| Public AV1 codec/layout contract | `Codec::Av1`, `EncodedLayout::Av1`; `cargo test -p video-hw --features "backend-nvidia backend-intel backend-vulkan" av1` | Confirms AV1 fMP4 length-prefixed samples are forwarded as OBU payload | Done |
| NVIDIA AV1 decode | `docs/status/AV1_BACKEND_STATUS_2026-05-06.md`; `output/benchmark-nv-precise-av1-1778049346.md`; `output/av1-psnr/av1-psnr-1778051481.md` | Benchmark and decode PSNR against FFmpeg reference are recorded | Done |
| NVIDIA AV1 encode | `NvEncoderAdapter` AV1 mapping; `output/benchmark-nv-precise-av1-1778049346.md`; `output/av1-psnr/av1-psnr-1778051481.md` | FFmpeg parity and encode PSNR recorded | Done |
| Intel oneVPL AV1 decode | `OneVplCodec::AV1`; `output/benchmark-intel-precise-av1-1778050647.md`; `output/av1-psnr/av1-psnr-1778051481.md` | Decode benchmark and PSNR recorded | Done |
| Intel oneVPL AV1 encode | `OneVplCodec::AV1`; `output/benchmark-intel-precise-av1-1778050647.md`; `output/av1-psnr/av1-psnr-1778051481.md` | Encode benchmark and PSNR recorded | Done |
| AV1 fMP4 writer | `video-hw-fmp4` writer tests; `scripts/check_av1_fmp4_roundtrip.rs`; `output/av1-fmp4-roundtrip/av1-fmp4-roundtrip-1778069094.md` | `av01` / `av1C` writer behavior and NVIDIA/Intel generated fMP4 roundtrip are verified | Done |
| AV1 fMP4 reader | `video-hw-fmp4` reader tests; `cargo test -p video-hw-fmp4 --features "backend-nvidia backend-intel backend-vulkan"`; `output/av1-fmp4-roundtrip/av1-fmp4-roundtrip-1778069094.md` | Reader detects AV1 codec/layout, exposes `av1C`, forwards OBU payload, and decodes generated MP4 via `decode_to_yuv --input-format mp4` | Done |
| fMP4 decode access/caching behavior | `scripts/benchmark_fmp4_decode_access.rs`; `output/benchmark-fmp4-decode-access-1778069696.md`; `output/benchmark-fmp4-decode-access-1778070138.md`; `cargo test -p video-hw-fmp4 --features "backend-nvidia backend-intel backend-vulkan"` | Sequential, random, reverse, reverse-prefetch, reverse-mismatch, and ping-pong access are benchmarked with MSE/PSNR and cache stats | Done for NVIDIA/Intel; Intel large run is a stress case |
| FFmpeg parity comparison | `scripts/benchmark_ffmpeg_backends.rs`; NV/Intel precise scripts; `output/*av1*.md` reports listed in `AV1_BACKEND_STATUS_2026-05-06.md` | NVIDIA/Intel AV1 parity is recorded; Vulkan keyframe-only AV1 decode parity is recorded | Done for NVIDIA/Intel; partial for Vulkan |
| PSNR/MSE verification | `scripts/check_av1_psnr.rs`; `scripts/check_av1_fmp4_roundtrip.rs`; `scripts/check_vulkan_av1_psnr.rs`; fMP4 access benchmark reports | Encode/decode and fMP4 decode correctness are checked against FFmpeg references | Done for supported NVIDIA/Intel paths; partial for Vulkan |
| Vulkan AV1 decode | `crates/video-hw-backend-vulkan/src/vulkan_av1_decode.rs`; `output/vulkan-av1-psnr/vulkan-av1-psnr-1778067206093.md`; `output/vulkan-av1-psnr/vulkan-av1-psnr-1778067206134.md`; `output/benchmark-vulkan-av1-1778068460.md`; `output/benchmark-vulkan-av1-1778068469.md` | Generated keyframe-only OBU and fMP4 decode on NVIDIA pass PSNR and outperform FFmpeg Vulkan for the measured scope | Partial |
| Vulkan AV1 inter-frame/GOP replay | `scripts/check_vulkan_av1_psnr.rs --gop-size 30`; `scripts/inspect_av1_frame_types.rs --expect-inter-frame`; reports `output/vulkan-av1-psnr/vulkan-av1-psnr-1778071155693.md`, `output/vulkan-av1-psnr/vulkan-av1-psnr-1778068145633.md`, `output/av1-frame-types/av1-frame-types-1778071020279919100.md`, `output/av1-frame-types/av1-frame-types-1778071024653131000.md` | The gap is explicitly detected: generated GOP input has `frame_type=1` after the keyframe, PSNR FAIL reports now include the exact `frame_type_gate`, and current Vulkan AV1 decode rejects it | Not done |
| Vulkan AV1 encode | `scripts/check_vulkan_av1_encode_bindings.rs`; `output/vulkan-av1-encode-bindings/vulkan-av1-encode-bindings-1778070521.md` | `ash 0.38.0+1.3.281` exposes AV1 decode bindings but no AV1 encode bindings | Blocked |
| Intel Vulkan AV1 | `output/benchmark-vulkan-av1-1778068460.md`; `docs/status/AV1_BACKEND_STATUS_2026-05-06.md` | FFmpeg Vulkan AV1 decode exits with Windows access violation on this host; FFmpeg `av1_vulkan` encode reports unsupported implementation | Not a passing target on this host |
| VideoToolbox AV1 | `crates/video-hw-backend-vt/src/vt_backend.rs`; `scripts/benchmark_ffmpeg_vt_precise.rs`; `cargo check -p video-hw-backend-vt --target x86_64-apple-darwin --features backend-vt --tests` | Current contract explicitly reports unsupported AV1 and cross-target check passes | Not implemented |

## Current Blockers

1. Vulkan AV1 inter-frame/GOP replay needs real reference-frame parsing and DPB
   management. The current generated GOP reports prove the failure is no longer
   ambiguous: frames after the keyframe are `frame_type=1`.
2. Vulkan AV1 encode cannot be implemented safely with the current `ash`
   binding set because the required `VK_KHR_video_encode_av1` symbols are not
   exposed.
3. VideoToolbox AV1 requires a macOS host with AV1 VideoToolbox support for
   format-description, packet layout, fMP4, FFmpeg parity, and PSNR validation.

## Conclusion

The objective is not complete. NVIDIA and Intel oneVPL AV1 encode/decode plus
AV1 fMP4 read/write are implemented and verified against FFmpeg. Vulkan AV1 is
verified only for generated keyframe-only decode on NVIDIA; Vulkan GOP replay,
Vulkan AV1 encode, and VideoToolbox AV1 remain open.
