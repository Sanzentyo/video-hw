use std::{fs, path::PathBuf};

use backend_contract::{Codec, DecoderConfig, Frame, VideoDecoder, VideoEncoder};
use vt_backend::{VtDecoderAdapter, VtEncoderAdapter};

#[cfg(target_os = "macos")]
fn sample_path(name: &str) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.join("../../../sample-videos").join(name)
}

#[cfg(target_os = "macos")]
fn decode_count(codec: Codec, file_name: &str, chunk_bytes: usize) -> usize {
    let mut decoder = VtDecoderAdapter::new(DecoderConfig {
        codec,
        fps: 30,
        require_hardware: false,
    });

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

#[cfg(target_os = "macos")]
#[test]
fn e2e_decode_h264_chunk_4096() {
    let decoded = decode_count(Codec::H264, "sample-10s.h264", 4096);
    assert_eq!(decoded, 303);
}

#[cfg(target_os = "macos")]
#[test]
fn e2e_decode_hevc_chunk_4096() {
    let decoded = decode_count(Codec::Hevc, "sample-10s.h265", 4096);
    assert_eq!(decoded, 303);
}

#[cfg(target_os = "macos")]
#[test]
fn e2e_decode_h264_chunk_1mb() {
    let decoded = decode_count(Codec::H264, "sample-10s.h264", 1024 * 1024);
    assert_eq!(decoded, 303);
}

#[cfg(target_os = "macos")]
#[test]
fn e2e_decode_hevc_chunk_1mb() {
    let decoded = decode_count(Codec::Hevc, "sample-10s.h265", 1024 * 1024);
    assert_eq!(decoded, 303);
}

#[cfg(target_os = "macos")]
#[test]
fn e2e_encode_vt_h264_generates_packets() {
    let mut encoder = VtEncoderAdapter::with_config(Codec::H264, 30, false);

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
