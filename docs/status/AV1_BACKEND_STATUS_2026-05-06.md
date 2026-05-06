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
| Vulkan AV1 decode/encode | `vulkan_av1_decode.rs` probes AV1 decode prerequisites, parses low-overhead OBUs, extracts sequence-header coded extent, builds a reduced-still `StdVideoAV1SequenceHeader`, and now builds relative decode-info source ranges; `CapabilityReport` remains false until submit/readback exists | Decode scaffolding only |
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

Current implementation progress:

- `vulkan_av1_decode.rs` contains an AV1 decode prerequisite probe for
  `VK_KHR_video_queue`, `VK_KHR_video_decode_queue`, `VK_KHR_video_decode_av1`,
  and `VIDEO_DECODE_KHR` queues advertising `DECODE_AV1`;
- low-overhead AV1 OBU inspection now reports OBU count, temporal-unit count,
  sequence-header presence, and frame-payload presence in Vulkan AV1 decode
  blocker messages;
- IVF container bytes passed to the Vulkan AV1 elementary-stream path are now
  rejected explicitly before OBU parsing;
- decode-info skeleton extraction now rejects streams where the sequence header
  appears after the first frame payload, rather than accepting a late header;
- sequence-header parsing now extracts reduced-still-picture coded width/height,
  core sequence flags, and maps them into an ash `StdVideoAV1SequenceHeader`
  skeleton for the later session-parameter builder;
- `extract_av1_std_sequence_header` exposes that mapping from a low-overhead AV1
  bitstream so session parameter creation can consume a Vulkan std header
  without depending on parser internals;
- the AV1 decode prerequisite probe now builds the Vulkan AV1 decode profile,
  queries `VideoCapabilitiesKHR` / `VideoDecodeAV1CapabilitiesKHR`, and requires
  at least one decode output format before reporting prerequisites as ready;
- the same probe now creates and destroys an AV1 `VideoSessionKHR` plus
  `VideoDecodeAV1SessionParametersCreateInfoKHR` using a reduced-still synthetic
  sequence header within the advertised coded-extent range;
- Vulkan AV1 decode blocker messages now also attempt session-parameter creation
  with the real bitstream sequence header and report the coded extent and selected
  picture format when that probe reaches `ready`;
- AV1 decode submit skeleton extraction now identifies the first frame payload,
  frame-header offset, tile offsets, and tile sizes for frame OBUs and
  frame-header + tile-group OBU pairs;
- AV1 submit/decode-info skeleton extraction now also has ordered multi-frame
  variants, so later command recording can iterate all temporal units instead of
  only probing the first frame;
- frame-header + tile-group submit extraction now groups multiple tile-group
  OBUs from the same temporal unit into a single AV1 picture-info tile list
  instead of treating each tile group as a separate frame;
- AV1 decode picture-info skeleton now builds key-frame `StdVideoDecodeAV1PictureInfo`,
  reference-name slot defaults, and tile metadata from that submit skeleton;
- the picture-info skeleton can now materialize `vk::VideoDecodeAV1PictureInfoKHR`
  with stable pointers to std picture info and tile arrays for the upcoming
  `VideoDecodeInfoKHR` chain;
- AV1 decode info skeleton extraction now computes the minimum consecutive
  `srcBufferOffset` / `srcBufferRange` covering the frame header and tiles,
  rebases AV1 frame/tile offsets relative to that range as Vulkan requires, and
  carries coded width/height from the parsed sequence header;
- that decode-info skeleton can now build the ash `vk::VideoDecodeInfoKHR`
  wrapper with `vk::VideoDecodeAV1PictureInfoKHR` in the `pNext` chain when
  given caller-owned Vulkan buffer and destination image view/base layer,
  including a HEVC-style destination picture resource with the parsed coded
  extent;
- key-frame AV1 setup-reference scaffolding now builds
  `StdVideoDecodeAV1ReferenceInfo`, `VideoDecodeAV1DpbSlotInfoKHR`, and
  `VideoReferenceSlotInfoKHR` so the next step can attach a reconstructed
  picture slot to the real decode command;
- key-frame command skeleton extraction now assigns ordered frames to rotating
  setup DPB slots so command recording can use deterministic slot indices
  before full inter-frame reference management is implemented;
- decode-info construction now has a helper that attaches the setup reference
  slot together with the AV1 picture-info chain, matching the shape needed by a
  future `vkCmdDecodeVideoKHR` call;
- key-frame command skeletons now also include begin-coding DPB slot bindings
  and can materialize HEVC-style begin picture resources over array layers;
- begin-coding helper scaffolding now builds AV1 std reference infos, AV1 DPB
  slot infos, and Vulkan reference-slot chains for the bound DPB resources;
- begin-coding scaffolding can now materialize `vk::VideoBeginCodingInfoKHR`
  plus the mandatory initial RESET `vk::VideoCodingControlInfoKHR`;
- coding-scope scaffolding now also materializes the default
  `vk::VideoEndCodingInfoKHR` used to close a future AV1 decode command scope;
- pure recording-step scaffolding now fixes the expected command order as
  begin-coding, RESET control, one decode step per frame, and end-coding;
- frame-level picture resource scaffolding now maps each planned decode frame
  to the destination array layer selected by its setup DPB slot;
- Vulkan AV1 capability is still false because real-bitstream session
  `vkCmdDecodeVideoKHR` submit, readback, PSNR, and benchmark gates are not
  implemented;
- Vulkan AV1 encode is blocked by the current `ash 0.38.0+1.3.281` binding set,
  which exposes `VK_KHR_video_decode_av1` but not `VK_KHR_video_encode_av1`.

Latest Vulkan AV1 scaffold verification:

- `cargo fmt --all --check`
- `cargo test -p video-hw-backend-vulkan --features backend-vulkan av1`
- `cargo check -p video-hw-backend-vulkan --features backend-vulkan`
- `cargo clippy -p video-hw-backend-vulkan --features backend-vulkan --all-targets`

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

1. Build the real Vulkan AV1 `VideoDecodeInfoKHR` chain from the decode-info
   skeleton, allocate resources, and record the first `vkCmdDecodeVideoKHR`
   key-frame submit.
2. Add Vulkan AV1 decode readback and PSNR check against FFmpeg software
   decode.
3. Update Vulkan bindings before adding AV1 encode, then enable encode only for
   adapters exposing encode queue and
   `VK_KHR_video_encode_av1`; report Intel encode as unavailable when FFmpeg also
   cannot encode.
4. On macOS AV1 hardware, prototype VideoToolbox AV1 encode/decode and update
   the VT unsupported contract only after fMP4, FFmpeg benchmark, and PSNR pass.
