# Video HW Quality Check — 2026-04-17

## Environment

- CPU: Intel Core i7-12700
- GPU 0: NVIDIA GeForce RTX 3080 (NVENC / NVDEC)
- GPU 1: Intel UHD Graphics 770 (Quick Sync)
- Vulkan driver: RTX 3080 (H.264 Video Decode/Encode KHR)
- OS: Windows 11
- FFmpeg: 8.1
- Rust toolchain: stable + nightly (for `cargo -Zscript`)

## Test Video

`sample-videos/foreman_cif.*` — Foreman CIF (352×288, 300 frames, 30 fps)  
Source: [Xiph.org Video Test Media](https://media.xiph.org/video/derf/) — **CC0 / public domain**  
See [`sample-videos/README.md`](../../sample-videos/README.md) for full attribution.

Reference generated with FFmpeg (CRF 20 H.264 / CRF 26 HEVC).

## Methodology

1. Extract all 300 frames from the reference MP4 as raw ARGB (FFmpeg software decode).
2. Encode those frames using each hardware backend via `encode_raw_argb` example (`--fps 30`).
3. Compute PSNR Y and weighted average using FFmpeg `psnr` lavfi filter (per-frame stats averaged).
4. Separately decode the sample bitstreams with each backend and compute PSNR against FFmpeg software-decoded RGB frames.
5. Verify frame counts for fMP4 round-trip.

Script: `cargo +nightly -Zscript scripts/quality_check.rs`

## Results

### Encode Quality (PSNR vs FFmpeg software-decode reference)

Threshold: **25.0 dB PSNR Y** (hardware encoders at moderate bitrate vs CRF20/CRF26 reference)

| Backend      | Codec | PSNR Y (dB) | PSNR Avg (dB) | Status  |
|:-------------|:------|------------:|--------------:|:--------|
| NVIDIA NVENC | H.264 | 26.00        | 27.68         | ✅ PASS |
| NVIDIA NVENC | HEVC  | 26.04        | 27.73         | ✅ PASS |
| Intel QSV    | H.264 | 26.03        | 27.72         | ✅ PASS |
| Intel QSV    | HEVC  | 25.45        | 27.15         | ✅ PASS |
| Vulkan Video | H.264 | 26.07        | 27.77         | ✅ PASS |
| Vulkan Video | HEVC  | —            | —             | ⚠️ N/A (encode path not yet wired; driver-level crash in probe) |

### Decode Pixel Quality (PSNR vs FFmpeg software decode)

Threshold: **25.0 dB PSNR Y**

| Backend      | Codec | PSNR Y (dB) | PSNR Avg (dB) | Status  |
|:-------------|:------|------------:|--------------:|:--------|
| Vulkan Video | H.264 | 34.69        | 36.39         | ✅ PASS |
| Vulkan Video | HEVC  | 13.78        | 15.49         | ⚠️ Known (experimental HEVC decode) |
| Intel QSV    | H.264 | —            | —             | ⚠️ Known (`NotFound`: VPL session decode not initialised) |
| Intel QSV    | HEVC  | —            | —             | ⚠️ Known (`NotFound`: VPL session decode not initialised) |
| NVIDIA NVDEC | *     | (via fMP4 round-trip, see below) | | |

### fMP4 Round-Trip Frame Count

| Backend      | Codec | Expected | Got | Status  |
|:-------------|:------|:--------:|:---:|:--------|
| NVIDIA NVDEC | H.264 | 300      | 300 | ✅ PASS |
| Intel QSV    | H.264 | 300      | 300 | ✅ PASS |
| Vulkan Video | H.264 | 300      | 300 | ✅ PASS |

## Bugs Fixed in This Work

### 1. NVIDIA NVENC — ARGB byte-order inversion
- **Symptom**: Blue cast on all encoded frames (~22 dB PSNR)
- **Root cause**: `NV_ENC_BUFFER_FORMAT_ARGB` is word-ordered `0xAARRGGBB` (LE = `[B,G,R,A]` in memory), but upload was sending true `[A,R,G,B]`
- **Fix**: `argb_to_nvenc_format()` reverses channel order per pixel before upload
- **File**: `crates/video-hw-backend-nvidia/src/nv_backend.rs`

### 2. NVIDIA NVENC — pitched upload ignored
- **Symptom**: Garbled horizontal stripe pattern on encoded frames
- **Root cause**: `lock.write(&argb)` flat-copies ignoring NVENC hardware row padding
- **Fix**: `lock.write_pitched(&data, width*4, height)`
- **File**: `crates/video-hw-backend-nvidia/src/nv_backend.rs`

### 3. Intel QSV HEVC — NV12 UV plane only half written (bright green chroma)
- **Symptom**: Bottom half of each frame was solid bright green (U=0, V=0 → R=0, G=255, B=0); PSNR ~13 dB
- **Root cause**: The `onevpl` Rust bindings return `u()` and `v()` slices covering only `(crop_height/2) × (pitch/2)` bytes — half the interleaved UV plane. For 352×288 with pitch=352: only UV rows 0–71 were written; rows 72–143 remained 0.
- **Fix**: `nv12_uv_plane_full_mut` unsafe helper extends the `u()` slice pointer to the full `(crop_height/2) × pitch` interleaved UV plane. Both `copy_nv12_to_surface` and `write_argb_to_nv12_surface` were fixed.
- **File**: `crates/video-hw-backend-intel/src/intel_backend.rs`

### 4. Vulkan H.264 encode — fps timebase mismatch in PSNR computation
- **Symptom**: PSNR ~17 dB (frame drift); Vulkan raw H.264 stream detected as 25 fps by FFmpeg
- **Root cause**: Raw AnnexB H.264 elementary streams have no container-level timing; FFmpeg defaults to 25 fps when reading with `-f h264`, causing 5-frame drift at frame 300 vs the 30 fps reference
- **Fix**: Added `-r 30` before the encoded stream input in `compute_psnr_encode()` in `scripts/quality_check.rs`
- **File**: `scripts/quality_check.rs`

### 5. Vulkan HEVC probe — driver crash in test
- **Symptom**: `ensure_vk_codec_supported_rejects_hevc_encode_with_actionable_message` test aborted with `STATUS_STACK_BUFFER_OVERRUN` (0xc0000409); caused entire test suite to fail
- **Root cause**: `hevc_encode_blocker_message_with_config` called `probe_hevc_encode_session_bootstrap` which attempts to create a live Vulkan HEVC encode session — the RTX 3080 driver crashes on this probe. Windows SEH (structured exceptions) cannot be caught with Rust's `catch_unwind`.
- **Fix**: Removed the `probe_hevc_encode_session_bootstrap` call from the error-message path; the blocker message is still informative. The probe code is preserved in `vulkan_hevc_encode.rs` (marked `#![allow(dead_code)]` as experimental).
- **Files**: `crates/video-hw-backend-vulkan/src/vulkan_backend.rs`, `crates/video-hw-backend-vulkan/src/vulkan_hevc_encode.rs`

## Known Remaining Limitations

| Area | Status | Notes |
|:-----|:-------|:------|
| Intel QSV decode | ⚠️ NotFound | VPL session init fails for decode on this system |
| Vulkan HEVC decode | ⚠️ 13.78 dB | Experimental decoder; bitstream parsing incomplete |
| Vulkan HEVC encode | ⚠️ Probe crash | RTX 3080 driver crashes during HEVC encode session bootstrap |
| CUDA 13.2 workaround | ℹ️ | `CUDARC_CUDA_VERSION = "13010"` in `.cargo/config.toml`; `cudarc 0.19.2` max is CUDA 13.1 |
