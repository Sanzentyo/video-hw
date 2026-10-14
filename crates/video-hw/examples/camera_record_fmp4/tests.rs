//! Unit tests for codec sample entries and finalized fMP4 timing.

use std::{num::NonZeroU32, time::Duration};

use shiguredo_mp4::{
    Decode, Mp4FileTime, TrackKind,
    boxes::{FtypBox, MoovBox, SampleEntry},
    mux::{Fmp4SegmentMuxer, Sample},
};

use super::codec_samples::{
    convert_duration_timescale, create_h264_sample_entry, patch_fmp4_init_segment_timing,
};

#[test]
fn duration_rescale_uses_wide_intermediate_arithmetic() {
    let three = NonZeroU32::new(3).expect("non-zero timescale");
    let two = NonZeroU32::new(2).expect("non-zero timescale");

    assert_eq!(
        convert_duration_timescale(u64::MAX, three, two),
        12_297_829_382_473_034_410
    );
}

#[test]
fn h264_sample_entry_sets_avcc_extended_fields() {
    let sample_entry = create_h264_sample_entry(
        1280,
        720,
        &[0x67, 0x64, 0x00, 0x1f],
        &[0x68, 0xee, 0x3c, 0x80],
    );

    let SampleEntry::Avc1(avc1) = sample_entry else {
        panic!("expected avc1 sample entry");
    };

    assert!(avc1.avcc_box.chroma_format.is_some());
    assert!(avc1.avcc_box.bit_depth_luma_minus8.is_some());
    assert!(avc1.avcc_box.bit_depth_chroma_minus8.is_some());
}

#[test]
fn h264_sample_entry_allows_init_segment_build() {
    let sample_entry = create_h264_sample_entry(
        1280,
        720,
        &[0x67, 0x64, 0x00, 0x1f],
        &[0x68, 0xee, 0x3c, 0x80],
    );
    let mut muxer = Fmp4SegmentMuxer::new().expect("muxer should initialize");
    let sample = Sample {
        track_kind: TrackKind::Video,
        sample_entry: Some(sample_entry),
        keyframe: true,
        timescale: NonZeroU32::new(90_000).expect("constant timescale is non-zero"),
        duration: 3_000,
        composition_time_offset: None,
        data_offset: 0,
        data_size: 4,
    };
    muxer
        .create_media_segment_metadata(&[sample])
        .expect("sample metadata should initialize track");

    let init = muxer
        .init_segment_bytes()
        .expect("init segment should be buildable");

    assert!(!init.is_empty());
}

#[test]
fn patch_fmp4_init_segment_timing_sets_creation_and_duration() {
    let sample_entry = create_h264_sample_entry(
        1280,
        720,
        &[0x67, 0x64, 0x00, 0x1f],
        &[0x68, 0xee, 0x3c, 0x80],
    );
    let mut muxer = Fmp4SegmentMuxer::new().expect("muxer should initialize");
    let first_sample = Sample {
        track_kind: TrackKind::Video,
        sample_entry: Some(sample_entry),
        keyframe: true,
        timescale: NonZeroU32::new(90_000).expect("constant timescale is non-zero"),
        duration: 3_000,
        composition_time_offset: None,
        data_offset: 0,
        data_size: 4,
    };
    let second_sample = Sample {
        sample_entry: None,
        data_offset: 4,
        ..first_sample.clone()
    };
    muxer
        .create_media_segment_metadata(&[first_sample, second_sample])
        .expect("sample metadata should initialize track");
    let init = muxer
        .init_segment_bytes()
        .expect("init segment should be buildable");

    let creation_timestamp = Duration::from_secs(1_700_000_000);
    let patched = patch_fmp4_init_segment_timing(
        &init,
        creation_timestamp,
        6_000,
        NonZeroU32::new(90_000).expect("constant timescale is non-zero"),
    )
    .expect("init patch should succeed");

    let (_, ftyp_size) = FtypBox::decode(&patched).expect("ftyp should decode");
    let (moov, _) = MoovBox::decode(&patched[ftyp_size..]).expect("moov should decode");
    let expected_creation = Mp4FileTime::from_unix_time(creation_timestamp).as_secs();

    assert_eq!(moov.mvhd_box.creation_time.as_secs(), expected_creation);
    assert_eq!(moov.mvhd_box.modification_time.as_secs(), expected_creation);
    assert_eq!(moov.mvhd_box.duration, 0);

    for trak in &moov.trak_boxes {
        assert_eq!(trak.tkhd_box.creation_time.as_secs(), expected_creation);
        assert_eq!(trak.tkhd_box.modification_time.as_secs(), expected_creation);
        assert_eq!(trak.tkhd_box.duration, 0);
        assert_eq!(
            trak.mdia_box.mdhd_box.creation_time.as_secs(),
            expected_creation
        );
        assert_eq!(
            trak.mdia_box.mdhd_box.modification_time.as_secs(),
            expected_creation
        );
        assert_eq!(trak.mdia_box.mdhd_box.duration, 0);
    }

    let mehd_duration = moov
        .mvex_box
        .and_then(|mvex| mvex.mehd_box)
        .map(|mehd| mehd.fragment_duration)
        .expect("mehd should exist");
    assert_eq!(
        mehd_duration,
        convert_duration_timescale(
            6_000,
            NonZeroU32::new(90_000).expect("constant timescale is non-zero"),
            moov.mvhd_box.timescale,
        )
    );
}
