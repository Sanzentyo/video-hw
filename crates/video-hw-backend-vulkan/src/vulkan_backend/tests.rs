use std::path::PathBuf;

use super::*;

#[test]
fn ensure_vk_codec_supported_accepts_h264() {
    ensure_vk_codec_supported(Codec::H264, "decode")
        .expect("H264 should be accepted by Vulkan backend");
    ensure_vk_codec_supported(Codec::H264, "encode")
        .expect("H264 should be accepted by Vulkan backend");
}

#[test]
fn ensure_vk_codec_supported_rejects_hevc_decode_with_actionable_message() {
    let err = ensure_vk_codec_supported(Codec::Hevc, "decode")
        .expect_err("HEVC must be rejected until ash-level path is implemented");
    match err {
        BackendError::UnsupportedConfig(message) => {
            assert!(
                message.contains("Vulkan HEVC decode initialization failed"),
                "expected HEVC decode initialization message: {message}"
            );
        }
        other => panic!("unexpected HEVC check error: {other:?}"),
    }
}

#[test]
fn ensure_vk_codec_supported_rejects_hevc_encode_with_actionable_message() {
    let err = ensure_vk_codec_supported(Codec::Hevc, "encode")
        .expect_err("HEVC encode must be rejected until ash-level path is implemented");
    match err {
        BackendError::UnsupportedConfig(message) => {
            assert!(
                message.contains("Vulkan HEVC encode initialization failed"),
                "expected HEVC encode initialization message: {message}"
            );
        }
        other => panic!("unexpected HEVC check error: {other:?}"),
    }
}

#[test]
fn ensure_vk_codec_supported_rejects_av1_with_actionable_message() {
    for operation in ["decode", "encode"] {
        let err = ensure_vk_codec_supported(Codec::Av1, operation)
            .expect_err("AV1 must be rejected until Vulkan AV1 path is implemented");
        match err {
            BackendError::UnsupportedConfig(message) => {
                assert!(
                    message.contains(&format!("Vulkan AV1 {operation} initialization failed")),
                    "expected Vulkan AV1 implementation message: {message}"
                );
            }
            other => panic!("unexpected AV1 check error: {other:?}"),
        }
    }
}

#[test]
fn av1_decode_blocker_message_with_bitstream_appends_obu_status() {
    let sequence_header = av1_test_obu(1, &av1_test_sequence_header_payload(320, 180));
    let mut bitstream = av1_test_obu(2, &[]);
    bitstream.extend_from_slice(&sequence_header);
    bitstream.extend_from_slice(&av1_test_obu(6, &[0x11, 0x12]));
    let message = av1_decode_blocker_message_with_bitstream(&bitstream);
    assert!(message.contains("Vulkan AV1 decode initialization failed"));
    assert!(message.contains("parsed AV1 OBUs"));
    assert!(message.contains("sequence_header=true"));
    assert!(message.contains("frame_headers=1"));
    assert!(message.contains("key_frames=1"));
    assert!(message.contains("inter_frames=0"));
    assert!(message.contains("show_existing_frames=0"));
    assert!(message.contains("coded=320x180"));
    assert!(message.contains("aligned_upload=ready"));
    assert!(message.contains("first_decode_info=true"));
    assert!(message.contains("decode_info_loop=1"));
    assert!(message.contains("command_sequence=true"));
    assert!(message.contains("record_summary=1/1/1/1"));
    assert!(message.contains("record_summary_valid=true"));
}

#[test]
fn hevc_encode_blocker_message_with_config_surfaces_base_message() {
    let message = hevc_encode_blocker_message_with_config(1920, 1080, 30);
    assert!(message.contains("Vulkan HEVC encode initialization failed"));
}

#[test]
fn hevc_capability_report_describes_both_implemented_directions() {
    let report = vulkan_capability_report(Codec::Hevc);
    assert!(report.contract.decode.supported);
    assert!(report.contract.encode.supported);
    assert_eq!(
        report.decode_runtime.hardware_acceleration,
        matches!(
            probe_hevc_decode_prerequisites(),
            HevcDecodePrerequisiteProbe::Ready
        )
    );
    assert_eq!(
        report.encode_runtime.hardware_acceleration,
        matches!(
            probe_hevc_encode_prerequisites(),
            HevcEncodePrerequisiteProbe::Ready
        )
    );
}

#[test]
fn av1_capability_report_describes_decode_only() {
    let report = vulkan_capability_report(Codec::Av1);
    assert!(report.contract.decode.supported);
    assert!(!report.contract.encode.supported);
    assert_eq!(
        report.decode_runtime.hardware_acceleration,
        matches!(
            probe_av1_decode_prerequisites(),
            Av1DecodePrerequisiteProbe::Ready
        )
    );
    assert!(!report.encode_runtime.hardware_acceleration);
}

#[test]
fn hevc_decode_blocker_message_with_bitstream_appends_parameter_set_status() {
    let message = hevc_decode_blocker_message_with_bitstream(&[0_u8, 1, 2, 3], None);
    assert!(message.contains("Vulkan HEVC decode initialization failed"));
    assert!(message.contains("parameter-set extraction failed"));
}

#[test]
fn hevc_metadata_decode_info_flags_set_probe_and_readback_when_covered() {
    let flags = build_hevc_metadata_decode_info_flags(3, true, true);
    assert_ne!(flags & HEVC_DECODE_INFO_PROBE_COVERED_FLAG, 0);
    assert_eq!(flags & HEVC_DECODE_INFO_READBACK_NON_ZERO_FLAG, 1);
    let slot_mask =
        !(HEVC_DECODE_INFO_PROBE_COVERED_FLAG | HEVC_DECODE_INFO_READBACK_NON_ZERO_FLAG);
    assert_eq!(flags & slot_mask, 3 << HEVC_DECODE_INFO_SLOT_SHIFT);
}

#[test]
fn hevc_metadata_decode_info_flags_clear_probe_and_readback_when_uncovered() {
    let flags = build_hevc_metadata_decode_info_flags(2, false, true);
    assert_eq!(flags & HEVC_DECODE_INFO_PROBE_COVERED_FLAG, 0);
    assert_eq!(flags & HEVC_DECODE_INFO_READBACK_NON_ZERO_FLAG, 0);
    let slot_mask =
        !(HEVC_DECODE_INFO_PROBE_COVERED_FLAG | HEVC_DECODE_INFO_READBACK_NON_ZERO_FLAG);
    assert_eq!(flags & slot_mask, 2 << HEVC_DECODE_INFO_SLOT_SHIFT);
}

#[test]
fn hevc_probe_readback_to_nv12_payload_supports_nv12_sample() {
    let readback_sample = vec![16_u8, 16, 16, 16, 128, 128];
    let nv12 = hevc_probe_readback_to_nv12_payload(
        vk::Format::G8_B8R8_2PLANE_420_UNORM,
        &readback_sample,
        2,
        2,
    )
    .expect("NV12 readback sample should produce an NV12 payload");
    assert_eq!(nv12.pitch, 2);
    assert_eq!(nv12.data, readback_sample);
}

#[test]
fn frame_to_nv12_payload_repacks_padded_rows() {
    let frame = Frame {
        width: 2,
        height: 2,
        pixel_format: Some(u32::from_le_bytes(*b"NV12")),
        pts_90k: None,
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        argb: None,
        rgb24: None,
        nv12: Some(Nv12FramePayload {
            pitch: 4,
            data: vec![1, 2, 9, 9, 3, 4, 9, 9, 5, 6, 9, 9],
        }),
        force_keyframe: false,
    };

    let packed = frame_to_nv12_payload(&frame, 2, 2).expect("valid padded NV12 should be repacked");
    assert_eq!(packed.frame, vec![1, 2, 3, 4, 5, 6]);
}

#[test]
fn frame_to_nv12_payload_rejects_short_padded_input() {
    let frame = Frame {
        width: 2,
        height: 2,
        pixel_format: Some(u32::from_le_bytes(*b"NV12")),
        pts_90k: None,
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        argb: None,
        rgb24: None,
        nv12: Some(Nv12FramePayload {
            pitch: 4,
            data: vec![0; 11],
        }),
        force_keyframe: false,
    };

    let error = match frame_to_nv12_payload(&frame, 2, 2) {
        Ok(_) => panic!("short padded NV12 must be rejected"),
        Err(error) => error,
    };
    assert!(matches!(error, BackendError::InvalidInput(_)));
}

#[test]
fn build_backend_frames_preserves_native_nv12_output() {
    let decoded = vec![VkFrame {
        data: RawFrameData {
            frame: vec![1, 2, 3, 4, 5, 6],
            width: 2,
            height: 2,
        },
        pts: None,
    }];

    let frames = build_backend_frames(decoded, DecodeOutputMode::Nv12, 90, 30)
        .expect("valid raw NV12 should remain NV12");
    assert!(frames[0].argb.is_none());
    assert!(frames[0].rgb24.is_none());
    assert_eq!(
        frames[0].nv12.as_ref().map(|payload| payload.pitch),
        Some(2)
    );
    assert_eq!(
        frames[0]
            .nv12
            .as_ref()
            .map(|payload| payload.data.as_slice()),
        Some([1, 2, 3, 4, 5, 6].as_slice())
    );
}

#[test]
fn build_backend_frames_populates_rgb24_contract_field() {
    let decoded = vec![VkFrame {
        data: RawFrameData {
            frame: vec![16, 16, 16, 16, 128, 128],
            width: 2,
            height: 2,
        },
        pts: None,
    }];

    let frames = build_backend_frames(decoded, DecodeOutputMode::Rgb24, 90, 30)
        .expect("valid raw NV12 should convert to RGB24");
    assert!(frames[0].argb.is_none());
    assert!(frames[0].nv12.is_none());
    assert_eq!(frames[0].rgb24.as_deref(), Some([0; 12].as_slice()));
}

#[test]
fn decode_pts_step_stays_positive_for_high_fps() {
    assert_eq!(decode_pts_step(100_000), 1);
}

#[test]
fn hevc_probe_readback_to_nv12_payload_rejects_unsupported_format() {
    let err = hevc_probe_readback_to_nv12_payload(vk::Format::D32_SFLOAT, &[0_u8; 6], 2, 2)
        .expect_err("unsupported format should fail");
    match err {
        BackendError::UnsupportedConfig(message) => {
            assert!(message.contains("requires G8_B8R8_2PLANE_420_UNORM"));
        }
        other => panic!("unexpected error for unsupported readback format: {other:?}"),
    }
}

#[test]
fn av1_readback_to_nv12_payload_uses_expected_even_extent_size() {
    let payload = av1_readback_to_nv12_payload(&[1, 2, 3, 4, 5, 6], 2, 2)
        .expect("2x2 NV12 readback should map to one payload");
    assert_eq!(payload.pitch, 2);
    assert_eq!(payload.data, vec![1, 2, 3, 4, 5, 6]);
}

#[test]
fn av1_readback_to_nv12_payload_rejects_odd_extent() {
    let err = av1_readback_to_nv12_payload(&[0; 16], 3, 2)
        .expect_err("odd AV1 readback extent should be rejected for facade NV12");
    assert!(matches!(err, BackendError::UnsupportedConfig(_)));
}

#[test]
fn hevc_non_metadata_probe_coverage_requires_full_stream_coverage() {
    let err = ensure_hevc_non_metadata_probe_coverage(false, 303, 16, 6144, 2, 16)
        .expect_err("partial probe coverage should fail for non-metadata mode");
    match err {
        BackendError::UnsupportedConfig(message) => {
            assert!(message.contains("supports up to 16 probe-covered access units"));
            assert!(message.contains("stream has 303"));
        }
        other => panic!("unexpected coverage error: {other:?}"),
    }
}

#[test]
fn hevc_non_metadata_probe_coverage_allows_fully_covered_stream() {
    ensure_hevc_non_metadata_probe_coverage(false, 16, 16, 6144, 2, 16)
        .expect("fully covered stream should pass in non-metadata mode");
}

#[test]
fn hevc_metadata_mode_ignores_probe_coverage_limit() {
    ensure_hevc_non_metadata_probe_coverage(true, 303, 0, 0, 0, 0)
        .expect("metadata mode should not require probe coverage");
}

#[test]
fn hevc_display_order_sorting_assigns_pts_in_display_order() {
    let mut poc30 = metadata_only_frame(2, 2, None);
    poc30.decode_info_flags = Some(30);
    let mut poc10 = metadata_only_frame(2, 2, None);
    poc10.decode_info_flags = Some(10);
    let mut poc20 = metadata_only_frame(2, 2, None);
    poc20.decode_info_flags = Some(20);

    let frames =
        sort_hevc_display_order_frames(vec![(30, poc30), (10, poc10), (20, poc20)], 9_000, 3_000);

    let pts_and_markers: Vec<(i64, u32)> = frames
        .into_iter()
        .map(|frame| {
            (
                frame
                    .pts_90k
                    .expect("display-order frames should receive pts"),
                frame.decode_info_flags.expect("test marker should be kept"),
            )
        })
        .collect();

    assert_eq!(
        pts_and_markers,
        vec![(9_000, 10), (12_000, 20), (15_000, 30)]
    );
}

#[test]
fn hevc_display_order_sorting_reorders_repository_decode_order_pocs() {
    let sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("foreman_cif.h265");
    let bitstream = std::fs::read(sample_path).expect("foreman_cif.h265 should be readable");
    let headers = extract_hevc_access_unit_headers(&bitstream)
        .expect("HEVC access-unit headers should parse");
    let first_gop_pocs = headers
        .iter()
        .take(5)
        .map(|header| header.poc_full)
        .collect::<Vec<_>>();
    assert_eq!(
        first_gop_pocs,
        vec![0, 4, 2, 1, 3],
        "repository sample should exercise display-order sorting"
    );

    let frames_with_poc = first_gop_pocs
        .iter()
        .map(|poc| {
            let mut frame = metadata_only_frame(2, 2, None);
            frame.decode_info_flags = Some(*poc as u32);
            (*poc, frame)
        })
        .collect::<Vec<_>>();
    let frames = sort_hevc_display_order_frames(frames_with_poc, 0, decode_pts_step(30));

    let pocs_and_pts = frames
        .into_iter()
        .map(|frame| {
            (
                frame.decode_info_flags.expect("poc marker should remain"),
                frame.pts_90k.expect("display-order PTS should be assigned"),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        pocs_and_pts,
        vec![(0, 0), (1, 3_000), (2, 6_000), (3, 9_000), (4, 12_000)]
    );
}

fn av1_test_obu(obu_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![(obu_type << 3) | 0x02];
    out.extend(av1_test_leb128(payload.len()));
    out.extend_from_slice(payload);
    out
}

fn av1_test_leb128(mut value: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
    out
}

fn av1_test_sequence_header_payload(width: u32, height: u32) -> Vec<u8> {
    let width_minus_1 = width.checked_sub(1).expect("width must be positive");
    let height_minus_1 = height.checked_sub(1).expect("height must be positive");
    let width_bits = 32 - width_minus_1.leading_zeros();
    let height_bits = 32 - height_minus_1.leading_zeros();
    let mut writer = Av1TestBitWriter::default();
    writer.write_bits(0, 3);
    writer.write_bits(1, 1);
    writer.write_bits(1, 1);
    writer.write_bits(0, 5);
    writer.write_bits(u64::from(width_bits - 1), 4);
    writer.write_bits(u64::from(height_bits - 1), 4);
    writer.write_bits(u64::from(width_minus_1), width_bits as usize);
    writer.write_bits(u64::from(height_minus_1), height_bits as usize);
    writer.write_bits(0, 1);
    writer.write_bits(0, 1);
    writer.write_bits(0, 1);
    writer.finish()
}

#[derive(Default)]
struct Av1TestBitWriter {
    data: Vec<u8>,
    bit_offset: usize,
}

impl Av1TestBitWriter {
    fn write_bits(&mut self, value: u64, count: usize) {
        for shift in (0..count).rev() {
            if self.bit_offset.is_multiple_of(8) {
                self.data.push(0);
            }
            let bit = ((value >> shift) & 1) as u8;
            let byte = self.data.last_mut().expect("byte exists after push");
            *byte |= bit << (7 - (self.bit_offset % 8));
            self.bit_offset += 1;
        }
    }

    fn finish(self) -> Vec<u8> {
        self.data
    }
}
