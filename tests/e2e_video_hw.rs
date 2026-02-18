use std::{fs, path::PathBuf};

use rstest::rstest;
use video_hw::{BackendKind, Codec, Decoder, DecoderConfig, Encoder, Frame};

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
fn sample_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("sample-videos")
        .join(name)
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
fn decode_count(codec: Codec, file_name: &str, chunk_bytes: usize) -> usize {
    let mut decoder = Decoder::new(
        BackendKind::VideoToolbox,
        DecoderConfig {
            codec,
            fps: 30,
            require_hardware: false,
        },
    );

    let path = sample_path(file_name);
    let data = fs::read(&path).expect("sample bitstream should exist");

    let mut total = 0usize;
    for chunk in data.chunks(chunk_bytes) {
        let frames = decoder
            .push_bitstream_chunk(chunk, None)
            .expect("decode chunk should succeed");
        total += frames.len();
    }

    total += decoder.flush().expect("flush should succeed").len();
    total
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
fn decode_total_and_summary(codec: Codec, file_name: &str, chunk_bytes: usize) -> (usize, usize) {
    let mut decoder = Decoder::new(
        BackendKind::VideoToolbox,
        DecoderConfig {
            codec,
            fps: 30,
            require_hardware: false,
        },
    );

    let path = sample_path(file_name);
    let data = fs::read(&path).expect("sample bitstream should exist");

    let mut total = 0usize;
    for chunk in data.chunks(chunk_bytes.max(1)) {
        total += decoder
            .push_bitstream_chunk(chunk, None)
            .expect("decode chunk should succeed")
            .len();
    }

    total += decoder.flush().expect("flush should succeed").len();
    let summary = decoder.decode_summary();
    (total, summary.decoded_frames)
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[rstest]
#[case(Codec::H264, "sample-10s.h264", 4096)]
#[case(Codec::H264, "sample-10s.h264", 1024 * 1024)]
#[case(Codec::Hevc, "sample-10s.h265", 4096)]
#[case(Codec::Hevc, "sample-10s.h265", 1024 * 1024)]
fn e2e_decode_expected_frames_matrix(
    #[case] codec: Codec,
    #[case] file_name: &str,
    #[case] chunk_bytes: usize,
) {
    let decoded = decode_count(codec, file_name, chunk_bytes);
    assert_eq!(decoded, 303);
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[rstest]
#[case(Codec::H264, "sample-10s.h264")]
#[case(Codec::Hevc, "sample-10s.h265")]
fn e2e_decode_summary_matches_observed_frames(#[case] codec: Codec, #[case] file_name: &str) {
    let (observed, summary_total) = decode_total_and_summary(codec, file_name, 4096);
    assert_eq!(observed, 303);
    assert_eq!(summary_total, observed);
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[test]
fn e2e_decode_flush_without_input_is_empty() {
    let mut decoder = Decoder::new(
        BackendKind::VideoToolbox,
        DecoderConfig {
            codec: Codec::H264,
            fps: 30,
            require_hardware: false,
        },
    );

    let flushed = decoder.flush().expect("flush should succeed");
    assert!(flushed.is_empty());
    assert_eq!(decoder.decode_summary().decoded_frames, 0);
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[test]
fn e2e_encode_h264_generates_packets() {
    let mut encoder = Encoder::new(BackendKind::VideoToolbox, Codec::H264, 30, false);

    for i in 0..30 {
        let frame = Frame {
            width: 640,
            height: 360,
            pixel_format: None,
            pts_90k: Some(i * 3000),
        };
        let packets = encoder
            .push_frame(frame)
            .expect("push_frame should succeed");
        assert!(packets.is_empty());
    }

    let packets = encoder.flush().expect("flush should succeed");
    assert!(!packets.is_empty());
}

#[cfg(not(all(target_os = "macos", feature = "backend-vt")))]
#[test]
fn e2e_vt_backend_reports_unsupported_when_unavailable() {
    let mut decoder = Decoder::new(
        BackendKind::VideoToolbox,
        DecoderConfig {
            codec: Codec::H264,
            fps: 30,
            require_hardware: false,
        },
    );

    let capability = decoder
        .query_capability(Codec::H264)
        .expect("capability query should not fail");
    assert!(!capability.decode_supported);

    match decoder.push_bitstream_chunk(&[0, 0, 1], None) {
        Err(video_hw::BackendError::UnsupportedConfig(message)) => {
            assert!(message.contains("VideoToolbox backend requires"));
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[cfg(feature = "backend-nvidia")]
#[test]
fn e2e_nvidia_backend_reports_unwired_bridge() {
    let mut decoder = Decoder::new(
        BackendKind::Nvidia,
        DecoderConfig {
            codec: Codec::H264,
            fps: 30,
            require_hardware: true,
        },
    );

    let capability = decoder
        .query_capability(Codec::H264)
        .expect("capability query should not fail");
    assert!(capability.decode_supported);
    assert!(capability.hardware_acceleration);

    match decoder.push_bitstream_chunk(&[0, 0, 1], Some(0)) {
        Err(video_hw::BackendError::UnsupportedConfig(message)) => {
            assert!(message.contains("nvidia-sdk bridge is not wired yet"));
        }
        other => panic!("unexpected result: {other:?}"),
    }

    let mut encoder = Encoder::new(BackendKind::Nvidia, Codec::H264, 30, true);
    let frame = Frame {
        width: 640,
        height: 360,
        pixel_format: None,
        pts_90k: Some(0),
    };
    match encoder.push_frame(frame) {
        Err(video_hw::BackendError::UnsupportedConfig(message)) => {
            assert!(message.contains("nvidia-sdk bridge is not wired yet"));
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[cfg(not(feature = "backend-nvidia"))]
#[test]
fn e2e_nvidia_backend_requires_feature_when_disabled() {
    let mut decoder = Decoder::new(
        BackendKind::Nvidia,
        DecoderConfig {
            codec: Codec::H264,
            fps: 30,
            require_hardware: true,
        },
    );

    let capability = decoder
        .query_capability(Codec::H264)
        .expect("capability query should not fail");
    assert!(!capability.decode_supported);

    match decoder.push_bitstream_chunk(&[0, 0, 1], Some(0)) {
        Err(video_hw::BackendError::UnsupportedConfig(message)) => {
            assert!(message.contains("NVIDIA backend requires backend-nvidia feature"));
        }
        other => panic!("unexpected result: {other:?}"),
    }
}
