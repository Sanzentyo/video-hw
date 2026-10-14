//! Unit tests for Intel surface conversion and adapter input contracts.

use super::surface::{i420_to_argb, nv12_to_argb};
use super::*;

#[test]
fn onevpl_loader_panics_are_reported_as_backend_errors() {
    let result =
        catch_onevpl_loader_initialization::<()>(|| panic!("synthetic oneVPL loader failure"));
    let error = result.expect_err("loader panic must become an error");
    assert!(matches!(error, BackendError::UnsupportedConfig(_)));
    assert!(
        error
            .to_string()
            .contains("synthetic oneVPL loader failure")
    );
}

#[test]
fn nv12_to_argb_neutral_chroma_produces_grayscale() {
    let nv12 = vec![
        16, 82, //
        145, 235, //
        128, 128, // UV
    ];
    let argb = nv12_to_argb(&nv12, 2, 2).expect("nv12->argb should succeed");
    assert_eq!(argb.len(), 16);

    let pixels = argb.chunks_exact(4).collect::<Vec<_>>();
    assert_eq!(pixels[0], &[255, 0, 0, 0]);
    assert!(pixels[3][1] >= 250 && pixels[3][2] >= 250 && pixels[3][3] >= 250);

    for px in pixels {
        assert_eq!(px[1], px[2]);
        assert_eq!(px[2], px[3]);
    }
}

#[test]
fn i420_to_argb_neutral_chroma_produces_grayscale() {
    let i420 = vec![
        16, 82, //
        145, 235, //
        128, // U
        128, // V
    ];
    let argb = i420_to_argb(&i420, 2, 2).expect("i420->argb should succeed");
    assert_eq!(argb.len(), 16);

    let pixels = argb.chunks_exact(4).collect::<Vec<_>>();
    assert_eq!(pixels[0], &[255, 0, 0, 0]);
    assert!(pixels[3][1] >= 250 && pixels[3][2] >= 250 && pixels[3][3] >= 250);

    for px in pixels {
        assert_eq!(px[1], px[2]);
        assert_eq!(px[2], px[3]);
    }
}

#[test]
fn encoder_requires_the_configured_payload_format() {
    let mut encoder = IntelEncoderAdapter::new(EncoderConfig::new(
        Codec::H264,
        30,
        false,
        EncodeInputFormat::Nv12,
    ));
    let error = encoder
        .push_frame(Frame {
            width: 2,
            height: 2,
            pixel_format: None,
            pts_90k: None,
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            argb: Some(vec![0; 16]),
            rgb24: None,
            nv12: None,
            force_keyframe: false,
        })
        .expect_err("NV12 configuration must not accept ARGB-only input");

    assert!(matches!(error, BackendError::InvalidInput(_)));
    assert!(encoder.pending_frames.is_empty());
    assert!(encoder.width.is_none());
    assert!(encoder.height.is_none());
}
