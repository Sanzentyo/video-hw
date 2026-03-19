#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use std::{
    num::{NonZeroU32, NonZeroUsize},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use anyhow::Result;
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use video_hw::{Backend, Codec};

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use crate::{
    Fmp4Reader, Fmp4ReaderConfig, Fmp4Writer, Fmp4WriterConfig, FragmentFrames, FrameRate,
    FrameSize, Pts90k, Ready, RgbaFrame,
};

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[test]
fn sync_writer_and_reader_roundtrip_h264() -> Result<()> {
    let frame_size = FrameSize::new(
        NonZeroU32::new(128).expect("non-zero width"),
        NonZeroU32::new(72).expect("non-zero height"),
    );
    let output_path = std::env::temp_dir().join(format!(
        "video-hw-fmp4-roundtrip-{}.mp4",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos()
    ));
    let config = Fmp4WriterConfig {
        output_path: output_path.clone(),
        frame_size,
        frame_rate: FrameRate::new(NonZeroU32::new(30).expect("non-zero fps")),
        backend: Backend::Auto,
        codec: Codec::H264,
        require_hardware: false,
        intel_force_software: false,
        fragment_frames: FragmentFrames::new(
            NonZeroUsize::new(10).expect("non-zero fragment frames"),
        ),
    };

    let mut writer = Fmp4Writer::<Ready>::new(config).into_sync_session()?;
    for index in 0..30_u64 {
        let frame = RgbaFrame::new(make_rgba(frame_size, index as u8), frame_size)?;
        writer.write_rgba(frame, Pts90k::new(index * 3_000))?;
    }
    let summary = writer.finish()?.into_summary();
    assert!(summary.bytes_written > 0);
    assert_eq!(summary.packets_seen, 30);

    let mut reader = Fmp4Reader::new(Fmp4ReaderConfig {
        input_path: output_path.clone(),
    })
    .into_sync_session()?;
    assert_eq!(reader.tracks().len(), 1);

    let mut samples = 0_u64;
    while let Some(sample) = reader.next_sample()? {
        if samples == 0 {
            assert!(sample.keyframe);
            assert_eq!(sample.codec(), Some(Codec::H264));
            assert!(!sample.to_annexb()?.is_empty());
            assert_eq!(reader.tracks()[0].codec(), Some(Codec::H264));
        }
        samples = samples.saturating_add(1);
    }
    assert_eq!(samples, 30);

    let _ = std::fs::remove_file(output_path);
    Ok(())
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
#[test]
fn sync_writer_and_reader_roundtrip_hevc() -> Result<()> {
    let frame_size = FrameSize::new(
        NonZeroU32::new(128).expect("non-zero width"),
        NonZeroU32::new(72).expect("non-zero height"),
    );
    let output_path = std::env::temp_dir().join(format!(
        "video-hw-fmp4-roundtrip-hevc-{}.mp4",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos()
    ));
    let config = Fmp4WriterConfig {
        output_path: output_path.clone(),
        frame_size,
        frame_rate: FrameRate::new(NonZeroU32::new(30).expect("non-zero fps")),
        backend: Backend::Auto,
        codec: Codec::Hevc,
        require_hardware: false,
        intel_force_software: false,
        fragment_frames: FragmentFrames::new(
            NonZeroUsize::new(10).expect("non-zero fragment frames"),
        ),
    };

    let mut writer = Fmp4Writer::<Ready>::new(config).into_sync_session()?;
    for index in 0..30_u64 {
        let frame = RgbaFrame::new(make_rgba(frame_size, index as u8), frame_size)?;
        writer.write_rgba(frame, Pts90k::new(index * 3_000))?;
    }
    let summary = writer.finish()?.into_summary();
    assert!(summary.bytes_written > 0);
    assert_eq!(summary.packets_seen, 30);

    let mut reader = Fmp4Reader::new(Fmp4ReaderConfig {
        input_path: output_path.clone(),
    })
    .into_sync_session()?;
    assert_eq!(reader.tracks().len(), 1);

    let mut samples = 0_u64;
    while let Some(sample) = reader.next_sample()? {
        if samples == 0 {
            assert!(sample.keyframe);
            assert_eq!(sample.codec(), Some(Codec::Hevc));
            assert!(!sample.to_annexb()?.is_empty());
            assert_eq!(reader.tracks()[0].codec(), Some(Codec::Hevc));
        }
        samples = samples.saturating_add(1);
    }
    assert_eq!(samples, 30);

    let _ = std::fs::remove_file(output_path);
    Ok(())
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
fn make_rgba(size: FrameSize, seed: u8) -> Vec<u8> {
    let width = size.width().get() as usize;
    let height = size.height().get() as usize;
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            rgba.extend_from_slice(&[
                ((x + seed as usize) % 256) as u8,
                ((y * 2 + seed as usize) % 256) as u8,
                (((x + y) * 3 + seed as usize) % 256) as u8,
                255,
            ]);
        }
    }
    rgba
}
