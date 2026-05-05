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

### Latest integrated run after fixes

- Command:
  `cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends nv,intel,vulkan --codecs h264,hevc --warmup 1 --repeat 3 --frame-count 30 --width 320 --height 180 --vulkan-adapter-indexes 0,1 --allow-failures true --verify`
- H.264 integrated: `output/benchmark-backends-h264-1777958931.md`
  - NVIDIA: PASS
  - Intel oneVPL: PASS
  - Vulkan NVIDIA: H.264 decode 597.305 fps vs FFmpeg Vulkan 512.060 fps; H.264 encode 112.450 fps vs FFmpeg Vulkan 90.921 fps
  - Vulkan Intel: not exposed by `vk-video` / `video-hw`; FFmpeg Vulkan decode exits `0xc0000005`, encode fails with `Function not implemented`
- HEVC integrated: `output/benchmark-backends-hevc-1777958995.md`
  - NVIDIA: PASS
  - Intel oneVPL: PASS
  - Vulkan NVIDIA: HEVC decode 680.103 fps vs FFmpeg Vulkan 495.702 fps; at that point production HEVC encode was still unavailable in `VulkanEncoderAdapter`
  - Vulkan Intel: not exposed by `vk-video` / `video-hw`; FFmpeg Vulkan decode works at 194.852 fps, encode fails with `Function not implemented`

### Latest Vulkan HEVC production smoke

- Command:
  `cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends vulkan --codec hevc --warmup 0 --repeat 1 --frame-count 3 --width 640 --height 360 --vulkan-adapter-indexes 0,1 --allow-failures true --verify`
- Integrated: `output/benchmark-backends-hevc-1777959697.md`
  - Vulkan NVIDIA decode: video-hw 690.765 fps vs FFmpeg Vulkan 502.376 fps
  - Vulkan NVIDIA encode: video-hw 1.868 fps vs FFmpeg Vulkan 9.164 fps
  - Vulkan Intel: not exposed by `vk-video` / `video-hw`; FFmpeg Vulkan decode works at 181.712 fps, FFmpeg Vulkan encode fails with `Function not implemented`
- Decode verification smoke:
  `cargo run -p video-hw --features backend-vulkan --example encode_synthetic --release -- --backend vulkan --codec hevc --fps 30 --frame-count 3 --width 640 --height 360 --output output\vulkan-hevc-production-smoke-1777959697.h265 --require-hardware --vulkan-adapter-index 0`
  followed by `ffprobe -f hevc -count_frames` and `ffmpeg -v error -f hevc -i ... -f null NUL`.
  Result: `codec_name=hevc`, `width=640`, `height=360`, `nb_read_frames=3`.
- Interpretation: Vulkan HEVC encode is no longer just a live probe on the NVIDIA adapter. `VulkanEncoderAdapter` can emit decodable Annex-B HEVC by prepending FFmpeg-generated leading non-VCL NALs to direct Vulkan slice output. This first production smoke was still IDR-only and recreated the video session per frame, so it was functionally useful for smoke/parity exploration but not yet FFmpeg performance parity.

### Latest Vulkan HEVC batched session reuse

- Command:
  `cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends vulkan --codec hevc --warmup 0 --repeat 1 --frame-count 30 --width 640 --height 360 --vulkan-adapter-indexes 0 --allow-failures true`
- Integrated: `output/benchmark-backends-hevc-1777960076.md`
  - Vulkan NVIDIA encode: video-hw 18.519 fps vs FFmpeg Vulkan 84.270 fps
  - Previous per-frame bootstrap run at the same size was 9.728 fps (`output/benchmark-backends-hevc-1777959888.md`), so reusing video session/session parameters within one `flush` improves throughput about 1.90x.
- Decode verification smoke:
  `cargo run -p video-hw --features backend-vulkan --example encode_synthetic --release -- --backend vulkan --codec hevc --fps 30 --frame-count 30 --width 640 --height 360 --output output\vulkan-hevc-batch-smoke-1777960076.h265 --require-hardware --vulkan-adapter-index 0`
  followed by `ffprobe -f hevc -count_frames` and `ffmpeg -v error -f hevc -i ... -f null NUL`.
  Result: `codec_name=hevc`, `width=640`, `height=360`, `nb_read_frames=30`.
- Interpretation: session/parameter reuse is now covered for a single `flush`. Without warmup the first process/driver initialization still dominates, but the subsequent full integrated run below shows steady-state Vulkan NVIDIA HEVC encode at FFmpeg parity or faster. Remaining engineering debt is per-submit resource churn, all-IDR packetization, and lack of a long-lived production encoder / reference-frame GOP.

### Latest full integrated parity run

- Command:
  `cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends nv,intel,vulkan --codecs h264,hevc --warmup 1 --repeat 3 --frame-count 30 --width 320 --height 180 --vulkan-adapter-indexes 0,1 --allow-failures true --verify`
- H.264 integrated: `output/benchmark-backends-h264-1777960325.md`
  - NVIDIA: PASS (`output/benchmark-nv-precise-h264-1777960272.md`)
  - Intel oneVPL: PASS (`output/benchmark-intel-precise-h264-1777960313.md`)
  - Vulkan NVIDIA: decode 602.917 fps vs FFmpeg Vulkan 511.830 fps; encode 132.253 fps vs FFmpeg Vulkan 91.132 fps
  - Vulkan Intel: `vulkaninfo` exposes Intel(R) Graphics as GPU0, but `video-hw` / `vk-video` does not expose it; FFmpeg Vulkan H.264 decode exits `0xc0000005` and encode fails with `Function not implemented`
- HEVC integrated: `output/benchmark-backends-hevc-1777960393.md`
  - NVIDIA: PASS (`output/benchmark-nv-precise-hevc-1777960334.md`)
  - Intel oneVPL: PASS (`output/benchmark-intel-precise-hevc-1777960372.md`)
  - Vulkan NVIDIA: decode 690.355 fps vs FFmpeg Vulkan 495.414 fps; encode 135.137 fps vs FFmpeg Vulkan 87.344 fps
  - Vulkan Intel: `vulkaninfo` exposes Intel(R) Graphics as GPU0, but `video-hw` / `vk-video` does not expose it; FFmpeg Vulkan HEVC decode works at 183.091 fps, while FFmpeg Vulkan HEVC encode fails with `Function not implemented`
- Adapter discovery:
  - `vulkaninfo --summary`: GPU0 Intel(R) Graphics (`vendorID=0x8086`, `deviceID=0x7d67`, driver 101.6629), GPU1 NVIDIA GeForce RTX 5070 Ti Laptop GPU (`vendorID=0x10de`, `deviceID=0x2f18`, driver 591.44)
  - `cargo run -p video-hw --features backend-vulkan --example list_vulkan_adapters --release`: only `0 NVIDIA GeForce RTX 5070 Ti Laptop GPU 4318 12056 true true`
  - Full `vulkaninfo` extension scan: Intel exposes `VK_KHR_video_decode_h264`, `VK_KHR_video_decode_h265`, `VK_KHR_video_decode_queue`, and `VK_KHR_video_queue`, but does not expose `VK_KHR_video_encode_queue` / H.264 or H.265 encode extensions. NVIDIA exposes both decode and encode queues for H.264/H.265.
- Interpretation: NV, Intel oneVPL, and the available `video-hw` Vulkan NVIDIA adapter are at FFmpeg parity or faster for the measured H.264/HEVC decode+encode cases. The remaining uncovered hardware is Intel Vulkan: it exists as a Vulkan physical device, but is not exposed by `vk-video` / `video-hw` on this driver, and FFmpeg's own Vulkan encode path fails on it. This is recorded as an environment/driver/backend availability gap, not a passing Vulkan parity case.

### Intel Vulkan direct ash HEVC decode probe

- Change: `VIDEO_HW_VULKAN_HEVC_DECODE_PHYSICAL_DEVICE_INDEX=<n>` can force the direct ash HEVC decode bootstrap to a Vulkan physical device index without changing the existing `--vulkan-adapter-index` / `vk-video` adapter semantics. The integrated Vulkan runner uses this only for `ffmpeg-only` HEVC decode candidates.
- Command:
  `cargo +nightly -Zscript scripts/benchmark_ffmpeg_backends.rs --backends vulkan --codec hevc --warmup 1 --repeat 3 --frame-count 30 --width 320 --height 180 --vulkan-adapter-indexes 0,1 --allow-failures true`
- Integrated: `output/benchmark-backends-hevc-1777960871.md`
  - NVIDIA Vulkan remains PASS: decode 680.183 fps vs FFmpeg Vulkan 531.841 fps; encode 135.721 fps vs FFmpeg Vulkan 88.375 fps
  - Intel direct ash HEVC decode was attempted with physical device index 0 and failed at `vkGetVideoSessionMemoryRequirementsKHR returned no memory requirements`
  - FFmpeg Vulkan HEVC decode on Intel still works, but FFmpeg Vulkan HEVC encode still fails with `Function not implemented`
- Interpretation: Intel Vulkan is now actively probed instead of only being marked unavailable. The driver advertises HEVC decode capability, but the direct ash session bootstrap does not produce usable session memory requirements on this environment. This remains the Vulkan/Intel-specific blocker.

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
| Vulkan NVIDIA | HEVC encode | Partial: experimental IDR-only production path now emits decodable 30-frame Annex-B HEVC at 18.519 fps in the latest 640x360 batched run; FFmpeg Vulkan HEVC encode on the same NVIDIA adapter measures 84.270 fps in that run, so performance parity remains incomplete |
| Vulkan Intel | HEVC decode/encode | Decode works in FFmpeg Vulkan at 196.72 fps but is not exposed by `vk-video` / `video-hw`; FFmpeg Vulkan encode fails with unsupported encode queue |

### Intel oneVPL verification follow-up

- Latest verification reports after fixing benchmark output retention:
  - H.264: `output/benchmark-intel-precise-h264-1777958803.md` (PASS, verify=ok)
  - HEVC: `output/benchmark-intel-precise-hevc-1777958762.md` (PASS, verify=ok)
- The Intel precise benchmark previously passed `--discard-output` even when
  `--verify` was requested, so the integrated runner reported verification as
  skipped despite successful encode runs. The script now keeps the video-hw
  output when verification is enabled.
- Intel HEVC hardware encode also needed packet collection to normalize
  oneVPL `mfxBitstream.DataOffset` before reading `DataLength`; otherwise the
  Annex-B stream could start from stale bytes and FFmpeg decoded only a subset
  of the requested frames. The HEVC ARGB->NV12 hardware path now uses the
  synchronous encode call for correctness; the 30-frame 320x180 verify run
  reports 30 decodable frames.

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
- `VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_VUI_SAFETY=auto|preserve|force-off`
- `VIDEO_HW_VULKAN_HEVC_ENCODE_OUTPUT_PATH=<Annex-B slice output>`

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
reusing the SPS profile-tier-level pointer, and session creation now uses the
device max coded extent like FFmpeg instead of the requested probe extent. HEVC
encode video profile pNext chains now also place H.265 profile info before
encode usage info, matching FFmpeg's profile chain order. With
`rate_control_mode=cbr` and `control_mode=ffmpeg`, slice `constant_qp=0` and
`slice_qp_delta=-26` now match FFmpeg's rate-control shape, but the 1920x1080
sample live probe still reports session-parameter feedback
`ERROR_OUT_OF_HOST_MEMORY` and fails at `vkEndCommandBuffer`. The aligned
`1920x1088` `empty-template` probe also still fails, so coded-size granularity
is not the remaining blocker. A size-matched `320x180` override using
`output/ffmpeg-vulkan-hevc-probe-1f.h265` generated by FFmpeg's own
`hevc_vulkan` encoder still has the same feedback and submit failure, so the
blocker is not only the repository sample's parameter-set shape. The probe now
also mirrors FFmpeg's source picture resource extent by using the input image's
aligned extent (`1920x1088` for the 1920x1080 sample), can omit codec-info pNext
from the begin reference slot via `begin_reference_slot_mode=ffmpeg`, and can
advance `dstBufferOffset` with `VIDEO_HW_VULKAN_HEVC_ENCODE_DST_PREFIX_BYTES`
to model FFmpeg's sequence-header/filler prefix. Encode image views now also use
an identity `VkSamplerYcbcrConversion` like FFmpeg, and
encode images now honor `vkGetImageMemoryRequirements2` dedicated allocation
requirements. On this NVIDIA driver the probe reports
`image_memory_dedicated=src:false|dpb:false`, so dedicated allocation is not the
missing requirement. The probe now carries
`VideoEncodeH265CapabilitiesKHR.std_syntax_flags` into diagnostics, keeps SPS
SAO aligned with the capability bits, and forces
`sps_temporal_mvp_enabled_flag=false` like FFmpeg. On this driver that yields
`parameter_set_sao=true` and `parameter_set_temporal_mvp=false`, but submit still
fails. `VIDEO_HW_VULKAN_HEVC_ENCODE_DPB_BARRIER_MODE=none` can omit the explicit
DPB image layout barrier to mirror FFmpeg's non-layered DPB path.
`VIDEO_HW_VULKAN_HEVC_ENCODE_REFERENCE_SLOT_POINTER_MODE=ffmpeg` can also keep a
non-null `pReferenceSlots` pointer while `referenceSlotCount=0`, matching
FFmpeg's IDR encode-info shape more closely. The disabled rate-control fixed QP
default now matches FFmpeg's "no rate control settings" fallback of 18, with
`VIDEO_HW_VULKAN_HEVC_ENCODE_CONSTANT_QP=<0..51>` retained as a probe override.
The encode quality-level default now matches FFmpeg's `quality=0`; use
`VIDEO_HW_VULKAN_HEVC_ENCODE_QUALITY_LEVEL=<n>` to override it for probes.
The source image is now initialized through a staging-buffer NV12 upload instead
of a clear-only path, so the probe no longer feeds a synthetic unfilled
multi-plane image to encode.
`VIDEO_HW_VULKAN_HEVC_ENCODE_SOURCE_PICTURE_RESOURCE_EXTENT_MODE=coded|image-aligned`
can also switch `srcPictureResource.codedExtent` independently; both 320x180 and
320x192 source resource extents still fail at `vkEndCommandBuffer` in the
FFmpeg-style 320x180 probe.
`VIDEO_HW_VULKAN_HEVC_ENCODE_SESSION_H265_CREATE_INFO_MODE=without` can also
omit `VkVideoEncodeH265SessionCreateInfoKHR` from `vkCreateVideoSessionKHR`,
matching FFmpeg's session-create shape more closely; this still fails at
`vkEndCommandBuffer`.
`VIDEO_HW_VULKAN_HEVC_ENCODE_BEGIN_PNEXT_MODE=rate-control` can attach the
rate-control pNext chain to `cmd_begin_video_coding_khr`, matching FFmpeg's
conditional begin-coding shape; this also still fails at `vkEndCommandBuffer`.
`VIDEO_HW_VULKAN_HEVC_ENCODE_DST_RANGE_MODE=ffmpeg-reserve-align` can reserve
the final bitstream size alignment from `VkVideoEncodeInfoKHR.dstBufferRange`
like FFmpeg; this changes the 320x180 probe range from `1048576` to `1048320`
but still fails at `vkEndCommandBuffer`.
`VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_SIZE_MODE=coded` can override the
sample-derived StdVideo SPS picture size to the probe coded size; the 320x180
sample-parameter probe now reaches `parameter_set_coded_match=true`, but still
fails at `vkEndCommandBuffer`.
As an additional check, FFmpeg `hevc_vulkan` on Vulkan adapter 1 generated a
320x180 Annex-B HEVC stream successfully; using that generated stream as
`VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_SAMPLE_PATH` also reaches
`parameter_set_coded=320x180` / `parameter_set_coded_match=true`. With the
previous default VUI safety behavior this still forced the effective parameter
mode to `sample-sps-vui-flag-off` and failed at `vkEndCommandBuffer`.
`VIDEO_HW_VULKAN_HEVC_ENCODE_PARAMETER_VUI_SAFETY=preserve` keeps the
FFmpeg-generated SPS VUI intact; under the FFmpeg-style 320x180 probe
combination (`parameter_mode=sample`, `parameter_size_mode=sample`,
`image_view_mode=no-ycbcr`, `dst_prefix=256`,
`dst_prefix_mode=parameter-sample`, `control_mode=ffmpeg`,
`begin_pnext_mode=ffmpeg`, `begin_reference_slot_mode=ffmpeg`,
`reference_slot_pointer_mode=ffmpeg`, `dpb_barrier_mode=none`,
`source_picture_resource_extent_mode=coded`,
`session_h265_create_info_mode=ffmpeg`, `dst_range_mode=ffmpeg`) the NVIDIA
live probe now reports `encode_submit_execution_probe: Ready` with
`bytes_written=47`. The feedback bitstream offset is relative to
`VkVideoEncodeInfoKHR::dstBufferOffset`, so the probe now reads the output at
`dstBufferOffset + feedback_offset`; the actual encoded slice starts with
`head16=000000012601af08a0bcea1fff7f661a`.
`VIDEO_HW_VULKAN_HEVC_ENCODE_IMAGE_VIEW_MODE=no-ycbcr` can omit
`VkSamplerYcbcrConversionInfo` from encode image views; image view creation
succeeds in this mode, and it is part of the successful FFmpeg-generated
parameter probe above.
`VIDEO_HW_VULKAN_HEVC_ENCODE_DST_PREFIX_MODE=parameter-sample` can write the
parameter sample bytes into the externally encoded prefix area before
`dstBufferOffset`; writing the first 256 bytes of the FFmpeg-generated 320x180
stream is also part of the successful submit diagnostic above. This does not
yet prove production HEVC encode support by itself: the driver output is the
slice only, so it is not independently decodable without parameter/header NALs.
`scripts/check_vulkan_hevc_encode_probe.rs` reproduces the diagnostic by
generating an FFmpeg `hevc_vulkan` 320x180 parameter sample, dumping the live
probe output slice, prepending the leading non-VCL NALs (VPS/SPS/PPS/prefix
SEI), decoding the combined stream with FFmpeg, and comparing the decoded NV12
frame against the probe's flat input. On this NVIDIA adapter the result is
`mse_y=0`, `mse_uv=0`, `mse_all=0`, `psnr_y=inf`, `psnr_uv=inf`, `psnr_all=inf`.
More complete HEVC packetization and multi-frame production wiring are still
required before enabling `Codec::Hevc` in `VulkanEncoderAdapter`.

## Notes

- NV and Intel decode throughput comparisons use `--decode-output-mode metadata`
  so they match FFmpeg null-sink decode instead of measuring pixel readback.
- NV encode comparisons now default to equal raw ARGB input. The older synthetic
  vs lavfi comparison could report a false encode parity failure.
- Vulkan is now adapter-addressable in `video-hw` options and examples. The
  integrated runner matches adapters by name/device id instead of assuming
  `video-hw` and FFmpeg use the same numeric Vulkan index.
- FFmpeg Vulkan device initialization now uses `-init_hw_device vulkan:<index>`
  for the matched adapter. On this Windows hybrid-GPU machine,
  `vulkan=vk:<index>` did not reliably select the requested physical device and
  could send the NVIDIA encode comparison to Intel, where Vulkan encode queues
  are unavailable.
- `video-hw` / `vk-video` exposes the NVIDIA Vulkan video adapter on this
  machine. Intel Vulkan is listed by `vulkaninfo`, but is not usable by either
  `video-hw` or FFmpeg Vulkan in this environment; Intel hardware parity is
  covered through the oneVPL/QSV backend above.
- Vulkan metadata decode now uses GPU texture decode instead of the byte decoder
  path, avoiding CPU NV12 readback for metadata-only benchmarks.
