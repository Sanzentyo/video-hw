#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    feature = "backend-nvidia"
))]
use std::{fs, path::PathBuf};

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use rstest::rstest;
use video_hw::{BackendDecoderOptions, BackendKind, Codec, Decoder, DecoderConfig};
#[cfg(feature = "backend-nvidia")]
use video_hw::NvidiaSessionConfig;
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use video_hw::VtSessionConfig;
#[cfg(feature = "backend-nvidia")]
use video_hw::{BackendEncoderOptions, EncoderConfig, NvidiaEncoderOptions};
#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    feature = "backend-nvidia"
))]
use video_hw::{Encoder, Frame};
#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    feature = "backend-nvidia"
))]
use video_hw::{SessionSwitchMode, SessionSwitchRequest};

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
            backend_options: BackendDecoderOptions::Default,
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
            backend_options: BackendDecoderOptions::Default,
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
            backend_options: BackendDecoderOptions::Default,
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
            argb: None,
            force_keyframe: false,
        };
        let packets = encoder
            .push_frame(frame)
            .expect("push_frame should succeed");
        assert!(packets.is_empty());
    }

    let packets = encoder.flush().expect("flush should succeed");
    assert!(!packets.is_empty());
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[test]
fn e2e_encode_h264_rejects_invalid_argb_payload() {
    let mut encoder = Encoder::new(BackendKind::VideoToolbox, Codec::H264, 30, false);
    let bad_frame = Frame {
        width: 640,
        height: 360,
        pixel_format: None,
        pts_90k: Some(0),
        argb: Some(vec![0_u8; 16]),
        force_keyframe: false,
    };

    let result = encoder.push_frame(bad_frame);
    match result {
        Err(video_hw::BackendError::InvalidInput(message)) => {
            assert!(message.contains("argb payload size mismatch"));
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[test]
fn e2e_encode_h264_packets_are_pts_monotonic() {
    let mut encoder = Encoder::new(BackendKind::VideoToolbox, Codec::H264, 30, false);

    for i in 0..30 {
        let frame = Frame {
            width: 640,
            height: 360,
            pixel_format: None,
            pts_90k: Some(i * 3000),
            argb: None,
            force_keyframe: i == 10,
        };
        let packets = encoder
            .push_frame(frame)
            .expect("push_frame should succeed");
        assert!(packets.is_empty());
    }

    let packets = encoder.flush().expect("flush should succeed");
    assert!(!packets.is_empty());

    let pts_list: Vec<i64> = packets.iter().filter_map(|p| p.pts_90k).collect();
    assert!(!pts_list.is_empty(), "encoded packets must include pts");
    assert!(
        pts_list.windows(2).all(|w| w[0] <= w[1]),
        "packet pts must be monotonic non-decreasing: {pts_list:?}"
    );
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[test]
fn e2e_vt_backend_accepts_explicit_session_switch_request() {
    let mut encoder = Encoder::new(BackendKind::VideoToolbox, Codec::H264, 30, false);
    let result = encoder.request_session_switch(SessionSwitchRequest::VideoToolbox {
        config: VtSessionConfig {
            force_keyframe_on_activate: true,
        },
        mode: SessionSwitchMode::Immediate,
    });
    assert!(result.is_ok());
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
            backend_options: BackendDecoderOptions::Default,
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
fn e2e_nv_backend_decode_and_encode_work() {
    let mut decoder = Decoder::new(
        BackendKind::Nvidia,
        DecoderConfig {
            codec: Codec::H264,
            fps: 30,
            require_hardware: true,
            backend_options: BackendDecoderOptions::Default,
        },
    );

    let capability = decoder
        .query_capability(Codec::H264)
        .expect("capability query should not fail");
    assert!(capability.decode_supported);
    assert!(capability.encode_supported);
    assert!(capability.hardware_acceleration);

    let data = fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("sample-videos")
            .join("sample-10s.h264"),
    )
    .expect("sample bitstream should exist");
    let mut decoded_frames = 0usize;
    for chunk in data.chunks(4096) {
        match decoder.push_bitstream_chunk(chunk, None) {
            Ok(frames) => decoded_frames += frames.len(),
            Err(video_hw::BackendError::UnsupportedConfig(message))
                if message.contains("CUDA context") =>
            {
                eprintln!("skip: CUDA/NVDEC unavailable: {message}");
                return;
            }
            Err(err) => panic!("unexpected decode error: {err:?}"),
        }
    }
    decoded_frames += decoder.flush().expect("flush should succeed").len();
    assert!(decoded_frames > 0);
    assert_eq!(decoder.decode_summary().decoded_frames, decoded_frames);

    let mut encoder = Encoder::new(BackendKind::Nvidia, Codec::H264, 30, true);
    for i in 0..30 {
        encoder
            .push_frame(Frame {
                width: 640,
                height: 360,
                pixel_format: None,
                pts_90k: Some(i * 3000),
                argb: None,
                force_keyframe: false,
            })
            .expect("push_frame should succeed");
    }
    match encoder.flush() {
        Ok(packets) => assert!(!packets.is_empty()),
        Err(video_hw::BackendError::UnsupportedConfig(message))
            if message.contains("CUDA context") =>
        {
            eprintln!("skip: CUDA/NVENC unavailable: {message}");
        }
        Err(err) => panic!("unexpected encode error: {err:?}"),
    }
}

#[cfg(feature = "backend-nvidia")]
#[test]
fn e2e_nv_backend_hevc_decode_sample() {
    let mut decoder = Decoder::new(
        BackendKind::Nvidia,
        DecoderConfig {
            codec: Codec::Hevc,
            fps: 30,
            require_hardware: true,
            backend_options: BackendDecoderOptions::Default,
        },
    );

    let data = fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("sample-videos")
            .join("sample-10s.h265"),
    )
    .expect("sample bitstream should exist");
    let mut decoded_frames = 0usize;
    for chunk in data.chunks(4096) {
        match decoder.push_bitstream_chunk(chunk, None) {
            Ok(frames) => decoded_frames += frames.len(),
            Err(video_hw::BackendError::UnsupportedConfig(message))
                if message.contains("CUDA context") || message.contains("unsupported") =>
            {
                eprintln!("skip: HEVC decode unsupported on this machine: {message}");
                return;
            }
            Err(err) => panic!("unexpected decode error: {err:?}"),
        }
    }

    match decoder.flush() {
        Ok(frames) => decoded_frames += frames.len(),
        Err(video_hw::BackendError::UnsupportedConfig(message))
            if message.contains("CUDA context") || message.contains("unsupported") =>
        {
            eprintln!("skip: HEVC decode unsupported on this machine: {message}");
            return;
        }
        Err(err) => panic!("unexpected decode flush error: {err:?}"),
    }

    assert!(decoded_frames > 0);
    assert_eq!(decoder.decode_summary().decoded_frames, decoded_frames);
}

#[cfg(feature = "backend-nvidia")]
#[test]
fn e2e_nv_backend_encode_accepts_backend_specific_options() {
    let mut config = EncoderConfig::new(Codec::H264, 30, true);
    config.backend_options = BackendEncoderOptions::Nvidia(NvidiaEncoderOptions {
        max_in_flight_outputs: 4,
        gop_length: None,
        frame_interval_p: None,
        ..Default::default()
    });
    let mut encoder = Encoder::with_config(BackendKind::Nvidia, config);

    for i in 0..30 {
        match encoder.push_frame(Frame {
            width: 640,
            height: 360,
            pixel_format: None,
            pts_90k: Some(i * 3000),
            argb: None,
            force_keyframe: false,
        }) {
            Ok(_) => {}
            Err(video_hw::BackendError::UnsupportedConfig(message))
                if message.contains("CUDA context") =>
            {
                eprintln!("skip: CUDA/NVENC unavailable: {message}");
                return;
            }
            Err(err) => panic!("unexpected encode error: {err:?}"),
        }
    }

    match encoder.flush() {
        Ok(packets) => assert!(!packets.is_empty()),
        Err(video_hw::BackendError::UnsupportedConfig(message))
            if message.contains("CUDA context") =>
        {
            eprintln!("skip: CUDA/NVENC unavailable: {message}");
        }
        Err(err) => panic!("unexpected encode flush error: {err:?}"),
    }
}

#[cfg(feature = "backend-nvidia")]
#[test]
fn e2e_nv_backend_accepts_explicit_session_switch_request() {
    let mut encoder = Encoder::new(BackendKind::Nvidia, Codec::H264, 30, true);
    let result = encoder.request_session_switch(SessionSwitchRequest::Nvidia {
        config: NvidiaSessionConfig {
            gop_length: Some(60),
            frame_interval_p: Some(1),
            force_idr_on_activate: true,
        },
        mode: SessionSwitchMode::Immediate,
    });
    assert!(result.is_ok());
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
            backend_options: BackendDecoderOptions::Default,
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
