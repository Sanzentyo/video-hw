# Vulkan HEVC Decode API Semantics Investigation

Date: 2026-04-29

## Focus

This note records the API semantics relevant to the HEVC P/B-frame corruption investigation:

- `VkVideoDecodeH265PictureInfoKHR::pSliceSegmentOffsets`
- `StdVideoDecodeH265PictureInfo::{RefPicSetStCurrBefore, RefPicSetStCurrAfter, RefPicSetLtCurr}`
- SPS DPB buffering count interpretation

## Findings

1. Khronos describes H.265 decode bitstream data as VCL NAL-unit data for the picture, with `pSliceSegmentOffsets` identifying the starting offsets of slice segment headers within the bitstream buffer range.

   Source: https://github.khronos.org/Vulkan-Site/spec/latest/chapters/videocoding.html

2. Khronos explicitly states that `RefPicSetStCurrBefore`, `RefPicSetStCurrAfter`, and `RefPicSetLtCurr` elements identify active reference pictures by DPB slot index, or use `STD_VIDEO_H265_NO_REFERENCE_PICTURE`.

   Source: https://github.khronos.org/Vulkan-Site/spec/latest/chapters/videocoding.html

3. Khronos maps `StdVideoH265DecPicBufMgr::max_dec_pic_buffering_minus1` to the H.265 SPS syntax element `sps_max_dec_pic_buffering_minus1`. Since the syntax element is a minus-one value, the number of DPB pictures allowed is `value + 1`.

   Source: https://github.khronos.org/Vulkan-Site/spec/latest/chapters/videocoding.html

## Impact On This Codebase

- `RefPicSetStCurrBefore/After` should continue to contain DPB slot indices, not indices into `pReferenceSlots`.
- The active-reference retention cap must use `sps_max_dec_pic_buffering_minus1 + 1`. Using the raw minus-one value evicted one reference too early near POC wrap and caused `poc=250 not found in DPB`.
- The observed behavior with FFmpeg and NVIDIA suggests the practical driver-facing slice offset should point at the Annex-B start code for the submitted slice buffer, while keeping the internal RBSP offset available for diagnostics.
