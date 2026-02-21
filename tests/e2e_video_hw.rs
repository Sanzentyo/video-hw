#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
use std::{fs, path::PathBuf};

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
use rstest::rstest;
#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
use video_hw::EncoderConfig;
#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
use video_hw::NvidiaSessionConfig;
#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
use video_hw::Timestamp90k;
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use video_hw::VtSessionConfig;
#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
use video_hw::{
    Backend, BackendDecoderOptions, BackendError, BitstreamInput, Codec, DecodeSession,
    DecoderConfig,
};
#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
use video_hw::{BackendEncoderOptions, NvidiaEncoderOptions};
#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
use video_hw::{Dimensions, EncodeFrame, EncodeSession, RawFrameBuffer};
#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
use video_hw::{SessionSwitchMode, SessionSwitchRequest};
#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
fn dims_640_360() -> Dimensions {
    Dimensions {
        width: std::num::NonZeroU32::new(640).expect("non-zero width"),
        height: std::num::NonZeroU32::new(360).expect("non-zero height"),
    }
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
fn make_argb_frame(index: i64) -> EncodeFrame {
    let dims = dims_640_360();
    let pixel_count = dims.width.get() as usize * dims.height.get() as usize;
    let mut argb = vec![0_u8; pixel_count * 4];
    for px in argb.chunks_exact_mut(4) {
        px[0] = 255;
        px[1] = (index as usize % 255) as u8;
        px[2] = 96;
        px[3] = 192;
    }
    EncodeFrame {
        dims,
        pts_90k: Some(video_hw::Timestamp90k(index * 3000)),
        buffer: RawFrameBuffer::Argb8888(argb),
        force_keyframe: index == 0,
    }
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
fn sample_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("sample-videos")
        .join(name)
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
fn decode_count(
    backend: Backend,
    codec: Codec,
    file_name: &str,
    chunk_bytes: usize,
    require_hardware: bool,
) -> Result<usize, BackendError> {
    let mut decoder = DecodeSession::new(
        backend,
        DecoderConfig {
            codec,
            fps: 30,
            require_hardware,
            backend_options: BackendDecoderOptions::Default,
        },
    );

    let path = sample_path(file_name);
    let data = fs::read(&path).expect("sample bitstream should exist");

    let mut total = 0usize;
    for chunk in data.chunks(chunk_bytes) {
        decoder.submit(BitstreamInput::AnnexBChunk {
            chunk: chunk.to_vec(),
            pts_90k: None,
        })?;
        while decoder.try_reap()?.is_some() {
            total += 1;
        }
    }

    total += decoder.flush()?.len();
    Ok(total)
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
fn decode_total_and_summary(
    backend: Backend,
    codec: Codec,
    file_name: &str,
    chunk_bytes: usize,
    require_hardware: bool,
) -> Result<(usize, usize), BackendError> {
    let mut decoder = DecodeSession::new(
        backend,
        DecoderConfig {
            codec,
            fps: 30,
            require_hardware,
            backend_options: BackendDecoderOptions::Default,
        },
    );

    let path = sample_path(file_name);
    let data = fs::read(&path).expect("sample bitstream should exist");

    let mut total = 0usize;
    for chunk in data.chunks(chunk_bytes.max(1)) {
        decoder.submit(BitstreamInput::AnnexBChunk {
            chunk: chunk.to_vec(),
            pts_90k: None,
        })?;
        while decoder.try_reap()?.is_some() {
            total += 1;
        }
    }

    total += decoder.flush()?.len();
    let summary = decoder.summary();
    Ok((total, summary.decoded_frames))
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
fn nv_runtime_unsupported(err: &BackendError) -> bool {
    match err {
        BackendError::UnsupportedConfig(message) => {
            message.contains("CUDA context") || message.contains("unsupported")
        }
        _ => false,
    }
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
    let decoded = decode_count(Backend::VideoToolbox, codec, file_name, chunk_bytes, false)
        .expect("decode should succeed");
    assert_eq!(decoded, 303);
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[rstest]
#[case(Codec::H264, "sample-10s.h264")]
#[case(Codec::Hevc, "sample-10s.h265")]
fn e2e_decode_summary_matches_observed_frames(#[case] codec: Codec, #[case] file_name: &str) {
    let (observed, summary_total) =
        decode_total_and_summary(Backend::VideoToolbox, codec, file_name, 4096, false)
            .expect("decode should succeed");
    assert_eq!(observed, 303);
    assert_eq!(summary_total, observed);
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
#[rstest]
#[case(Codec::H264, "sample-10s.h264", 4096)]
#[case(Codec::H264, "sample-10s.h264", 1024 * 1024)]
#[case(Codec::Hevc, "sample-10s.h265", 4096)]
#[case(Codec::Hevc, "sample-10s.h265", 1024 * 1024)]
fn e2e_nv_decode_expected_frames_matrix(
    #[case] codec: Codec,
    #[case] file_name: &str,
    #[case] chunk_bytes: usize,
) {
    match decode_count(Backend::Nvidia, codec, file_name, chunk_bytes, true) {
        Ok(decoded) => assert_eq!(decoded, 303),
        Err(err) if nv_runtime_unsupported(&err) => {
            eprintln!("skip: NV decode unavailable: {err}");
        }
        Err(err) => panic!("unexpected NV decode error: {err:?}"),
    }
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
#[rstest]
#[case(Codec::H264, "sample-10s.h264")]
#[case(Codec::Hevc, "sample-10s.h265")]
fn e2e_nv_decode_summary_matches_observed_frames(#[case] codec: Codec, #[case] file_name: &str) {
    match decode_total_and_summary(Backend::Nvidia, codec, file_name, 4096, true) {
        Ok((observed, summary_total)) => {
            assert_eq!(observed, 303);
            assert_eq!(summary_total, observed);
        }
        Err(err) if nv_runtime_unsupported(&err) => {
            eprintln!("skip: NV decode unavailable: {err}");
        }
        Err(err) => panic!("unexpected NV decode error: {err:?}"),
    }
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[test]
fn e2e_vt_decode_metadata_includes_pts_and_decode_flags() {
    let mut decoder = DecodeSession::new(
        Backend::VideoToolbox,
        DecoderConfig {
            codec: Codec::H264,
            fps: 30,
            require_hardware: false,
            backend_options: BackendDecoderOptions::Default,
        },
    );
    let data = fs::read(sample_path("sample-10s.h264")).expect("sample bitstream should exist");

    let mut first = None;
    for chunk in data.chunks(4096) {
        decoder
            .submit(BitstreamInput::AnnexBChunk {
                chunk: chunk.to_vec(),
                pts_90k: None,
            })
            .expect("decode chunk should succeed");
        while let Some(frame) = decoder.try_reap().expect("try_reap should succeed") {
            first = Some(frame);
            break;
        }
        if first.is_some() {
            break;
        }
    }

    let frame = first.expect("expected at least one decoded frame");
    match frame {
        video_hw::DecodedFrame::Metadata {
            pts_90k,
            decode_info_flags,
            color,
            ..
        } => {
            assert!(pts_90k.is_some());
            assert!(decode_info_flags.is_some());
            if let Some(color) = color {
                assert!(
                    color.color_primaries.is_some()
                        || color.transfer_function.is_some()
                        || color.ycbcr_matrix.is_some()
                );
            }
        }
        other => panic!("unexpected decoded frame: {other:?}"),
    }
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[test]
fn e2e_decode_flush_without_input_is_empty() {
    let mut decoder = DecodeSession::new(
        Backend::VideoToolbox,
        DecoderConfig {
            codec: Codec::H264,
            fps: 30,
            require_hardware: false,
            backend_options: BackendDecoderOptions::Default,
        },
    );

    let flushed = decoder.flush().expect("flush should succeed");
    assert!(flushed.is_empty());
    assert_eq!(decoder.summary().decoded_frames, 0);
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
#[test]
fn e2e_nv_decode_flush_without_input_is_empty() {
    let mut decoder = DecodeSession::new(
        Backend::Nvidia,
        DecoderConfig {
            codec: Codec::H264,
            fps: 30,
            require_hardware: true,
            backend_options: BackendDecoderOptions::Default,
        },
    );

    match decoder.flush() {
        Ok(flushed) => {
            assert!(flushed.is_empty());
            assert_eq!(decoder.summary().decoded_frames, 0);
        }
        Err(err) if nv_runtime_unsupported(&err) => {
            eprintln!("skip: NV decode unavailable: {err}");
        }
        Err(err) => panic!("unexpected NV flush error: {err:?}"),
    }
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[test]
fn e2e_encode_h264_generates_packets() {
    let mut encoder = EncodeSession::new(
        Backend::VideoToolbox,
        EncoderConfig::new(Codec::H264, 30, false),
    );

    for i in 0..30 {
        encoder
            .submit(make_argb_frame(i as i64))
            .expect("submit should succeed");
        assert!(
            encoder
                .try_reap()
                .expect("try_reap should succeed")
                .is_none()
        );
    }

    let packets = encoder.flush().expect("flush should succeed");
    assert!(!packets.is_empty());
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[test]
fn e2e_encode_h264_rejects_invalid_argb_payload() {
    let mut encoder = EncodeSession::new(
        Backend::VideoToolbox,
        EncoderConfig::new(Codec::H264, 30, false),
    );
    let bad_frame = EncodeFrame {
        dims: dims_640_360(),
        pts_90k: Some(Timestamp90k(0)),
        buffer: RawFrameBuffer::Argb8888(vec![0_u8; 16]),
        force_keyframe: false,
    };

    let result = encoder.submit(bad_frame);
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
    let mut encoder = EncodeSession::new(
        Backend::VideoToolbox,
        EncoderConfig::new(Codec::H264, 30, false),
    );

    for i in 0..30 {
        let mut frame = make_argb_frame(i as i64);
        frame.force_keyframe = i == 10;
        encoder.submit(frame).expect("submit should succeed");
    }

    let packets = encoder.flush().expect("flush should succeed");
    assert!(!packets.is_empty());

    let pts_list: Vec<i64> = packets
        .iter()
        .filter_map(|p| p.pts_90k.map(|v| v.0))
        .collect();
    assert!(!pts_list.is_empty(), "encoded packets must include pts");
    assert!(
        pts_list.windows(2).all(|w| w[0] <= w[1]),
        "packet pts must be monotonic non-decreasing: {pts_list:?}"
    );
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
#[test]
fn e2e_nv_encode_h264_packets_are_pts_monotonic() {
    let mut encoder =
        EncodeSession::new(Backend::Nvidia, EncoderConfig::new(Codec::H264, 30, true));

    for i in 0..30 {
        let mut frame = make_argb_frame(i as i64);
        frame.force_keyframe = i == 10;
        if let Err(err) = encoder.submit(frame) {
            if nv_runtime_unsupported(&err) {
                eprintln!("skip: CUDA/NVENC unavailable: {err}");
                return;
            }
            panic!("unexpected NV encode submit error: {err:?}");
        }
    }

    match encoder.flush() {
        Ok(packets) => {
            assert!(!packets.is_empty());
            let pts_list: Vec<i64> = packets
                .iter()
                .filter_map(|p| p.pts_90k.map(|v| v.0))
                .collect();
            assert!(!pts_list.is_empty(), "encoded packets must include pts");
            assert!(
                pts_list.windows(2).all(|w| w[0] <= w[1]),
                "packet pts must be monotonic non-decreasing: {pts_list:?}"
            );
        }
        Err(err) if nv_runtime_unsupported(&err) => {
            eprintln!("skip: CUDA/NVENC unavailable: {err}");
        }
        Err(err) => panic!("unexpected NV encode flush error: {err:?}"),
    }
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
#[test]
fn e2e_nv_encode_h264_rejects_invalid_argb_payload() {
    let mut encoder =
        EncodeSession::new(Backend::Nvidia, EncoderConfig::new(Codec::H264, 30, true));
    let bad_frame = EncodeFrame {
        dims: dims_640_360(),
        pts_90k: Some(Timestamp90k(0)),
        buffer: RawFrameBuffer::Argb8888(vec![0_u8; 16]),
        force_keyframe: false,
    };

    encoder
        .submit(bad_frame)
        .expect("submit should enqueue frame before validation");
    match encoder.flush() {
        Err(BackendError::InvalidInput(message)) => {
            assert!(message.contains("argb payload size mismatch"));
        }
        Err(err) if nv_runtime_unsupported(&err) => {
            eprintln!("skip: CUDA/NVENC unavailable: {err}");
        }
        other => panic!("unexpected NV invalid-payload result: {other:?}"),
    }
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[test]
fn e2e_vt_backend_accepts_explicit_session_switch_request() {
    let mut encoder = EncodeSession::new(
        Backend::VideoToolbox,
        EncoderConfig::new(Codec::H264, 30, false),
    );
    let result = encoder.request_session_switch(SessionSwitchRequest::VideoToolbox {
        config: VtSessionConfig {
            force_keyframe_on_activate: true,
        },
        mode: SessionSwitchMode::Immediate,
    });
    assert!(result.is_ok());
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
#[test]
fn e2e_nv_backend_decode_and_encode_work() {
    let mut decoder = DecodeSession::new(
        Backend::Nvidia,
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
        match decoder.submit(BitstreamInput::AnnexBChunk {
            chunk: chunk.to_vec(),
            pts_90k: None,
        }) {
            Ok(()) => {
                while decoder
                    .try_reap()
                    .expect("try_reap should succeed")
                    .is_some()
                {
                    decoded_frames += 1;
                }
            }
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
    assert_eq!(decoder.summary().decoded_frames, decoded_frames);

    let mut encoder =
        EncodeSession::new(Backend::Nvidia, EncoderConfig::new(Codec::H264, 30, true));
    for i in 0..30 {
        encoder
            .submit(make_argb_frame(i as i64))
            .expect("submit should succeed");
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

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
#[test]
fn e2e_nv_backend_hevc_decode_sample() {
    let mut decoder = DecodeSession::new(
        Backend::Nvidia,
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
        match decoder.submit(BitstreamInput::AnnexBChunk {
            chunk: chunk.to_vec(),
            pts_90k: None,
        }) {
            Ok(()) => {
                while decoder
                    .try_reap()
                    .expect("try_reap should succeed")
                    .is_some()
                {
                    decoded_frames += 1;
                }
            }
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
    assert_eq!(decoder.summary().decoded_frames, decoded_frames);
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
#[test]
fn e2e_nv_backend_encode_accepts_backend_specific_options() {
    let mut config = EncoderConfig::new(Codec::H264, 30, true);
    config.backend_options = BackendEncoderOptions::Nvidia(NvidiaEncoderOptions {
        max_in_flight_outputs: 4,
        gop_length: None,
        frame_interval_p: None,
        ..Default::default()
    });
    let mut encoder = EncodeSession::new(Backend::Nvidia, config);

    for i in 0..30 {
        match encoder.submit(make_argb_frame(i as i64)) {
            Ok(()) => {}
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

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
#[test]
fn e2e_nv_backend_accepts_explicit_session_switch_request() {
    let mut encoder =
        EncodeSession::new(Backend::Nvidia, EncoderConfig::new(Codec::H264, 30, true));
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

#[cfg(not(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
)))]
#[test]
fn e2e_build_without_enabled_backends_compiles() {
    assert!(true);
}
