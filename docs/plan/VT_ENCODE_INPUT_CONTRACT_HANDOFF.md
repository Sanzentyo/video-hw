# VideoToolbox Encode Input Contract Handoff

## Summary

This Windows/NVIDIA pass updated the shared encode contract and verified the
NVIDIA path on a machine with an NVIDIA GPU. The VT source was updated
statically, but VideoToolbox is macOS-only, so the remaining work is macOS
verification and test coverage.

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

## Required macOS Verification

1. Build and run the VT backend tests on macOS.
2. Add or enable macOS tests for:
   - VT H.264/HEVC ARGB encode still succeeds.
   - VT missing-ARGB encode is rejected.
   - VT NV12 encode input is rejected before output packets are produced.

## Validation Commands

Run these on macOS:

```bash
cargo test -p video-hw-backend-vt --features backend-vt -- --nocapture
cargo test -p video-hw --features backend-vt e2e_vt_backend_decode_and_encode_work -- --nocapture --test-threads=1
cargo test -p video-hw --features backend-vt e2e_vt_backend_encode_accepts_backend_specific_options -- --nocapture --test-threads=1
```

The tests should fail if any VT public encode path silently replaces missing
input pixels with synthetic content.
