//! Unit tests for VideoToolbox contracts, switching, and sample inspection.

use super::*;
use video_hw_core::Nv12FramePayload;

fn vt_encoder(codec: Codec) -> VtEncoderAdapter {
    VtEncoderAdapter::new(EncoderConfig::new(
        codec,
        30,
        false,
        EncodeInputFormat::Argb8888,
    ))
}

#[test]
fn detect_h264_keyframe_from_length_prefixed_payload() {
    let mut payload = Vec::new();
    let idr = [0x65, 0x88, 0x84, 0x21];
    payload.extend_from_slice(&(idr.len() as u32).to_be_bytes());
    payload.extend_from_slice(&idr);
    assert_eq!(
        detect_keyframe_from_avcc_hvcc_payload(Codec::H264, &payload),
        Some(true)
    );
}

#[test]
fn detect_h264_non_keyframe_from_length_prefixed_payload() {
    let mut payload = Vec::new();
    let non_idr = [0x41, 0x9a, 0x22];
    payload.extend_from_slice(&(non_idr.len() as u32).to_be_bytes());
    payload.extend_from_slice(&non_idr);
    assert_eq!(
        detect_keyframe_from_avcc_hvcc_payload(Codec::H264, &payload),
        Some(false)
    );
}

#[test]
fn detect_hevc_keyframe_from_length_prefixed_payload() {
    let mut payload = Vec::new();
    // nal_unit_type=19 (IDR_W_RADL): first byte bits[6:1] = 0b010011.
    let idr = [0b0010_0110, 0x01, 0xaa, 0xbb];
    payload.extend_from_slice(&(idr.len() as u32).to_be_bytes());
    payload.extend_from_slice(&idr);
    assert_eq!(
        detect_keyframe_from_avcc_hvcc_payload(Codec::Hevc, &payload),
        Some(true)
    );
}

#[test]
fn av1_capability_report_claims_decode_only_vt_support() {
    let decoder = VtDecoderAdapter::new(DecoderConfig {
        codec: Codec::Av1,
        fps: 30,
        require_hardware: false,
        output_mode: DecodeOutputMode::Metadata,
        backend_options: BackendDecoderOptions::Default,
    });
    let decode_report = decoder.query_capability(Codec::Av1).unwrap();
    assert!(decode_report.contract.decode.supported);
    assert!(!decode_report.contract.encode.supported);
    assert!(!decode_report.contract.decode.output_modes.is_empty());

    let encoder = vt_encoder(Codec::Av1);
    let encode_report = encoder.query_capability(Codec::Av1).unwrap();
    assert!(encode_report.contract.decode.supported);
    assert!(!encode_report.contract.encode.supported);
    assert!(encode_report.contract.encode.input_formats.is_empty());
    assert!(encode_report.contract.encode.encoded_layouts.is_empty());
    assert!(!encode_report.contract.decode.output_modes.is_empty());
}

#[test]
fn vt_h264_hevc_capability_reports_argb_only_encode_contract() {
    for (codec, encoded_layout) in [
        (Codec::H264, EncodedLayout::Avcc),
        (Codec::Hevc, EncodedLayout::Hvcc),
    ] {
        let encoder = vt_encoder(codec);
        let report = encoder.query_capability(codec).unwrap();
        assert!(report.contract.encode.supported);
        assert_eq!(
            report.contract.encode.input_formats,
            vec![EncodeInputFormat::Argb8888]
        );
        assert_eq!(report.contract.encode.encoded_layouts, vec![encoded_layout]);
    }
}

#[test]
fn vt_encode_rejects_missing_argb_without_synthetic_output() {
    for nv12 in [
        None,
        Some(Nv12FramePayload {
            pitch: 16,
            data: vec![128; 16 * 16 * 3 / 2],
        }),
    ] {
        let mut encoder = vt_encoder(Codec::H264);
        let result = encoder.push_frame(Frame {
            width: 16,
            height: 16,
            pixel_format: None,
            pts_90k: Some(0),
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            argb: None,
            rgb24: None,
            nv12,
            force_keyframe: true,
        });
        assert!(matches!(result, Err(BackendError::InvalidInput(_))));
        assert!(encoder.pending_frames.is_empty());
        assert!(encoder.try_reap().unwrap().is_empty());
        assert!(encoder.flush().unwrap().is_empty());
    }
}

#[test]
fn vt_av1_runtime_paths_return_actionable_unsupported_errors() {
    let mut decoder = VtDecoderAdapter::new(DecoderConfig {
        codec: Codec::Av1,
        fps: 30,
        require_hardware: false,
        output_mode: DecodeOutputMode::Metadata,
        backend_options: BackendDecoderOptions::Default,
    });
    let err = decoder
        .push_bitstream_chunk(&[0x12, 0x00], None)
        .unwrap_err();
    match err {
        BackendError::UnsupportedConfig(message) => {
            assert!(message.contains("VideoToolbox AV1 decode requires fMP4 track width"));
        }
        other => panic!("unexpected VT AV1 decode error: {other:?}"),
    }

    let mut encoder = vt_encoder(Codec::Av1);
    let err = encoder
        .push_frame(Frame {
            width: 16,
            height: 16,
            pixel_format: None,
            pts_90k: Some(0),
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            argb: Some(vec![0; 16 * 16 * 4]),
            rgb24: None,
            nv12: None,
            force_keyframe: true,
        })
        .unwrap_err();
    match err {
        BackendError::UnsupportedConfig(message) => {
            assert!(message.contains("VideoToolbox AV1 encode is not implemented"));
        }
        other => panic!("unexpected VT AV1 encode error: {other:?}"),
    }
}

#[test]
fn vt_switch_immediate_updates_generation() {
    let mut adapter = vt_encoder(Codec::H264);
    assert_eq!(adapter.configured_generation(), 1);
    let drained = adapter
        .apply_vt_session_switch(
            VtSessionConfig {
                force_keyframe_on_activate: false,
            },
            SessionSwitchMode::Immediate,
        )
        .unwrap();
    assert!(drained.is_empty());
    assert_eq!(adapter.configured_generation(), 2);
    assert!(adapter.session_reconfigure_pending);
}

#[test]
fn vt_switch_on_next_keyframe_forces_the_next_frame() {
    let mut adapter = vt_encoder(Codec::H264);
    let drained = adapter
        .apply_vt_session_switch(
            VtSessionConfig {
                force_keyframe_on_activate: false,
            },
            SessionSwitchMode::OnNextKeyframe,
        )
        .unwrap();
    assert!(drained.is_empty());
    assert!(adapter.pending_switch.is_none());
    assert!(adapter.force_next_keyframe);
    assert_eq!(adapter.configured_generation(), 2);
}

#[test]
fn vt_invalid_frame_does_not_consume_forced_keyframe() {
    let mut adapter = vt_encoder(Codec::H264);
    adapter.force_next_keyframe = true;

    let error = adapter
        .push_frame(Frame {
            width: 16,
            height: 16,
            pixel_format: None,
            pts_90k: None,
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            argb: None,
            rgb24: None,
            nv12: None,
            force_keyframe: false,
        })
        .expect_err("missing payload must be rejected");

    assert!(matches!(error, BackendError::InvalidInput(_)));
    assert!(adapter.force_next_keyframe);
    assert!(adapter.pending_frames.is_empty());
}

#[test]
fn vt_configured_nv12_input_is_rejected_explicitly() {
    let mut adapter = VtEncoderAdapter::new(EncoderConfig::new(
        Codec::H264,
        30,
        false,
        EncodeInputFormat::Nv12,
    ));

    let error = adapter
        .push_frame(Frame {
            width: 16,
            height: 16,
            pixel_format: None,
            pts_90k: None,
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            argb: None,
            rgb24: None,
            nv12: Some(Nv12FramePayload {
                pitch: 16,
                data: vec![0; 16 * 16 * 3 / 2],
            }),
            force_keyframe: false,
        })
        .expect_err("VideoToolbox must reject unsupported configured input formats");

    assert!(matches!(error, BackendError::UnsupportedConfig(_)));
    assert!(adapter.pending_frames.is_empty());
}

#[test]
fn vt_encoder_options_control_metrics() {
    let mut config = EncoderConfig::new(Codec::H264, 30, false, EncodeInputFormat::Argb8888);
    config.backend_options = BackendEncoderOptions::VideoToolbox(VtEncoderOptions {
        report_metrics: Some(true),
    });
    let adapter = VtEncoderAdapter::new(config);

    assert!(adapter.report_metrics);
}

#[test]
fn vt_decoder_options_control_metrics() {
    let adapter = VtDecoderAdapter::new(DecoderConfig {
        codec: Codec::H264,
        fps: 30,
        require_hardware: false,
        output_mode: DecodeOutputMode::Metadata,
        backend_options: BackendDecoderOptions::VideoToolbox(VtDecoderOptions {
            report_metrics: Some(true),
            ..Default::default()
        }),
    });

    assert!(adapter.report_metrics);
}
