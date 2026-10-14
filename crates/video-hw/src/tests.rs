use std::str::FromStr;

use super::*;

#[test]
fn backend_names_are_stable_even_when_the_implementation_is_not_compiled() {
    assert_eq!(Backend::from_str("android").unwrap(), Backend::Android);
    assert_eq!(
        Backend::from_str("videotoolbox").unwrap(),
        Backend::VideoToolbox
    );
    assert_eq!(Backend::from_str("nvidia").unwrap(), Backend::Nvidia);
    assert_eq!(Backend::from_str("intel").unwrap(), Backend::Intel);
    assert_eq!(Backend::from_str("vulkan").unwrap(), Backend::Vulkan);
}

#[test]
fn unavailable_explicit_backend_returns_a_typed_error() {
    let unavailable = [
        BackendKind::Android,
        BackendKind::VideoToolbox,
        BackendKind::Nvidia,
        BackendKind::Intel,
        BackendKind::Vulkan,
    ]
    .into_iter()
    .find(|kind| !kind.is_compiled());
    let Some(unavailable) = unavailable else {
        return;
    };
    let backend = match unavailable {
        BackendKind::Android => Backend::Android,
        BackendKind::VideoToolbox => Backend::VideoToolbox,
        BackendKind::Nvidia => Backend::Nvidia,
        BackendKind::Intel => Backend::Intel,
        BackendKind::Vulkan => Backend::Vulkan,
    };

    let error = backend
        .resolve_decoder(&DecoderConfig::new(Codec::H264, 30, false))
        .expect_err("uncompiled backend must not resolve");
    assert_eq!(error.kind(), BackendErrorKind::UnsupportedConfig);
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
#[test]
fn backend_explicit_nvidia_resolution_returns_concrete_kind() {
    let decode = Backend::Nvidia
        .resolve_decoder(&DecoderConfig::new(Codec::H264, 30, false))
        .expect("nvidia explicit backend resolution should succeed");
    assert_eq!(decode, BackendKind::Nvidia);

    let encode = Backend::Nvidia
        .resolve_encoder(&EncoderConfig::new(
            Codec::H264,
            30,
            false,
            EncodeInputFormat::Argb8888,
        ))
        .expect("nvidia explicit backend resolution should succeed");
    assert_eq!(encode, BackendKind::Nvidia);
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
#[test]
fn static_nvidia_backend_sessions_report_backend_kind() {
    assert_eq!(
        DecodeSession::<NvDecoderAdapter>::backend_kind_static(),
        BackendKind::Nvidia
    );
    assert_eq!(
        EncodeSession::<NvEncoderAdapter>::backend_kind_static(),
        BackendKind::Nvidia
    );

    let decode_session =
        DecodeSession::<NvDecoderAdapter>::new(DecoderConfig::new(Codec::H264, 30, false));
    assert!(
        decode_session
            .query_capability(Codec::H264)
            .map(|cap| cap.contract.decode.supported)
            .unwrap_or(false)
    );

    let encode_session = EncodeSession::<NvEncoderAdapter>::new(EncoderConfig::new(
        Codec::H264,
        30,
        false,
        EncodeInputFormat::Argb8888,
    ));
    assert_eq!(encode_session.backend_kind(), BackendKind::Nvidia);
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
#[test]
fn strict_session_switch_api_is_available_for_nvidia_static_session() {
    let mut session = EncodeSession::<NvEncoderAdapter>::new(EncoderConfig::new(
        Codec::H264,
        30,
        false,
        EncodeInputFormat::Argb8888,
    ));
    let _ = session.request_session_switch_strict(SessionSwitchRequest::Nvidia {
        config: NvidiaSessionConfig {
            gop_length: Some(60),
            frame_interval_p: Some(1),
            force_idr_on_activate: true,
        },
        mode: SessionSwitchMode::Immediate,
    });
}

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
#[test]
fn static_intel_backend_sessions_report_backend_kind() {
    assert_eq!(
        DecodeSession::<IntelDecoderAdapter>::backend_kind_static(),
        BackendKind::Intel
    );
    assert_eq!(
        EncodeSession::<IntelEncoderAdapter>::backend_kind_static(),
        BackendKind::Intel
    );

    let decode_session =
        DecodeSession::<IntelDecoderAdapter>::new(DecoderConfig::new(Codec::Hevc, 30, false));
    assert!(
        decode_session
            .query_capability(Codec::Hevc)
            .map(|cap| cap.contract.decode.supported)
            .unwrap_or(false)
    );

    let encode_session = EncodeSession::<IntelEncoderAdapter>::new(EncoderConfig::new(
        Codec::Hevc,
        30,
        false,
        EncodeInputFormat::Argb8888,
    ));
    assert_eq!(encode_session.backend_kind(), BackendKind::Intel);
}

#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
#[test]
fn static_vulkan_backend_sessions_report_backend_kind() {
    assert_eq!(
        DecodeSession::<VulkanDecoderAdapter>::backend_kind_static(),
        BackendKind::Vulkan
    );
    assert_eq!(
        EncodeSession::<VulkanEncoderAdapter>::backend_kind_static(),
        BackendKind::Vulkan
    );

    let decode_session =
        DecodeSession::<VulkanDecoderAdapter>::new(DecoderConfig::new(Codec::H264, 30, false));
    assert!(
        decode_session
            .query_capability(Codec::H264)
            .map(|cap| cap.contract.decode.supported)
            .unwrap_or(false)
    );

    let encode_session = EncodeSession::<VulkanEncoderAdapter>::new(EncoderConfig::new(
        Codec::H264,
        30,
        false,
        EncodeInputFormat::Argb8888,
    ));
    assert_eq!(encode_session.backend_kind(), BackendKind::Vulkan);
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[test]
fn static_vt_backend_sessions_report_backend_kind() {
    assert_eq!(
        DecodeSession::<VtDecoderAdapter>::backend_kind_static(),
        BackendKind::VideoToolbox
    );
    assert_eq!(
        EncodeSession::<VtEncoderAdapter>::backend_kind_static(),
        BackendKind::VideoToolbox
    );
}

#[test]
fn unpack_length_prefixed_sample_to_annexb_converts_nals() {
    let sample = [
        0, 0, 0, 2, 0x67, 0x64, //
        0, 0, 0, 3, 0x68, 0xEE, 0x3C,
    ];
    let annexb = bitstream::length_prefixed_to_annexb(
        bitstream::LengthPrefixedSampleRef::new(&sample),
        bitstream::NalLengthSize::Four,
    )
    .unwrap()
    .into_inner();
    assert_eq!(
        annexb,
        vec![
            0, 0, 0, 1, 0x67, 0x64, //
            0, 0, 0, 1, 0x68, 0xEE, 0x3C
        ]
    );
}

#[test]
fn av1_length_prefixed_sample_is_forwarded_as_obu_payload() {
    let sample = vec![0x12, 0x00, 0x0a, 0x0b, 0x14, 0x00, 0x24];
    let (chunk, pts) = normalize_bitstream_input(
        BitstreamInput::Av1ObuSample {
            sample: sample.clone(),
            pts_90k: Some(Timestamp90k(1234)),
        },
        Codec::Av1,
    )
    .expect("AV1 MP4 sample should be forwarded as OBU payload");

    assert_eq!(chunk, sample);
    assert_eq!(pts, Some(1234));
}

#[test]
fn tagged_bitstream_input_rejects_a_codec_mismatch() {
    let error = normalize_bitstream_input(
        BitstreamInput::LengthPrefixedSample {
            codec: Codec::Hevc,
            sample: vec![0, 0, 0, 1, 0x26],
            nal_length_size: bitstream::NalLengthSize::Four,
            pts_90k: None,
        },
        Codec::H264,
    )
    .unwrap_err();

    assert!(
        matches!(error, BackendError::InvalidInput(message) if message.contains("decoder expects h264") && message.contains("declares hevc"))
    );
}

#[test]
fn av1_raw_nal_input_is_rejected() {
    let error = normalize_bitstream_input(
        BitstreamInput::AccessUnitRawNal {
            codec: Codec::Av1,
            nalus: vec![vec![0x12, 0x00]],
            pts_90k: None,
        },
        Codec::Av1,
    )
    .unwrap_err();

    assert!(
        matches!(error, BackendError::InvalidInput(message) if message.contains("OBU payloads"))
    );
}

#[test]
fn tagged_bitstream_input_rejects_empty_complete_units() {
    let empty_access_unit = normalize_bitstream_input(
        BitstreamInput::AccessUnitRawNal {
            codec: Codec::H264,
            nalus: Vec::new(),
            pts_90k: None,
        },
        Codec::H264,
    )
    .unwrap_err();
    assert!(
        matches!(empty_access_unit, BackendError::InvalidInput(message) if message.contains("does not contain any NAL units"))
    );

    let empty_av1_sample = normalize_bitstream_input(
        BitstreamInput::Av1ObuSample {
            sample: Vec::new(),
            pts_90k: None,
        },
        Codec::Av1,
    )
    .unwrap_err();
    assert!(matches!(
        empty_av1_sample,
        BackendError::InvalidInput(message) if message.contains("sample is empty")
    ));

    let empty_annexb = normalize_bitstream_input(
        BitstreamInput::AnnexBChunk {
            chunk: Vec::new(),
            pts_90k: None,
        },
        Codec::H264,
    )
    .unwrap_err();
    assert!(matches!(
        empty_annexb,
        BackendError::InvalidInput(message) if message.contains("use flush()")
    ));
}

#[test]
fn length_prefixed_input_uses_its_declared_nal_length_size() {
    let (annexb, pts) = normalize_bitstream_input(
        BitstreamInput::LengthPrefixedSample {
            codec: Codec::H264,
            sample: vec![0, 2, 0x67, 0x64, 0, 1, 0x68],
            nal_length_size: bitstream::NalLengthSize::Two,
            pts_90k: Some(Timestamp90k(42)),
        },
        Codec::H264,
    )
    .expect("two-byte NAL lengths should be honored");

    assert_eq!(annexb, vec![0, 0, 0, 1, 0x67, 0x64, 0, 0, 0, 1, 0x68]);
    assert_eq!(pts, Some(42));
}

#[test]
fn decoder_config_default_output_mode_is_metadata() {
    let config = DecoderConfig::new(Codec::H264, 30, false);
    assert_eq!(config.output_mode, DecodeOutputMode::Metadata);
}

#[test]
fn decode_nv12_mode_rejects_missing_pixel_payload() {
    let frame = Frame {
        width: 640,
        height: 360,
        pixel_format: None,
        pts_90k: Some(0),
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        argb: None,
        rgb24: None,
        nv12: None,
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        force_keyframe: false,
    };

    let err = backend_frame_to_decoded_frame(frame, DecodeOutputMode::Nv12).unwrap_err();
    assert!(matches!(err, BackendError::UnsupportedConfig(_)));
}

#[test]
fn decode_rgb24_mode_rejects_missing_pixel_payload() {
    let frame = Frame {
        width: 640,
        height: 360,
        pixel_format: None,
        pts_90k: Some(0),
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        argb: None,
        rgb24: None,
        nv12: None,
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        force_keyframe: false,
    };

    let err = backend_frame_to_decoded_frame(frame, DecodeOutputMode::Rgb24).unwrap_err();
    assert!(matches!(err, BackendError::UnsupportedConfig(_)));
}

#[test]
fn decode_nv12_mode_uses_native_nv12_payload() {
    let data = vec![16, 32, 48, 64, 128, 128];
    let frame = Frame {
        width: 2,
        height: 2,
        pixel_format: Some(u32::from_le_bytes(*b"NV12")),
        pts_90k: Some(9000),
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        argb: None,
        rgb24: None,
        nv12: Some(Nv12FramePayload {
            pitch: 2,
            data: data.clone(),
        }),
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        force_keyframe: false,
    };

    let out = backend_frame_to_decoded_frame(frame, DecodeOutputMode::Nv12).unwrap();
    match out {
        DecodedFrame::Nv12 {
            pitch,
            data: out_data,
            ..
        } => {
            assert_eq!(pitch, 2);
            assert_eq!(out_data, data);
        }
        other => panic!("unexpected frame: {other:?}"),
    }
}

#[test]
fn decode_rgb24_mode_converts_native_nv12_payload() {
    let frame = Frame {
        width: 2,
        height: 2,
        pixel_format: Some(u32::from_le_bytes(*b"NV12")),
        pts_90k: Some(9000),
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        argb: None,
        rgb24: None,
        nv12: Some(Nv12FramePayload {
            pitch: 2,
            data: vec![128; 6],
        }),
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        force_keyframe: false,
    };

    let out = backend_frame_to_decoded_frame(frame, DecodeOutputMode::Rgb24).unwrap();
    match out {
        DecodedFrame::Rgb24 { data, .. } => assert_eq!(data.len(), 2 * 2 * 3),
        other => panic!("unexpected frame: {other:?}"),
    }
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        any(
            feature = "backend-nvidia",
            feature = "backend-intel",
            feature = "backend-vulkan"
        ),
        any(target_os = "linux", target_os = "windows")
    )
))]
#[test]
fn decode_rgb24_mode_converts_argb_payload() {
    let frame = Frame {
        width: 2,
        height: 1,
        pixel_format: None,
        pts_90k: Some(9000),
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        argb: Some(vec![255, 10, 20, 30, 255, 40, 50, 60]),
        rgb24: None,
        nv12: None,
        force_keyframe: false,
    };

    let out = backend_frame_to_decoded_frame(frame, DecodeOutputMode::Rgb24).unwrap();
    match out {
        DecodedFrame::Rgb24 { data, .. } => assert_eq!(data, vec![10, 20, 30, 40, 50, 60]),
        other => panic!("unexpected frame: {other:?}"),
    }
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        any(
            feature = "backend-nvidia",
            feature = "backend-intel",
            feature = "backend-vulkan"
        ),
        any(target_os = "linux", target_os = "windows")
    )
))]
#[test]
fn decode_nv12_mode_converts_argb_payload() {
    let frame = Frame {
        width: 2,
        height: 2,
        pixel_format: None,
        pts_90k: Some(9000),
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        argb: Some(vec![
            255, 10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120,
        ]),
        rgb24: None,
        nv12: None,
        force_keyframe: false,
    };

    let out = backend_frame_to_decoded_frame(frame, DecodeOutputMode::Nv12).unwrap();
    match out {
        DecodedFrame::Nv12 { pitch, data, .. } => {
            assert_eq!(pitch, 2);
            assert_eq!(data.len(), 2 * 2 + (2 * 2 / 2));
        }
        other => panic!("unexpected frame: {other:?}"),
    }
}

#[test]
fn encoded_packet_metadata_is_preserved_at_the_facade_boundary() {
    let chunk = backend_packet_to_encoded_chunk(EncodedPacket {
        codec: Codec::H264,
        layout: EncodedLayout::Avcc,
        kind: EncodedContentKind::CodecConfig,
        data: vec![1, 2, 3],
        pts_90k: None,
        is_keyframe: false,
    });

    assert_eq!(chunk.codec, Codec::H264);
    assert_eq!(chunk.layout, EncodedLayout::Avcc);
    assert_eq!(chunk.kind, EncodedContentKind::CodecConfig);
    assert!(!chunk.is_keyframe);
}

#[test]
fn encode_frame_into_backend_frame_rejects_invalid_nv12_payload() {
    let dims = Dimensions {
        width: std::num::NonZeroU32::new(640).unwrap(),
        height: std::num::NonZeroU32::new(360).unwrap(),
    };
    let result = encode_frame_into_backend_frame(EncodeFrame {
        dims,
        pts_90k: Some(Timestamp90k(0)),
        buffer: RawFrameBuffer::Nv12 {
            pitch: 320,
            data: vec![0; 16],
        },
        force_keyframe: false,
    });
    assert!(matches!(result, Err(BackendError::InvalidInput(_))));
}

#[test]
fn raw_frame_buffer_reports_input_format() {
    assert_eq!(
        RawFrameBuffer::Argb8888(vec![0; 4]).input_format(),
        EncodeInputFormat::Argb8888
    );
    assert_eq!(
        RawFrameBuffer::Nv12 {
            pitch: 2,
            data: vec![0; 6]
        }
        .input_format(),
        EncodeInputFormat::Nv12
    );
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
#[test]
fn preflight_encode_accepts_nvidia_nv12_by_contract() {
    let report = preflight_encode(EncodePreflightRequest {
        backend: Backend::Nvidia,
        codec: Codec::H264,
        input_format: EncodeInputFormat::Nv12,
        require_hardware: true,
        expected_layout: Some(EncodedLayout::AnnexB),
    });
    assert_eq!(report.resolved_backend, Some(BackendKind::Nvidia));
    assert!(report.supported_by_contract);
    assert_eq!(
        report.accepted_input_formats,
        vec![EncodeInputFormat::Argb8888, EncodeInputFormat::Nv12]
    );
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[test]
fn preflight_encode_rejects_vt_nv12_by_contract() {
    let report = preflight_encode(EncodePreflightRequest {
        backend: Backend::VideoToolbox,
        codec: Codec::H264,
        input_format: EncodeInputFormat::Nv12,
        require_hardware: false,
        expected_layout: Some(EncodedLayout::Avcc),
    });
    assert_eq!(report.resolved_backend, Some(BackendKind::VideoToolbox));
    assert!(!report.supported_by_contract);
    assert!(!report.usable_in_current_runtime);
    assert_eq!(
        report.accepted_input_formats,
        vec![EncodeInputFormat::Argb8888]
    );
    assert_eq!(report.encoded_layouts, vec![EncodedLayout::Avcc]);
}

#[cfg(all(
    feature = "backend-nvidia",
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
#[test]
fn auto_encode_prefers_nvidia_for_supported_nv12_input() {
    let config = EncoderConfig::new(Codec::H264, 30, true, EncodeInputFormat::Nv12);
    assert_eq!(
        Backend::Auto.resolve_encoder(&config).unwrap(),
        BackendKind::Nvidia
    );
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        any(
            feature = "backend-nvidia",
            feature = "backend-intel",
            feature = "backend-vulkan"
        ),
        any(target_os = "linux", target_os = "windows")
    )
))]
#[test]
fn decode_reap_timeout_waits_until_deadline_when_empty() {
    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    let mut session =
        DecodeSession::<VtDecoderAdapter>::new(DecoderConfig::new(Codec::H264, 30, false));
    #[cfg(all(
        not(all(target_os = "macos", feature = "backend-vt")),
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    let mut session =
        DecodeSession::<NvDecoderAdapter>::new(DecoderConfig::new(Codec::H264, 30, false));
    #[cfg(all(
        not(all(target_os = "macos", feature = "backend-vt")),
        not(feature = "backend-nvidia"),
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ))]
    let mut session =
        DecodeSession::<IntelDecoderAdapter>::new(DecoderConfig::new(Codec::H264, 30, false));
    #[cfg(all(
        not(all(target_os = "macos", feature = "backend-vt")),
        not(feature = "backend-nvidia"),
        not(feature = "backend-intel"),
        feature = "backend-vulkan",
        any(target_os = "linux", target_os = "windows")
    ))]
    let mut session =
        DecodeSession::<VulkanDecoderAdapter>::new(DecoderConfig::new(Codec::H264, 30, false));
    let timeout = std::time::Duration::from_millis(8);
    let start = std::time::Instant::now();
    let out = session.reap_timeout(timeout).unwrap();
    let elapsed = start.elapsed();

    assert!(out.is_none());
    assert!(elapsed >= timeout);
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        any(
            feature = "backend-nvidia",
            feature = "backend-intel",
            feature = "backend-vulkan"
        ),
        any(target_os = "linux", target_os = "windows")
    )
))]
#[test]
fn encode_reap_timeout_waits_until_deadline_when_empty() {
    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    let mut session = EncodeSession::<VtEncoderAdapter>::new(EncoderConfig::new(
        Codec::H264,
        30,
        false,
        EncodeInputFormat::Argb8888,
    ));
    #[cfg(all(
        not(all(target_os = "macos", feature = "backend-vt")),
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    let mut session = EncodeSession::<NvEncoderAdapter>::new(EncoderConfig::new(
        Codec::H264,
        30,
        false,
        EncodeInputFormat::Argb8888,
    ));
    #[cfg(all(
        not(all(target_os = "macos", feature = "backend-vt")),
        not(feature = "backend-nvidia"),
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ))]
    let mut session = EncodeSession::<IntelEncoderAdapter>::new(EncoderConfig::new(
        Codec::H264,
        30,
        false,
        EncodeInputFormat::Argb8888,
    ));
    #[cfg(all(
        not(all(target_os = "macos", feature = "backend-vt")),
        not(feature = "backend-nvidia"),
        not(feature = "backend-intel"),
        feature = "backend-vulkan",
        any(target_os = "linux", target_os = "windows")
    ))]
    let mut session = EncodeSession::<VulkanEncoderAdapter>::new(EncoderConfig::new(
        Codec::H264,
        30,
        false,
        EncodeInputFormat::Argb8888,
    ));
    let timeout = std::time::Duration::from_millis(8);
    let start = std::time::Instant::now();
    let out = session.reap_timeout(timeout).unwrap();
    let elapsed = start.elapsed();

    assert!(out.is_none());
    assert!(elapsed >= timeout);
}

struct FailingFlushDecoder;

impl VideoDecoder for FailingFlushDecoder {
    fn query_capability(&self, _codec: Codec) -> Result<CapabilityReport, BackendError> {
        Err(BackendError::Backend("unused test query".to_string()))
    }

    fn push_bitstream_chunk(
        &mut self,
        _chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        Ok(vec![test_metadata_frame(pts_90k)])
    }

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        Err(BackendError::Backend(
            "intentional flush failure".to_string(),
        ))
    }

    fn decode_summary(&self) -> DecodeSummary {
        DecodeSummary {
            decoded_frames: 1,
            width: Some(1),
            height: Some(1),
            pixel_format: None,
        }
    }
}

struct QueueingEncoder {
    fail_flush: bool,
}

impl VideoEncoder for QueueingEncoder {
    fn query_capability(&self, _codec: Codec) -> Result<CapabilityReport, BackendError> {
        Err(BackendError::Backend("unused test query".to_string()))
    }

    fn push_frame(&mut self, _frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        Ok(vec![test_encoded_packet(1)])
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        if self.fail_flush {
            Err(BackendError::Backend(
                "intentional flush failure".to_string(),
            ))
        } else {
            Ok(vec![test_encoded_packet(3)])
        }
    }

    fn request_session_switch(
        &mut self,
        _request: SessionSwitchRequest,
    ) -> Result<Vec<EncodedPacket>, BackendError> {
        Ok(vec![test_encoded_packet(2)])
    }
}

#[test]
fn decode_flush_failure_preserves_already_queued_frames() {
    let mut session =
        DecodeSession::from_decoder(Codec::H264, DecodeOutputMode::Metadata, FailingFlushDecoder);
    session
        .submit(BitstreamInput::AnnexBChunk {
            chunk: vec![0, 0, 1, 0x65],
            pts_90k: Some(Timestamp90k(90)),
        })
        .expect("test decoder should queue one frame");

    assert!(session.flush().is_err());
    let frame = session
        .try_reap()
        .expect("queued frame should remain readable")
        .expect("queued frame should not be lost");
    match frame {
        DecodedFrame::Metadata { pts_90k, .. } => {
            assert_eq!(pts_90k, Some(Timestamp90k(90)));
        }
        other => panic!("unexpected queued frame: {other:?}"),
    }
}

#[test]
fn encode_flush_failure_preserves_already_queued_chunks() {
    let mut session = EncodeSession::from_encoder(
        BackendKind::Nvidia,
        EncodeInputFormat::Argb8888,
        QueueingEncoder { fail_flush: true },
    );
    session
        .submit(test_encode_frame())
        .expect("test encoder should queue one chunk");

    assert!(session.flush().is_err());
    let chunk = session
        .try_reap()
        .expect("queued chunk should remain readable")
        .expect("queued chunk should not be lost");
    assert_eq!(chunk.data, vec![1]);
}

#[test]
fn session_switch_returns_existing_ready_chunks_before_switch_drain() {
    let mut session = EncodeSession::from_encoder(
        BackendKind::Nvidia,
        EncodeInputFormat::Argb8888,
        QueueingEncoder { fail_flush: false },
    );
    session
        .submit(test_encode_frame())
        .expect("test encoder should queue one chunk");

    let chunks = session
        .request_session_switch(SessionSwitchRequest::Nvidia {
            config: NvidiaSessionConfig {
                gop_length: Some(30),
                frame_interval_p: Some(1),
                force_idr_on_activate: true,
            },
            mode: SessionSwitchMode::DrainThenSwap,
        })
        .expect("test session switch should succeed");

    assert_eq!(
        chunks
            .iter()
            .map(|chunk| chunk.data.as_slice())
            .collect::<Vec<_>>(),
        vec![&[1][..], &[2][..]]
    );
}

fn test_metadata_frame(pts_90k: Option<i64>) -> Frame {
    Frame {
        width: 1,
        height: 1,
        pixel_format: None,
        pts_90k,
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        argb: None,
        rgb24: None,
        nv12: None,
        force_keyframe: false,
    }
}

fn test_encoded_packet(marker: u8) -> EncodedPacket {
    EncodedPacket {
        codec: Codec::H264,
        layout: EncodedLayout::AnnexB,
        kind: EncodedContentKind::Frame,
        data: vec![marker],
        pts_90k: Some(i64::from(marker)),
        is_keyframe: marker == 1,
    }
}

fn test_encode_frame() -> EncodeFrame {
    EncodeFrame {
        dims: Dimensions::new(1, 1).expect("test dimensions are valid"),
        pts_90k: Some(Timestamp90k(0)),
        buffer: RawFrameBuffer::Argb8888(vec![0; 4]),
        force_keyframe: false,
    }
}
