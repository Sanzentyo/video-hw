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
| Vulkan AV1 decode/encode | `vulkan_av1_decode.rs` probes AV1 decode prerequisites, parses low-overhead OBUs, extracts sequence-header coded extent, builds a reduced-still `StdVideoAV1SequenceHeader`, builds relative decode-info source ranges, and now plans aligned bitstream upload ranges for `vkCmdDecodeVideoKHR`; `CapabilityReport` remains false until submit/readback exists | Decode scaffolding only |
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
- Intel adapter: FFmpeg sees `VK_KHR_video_decode_av1`, reports AV1 decode
  capabilities, and initializes the Vulkan decoder, but a short AV1 decode smoke
  terminates with Windows access-violation exit `-1073741819`; encode is still
  unavailable because the device does not expose the encode queue / AV1 encode
  path. This is not a passing video-hw AV1 target.

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
- AV1 session bootstrap probing now also queries
  `vkGetVideoSessionMemoryRequirementsKHR` and reports requirement count, total
  size, and maximum alignment, then allocates, binds, and frees the required
  session memory before creating session parameters;
- real-bitstream AV1 probing now builds a capability-aligned source upload plan,
  creates a `VIDEO_DECODE_SRC_KHR` buffer, binds HOST_VISIBLE|HOST_COHERENT
  memory, uploads the compact bitstream bytes, and destroys the buffer/memory;
- AV1 command scaffolding now derives a decode output/DPB image plan from the
  command skeleton, including selected format, coded extent, DPB array layers,
  and `VIDEO_DECODE_DST_KHR|VIDEO_DECODE_DPB_KHR|TRANSFER_SRC` usage;
- real-bitstream AV1 probing now creates that decode output/DPB image with an
  AV1 `VideoProfileListInfoKHR`, binds image memory, creates a 2D-array image
  view, and destroys the probe resources;
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
- frame record bundles now tie each decode-info index to its frame metadata,
  source range, setup slot, and destination base array layer before the future
  `vkCmdDecodeVideoKHR` loop consumes them;
- aligned AV1 bitstream upload planning now copies each decode unit into a
  compact buffer with caller-provided `srcBufferOffset` and `srcBufferRange`
  alignment, preserving picture-info offsets relative to the aligned
  `srcBufferOffset`; bitstream session probing now reports the adapter's
  `minBitstreamBufferOffsetAlignment` and `minBitstreamBufferSizeAlignment`, and
  blocker diagnostics use those capability-derived alignments when available
  with a 4096/4096 fallback; upload planning now also validates that every
  command-frame source range maps back into the aligned upload buffer before a
  future `vkCmdDecodeVideoKHR` loop consumes it, and produces per-frame submit
  bundles tying decode-info index, DPB setup slot, destination array layer, and
  upload byte range together; blocker diagnostics now also materialize the first
  frame's `VideoDecodeInfoKHR`/AV1 picture-info/setup-slot chain from the
  aligned plan and can walk all planned frames in command order with
  callback-scoped decode-info chains; a command-sequence visitor now materializes
  begin-coding, RESET, per-frame decode-info, and end-coding structs in the same
  order the future command buffer recorder will consume them; a result-returning
  record callback wrapper now reports begin/reset/decode/end counts so unsafe
  ash calls can be inserted without changing the sequencing API, and validates
  those counts against the planned frame count; real-bitstream probing now keeps
  the uploaded source buffer, decode image/view, video session, session
  parameters, and bound session memory alive together long enough to materialize
  the command sequence with non-null resource handles; command-buffer setup now
  has pure builders for the HOST_WRITE -> VIDEO_DECODE_READ source-buffer memory
  barrier and UNDEFINED -> VIDEO_DECODE_DST_KHR decode-image initialization
  barrier; an opt-in `VIDEO_HW_VULKAN_AV1_RECORD_COMMAND_BUFFER=1` probe can
  record a real command buffer with `vkCmdBeginVideoCodingKHR`,
  `vkCmdControlVideoCodingKHR(RESET)`, `vkCmdDecodeVideoKHR`, and
  `vkCmdEndVideoCodingKHR` using those live resources, while the default probe
  still avoids issuing the driver command path until submit/readback has a
  dedicated gate; `scripts/check_vulkan_av1_record_probe.rs` runs the ignored
  live probe and writes a small command-record report under
  `output/vulkan-av1-record-probe/`; the script supports
  `--record-mode barrier_only|begin_end|reset_end|first_decode|full` for
  command-buffer crash localization and `--submit-command-buffer` for an
  opt-in queue-submit/fence-wait probe;
- FFmpeg's Vulkan AV1 decode path passes concrete AV1 std substructures
  (`pTileInfo`, `pQuantization`, `pSegmentation`, `pLoopFilter`, `pCDEF`,
  `pLoopRestoration`, `pGlobalMotion`) with each picture. The AV1 record path
  now materializes key-frame/single-tile default instances for those pointers
  instead of leaving them NULL; on the current Windows/Intel-visible Vulkan AV1
  adapter, `--no-record-command-buffer`, `barrier_only`, `begin_end`,
  `reset_end`, `first_decode`, and `full` command-buffer record probes all pass
  with `coded=320x180`, `format=G8_B8R8_2PLANE_420_UNORM`, `upload_bytes=256`,
  `image_layers=16`, `barrier_layers=16`, and `record_decodes=1` for decode
  modes; the `full --submit-command-buffer` probe also passes with
  `command_buffer_submitted=true`, reaching queue submit and fence wait; the
  `--readback` probe now records a `VIDEO_DECODE_DST_KHR ->
  TRANSFER_SRC_OPTIMAL` image transition, copies the decoded NV12 planes into a
  host-visible buffer, orders `TRANSFER_WRITE -> HOST_READ`, waits on the submit
  fence, maps the readback allocation, and passes on the current
  Windows/Intel-visible adapter with `readback_bytes=86400`,
  `readback_mapped_bytes=86400`, `readback_non_zero=true`, and
  `readback_sample_len=86400`; the live probe can now read an external AV1
  low-overhead OBU elementary stream through
  `VIDEO_HW_VULKAN_AV1_PROBE_BITSTREAM_PATH`, and
  `scripts/check_vulkan_av1_record_probe.rs --generate-ffmpeg-obu --readback`
  generates a one-frame `libaom-av1` OBU with FFmpeg and passes the same
  submit/readback gate with `upload_bytes=1536`; the explicit Vulkan backend
  decode path now reuses that submit/readback path for one-frame key-frame OBU
  inputs, so `decode_to_yuv --backend vulkan --codec av1` returns one frame for
  `metadata`, writes 86,400 bytes for `nv12`, and writes 172,800 bytes for
  `rgb24` at 320x180; the Frame OBU path now parses the generated key-frame
  header far enough to split frame-header bytes from tile payload bytes and to
  feed observed base quantizer, loop-filter, CDEF, tx-mode, and sequence-header
  feature flags into the std AV1 structs; NV12 AV1 readback planning mirrors the HEVC plane-copy layout
  for `G8_B8R8_2PLANE_420_UNORM`, including odd-dimension chroma rounding and
  4-byte plane offset alignment, and bitstream session diagnostics now report
  planned and mapped readback byte counts;
- Vulkan AV1 capability is still false because FFmpeg-reference PSNR is not
  passing yet: `scripts/check_vulkan_av1_psnr.rs --skip-build --min-psnr-y 0`
  records the current one-frame generated-OBU result as `psnr_y_min=5.6200`,
  unchanged after the first key-frame header parser pass, which confirms the
  output path is live but the AV1 picture/session modeling is not yet bit-exact
  enough to claim decode support;
- Vulkan AV1 encode is blocked by the current `ash 0.38.0+1.3.281` binding set,
  which exposes `VK_KHR_video_decode_av1` but not `VK_KHR_video_encode_av1`.

Latest Vulkan AV1 scaffold verification:

- `cargo fmt --all --check`
- `cargo test -p video-hw-backend-vulkan --features backend-vulkan av1`
- `cargo check -p video-hw-backend-vulkan --features backend-vulkan`
- `cargo clippy -p video-hw-backend-vulkan --features backend-vulkan --all-targets`
- `cargo +nightly -Zscript scripts/check_vulkan_av1_record_probe.rs --skip-build --readback`
- `cargo +nightly -Zscript scripts/check_vulkan_av1_record_probe.rs --skip-build --readback --generate-ffmpeg-obu --width 320 --height 180 --frames 1`
- `cargo run -p video-hw --features backend-vulkan --example decode_to_yuv -- --backend vulkan --codec av1 --input output/vulkan-av1-record-probe/ffmpeg-av1-probe-1778061973933.obu --output-mode metadata`
- `cargo run -p video-hw --features backend-vulkan --example decode_to_yuv -- --backend vulkan --codec av1 --input output/vulkan-av1-record-probe/ffmpeg-av1-probe-1778061973933.obu --output-mode nv12 --output output/vulkan-av1-record-probe/av1-vulkan-decode.nv12`
- `cargo run -p video-hw --features backend-vulkan --example decode_to_yuv -- --backend vulkan --codec av1 --input output/vulkan-av1-record-probe/ffmpeg-av1-probe-1778061973933.obu --output-mode rgb24 --output output/vulkan-av1-record-probe/av1-vulkan-decode.rgb`
- `cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --skip-build --min-psnr-y 0`

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

1. Continue comparing FFmpeg's populated `StdVideoDecodeAV1PictureInfo`/
   `StdVideoAV1SequenceHeader` fields against video-hw's generated structs; the
   first key-frame parser pass did not improve PSNR.
2. Keep the one-frame `decode_to_yuv` path passing while adding multi-frame
   key-frame-only coverage.
3. Update Vulkan bindings before adding AV1 encode, then enable encode only for
   adapters exposing encode queue and
   `VK_KHR_video_encode_av1`; report Intel encode as unavailable when FFmpeg also
   cannot encode.
4. On macOS AV1 hardware, prototype VideoToolbox AV1 encode/decode and update
   the VT unsupported contract only after fMP4, FFmpeg benchmark, and PSNR pass.
