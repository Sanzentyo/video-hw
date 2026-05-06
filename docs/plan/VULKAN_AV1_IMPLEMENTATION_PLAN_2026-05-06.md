# Vulkan AV1 Implementation Plan - 2026-05-06

## Goal

Add real Vulkan AV1 decode/encode support where the driver exposes Vulkan Video
AV1 capability, then validate fMP4 integration, PSNR, and FFmpeg parity. This
plan covers the missing work tracked in
`docs/status/AV1_BACKEND_STATUS_2026-05-06.md`.

## Current Boundary

- NVIDIA and Intel oneVPL AV1 are implemented and verified.
- Vulkan AV1 currently returns explicit `UnsupportedConfig`.
- Vulkan AV1 decode prerequisite probing and low-overhead OBU inspection are now
  implemented in `crates/video-hw-backend-vulkan/src/vulkan_av1_decode.rs`.
- FFmpeg `av1_vulkan` works on the NVIDIA adapter in this Windows environment.
- Intel Vulkan AV1 encode is not available here because FFmpeg also fails on the
  Intel adapter without the required encode queue/path.

## Decode-First Scope

Start with decode. It has a clearer acceptance gate than encode: the backend
must decode an AV1 OBU elementary stream to `Metadata`, `Nv12`, and `Rgb24`, and
the pixel output must pass PSNR against FFmpeg software decode.

### Phase 1: Capability And Device Plumbing

Files:

- `crates/video-hw-backend-vulkan/src/lib.rs`
- `crates/video-hw-backend-vulkan/src/vulkan_backend.rs`
- new `crates/video-hw-backend-vulkan/src/vulkan_av1_decode.rs`

Tasks:

1. Add a Vulkan AV1 decode prerequisite probe beside the HEVC probe. Done.
2. Query `VK_KHR_video_queue`, `VK_KHR_video_decode_queue`, and
   `VK_KHR_video_decode_av1`. Done.
3. Build `vk::VideoProfileInfoKHR` with
   `vk::VideoCodecOperationFlagsKHR::DECODE_AV1` and
   `vk::VideoDecodeAV1ProfileInfoKHR`. Done.
4. Query `vk::VideoDecodeAV1CapabilitiesKHR` and output formats. Done.
5. Keep `CapabilityReport` false until submit/readback is proven by the
   bootstrap probe, matching the existing Vulkan HEVC safety pattern.

Relevant ash binding names already available:

- `vk::VideoDecodeAV1ProfileInfoKHR`
- `vk::VideoDecodeAV1CapabilitiesKHR`
- `vk::VideoDecodeAV1SessionParametersCreateInfoKHR`
- `vk::VideoDecodeAV1PictureInfoKHR`
- `vk::VideoDecodeAV1DpbSlotInfoKHR`
- `vk::VideoCodecOperationFlagsKHR::DECODE_AV1`

### Phase 2: OBU Parser And Sequence Header Model

Files:

- new `vulkan_av1_decode.rs`
- share parser ideas with `video-hw-fmp4` writer helpers only if the shared API
  stays small and codec-local.

Tasks:

1. Parse low-overhead AV1 OBUs with LEB128 size fields.
2. Extract sequence header OBU and populate `StdVideoAV1SequenceHeader`.
   Low-overhead OBU extraction and reduced-still coded extent parsing are done;
   reduced-still `StdVideoAV1SequenceHeader` skeleton population is done; full
   color/timing and non-reduced header coverage remains.
3. Split temporal units / frame OBUs into access units suitable for Vulkan
   submit. Ordered multi-frame submit/decode-info skeleton extraction is now
   covered for temporal-delimiter-separated frame OBUs, and multiple tile-group
   OBUs following one frame header are grouped into one submit skeleton.
4. Reject unsupported input forms explicitly:
   - IVF container bytes passed as elementary stream. Done.
   - OBUs without size fields if the submit path cannot packetize them safely;
   - missing sequence header before first frame. Done for decode-info skeleton
     extraction, including late sequence-header rejection.
5. Unit tests:
   - sequence header extraction;
   - temporal delimiter + sequence header + frame split;
   - truncated LEB128 / truncated payload rejection;
   - multiple frames keep presentation order.

### Phase 3: Session Bootstrap

Tasks:

1. Create video session using `VideoDecodeAV1SessionParametersCreateInfoKHR`
   chained to `VideoSessionParametersCreateInfoKHR`. Prerequisite probe now
   covers synthetic reduced-still session/session-parameters creation; decode
   blocker messages now also probe real-bitstream session-parameters creation.
2. Bind video session memory using the same non-zero memory-requirement contract
   used by HEVC. Do not enable the Intel zero-memory workaround experiments for
   AV1 without a valid Vulkan contract.
3. Allocate output and DPB images from the formats accepted by the AV1 profile.
4. Build a skeleton submit probe that records begin/control/end without frame
   decode, then a submit execution probe for a single key frame. Bitstream
   submit skeleton extraction now covers frame-header offset and tile
   offset/size discovery, and key-frame `StdVideoDecodeAV1PictureInfo` skeleton
   plus `vk::VideoDecodeAV1PictureInfoKHR` construction is covered; Vulkan
   decode-info source range construction now covers `srcBufferOffset`,
   `srcBufferRange`, coded extent propagation, and relative AV1 frame/tile
   offsets within the range; ash `VideoDecodeInfoKHR` construction with the AV1
   picture-info `pNext` chain and HEVC-style destination picture resource
   construction are now covered. Key-frame setup-reference scaffolding now
   builds AV1 std reference info, AV1 DPB slot info, and the Vulkan reference
   slot chain; key-frame command skeleton extraction also assigns frames to
   deterministic rotating setup slots, and decode-info construction can attach
   that setup slot. Begin-coding DPB slot bindings and picture resources are
   now modeled, including AV1 DPB slot info and Vulkan reference-slot chains.
   `VideoBeginCodingInfoKHR` and the mandatory first RESET control info are now
   modeled, along with default `VideoEndCodingInfoKHR`. Vulkan command
   recording order is now modeled as begin, RESET, decode frames, end; actual
   command buffer calls remain. Frame-level destination picture resources are
   now mapped from the planned setup DPB slots, and frame record bundles tie
   decode-info indices to destination base layers. Aligned bitstream upload
   planning now rewrites per-frame `srcBufferOffset` / `srcBufferRange` to
   caller-provided alignment boundaries while copying only the minimal decode
   payload bytes into a compact submit buffer; bitstream session diagnostics now
   surface the adapter's Vulkan Video bitstream offset/range alignments so the
   plan can use capability-derived values instead of a fixed constant.
5. Cache bootstrap results by bitstream hash, access-unit limit, and optional
   physical-device index as HEVC does.

Acceptance:

- invalid AV1 bitstream returns `UnsupportedConfig` with sequence-header status;
- valid FFmpeg-generated AV1 input reaches a non-crashing submit probe on NVIDIA.

### Phase 4: Decode Submit And Readback

Tasks:

1. Populate `StdVideoDecodeAV1PictureInfo` for key-frame-only streams first.
2. Chain `VideoDecodeAV1PictureInfoKHR` onto `VideoDecodeInfoKHR`. The pure
   decode-info skeleton now validates the source bitstream range and relative
   AV1 offsets, and can build the ash decode-info chain with caller-owned
   source buffers plus destination image views/base layers; real buffer/image
   allocation and command submission remain.
3. For reference frames, populate `StdVideoDecodeAV1ReferenceInfo` and chain
   `VideoDecodeAV1DpbSlotInfoKHR` on setup/reference slots. Key-frame setup
   reference construction is covered; inter-frame reference lists remain.
4. Reuse the HEVC readback path for NV12 output only after verifying the output
   image format and coded extent constraints match AV1 capabilities.
5. Convert NV12 to RGB24 through the existing facade conversion path.

Acceptance:

- `decode_to_yuv --backend vulkan --codec av1 --output-mode metadata` returns
  the expected frame count for generated key-frame-only input.
- `--output-mode nv12` and `--output-mode rgb24` return non-empty payloads.
- `scripts/check_av1_psnr.rs` gains a Vulkan backend option and passes decode
  PSNR against FFmpeg software decode on NVIDIA.

### Phase 5: Integrated Benchmark

Tasks:

1. Update `scripts/benchmark_ffmpeg_backends.rs` so Vulkan AV1 video-hw decode
   is measured when the bootstrap probe passes.
2. Keep FFmpeg `av1_vulkan` adapter comparison in the report.
3. Treat Intel Vulkan encode as unavailable when FFmpeg also cannot encode on
   that adapter; do not claim parity for unsupported hardware.

Acceptance:

- integrated AV1 report has a passing Vulkan NVIDIA decode row;
- failure rows remain explicit for unsupported adapters;
- docs/status is updated with exact report paths and fps/seconds.

## Encode Scope

Only start encode after decode is stable.

Tasks:

1. Probe `VK_KHR_video_encode_queue` and `VK_KHR_video_encode_av1`.
2. Add a separate `vulkan_av1_encode.rs`; do not overload HEVC encode structs.
3. Generate key-frame-only AV1 first, then consider reference-frame/GOP encode.
4. Produce low-overhead OBU output compatible with `EncodedLayout::Av1` and the
   fMP4 writer's `av01` path.
5. Compare against FFmpeg `av1_vulkan` on the same adapter and against software
   FFmpeg decode for PSNR.

Acceptance:

- `encode_synthetic --backend vulkan --codec av1` emits decodable OBU output on
  adapters exposing AV1 encode.
- `write_synthetic_fmp4 --backend vulkan --codec av1` writes an `av01` MP4 that
  `ffprobe` and FFmpeg decode accept.
- integrated benchmark records video-hw vs FFmpeg `av1_vulkan` encode parity.

Current binding blocker: `ash 0.38.0+1.3.281` exposes
`VK_KHR_video_decode_av1` bindings but not `VK_KHR_video_encode_av1`. Update the
Vulkan binding stack before starting this section.

## Non-Goals For First Merge

- AV1 film grain synthesis parity.
- Full reference-frame GOP encode.
- Supporting Intel Vulkan AV1 encode when the driver/FFmpeg path lacks encode
  queue support.
- Treating capability extension presence alone as support.

## Regression Gates Before Claiming Support

Run on a host with Vulkan AV1 hardware:

```powershell
cargo fmt --all --check
cargo test -p video-hw-backend-vulkan --features backend-vulkan av1
cargo check -p video-hw --features backend-vulkan --examples
cargo +nightly -Zscript scripts/check_av1_psnr.rs --backends vulkan --release true --require-hardware true
cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends vulkan --codec av1 --warmup 1 --repeat 3 --verify --allow-failures true
```

Do not update `CapabilityReport` to claim Vulkan AV1 support until these gates
pass and the status document records the exact report paths.
