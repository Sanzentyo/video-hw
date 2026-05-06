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
| AV1 fMP4 reader | `video-hw-fmp4` reader tests; `cargo test -p video-hw-fmp4 --features "backend-nvidia backend-intel backend-vulkan"`; `output/av1-fmp4-roundtrip/av1-fmp4-roundtrip-1778069094.md` | Reader detects AV1 codec/layout, exposes `av1C`, keeps `to_annexb()` as OBU passthrough, prepares decoder payloads with `av1C.config_obus` on keyframes, and decodes generated MP4 via `decode_to_yuv --input-format mp4` | Done |
| fMP4 decode access/caching behavior | `scripts/benchmark_fmp4_decode_access.rs`; `output/benchmark-fmp4-decode-access-1778069696.md`; `output/benchmark-fmp4-decode-access-1778070138.md`; `cargo test -p video-hw-fmp4 --features "backend-nvidia backend-intel backend-vulkan"` | Sequential, random, reverse, reverse-prefetch, reverse-mismatch, and ping-pong access are benchmarked with MSE/PSNR and cache stats | Done for NVIDIA/Intel; Intel large run is a stress case |
| FFmpeg parity comparison | `scripts/benchmark_ffmpeg_backends.rs`; NV/Intel precise scripts; `output/*av1*.md` reports listed in `AV1_BACKEND_STATUS_2026-05-06.md` | NVIDIA/Intel AV1 parity is recorded; Vulkan keyframe-only AV1 decode parity is recorded | Done for NVIDIA/Intel; partial for Vulkan |
| PSNR/MSE verification | `scripts/check_av1_psnr.rs`; `scripts/check_av1_fmp4_roundtrip.rs`; `scripts/check_vulkan_av1_psnr.rs`; fMP4 access benchmark reports | Encode/decode and fMP4 decode correctness are checked against FFmpeg references; Vulkan generated OBU and fMP4 GOP30 now pass at `psnr_y_min=inf` | Done for supported NVIDIA/Intel paths; generated-GOP partial for Vulkan |
| Vulkan AV1 decode | `crates/video-hw-backend-vulkan/src/vulkan_av1_decode.rs`; `output/vulkan-av1-psnr/vulkan-av1-psnr-1778067206093.md`; `output/vulkan-av1-psnr/vulkan-av1-psnr-1778067206134.md`; `output/vulkan-av1-psnr/vulkan-av1-psnr-1778074633347.md`; `output/vulkan-av1-psnr/vulkan-av1-psnr-1778075814861.md`; `output/vulkan-av1-psnr/vulkan-av1-psnr-1778075801683.md`; `output/vulkan-av1-psnr/vulkan-av1-psnr-1778076585814.md`; `output/vulkan-av1-psnr/vulkan-av1-psnr-1778076599799.md`; `output/vulkan-av1-psnr/vulkan-av1-psnr-1778077258665.md`; `output/benchmark-vulkan-av1-1778068460.md`; `output/benchmark-vulkan-av1-1778068469.md`; `output/benchmark-vulkan-av1-1778076588.md`; `output/benchmark-vulkan-av1-1778076603.md` | Generated keyframe-only OBU/fMP4, short GOP OBU, long-GOP OBU, and long-GOP fMP4 decode on NVIDIA pass PSNR; integrated Vulkan benchmark `--verify` records passing OBU and fMP4 PSNR rows for generated GOP30/lag0 input using the measured `decode_to_yuv` binary | Partial |
| Vulkan AV1 inter-frame/GOP replay | `scripts/check_vulkan_av1_psnr.rs --gop-size 2`; `scripts/check_vulkan_av1_psnr.rs --gop-size 30`; `scripts/check_vulkan_av1_psnr.rs --gop-size 30 --lag-in-frames 25`; `scripts/check_vulkan_av1_corpus_matrix.rs`; reports `output/vulkan-av1-corpus-matrix/vulkan-av1-corpus-matrix-1778079826467.md`, `output/vulkan-av1-psnr/vulkan-av1-psnr-1778079834167.md`, `output/vulkan-av1-psnr/vulkan-av1-psnr-1778079835765.md`, `output/vulkan-av1-psnr/vulkan-av1-psnr-1778079840868.md`, `output/vulkan-av1-psnr/vulkan-av1-psnr-1778079842827.md`; `output/benchmark-vulkan-av1-1778078784.md`; `output/benchmark-vulkan-av1-1778078783.md` | The matrix covers OBU/fMP4 keyframe-only, generated GOP30, 16-frame alt-ref/show-existing, 32-frame GOP16/lag8, and expected unsupported 32-frame GOP30/lag25 alias cases. Passing cases report `psnr_y_min=inf`; alias cases are accepted only when they fail with `aliases Vulkan DPB slot`, preventing silent low-PSNR output. | Partial |
| Vulkan AV1 encode | `scripts/check_vulkan_av1_encode_bindings.rs`; `cargo info ash`; `output/vulkan-av1-encode-bindings/vulkan-av1-encode-bindings-1778079390.md` | crates.io still reports `ash 0.38.0+1.3.281`; the local source exposes AV1 decode bindings but no `VK_KHR_video_encode_av1` / `VideoEncodeAV1` bindings | Blocked |
| Intel Vulkan AV1 | `output/benchmark-vulkan-av1-1778068460.md`; `docs/status/AV1_BACKEND_STATUS_2026-05-06.md` | FFmpeg Vulkan AV1 decode exits with Windows access violation on this host; FFmpeg `av1_vulkan` encode reports unsupported implementation | Not a passing target on this host |
| VideoToolbox AV1 | `crates/video-hw-backend-vt/src/vt_backend.rs`; `crates/video-hw-core/src/lib.rs`; `crates/video-hw-fmp4/src/fmp4_reader/decode.rs`; `crates/video-hw/examples/decode_to_yuv.rs`; `cargo check -p video-hw-backend-vt --target x86_64-apple-darwin --features backend-vt --tests`; `cargo test -p video-hw-fmp4 --features "backend-nvidia backend-intel backend-vulkan" av1_track_options_preserve_fmp4_av1c_record` | fMP4 AV1 decode bootstrap is scaffolded for VT by passing `av1C` record/config OBUs and track dimensions into `VtDecoderOptions`, creating a `CMVideoFormatDescription` with `SampleDescriptionExtensionAtoms.av1C`, and passing AV1 sample OBUs without AVCC/HVCC packing. Actual macOS hardware decode, FFmpeg parity, and PSNR are not verified on this Windows host; AV1 encode is still explicitly unsupported. | Decode scaffolded; encode not implemented |

## Current Blockers

1. Vulkan AV1 decode is still not an arbitrary-stream support claim. Generated
   keyframe-only, short GOP, long GOP OBU/fMP4, and 16-frame
   `lag-in-frames 25` alt-ref/show-existing OBU/fMP4 now pass PSNR on NVIDIA.
   A 32-frame `lag-in-frames 25` stress is explicitly rejected because the
   stream needs reference-slot aliasing that the current Vulkan DPB model cannot
   represent without returning incorrect pixels.
2. Vulkan AV1 encode cannot be implemented safely with the current `ash`
   binding set because the required `VK_KHR_video_encode_av1` symbols are not
   exposed.
3. VideoToolbox AV1 decode now has an fMP4 bootstrap path, but requires a macOS
   host with AV1 VideoToolbox support for real decode, FFmpeg parity, and PSNR
   validation. VideoToolbox AV1 encode remains unimplemented.

## Conclusion

The objective is not complete. NVIDIA and Intel oneVPL AV1 encode/decode plus
AV1 fMP4 read/write are implemented and verified against FFmpeg. Vulkan AV1 is
verified for generated keyframe-only, short-GOP, long-GOP OBU/fMP4, and
16-frame alt-ref/show-existing OBU/fMP4 decode on NVIDIA. VideoToolbox AV1 fMP4
decode is scaffolded but not hardware-verified. Broader arbitrary-stream Vulkan
decode hardening, Vulkan AV1 encode, VideoToolbox AV1 decode validation, and
VideoToolbox AV1 encode remain open.
