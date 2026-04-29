# NVIDIA Vulkan Video HEVC Investigation

Date: 2026-04-29

## Focus

This note records related implementation evidence for the NVIDIA Vulkan Video HEVC issue where IDR decoded correctly but non-IDR frames initially behaved like reference copies or concealment output.

## Findings

1. FFmpeg's Vulkan HEVC path calls `ff_vk_decode_add_slice(..., add_startcode = 1, ..., &pSliceSegmentOffsets)`.

   Source: https://ffmpeg.org/doxygen/7.1/vulkan__hevc_8c_source.html

2. FFmpeg's shared Vulkan decode helper uses a 3-byte Annex-B prefix (`00 00 01`), stores the slice offset before writing that prefix, then appends the slice data after the prefix.

   Source: https://www.ffmpeg.org/doxygen/7.0/vulkan__decode_8c_source.html

3. Matching that submission shape in this repository fixed the first P-frame failure:

   - Before: normalized 4-byte start code plus RBSP offset caused the second decoded frame to match the IDR/reference frame (`diff=0`, `psnr_y=21.87 dB` against the real P-frame).
   - After: 3-byte start code plus start-code slice offset gave `psnr_y=48.45 dB` for the second frame of `foreman_first2.h265`.

4. Full-stream validation also required respecting the SPS DPB buffering count as a minus-one value. After changing active reference retention to `sps_max_dec_pic_buffering_minus1 + 1`, the 300-frame `foreman_cif.h265` raw-vs-raw check stayed above `psnr_y=47.49 dB` and no longer logged `poc=250 not found in DPB`.

## Reproduction Notes

The most useful local checks were:

- Decode `sample-videos/foreman_first2.h265` and compare frame 2 against FFmpeg raw NV12 output.
- Decode `sample-videos/foreman_cif.h265`, compare raw NV12 vs raw NV12 reference, and inspect the minimum per-frame Y PSNR.

The encoded-input-vs-raw-input PSNR path can produce misleading mid-stream frame pairing, so raw-vs-raw comparison is preferred for this diagnostic.

## Follow-up Check

`cargo +nightly -Zscript scripts/check_vulkan_hevc_psnr.rs` is the focused regression check for this issue. It decodes HEVC with the Vulkan backend to NV12, decodes the same stream with FFmpeg to NV12, and computes raw-vs-raw PSNR.

On 2026-04-30, `sample-videos/foreman_cif.h265` produced:

- `psnr_y_avg=48.2443 dB`
- `psnr_y_min=47.4900 dB` at frame 220
- `psnr_avg=49.6294 dB`

This keeps the repaired inter-frame path well above the 40 dB regression threshold and covers the earlier POC-wrap failure area.
