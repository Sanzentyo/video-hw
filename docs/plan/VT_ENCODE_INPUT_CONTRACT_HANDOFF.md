# VideoToolbox Encode Input Contract Handoff

## Summary

This Windows/NVIDIA pass updated the shared encode contract and verified the
NVIDIA path on a machine with an NVIDIA GPU. The follow-up macOS pass verified
the VideoToolbox contract and added regression coverage for the remaining VT
encode input cases.

The intended contract is:

- VideoToolbox encode accepts `EncodeInputFormat::Argb8888` only.
- `RawFrameBuffer::Nv12` must not be accepted by VT encode.
- Missing ARGB payloads must fail with `BackendError::InvalidInput`.
- VT must never synthesize replacement pixels in the public encode path.

## Implemented In Shared Branch

- `VtEncoderAdapter::push_frame` requires `frame.argb.is_some()`.
- `make_bgra_frame` accepts `argb: &[u8]`.
- The public VT encode path no longer has a missing-ARGB synthetic branch.
- VT capability reporting is:
  - H.264 encode: `input_formats=[Argb8888]`, `encoded_layouts=[Avcc]`
  - HEVC encode: `input_formats=[Argb8888]`, `encoded_layouts=[Hvcc]`
  - AV1 encode: unsupported

## macOS Verification

Completed on macOS, 2026-05-16:

- VT backend tests pass with encode capability and missing-ARGB regression
  coverage.
- VT H.264/HEVC ARGB encode succeeds and returns `Avcc` / `Hvcc`
  respectively.
- VT missing-ARGB encode is rejected.
- VT NV12 encode input is rejected before output packets are produced.

## Validation Commands

Run these on macOS:

```bash
cargo test -p video-hw-backend-vt --features backend-vt -- --nocapture
cargo test -p video-hw --features backend-vt e2e_vt -- --nocapture --test-threads=1
cargo test -p video-hw --features backend-vt preflight_encode_rejects_vt_nv12_by_contract -- --nocapture
```

The tests should fail if any VT public encode path silently replaces missing
input pixels with synthetic content.
