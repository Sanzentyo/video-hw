use std::error::Error;
use std::fs;
use std::num::NonZeroU32;

use video_hw::{
    AndroidDecoderOptions, AndroidEncoderOptions, AnyDecodeSession, AnyEncodeSession, Backend,
    BackendDecoderOptions, BackendEncoderOptions, BitstreamInput, Codec, DecodeOutputMode,
    DecodedFrame, DecoderConfig, Dimensions, EncodeFrame, EncodeInputFormat, EncodedChunk,
    EncoderConfig, RawFrameBuffer, Timestamp90k,
};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;
const FPS: i32 = 30;
const FRAME_COUNT: usize = 30;

fn main() -> Result<(), Box<dyn Error>> {
    let encoded = encode_h264()?;
    fs::write(
        "/data/local/tmp/video_hw_rust_roundtrip.h264",
        encoded
            .iter()
            .flat_map(|chunk| chunk.data.iter().copied())
            .collect::<Vec<_>>(),
    )?;
    let decoded_frames = decode_h264_metadata(&encoded)?;
    let encoded_bytes = encoded.iter().map(|chunk| chunk.data.len()).sum::<usize>();
    let keyframes = encoded.iter().filter(|chunk| chunk.is_keyframe).count();
    let status = if decoded_frames == FRAME_COUNT {
        "PASS"
    } else {
        "FAIL"
    };

    println!(
        "{{\"codec\":\"h264\",\"width\":{},\"height\":{},\"frames_in\":{},\"encoded_packets\":{},\"encoded_bytes\":{},\"keyframes\":{},\"decoded_frames\":{},\"status\":\"{}\"}}",
        WIDTH,
        HEIGHT,
        FRAME_COUNT,
        encoded.len(),
        encoded_bytes,
        keyframes,
        decoded_frames,
        status
    );
    if status == "PASS" {
        Ok(())
    } else {
        Err(format!("decoded {decoded_frames} frames, expected {FRAME_COUNT}").into())
    }
}

fn encode_h264() -> Result<Vec<EncodedChunk>, Box<dyn Error>> {
    let mut config = EncoderConfig::new(Codec::H264, FPS, false, EncodeInputFormat::Argb8888);
    config.backend_options = BackendEncoderOptions::Android(AndroidEncoderOptions {
        bitrate: Some(1_000_000),
        ..Default::default()
    });
    let mut encoder = AnyEncodeSession::new(Backend::Android, config)?;
    let dims = Dimensions {
        width: NonZeroU32::new(WIDTH).expect("non-zero width"),
        height: NonZeroU32::new(HEIGHT).expect("non-zero height"),
    };
    for frame_index in 0..FRAME_COUNT {
        encoder.submit(EncodeFrame {
            dims,
            pts_90k: Some(Timestamp90k(frame_index as i64 * 90_000 / i64::from(FPS))),
            buffer: RawFrameBuffer::Argb8888(synthetic_argb(frame_index)),
            force_keyframe: frame_index == 0,
        })?;
    }
    Ok(encoder.flush()?)
}

fn decode_h264_metadata(encoded: &[EncodedChunk]) -> Result<usize, Box<dyn Error>> {
    let mut config = DecoderConfig::new(Codec::H264, FPS, false);
    config.output_mode = DecodeOutputMode::Metadata;
    config.backend_options = BackendDecoderOptions::Android(AndroidDecoderOptions {
        video_width: Some(WIDTH as u16),
        video_height: Some(HEIGHT as u16),
        ..Default::default()
    });
    let mut decoder = AnyDecodeSession::new(Backend::Android, config)?;
    for chunk in encoded {
        decoder.submit(BitstreamInput::AnnexBChunk {
            chunk: chunk.data.clone(),
            pts_90k: chunk.pts_90k,
        })?;
    }
    Ok(decoder
        .flush()?
        .iter()
        .filter(|frame| matches!(frame, DecodedFrame::Metadata { .. }))
        .count())
}

fn synthetic_argb(frame_index: usize) -> Vec<u8> {
    let mut out = vec![0_u8; WIDTH as usize * HEIGHT as usize * 4];
    for y in 0..HEIGHT as usize {
        for x in 0..WIDTH as usize {
            let i = (y * WIDTH as usize + x) * 4;
            out[i] = 255;
            out[i + 1] = ((x + frame_index * 7) & 0xff) as u8;
            out[i + 2] = ((y + frame_index * 3) & 0xff) as u8;
            out[i + 3] = ((x + y + frame_index * 5) & 0xff) as u8;
        }
    }
    out
}
