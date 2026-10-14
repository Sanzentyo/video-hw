use super::*;

fn h264_encoder(input_format: EncodeInputFormat) -> NvEncoderAdapter {
    NvEncoderAdapter::new(EncoderConfig::new(Codec::H264, 30, true, input_format))
}

#[test]
fn switch_on_next_keyframe_forces_the_next_frame() {
    let mut adapter = h264_encoder(EncodeInputFormat::Argb8888);
    let drained = adapter
        .apply_nvidia_session_switch(
            NvidiaSessionConfig {
                gop_length: Some(60),
                frame_interval_p: Some(1),
                force_idr_on_activate: false,
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
fn switch_immediate_updates_config_even_without_active_session() {
    let mut adapter = h264_encoder(EncodeInputFormat::Argb8888);
    let drained = adapter
        .apply_nvidia_session_switch(
            NvidiaSessionConfig {
                gop_length: Some(48),
                frame_interval_p: Some(1),
                force_idr_on_activate: true,
            },
            SessionSwitchMode::Immediate,
        )
        .unwrap();

    assert!(drained.is_empty());
    assert_eq!(adapter.gop_length, Some(48));
    assert_eq!(adapter.frame_interval_p, Some(1));
    assert!(adapter.force_next_keyframe);
}

#[test]
fn invalid_frame_does_not_consume_forced_keyframe() {
    let mut adapter = h264_encoder(EncodeInputFormat::Argb8888);
    adapter.force_next_keyframe = true;

    let error = adapter
        .push_frame(Frame {
            width: 2,
            height: 2,
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
fn argb_to_nvenc_abgr_format_preserves_logical_channels() {
    let argb = vec![255, 10, 20, 30, 128, 40, 50, 60];
    assert_eq!(
        argb_to_nvenc_format(&argb),
        vec![10, 20, 30, 255, 40, 50, 60, 128]
    );
}

#[test]
fn push_frame_rejects_missing_argb_payload() {
    let mut adapter = h264_encoder(EncodeInputFormat::Argb8888);
    let err = adapter
        .push_frame(Frame {
            width: 640,
            height: 360,
            pixel_format: None,
            pts_90k: Some(0),
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            argb: None,
            rgb24: None,
            nv12: Some(Nv12FramePayload {
                pitch: 640,
                data: vec![0; 640 * 360 * 3 / 2],
            }),
            force_keyframe: false,
        })
        .expect_err("NVIDIA encode must not synthesize missing ARGB input");
    assert!(matches!(err, BackendError::InvalidInput(_)));
}

#[test]
fn configured_nv12_input_rejects_argb_only_frame() {
    let mut adapter = h264_encoder(EncodeInputFormat::Nv12);
    let error = adapter
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
    assert!(adapter.pending_frames.is_empty());
    assert!(adapter.width.is_none());
    assert!(adapter.height.is_none());
}
