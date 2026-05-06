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
| AV1 fMP4 writer | `av01` sample entry / `av1C` box; writer helper tests cover OBU passthrough and `config_obus`; `scripts/check_av1_fmp4_roundtrip.rs` verifies NVIDIA/Intel hardware AV1 fMP4 generation | Done |
| AV1 fMP4 reader | reader tests cover AV1 codec/layout detection, `av1C` parameter access, and OBU passthrough; roundtrip smoke verifies reader sample count plus `decode_to_yuv --input-format mp4` metadata and RGB24 decode PSNR vs FFmpeg | Done |
| FFmpeg comparison | `scripts/benchmark_ffmpeg_backends.rs`, NV/Intel precise scripts, reports in `output/*av1*.md` | Done for NVIDIA/Intel |
| PSNR/MSE verification | `scripts/check_av1_psnr.rs`; latest report `output/av1-psnr/av1-psnr-1778051481.md` | Done for NVIDIA/Intel |
| Vulkan AV1 decode/encode | `vulkan_av1_decode.rs` implements generated AV1 OBU/fMP4 decode through submit/readback on NVIDIA for keyframe-only, short GOP, and generated long-GOP replay cases; PSNR gates pass against FFmpeg software reference for those scopes. AV1 encode is blocked by current ash bindings lacking `VK_KHR_video_encode_av1` | Decode generated-GOP partial, encode blocked |
| VideoToolbox AV1 decode/encode | `vt_backend.rs` now has an AV1 fMP4 decode bootstrap path using `av1C`/track dimensions in `VtDecoderOptions`; encode still returns explicit `UnsupportedConfig`; macOS target check covers the backend crate | Decode scaffolded, encode not implemented |

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
- AV1 fMP4 roundtrip smoke:
  - `output/av1-fmp4-roundtrip/av1-fmp4-roundtrip-1778069094.md`
  - NVIDIA: 30 reader samples, 30 `decode_to_yuv --input-format mp4` metadata frames, RGB decode PSNR min 45.98 dB, `codec=av1`, `tag=av01`, duration `1.000000`
  - Intel: 30 reader samples, 30 metadata frames, RGB decode PSNR min 46.07 dB, `codec=av1`, `tag=av01`, duration `1.000000`
- AV1 fMP4 decode access-order benchmark:
  - NVIDIA report `output/benchmark-fmp4-decode-access-1778069696.md`: 90
    frames, contiguous range 1.049s, sequential no-cache 7.430s, reverse
    no-cache 7.149s, cached reverse-before 0.846s, cached reverse-after 7.458s,
    min PSNR 45.950 dB.
  - Intel report `output/benchmark-fmp4-decode-access-1778070138.md`: 24
    frames, contiguous range 1.276s, sequential no-cache 29.358s, reverse
    no-cache 31.215s, cached reverse-before 6.879s, cached reverse-after
    31.230s, min PSNR 46.064 dB. The 90-frame Intel run exceeded the local
    240-second timeout.
  - Wrapper generation smoke `output/benchmark-fmp4-decode-access-1778070314.md`:
    `scripts/benchmark_fmp4_decode_access.rs --generate-codec av1
    --generate-backend nvidia` generated an 8-frame 320x180 AV1 fMP4 and ran all
    access cases with min PSNR 46.045 dB.
- Vulkan AV1 integrated benchmark:
  - `output/benchmark-backends-av1-1778068460.md`
  - detail: `output/benchmark-vulkan-av1-1778068460.md`
  - NVIDIA: video-hw Vulkan AV1 decode 0.145s / 55.105 fps vs FFmpeg Vulkan decode 0.316s / 25.293 fps for 8 generated keyframe-only OBU frames at 320x180 (`--release true`, warmup 1, repeat 3)
  - NVIDIA FFmpeg `av1_vulkan` encode 0.316s / 25.348 fps; video-hw Vulkan AV1 encode is `unavailable` because current ash bindings do not expose `VK_KHR_video_encode_av1`
  - Intel Vulkan: FFmpeg AV1 decode exits with Windows access violation `0xc0000005`; FFmpeg `av1_vulkan` encode reports `Function not implemented`
- Vulkan AV1 fMP4 integrated benchmark:
  - `output/benchmark-backends-av1-1778068469.md`
  - detail: `output/benchmark-vulkan-av1-1778068469.md`
  - generated fragmented MP4 `av01` decode input is recorded as `decode_input_format: fmp4`
  - NVIDIA: video-hw Vulkan AV1 fMP4 decode 0.144s / 55.412 fps vs FFmpeg Vulkan fMP4 decode 0.311s / 25.712 fps for 8 generated keyframe-only frames at 320x180 (`--release true`, warmup 1, repeat 3)
- AV1 frame-type inspection:
  - `output/av1-frame-types/av1-frame-types-1778071015345679600.md`: generated OBU,
    `--gop-size 1`, 8 frame headers, all `frame_type=0`.
  - `output/av1-frame-types/av1-frame-types-1778071020279919100.md`: generated OBU,
    `--gop-size 30`, 8 frame headers, frame 0 is `frame_type=0` and frames 1-7
    are `frame_type=1`.
  - `output/av1-frame-types/av1-frame-types-1778071024653131000.md`: generated fMP4
    extracted to OBU, `--gop-size 30`, same `frame_type=1` inter-frame pattern
    after the keyframe.

## Vulkan AV1 Status

Vulkan AV1 is still not a full backend-support claim. The implemented decode
scope is generated keyframe-only AV1 OBU and fMP4 (`av01`) input on the NVIDIA
Vulkan Video path. The integrated benchmark now measures that path and records
unsupported or failing adapter rows explicitly:

- `output/benchmark-vulkan-av1-1778068460.md`
- `output/benchmark-backends-av1-1778068460.md`

Observed FFmpeg behavior on this Windows host:

- NVIDIA adapter: FFmpeg `av1_vulkan` encode/decode can run. The integrated
  benchmark uses null muxer output for FFmpeg encode so AV1 OBU muxer timestamp
  constraints do not mask encoder throughput.
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
- generated-keyframe submit extraction now also rejects normal non-reduced AV1
  frame headers that the current parser cannot safely split, such as
  `show_existing_frame`, instead of treating those bytes as tile payload;
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
- real AV1 frame-header parsing now uses `oxideav-av1` when it can parse the
  frame OBU/header. Inter-frame GOP inputs now map the parsed header into the
  picture-info skeleton and then stop at the explicit reference-slot replay
  gate; diagnostics include `order_hint`, `primary_ref_frame`,
  `refresh_frame_flags`, `ref_frame_idx`, and the tile payload offset for the
  first parsed inter frame;
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
  instead of leaving them NULL; on the current Windows Vulkan AV1
  adapter, `--no-record-command-buffer`, `barrier_only`, `begin_end`,
  `reset_end`, `first_decode`, and `full` command-buffer record probes all pass
  with `coded=320x180`, `format=G8_B8R8_2PLANE_420_UNORM`, `upload_bytes=256`,
  `image_layers=16`, `barrier_layers=16`, and `record_decodes=1` for decode
  modes; the `full --submit-command-buffer` probe also passes with
  `command_buffer_submitted=true`, reaching queue submit and fence wait; the
  `--readback` probe now records a `VIDEO_DECODE_DST_KHR ->
  TRANSFER_SRC_OPTIMAL` image transition, copies the decoded NV12 planes into a
  host-visible buffer, orders `TRANSFER_WRITE -> HOST_READ`, waits on the submit
  fence, maps the readback allocation, and initializes the readback buffer with
  a sentinel before submit so unwritten buffers are detectable. Readback now
  requires the selected queue family to support both `VIDEO_DECODE` and
  `TRANSFER`; this uses NVIDIA on the current Windows host, while Intel's
  decode-only queue still needs a separate transfer queue/ownership-transfer
  implementation. The live probe reports `readback_bytes=86400`,
  `readback_mapped_bytes=86400`, and `readback_sample_len=86400`; the live probe
  can now read an external AV1
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
  feature flags into the std AV1 structs; the fallback key-frame parser now
  skips AV1 loop-filter ref/mode delta update payloads instead of rejecting the
  stream at `loop_filter_delta_update`, with a unit test covering the bit
  consumption; the same fallback parser now also reads AV1 quantization signed
  deltas and qmatrix fields into `StdVideoAV1Quantization` instead of rejecting
  `using_qmatrix`; NV12 AV1 readback planning mirrors the HEVC plane-copy layout
  for `G8_B8R8_2PLANE_420_UNORM`, including odd-dimension chroma rounding and
  4-byte plane offset alignment, and bitstream session diagnostics now report
  planned and mapped readback byte counts;
- Vulkan AV1 capability is still conservative because the implemented scope is
  generated keyframe-only, short GOP, and generated long-GOP decode replay and
  does not yet include arbitrary AV1 stream coverage or encode. Earlier FFmpeg-reference PSNR work recorded a one-frame
  generated-OBU result as `psnr_y_min=12.9600` after fixing the explicit submit
  path and proving the readback copy overwrites a sentinel buffer. Follow-up
  parity work now also gives AV1 session parameters an 8-bit
  4:2:0 color config and matches FFmpeg's inactive begin-coding reference-slot
  shape for the current reconstruction. Follow-up FFmpeg parity work now also
  passes a zeroed `StdVideoAV1TimingInfo`, uses FFmpeg's `[1, 1, 1]`
  `LoopRestorationSize` default when restoration is disabled, and submits the
  initial decoder RESET in a separate command buffer before the frame decode
  submit path. The decode image transition now also uses
  `VIDEO_DECODE_DPB_KHR`, matching FFmpeg's non-layered AV1 path, and the decode
  image view uses `TYPE_2D` when only one array layer is allocated, with an
  explicit `VkImageViewUsageCreateInfo` for
  `VIDEO_DECODE_DST_KHR|VIDEO_DECODE_DPB_KHR`; the reference/DPB binding now
  uses a separate image view with `VIDEO_DECODE_DPB_KHR` usage on the same image,
  matching FFmpeg's separate output/reference view shape. Video session creation
  now uses the capability maximum coded extent, matching FFmpeg's session scope
  instead of constraining the session to the input frame size. The sequence
  header parser now follows FFmpeg `trace_headers` ordering for
  `seq_choose_screen_content_tools`/`seq_choose_integer_mv` before
  `order_hint_bits_minus_1`, and AV1 global-motion defaults now use FFmpeg's
  identity matrix values (`gm_params[2]` and `[5]` set to `1 << 16`) rather than
  all-zero params. With these changes the generated-OBU Vulkan AV1 path now
  matches the FFmpeg software-reference Y plane exactly for generated
  key-frame-only cases. The `decode_to_yuv --input-format mp4` path now treats
  AV1 `LengthPrefixedSample` input as OBU payload instead of NAL-length-prefixed
  data, prepends `av1C.config_obus` for keyframes, and the fMP4 PSNR gate also
  passes at `psnr_y_min=inf` when the generated MP4 uses `delay_moov` so the
  `av1C` box contains the sequence header OBU. Multi-frame keyframe-only
  readback now copies each decode image layer into a distinct NV12 sample; OBU
  and fMP4 8-frame gates both pass with `psnr_y_min=inf`.
  An opt-in
  `VIDEO_HW_VULKAN_AV1_QUERY_STATUS=1` diagnostic now wraps the decode command
  in a `VK_QUERY_TYPE_RESULT_STATUS_ONLY_KHR` query; on NVIDIA the current
  generated-OBU readback probe reports `decode_query_status_raw=Some(1)`;
- Vulkan AV1 encode is blocked by the current `ash 0.38.0+1.3.281` binding set,
  which exposes `VK_KHR_video_decode_av1` but not `VK_KHR_video_encode_av1`.
  `cargo info ash` currently reports the same latest version
  (`0.38.0+1.3.281`), and local registry inspection finds
  `VK_KHR_video_decode_av1` / `VideoDecodeAV1*` bindings but no
  `VK_KHR_video_encode_av1`, `video_encode_av1`, `VideoEncodeAV1*`, or
  `ENCODE_AV1` bindings. The reproducible binding check is
  `scripts/check_vulkan_av1_encode_bindings.rs`; latest report
  `output/vulkan-av1-encode-bindings/vulkan-av1-encode-bindings-1778081150.md`
  records decode symbols present and encode symbols absent. Vulkan AV1 encode
  therefore needs a binding stack update or local generated bindings before
  implementation can start safely.

Latest Vulkan AV1 scaffold verification:

- `cargo fmt --all --check`
- `cargo test -p video-hw-backend-vulkan --features backend-vulkan loop_filter_delta_updates_skip_ref_and_mode_deltas`
- `cargo test -p video-hw-backend-vulkan --features backend-vulkan quantization_parser_reads_signed_deltas_and_qmatrix`
- `cargo test -p video-hw-backend-vulkan --features backend-vulkan av1`
  includes `decode_submit_skeleton_rejects_show_existing_frame_obu`, which
  guards the unsupported-frame-header rejection path added in `d0ee5e9`.
- `cargo check -p video-hw-backend-vulkan --features backend-vulkan`
- `cargo clippy -p video-hw-backend-vulkan --features backend-vulkan --all-targets`
- `cargo test -p video-hw --features "backend-nvidia backend-intel backend-vulkan" av1`
  verifies that AV1 length-prefixed fMP4 samples are forwarded as OBU payload.
- `cargo +nightly -Zscript scripts/check_vulkan_av1_record_probe.rs --skip-build --readback`
- `cargo +nightly -Zscript scripts/check_vulkan_av1_record_probe.rs --skip-build --readback --generate-ffmpeg-obu --width 320 --height 180 --frames 1`
- `VIDEO_HW_VULKAN_AV1_QUERY_STATUS=1 cargo +nightly -Zscript scripts/check_vulkan_av1_record_probe.rs --readback --generate-ffmpeg-obu --width 320 --height 180 --frames 1`
- latest generated-OBU query/readback probe report:
  `output/vulkan-av1-record-probe/vulkan-av1-record-probe-1778066474411.md`
- `cargo run -p video-hw --features backend-vulkan --example decode_to_yuv -- --backend vulkan --codec av1 --input output/vulkan-av1-record-probe/ffmpeg-av1-probe-1778061973933.obu --output-mode metadata`
- `cargo run -p video-hw --features backend-vulkan --example decode_to_yuv -- --backend vulkan --codec av1 --input output/vulkan-av1-record-probe/ffmpeg-av1-probe-1778061973933.obu --output-mode nv12 --output output/vulkan-av1-record-probe/av1-vulkan-decode.nv12`
- `cargo run -p video-hw --features backend-vulkan --example decode_to_yuv -- --backend vulkan --codec av1 --input output/vulkan-av1-record-probe/ffmpeg-av1-probe-1778061973933.obu --output-mode rgb24 --output output/vulkan-av1-record-probe/av1-vulkan-decode.rgb`
- `cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --skip-build --min-psnr-y 0`
- `cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --min-psnr-y 0`
- `cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --min-psnr-y 60`
  (`output/vulkan-av1-psnr/vulkan-av1-psnr-1778067206093.md`,
  `psnr_y_min=inf`)
- `cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --frames 1 --skip-build --min-psnr-y 60`
  (`output/vulkan-av1-psnr/vulkan-av1-psnr-1778071142061.md`,
  `psnr_y_min=inf`, report includes `frame_type_gate`)
- `cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --frames 2 --gop-size 30 --skip-build --min-psnr-y 60`
  intentionally fails on inter-frame input
  (`output/vulkan-av1-psnr/vulkan-av1-psnr-1778071155693.md`,
  `failed_stage=decode Vulkan AV1 to NV12`, report includes `frame_type_gate`)
- `cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --frames 2 --gop-size 30 --min-psnr-y 60`
  intentionally fails after rebuilding `decode_to_yuv`, and the Vulkan backend
  blocker message now includes `frame_headers=2`, `key_frames=1`, and
  `inter_frames=1`.
- `cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --width 128 --height 72 --frames 2 --gop-size 2 --min-psnr-y 60`
  intentionally fails after mapping the first inter-frame header into
  decode-info and then stopping at reference-slot replay
  (`output/vulkan-av1-psnr/vulkan-av1-psnr-1778072585372.md`). The failure now
  reports `decode_info_count=2`, `frame_type=1`, `order_hint=1`, `primary_ref_frame=6`,
  `refresh_frame_flags=0x02`, `ref_frame_idx=[0, 0, 0, 0, 0, 0, 0]`, and
  `tile_payload_offset=15`, confirming that the next missing layer is
  inter-frame reference-slot mapping plus DPB replay.
- Follow-up reference replay work now removes that explicit inter-frame command
  skeleton gate, carries per-frame reference slot lists into
  `VideoDecodeInfoKHR::referenceSlots`, keeps begin-coding active references
  empty after RESET, and adds a first-pass AV1 reference-frame-state replay that
  maps `ref_frame_idx` through `refresh_frame_flags` before filling Vulkan
  `referenceNameSlotIndices`. The same pass also preserves Frame OBU header
  bytes in the submitted source range instead of starting the range at tile
  payload. Unit and clippy gates pass, but live PSNR still fails for normal
  non-reduced key-frame streams: `output/vulkan-av1-psnr/vulkan-av1-psnr-1778073517030.md`
  (`--frames 1 --gop-size 2`, `psnr_y_min=12.5300`) and
  `output/vulkan-av1-psnr/vulkan-av1-psnr-1778073517033.md`
  (`--frames 2 --gop-size 2`, `psnr_y_min=12.5300`). This shows the remaining
  blocker is full AV1 frame-header/std-picture parity before GOP replay can be
  accepted by PSNR. The next pass maps oxideav frame-header fields into the
  Vulkan std picture structures more completely (`disable_cdf_update`,
  screen-content/integer-MV flags, frame-size override, intrabc,
  `disable_frame_end_update_cdf`, reduced-tx-set, and delta-q params), submits
  the source range from the Frame/Header OBU start as required by Vulkan, and
  fixes planned DPB slot rotation to use allocated slots. Short-GOP live PSNR
  now passes exactly:
  `output/vulkan-av1-psnr/vulkan-av1-psnr-1778074489393.md`
  (`--frames 1 --gop-size 2`, `psnr_y_min=inf`),
  `output/vulkan-av1-psnr/vulkan-av1-psnr-1778074616975.md`
  (`--frames 2 --gop-size 2`, `psnr_y_min=inf`), and
  `output/vulkan-av1-psnr/vulkan-av1-psnr-1778074633347.md`
  (`--frames 8 --gop-size 2`, `psnr_y_min=inf`). The long-GOP failure was
  traced to AV1 reference-name state: `OrderHints` must be populated in
  INTRA/LAST..ALTREF reference-name order, not raw DPB-slot order, and
  `StdVideoDecodeAV1ReferenceInfo.RefFrameSignBias` must be derived from
  relative reference/current `OrderHint` values as FFmpeg does. After making
  replay DPB-aware through `oxideav-av1`, carrying per-reference
  `SavedOrderHints`, fixing reference-name `OrderHints`, and setting
  `RefFrameSignBias`, generated long-GOP OBU replay now passes exactly:
  `output/vulkan-av1-psnr/vulkan-av1-psnr-1778075814861.md`
  (`--frames 8 --gop-size 30`, `psnr_y_min=inf`, threshold 60 dB).
- Generated AV1 fMP4 long-GOP replay also passes:
  `output/vulkan-av1-psnr/vulkan-av1-psnr-1778075801683.md`
  (`--input-format fmp4 --frames 8 --gop-size 30`, `psnr_y_min=inf`,
  threshold 60 dB).
- `cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --input-format fmp4 --skip-build --min-psnr-y 60`
  (`output/vulkan-av1-psnr/vulkan-av1-psnr-1778067206134.md`,
  `psnr_y_min=inf`)
- `cargo +nightly -Zscript scripts/check_vulkan_av1_encode_bindings.rs --fail-on-missing`
  (`output/vulkan-av1-encode-bindings/vulkan-av1-encode-bindings-1778081150.md`,
  `encode_bindings_present=false`, expected nonzero exit)
- `cargo +nightly -Zscript scripts/inspect_av1_frame_types.rs --frames 8 --gop-size 1 --expect-inter-frame false`
  (`output/av1-frame-types/av1-frame-types-1778071015345679600.md`,
  `has_inter_frame=false`)
- `cargo +nightly -Zscript scripts/inspect_av1_frame_types.rs --frames 8 --gop-size 30 --expect-inter-frame true`
  (`output/av1-frame-types/av1-frame-types-1778071020279919100.md`,
  `has_inter_frame=true`)
- `cargo +nightly -Zscript scripts/inspect_av1_frame_types.rs --input-format fmp4 --frames 8 --gop-size 30 --expect-inter-frame true`
  (`output/av1-frame-types/av1-frame-types-1778071024653131000.md`,
  `has_inter_frame=true`)
- `cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends vulkan --codec av1 --warmup 1 --repeat 3 --frame-count 8 --width 320 --height 180 --release true --allow-failures true`
  (`output/benchmark-backends-av1-1778068460.md`,
  `output/benchmark-vulkan-av1-1778068460.md`)
- `cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends vulkan --codec av1 --vulkan-decode-input-format fmp4 --warmup 1 --repeat 3 --frame-count 8 --width 320 --height 180 --release true --allow-failures true`
  (`output/benchmark-backends-av1-1778068469.md`,
  `output/benchmark-vulkan-av1-1778068469.md`)
- `scripts/benchmark_ffmpeg_backends.rs` now accepts
  `--vulkan-av1-gop-size` and defaults generated Vulkan AV1 decode inputs to
  `-g 30 -lag-in-frames 0`, so integrated Vulkan AV1 decode comparisons no
  longer silently measure keyframe-only input unless `--vulkan-av1-gop-size 1`
  is requested.
- Short smoke reports for the updated generated GOP30 integrated benchmark:
  `output/benchmark-backends-av1-1778076098.md` /
  `output/benchmark-vulkan-av1-1778076098.md` for OBU input and
  `output/benchmark-backends-av1-1778076116.md` /
  `output/benchmark-vulkan-av1-1778076116.md` for fMP4 input. The NVIDIA
  decode rows pass in both reports; unsupported/failed Vulkan encode and Intel
  Vulkan rows remain explicit. The runner now also marks an empty adapter
  selection as failed instead of writing a passing report with no cases
  (`output/benchmark-backends-av1-1778076131.md`).
- Vulkan AV1 integrated `--verify` now invokes
  `scripts/check_vulkan_av1_psnr.rs` for each `video-hw` Vulkan adapter using
  the same generated benchmark input and the same `decode_to_yuv` binary that
  the benchmark measured, then records a `video-hw PSNR verify` row.
  Fresh 320x180 generated-GOP30 reports:
  `output/benchmark-backends-av1-1778076588.md` /
  `output/benchmark-vulkan-av1-1778076588.md` for OBU input and
  `output/benchmark-backends-av1-1778076603.md` /
  `output/benchmark-vulkan-av1-1778076603.md` for fMP4 input. NVIDIA OBU decode
  is 54.968 fps vs FFmpeg Vulkan decode 25.879 fps; NVIDIA fMP4 decode is
  54.097 fps vs FFmpeg Vulkan decode 25.779 fps. Both reports include passing
  `video-hw PSNR verify` rows; generated PSNR reports are
  `output/vulkan-av1-psnr/vulkan-av1-psnr-1778076585814.md` and
  `output/vulkan-av1-psnr/vulkan-av1-psnr-1778076599799.md`
  (`--frames 8 --gop-size 30 --vulkan-adapter-index 0`, `psnr_y_min=inf`).
- Vulkan AV1 generation controls now expose libaom lookahead:
  `scripts/benchmark_ffmpeg_backends.rs --vulkan-av1-lag-in-frames <N>`,
  `scripts/check_vulkan_av1_psnr.rs --lag-in-frames <N>`, and
  `scripts/inspect_av1_frame_types.rs --lag-in-frames <N>`. The default stays
  `0`, preserving the passing generated-GOP30 scope. The
  `reference_name_slot_replay_rejects_conflicting_slot_aliases` unit test
  verifies that reference-name replay rejects cases that would alias one Vulkan
  DPB slot to multiple reference image layers in a single decode.
  Stress report `output/benchmark-backends-av1-1778078784.md` /
  `output/benchmark-vulkan-av1-1778078784.md` uses
  `--frames 16 --gop-size 30 --vulkan-av1-lag-in-frames 25 --verify`. NVIDIA
  video-hw metadata decode now separates command submit from NV12 readback and
  reports the display-frame count (`decode_to_yuv ... --output-mode metadata`
  prints `frames=16`) instead of the 17 decode-command count. NV12 readback now
  decouples logical DPB slots from output image layers and maps display frames,
  including show-existing frames, back to the correct readback layer. The
  integrated run passes video-hw decode at 0.452s with PSNR verify PASS at
  0.348s, while FFmpeg Vulkan decode passes at 0.377s; direct PSNR also passes
  with `psnr_y_min=inf`
  (`output/vulkan-av1-psnr/vulkan-av1-psnr-1778078756687.md`). The generated
  stream has 16 temporal units, 22 frame headers, 16 inter frames, and 5
  show-existing frames; the command skeleton reaches 17 decode commands with
  bounded slots `0/1/2/3/...`.
  The same lag25 stress through fMP4 input also preserves the display-frame
  count (`decode_to_yuv ... --input-format mp4 --output-mode metadata` prints
  `frames=16`) and passes video-hw decode at 0.455s with PSNR verify PASS at
  0.270s vs FFmpeg Vulkan fMP4 decode 0.394s:
  `output/benchmark-backends-av1-1778078783.md` /
  `output/benchmark-vulkan-av1-1778078783.md`. Direct fMP4 PSNR passes with
  `psnr_y_min=inf`
  (`output/vulkan-av1-psnr/vulkan-av1-psnr-1778078756691.md`).
- A broader 32-frame `--gop-size 30 --lag-in-frames 25` stress uncovered a
  reference-slot aliasing case after the first alt-ref group. Before the guard,
  OBU/fMP4 decode returned frames with `psnr_y_min=20.0100`
  (`output/vulkan-av1-psnr/vulkan-av1-psnr-1778078962164.md`,
  `output/vulkan-av1-psnr/vulkan-av1-psnr-1778078962159.md`). The backend now
  rejects this unsupported shape before returning pixels when one decode needs
  multiple AV1 reference frames that alias the same Vulkan DPB slot but different
  image layers. The guard is recorded by
  `output/vulkan-av1-psnr/vulkan-av1-psnr-1778079216172.md` (OBU) and
  `output/vulkan-av1-psnr/vulkan-av1-psnr-1778079216167.md` (fMP4). The frame
  inspection helper now reports `show_frame` and `frame_to_show_map_idx` to make
  these display/reference cases diagnosable.
- `scripts/check_vulkan_av1_corpus_matrix.rs` now runs the reusable Vulkan AV1
  corpus gate. Latest matrix
  `output/vulkan-av1-corpus-matrix/vulkan-av1-corpus-matrix-1778079826467.md`
  covers 10 OBU/fMP4 cases: keyframe-only, generated GOP30, 16-frame
  alt-ref/show-existing, 32-frame GOP16/lag8, and expected unsupported
  32-frame GOP30/lag25 alias cases. All expected-pass rows reported
  `psnr_y_min=inf`; both alias rows failed only with the explicit
  `aliases Vulkan DPB slot` guard.
- `cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --frames 8 --skip-build --min-psnr-y 60 --gop-size 1`
  (`output/vulkan-av1-psnr/vulkan-av1-psnr-1778068068960.md`,
  `psnr_y_min=inf`)
- `cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --frames 8 --skip-build --min-psnr-y 60 --gop-size 30`
  (`output/vulkan-av1-psnr/vulkan-av1-psnr-1778075814861.md`,
  `psnr_y_min=inf`)
- `cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --input-format fmp4 --frames 8 --skip-build --min-psnr-y 60 --gop-size 1`
  (`output/vulkan-av1-psnr/vulkan-av1-psnr-1778068139781.md`,
  `psnr_y_min=inf`)
- `cargo +nightly -Zscript scripts/check_vulkan_av1_psnr.rs --input-format fmp4 --frames 8 --skip-build --min-psnr-y 60 --gop-size 30`
  (`output/vulkan-av1-psnr/vulkan-av1-psnr-1778075801683.md`,
  `psnr_y_min=inf`)

The detailed implementation plan is
`docs/plan/VULKAN_AV1_IMPLEMENTATION_PLAN_2026-05-06.md`.

## VideoToolbox AV1 Status

VideoToolbox AV1 decode is now scaffolded for fMP4 inputs that provide `av1C`
and track dimensions. `VtDecoderOptions` carries the `av1C` record, config OBUs,
and coded width/height; the VT backend creates a `CMVideoFormatDescription`
with `SampleDescriptionExtensionAtoms.av1C`, passes AV1 sample OBUs through
without AVCC/HVCC length-prefix conversion, and strips prepended config OBUs
from fMP4 keyframe payloads before submit.

Current safeguards and evidence:

- AV1 encode still returns `VideoToolbox AV1 encode is not implemented in video-hw yet`;
- AV1 decode without fMP4 `av1C`/track dimensions returns actionable
  `UnsupportedConfig` instead of claiming arbitrary OBU support;
- `video-hw-fmp4` attaches VT AV1 decode options from `av01` sample entries;
- `decode_to_yuv --input-format mp4` lazily creates VT AV1 sessions after the
  first `av01` sample entry is available;
- `scripts/benchmark_ffmpeg_vt_precise.rs --codec av1 --verify` now generates
  an AV1 fMP4 input and records decode-only video-hw VT vs FFmpeg VT results,
  including PSNR-Y against an FFmpeg software NV12 reference;
  `scripts/run_vt_precise_suite.rs --include-av1` includes that AV1 pass in the
  serial VT suite;
- `cargo check -p video-hw-backend-vt --target x86_64-apple-darwin --features backend-vt --tests`
  passes on the cross target;
- macOS-target full example/fMP4 checks from this Windows host are blocked by a
  missing cross `cc` tool before reaching project code.

Remaining work requires a macOS host with AV1 VideoToolbox hardware support to
run the actual fMP4 decode path, compare with FFmpeg `av1_videotoolbox`, and
record PSNR/performance. AV1 encode remains unimplemented.

## Next Concrete Work

1. Expand Vulkan AV1 parser/reference-state coverage beyond generated libaom
   fixtures; keyframe-only, short GOP, long GOP OBU, and long GOP fMP4 gates now
   pass, but arbitrary AV1 streams are not yet a support claim.
2. Keep OBU and fMP4 PSNR gates passing while adding broader inter-frame
   fixtures, larger dimensions, and access-order benches.
3. Update Vulkan bindings before adding AV1 encode, then enable encode only for
   adapters exposing encode queue and
   `VK_KHR_video_encode_av1`; report Intel encode as unavailable when FFmpeg also
   cannot encode.
4. On macOS AV1 hardware, run the new VideoToolbox AV1 fMP4 decode path and
   update the VT status only after FFmpeg benchmark and PSNR pass; AV1 encode
   still needs a separate implementation.
