use std::num::NonZeroU32;
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
use std::{
    fs,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use super::*;
use crate::fmp4_reader::{Fmp4ReaderConfig, MediaTime, TrackKind};
use shiguredo_mp4::{
    Uint,
    boxes::{Av01Box, Av1cBox, VisualSampleEntryFields},
};
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
use video_hw::{Codec, DecoderConfig, Dimensions};

#[test]
fn av1_track_options_preserve_fmp4_av1c_record() {
    let entry = Av01Box {
        visual: VisualSampleEntryFields {
            data_reference_index: VisualSampleEntryFields::DEFAULT_DATA_REFERENCE_INDEX,
            width: 320,
            height: 180,
            horizresolution: VisualSampleEntryFields::DEFAULT_HORIZRESOLUTION,
            vertresolution: VisualSampleEntryFields::DEFAULT_VERTRESOLUTION,
            frame_count: VisualSampleEntryFields::DEFAULT_FRAME_COUNT,
            compressorname: VisualSampleEntryFields::NULL_COMPRESSORNAME,
            depth: VisualSampleEntryFields::DEFAULT_DEPTH,
        },
        av1c_box: Av1cBox {
            seq_profile: Uint::new(0),
            seq_level_idx_0: Uint::new(31),
            seq_tier_0: Uint::new(1),
            high_bitdepth: Uint::new(0),
            twelve_bit: Uint::new(0),
            monochrome: Uint::new(0),
            chroma_subsampling_x: Uint::new(1),
            chroma_subsampling_y: Uint::new(1),
            chroma_sample_position: Uint::new(0),
            initial_presentation_delay_minus_one: Some(Uint::new(3)),
            config_obus: vec![0x12, 0x00, 0x0a],
        },
        unknown_boxes: vec![],
    };
    let track = Fmp4Track {
        track_id: TrackId(1),
        kind: TrackKind::Video,
        duration: 1,
        timescale: NonZeroU32::new(90_000).unwrap(),
        sample_entry: Some(SampleEntry::Av01(entry)),
    };
    let mut config = DecoderConfig::new(Codec::Av1, 30, false);
    apply_track_decoder_options(&mut config, &track);
    let BackendDecoderOptions::VideoToolbox(options) = config.backend_options else {
        panic!("AV1 fMP4 track should attach VideoToolbox decoder options");
    };
    assert_eq!(options.video_width, Some(320));
    assert_eq!(options.video_height, Some(180));
    assert_eq!(options.av1_config_obus, Some(vec![0x12, 0x00, 0x0a]));
    assert_eq!(
        options.av1c_record,
        Some(vec![0x81, 0x1f, 0x8c, 0x13, 0x12, 0x00, 0x0a])
    );
}

#[test]
fn frame_decode_request_defaults_to_auto_rgb() {
    let request = FrameDecodeRequest::new(TrackId(1), SampleId(2));
    assert_eq!(request.track_id, TrackId(1));
    assert_eq!(request.target_sample, SampleId(2));
    assert_eq!(request.backend, Backend::Auto);
    assert_eq!(request.output_mode, DecodeOutputMode::Rgb24);
    assert!(!request.require_hardware);
}

#[test]
fn frame_decode_range_request_defaults_to_auto_rgb() {
    let range = SampleRange {
        track_id: TrackId(1),
        start_sample: SampleId(2),
        end_sample_exclusive: SampleId(4),
    };
    let request = FrameDecodeRangeRequest::new(range);
    assert_eq!(request.range, range);
    assert_eq!(request.backend, Backend::Auto);
    assert_eq!(request.output_mode, DecodeOutputMode::Rgb24);
    assert!(!request.require_hardware);
}

#[test]
fn frame_decode_window_request_defaults_to_auto_rgb() {
    let request = FrameDecodeWindowRequest::new(TrackId(1), SampleId(2));
    assert_eq!(request.track_id, TrackId(1));
    assert_eq!(request.center_sample, SampleId(2));
    assert_eq!(request.before, 0);
    assert_eq!(request.after, 0);
    assert_eq!(request.backend, Backend::Auto);
    assert_eq!(request.output_mode, DecodeOutputMode::Rgb24);
    assert!(!request.require_hardware);
}

#[test]
fn decoded_frame_cache_config_has_bounded_default() {
    let config = DecodedFrameCacheConfig::default();
    assert_eq!(config.max_frames, 64);
    assert_eq!(config.max_bytes, 256 * 1024 * 1024);
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
fn decoded_frame_cache_tracks_hits_lru_and_frame_evictions() {
    let backend = test_resolved_backend();
    let mut cache = DecodedFrameCache::new(DecodedFrameCacheConfig {
        max_frames: 2,
        max_bytes: usize::MAX,
    });
    let key1 = test_cache_key(SampleId(1));
    let key2 = test_cache_key(SampleId(2));
    let key3 = test_cache_key(SampleId(3));
    cache.insert(
        key1.clone(),
        test_decoded_frame(SampleId(1), 4),
        backend,
        false,
        None,
    );
    cache.insert(
        key2.clone(),
        test_decoded_frame(SampleId(2), 4),
        backend,
        false,
        None,
    );
    assert!(cache.get(&key1).is_some());
    cache.insert(
        key3.clone(),
        test_decoded_frame(SampleId(3), 4),
        backend,
        false,
        None,
    );

    assert!(cache.get(&key1).is_some(), "recent hit should protect key1");
    assert!(cache.get(&key2).is_none(), "oldest key should be evicted");
    assert!(cache.get(&key3).is_some());
    let stats = cache.stats();
    assert_eq!(stats.inserts, 3);
    assert_eq!(stats.evictions, 1);
    assert_eq!(stats.resident_frames, 2);
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
fn decoded_frame_cache_enforces_byte_budget_and_clear() {
    let backend = test_resolved_backend();
    let mut cache = DecodedFrameCache::new(DecodedFrameCacheConfig {
        max_frames: 10,
        max_bytes: 12,
    });
    cache.insert(
        test_cache_key(SampleId(1)),
        test_decoded_frame(SampleId(1), 12),
        backend,
        false,
        None,
    );
    cache.insert(
        test_cache_key(SampleId(2)),
        test_decoded_frame(SampleId(2), 12),
        backend,
        false,
        None,
    );
    let stats = cache.stats();
    assert_eq!(stats.evictions, 1);
    assert_eq!(stats.resident_frames, 1);
    assert_eq!(stats.resident_bytes, 12);

    cache.clear();
    let stats = cache.stats();
    assert_eq!(stats.resident_frames, 0);
    assert_eq!(stats.resident_bytes, 0);
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
fn decoded_frame_cache_key_separates_backend_mode_and_fps() {
    let auto_rgb = cache_key_for_request(
        TrackId(1),
        SampleId(1),
        Backend::Auto,
        false,
        DecodeOutputMode::Rgb24,
        30,
    );
    let auto_nv12 = cache_key_for_request(
        TrackId(1),
        SampleId(1),
        Backend::Auto,
        false,
        DecodeOutputMode::Nv12,
        30,
    );
    let auto_rgb_60 = cache_key_for_request(
        TrackId(1),
        SampleId(1),
        Backend::Auto,
        false,
        DecodeOutputMode::Rgb24,
        60,
    );
    assert_ne!(auto_rgb, auto_nv12);
    assert_ne!(auto_rgb, auto_rgb_60);

    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    assert_ne!(
        auto_rgb,
        cache_key_for_request(
            TrackId(1),
            SampleId(1),
            Backend::Nvidia,
            false,
            DecodeOutputMode::Rgb24,
            30,
        )
    );
    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    assert_ne!(
        auto_rgb,
        cache_key_for_request(
            TrackId(1),
            SampleId(1),
            Backend::VideoToolbox,
            false,
            DecodeOutputMode::Rgb24,
            30,
        )
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
fn test_resolved_backend() -> BackendKind {
    Backend::Auto
        .resolve_decoder(&DecoderConfig::new(Codec::H264, 30, false))
        .expect("a decoder backend should be available with enabled test features")
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
fn test_cache_key(sample_id: SampleId) -> DecodedFrameCacheKey {
    cache_key_for_request(
        TrackId(1),
        sample_id,
        Backend::Auto,
        false,
        DecodeOutputMode::Rgb24,
        30,
    )
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
fn test_decoded_frame(sample_id: SampleId, byte_len: usize) -> DecodedSampleFrame {
    DecodedSampleFrame {
        sample_id: Some(sample_id),
        sample_meta: None,
        presentation_index: None,
        frame: DecodedFrame::Rgb24 {
            dims: Dimensions {
                width: NonZeroU32::new(2).expect("non-zero width"),
                height: NonZeroU32::new(2).expect("non-zero height"),
            },
            pts_90k: None,
            data: vec![sample_id.0 as u8; byte_len],
        },
    }
}

#[test]
fn gop_cursor_starts_from_previous_keyframe() {
    let input_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("sample-10s.mp4");
    let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(input_path))
        .into_sync_session()
        .expect("sample should open");
    let video_track = reader
        .tracks()
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .expect("video track should exist")
        .track_id;
    let samples = reader.samples(video_track).expect("video samples");
    let start = samples
        .iter()
        .find(|sample| !sample.keyframe)
        .expect("sample should contain inter frames")
        .sample_id;
    let range = SampleRange {
        track_id: video_track,
        start_sample: start,
        end_sample_exclusive: SampleId(u64::MAX),
    };
    let cursor = GopCursor::new(&mut reader, range).expect("cursor should resolve");

    assert_eq!(cursor.range, range);
    assert_eq!(
        cursor.decode_start_sample,
        reader
            .keyframe_before(start)
            .expect("keyframe lookup should succeed")
            .expect("previous keyframe should exist")
    );
    assert_eq!(cursor.next_sample, cursor.decode_start_sample);
}

#[test]
fn sample_timestamp_uses_sample_timescale() {
    let meta = SampleMeta {
        sample_id: SampleId(0),
        track_id: TrackId(1),
        offset: 0,
        size: 0,
        dts: MediaTime::new(0, NonZeroU32::new(30_000).expect("timescale")),
        pts: MediaTime::new(15_000, NonZeroU32::new(30_000).expect("timescale")),
        duration: 1_001,
        composition_time_offset: None,
        keyframe: true,
    };
    assert_eq!(sample_timestamp_to_90k(&meta), Some(Timestamp90k(45_000)));
}

#[test]
fn presentation_window_uses_pts_order_not_decode_order() {
    let timescale = NonZeroU32::new(30_000).expect("timescale");
    let samples = [
        (0, 0, 2_002),
        (1, 1_001, 6_006),
        (2, 2_002, 4_004),
        (3, 3_003, 3_003),
        (4, 4_004, 5_005),
    ]
    .into_iter()
    .map(|(sample_id, dts, pts)| SampleMeta {
        sample_id: SampleId(sample_id),
        track_id: TrackId(1),
        offset: 0,
        size: 0,
        dts: MediaTime::new(dts, timescale),
        pts: MediaTime::new(pts, timescale),
        duration: 1_001,
        composition_time_offset: None,
        keyframe: sample_id == 0,
    })
    .collect::<Vec<_>>();

    assert_eq!(
        presentation_window_sample_ids(&samples, SampleId(2), 2, 1).expect("window should resolve"),
        vec![SampleId(0), SampleId(3), SampleId(2), SampleId(4)]
    );
    let wanted = [SampleId(0), SampleId(3), SampleId(2), SampleId(4)]
        .into_iter()
        .collect::<HashSet<_>>();
    assert_eq!(
        decode_span_for_sample_ids(&samples, &wanted).expect("span should resolve"),
        (0, 5)
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
fn decode_window_returns_presentation_order_for_reordered_mp4() {
    let input_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("foreman_cif.mp4");
    let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(input_path))
        .into_sync_session()
        .expect("sample should open");
    let video_track = reader
        .tracks()
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .expect("video track should exist")
        .track_id;
    let samples = reader.samples(video_track).expect("video samples");
    assert_eq!(samples[0].sample_id, SampleId(0));
    assert_eq!(samples[0].pts.ticks, 2_002);
    assert_eq!(samples[1].sample_id, SampleId(1));
    assert_eq!(samples[1].pts.ticks, 6_006);
    assert_eq!(samples[2].sample_id, SampleId(2));
    assert_eq!(samples[2].pts.ticks, 4_004);
    assert_eq!(samples[3].sample_id, SampleId(3));
    assert_eq!(samples[3].pts.ticks, 3_003);
    assert_eq!(samples[4].sample_id, SampleId(4));
    assert_eq!(samples[4].pts.ticks, 5_005);

    let mut request = FrameDecodeWindowRequest::new(video_track, SampleId(2));
    request.before = 2;
    request.after = 1;
    request.output_mode = DecodeOutputMode::Metadata;
    let mut decoder = FrameDecoder::new(&mut reader);
    let result = match decoder.decode_window(request) {
        Ok(result) => result,
        Err(err) => {
            let error = format!("{err:#}");
            if error.contains("failed to create decoder session")
                || error.contains("auto backend selection failed")
                || error.contains("unsupported config")
            {
                eprintln!("skipping decode-window reorder test: {error}");
                return;
            }
            panic!("decode_window failed unexpectedly: {error}");
        }
    };
    let returned = result
        .frames
        .iter()
        .map(|frame| frame.sample_id.expect("decoded frame should map to sample"))
        .collect::<Vec<_>>();
    assert_eq!(
        returned,
        vec![SampleId(0), SampleId(3), SampleId(2), SampleId(4)]
    );
    for (index, frame) in result.frames.iter().enumerate() {
        assert_eq!(frame.presentation_index, Some(index));
        assert_eq!(
            frame.sample_meta.as_ref().map(|sample| sample.sample_id),
            frame.sample_id
        );
    }
    assert_eq!(result.diagnostics.requested_sample_count, 4);
    assert_eq!(result.diagnostics.returned_sample_count, 4);
    assert_eq!(result.diagnostics.frames_with_sample_metadata_count, 4);
    assert!(result.diagnostics.ambiguous_sample_association_count <= 4);
    assert!(matches!(
        result.diagnostics.returned_frame_order,
        ReturnedFrameOrder::Presentation
    ));
    assert!(result.diagnostics.missing_sample_ids.is_empty());
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
fn decode_window_matches_synthetic_markers_for_bframe_and_no_bframe_inputs() {
    if Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("skipping synthetic marker alignment test: ffmpeg is not available");
        return;
    }

    const WIDTH: u32 = 128;
    const HEIGHT: u32 = 72;
    const FRAMES: usize = 18;
    const WINDOW_START: usize = 4;
    const WINDOW_FRAMES: usize = 10;
    let temp_dir = std::env::temp_dir().join(format!(
        "video-hw-fmp4-marker-alignment-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let source_rgb = make_marker_rgb(WIDTH as usize, HEIGHT as usize, FRAMES);
    let source_raw = temp_dir.join("source.rgb");
    let bframe_mp4 = temp_dir.join("bframes.mp4");
    let no_bframe_mp4 = temp_dir.join("no-bframes.mp4");
    fs::write(&source_raw, &source_rgb).expect("source raw should be written");

    encode_marker_mp4(&source_raw, &bframe_mp4, WIDTH, HEIGHT, 2);
    encode_marker_mp4(&source_raw, &no_bframe_mp4, WIDTH, HEIGHT, 0);

    assert_decode_window_matches_markers(
        &bframe_mp4,
        WIDTH,
        HEIGHT,
        FRAMES,
        WINDOW_START,
        WINDOW_FRAMES,
        true,
    );
    assert_decode_window_matches_markers(
        &no_bframe_mp4,
        WIDTH,
        HEIGHT,
        FRAMES,
        WINDOW_START,
        WINDOW_FRAMES,
        false,
    );

    let _ = fs::remove_dir_all(temp_dir);
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
fn decode_window_roundtrip_preserves_presentation_frame_alignment() {
    if Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("skipping decode-window roundtrip test: ffmpeg is not available");
        return;
    }

    let input_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("foreman_cif.mp4");
    let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(input_path.clone()))
        .into_sync_session()
        .expect("sample should open");
    let video_track = reader
        .tracks()
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .expect("video track should exist")
        .track_id;
    let mut request = FrameDecodeWindowRequest::new(video_track, SampleId(2));
    request.before = 2;
    request.after = 1;
    request.output_mode = DecodeOutputMode::Rgb24;
    let mut decoder = FrameDecoder::new(&mut reader);
    let result = match decoder.decode_window(request) {
        Ok(result) => result,
        Err(err) => {
            let error = format!("{err:#}");
            if error.contains("failed to create decoder session")
                || error.contains("auto backend selection failed")
                || error.contains("unsupported config")
            {
                eprintln!("skipping decode-window roundtrip test: {error}");
                return;
            }
            panic!("decode_window failed unexpectedly: {error}");
        }
    };
    assert_eq!(result.frames.len(), 4);

    let (width, height, decoded_rgb) = rgb24_window_payload(&result.frames);
    let temp_dir = std::env::temp_dir().join(format!(
        "video-hw-fmp4-frame-order-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let decoded_raw = temp_dir.join("decoded.rgb");
    let source_raw = temp_dir.join("source.rgb");
    let roundtrip_mp4 = temp_dir.join("roundtrip.mp4");
    let roundtrip_raw = temp_dir.join("roundtrip.rgb");
    fs::write(&decoded_raw, &decoded_rgb).expect("decoded raw should be written");

    run_ffmpeg(&[
        "-y",
        "-i",
        input_path.to_str().expect("utf-8 input path"),
        "-frames:v",
        "4",
        "-pix_fmt",
        "rgb24",
        "-f",
        "rawvideo",
        source_raw.to_str().expect("utf-8 source path"),
    ]);
    run_ffmpeg(&[
        "-y",
        "-f",
        "rawvideo",
        "-pixel_format",
        "rgb24",
        "-video_size",
        &format!("{width}x{height}"),
        "-framerate",
        "30",
        "-i",
        decoded_raw.to_str().expect("utf-8 decoded path"),
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        roundtrip_mp4.to_str().expect("utf-8 roundtrip path"),
    ]);
    run_ffmpeg(&[
        "-y",
        "-i",
        roundtrip_mp4.to_str().expect("utf-8 roundtrip path"),
        "-frames:v",
        "4",
        "-pix_fmt",
        "rgb24",
        "-f",
        "rawvideo",
        roundtrip_raw.to_str().expect("utf-8 roundtrip raw path"),
    ]);

    let source = fs::read(&source_raw).expect("source raw should exist");
    let roundtrip = fs::read(&roundtrip_raw).expect("roundtrip raw should exist");
    assert_diagonal_best_match(&source, &roundtrip, width as usize * height as usize * 3, 4);

    let _ = fs::remove_dir_all(temp_dir);
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
fn cached_frame_decoder_reuses_decoded_target_frame() {
    let input_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("foreman_cif.mp4");
    let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(input_path))
        .into_sync_session()
        .expect("sample should open");
    let video_track = reader
        .tracks()
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .expect("video track should exist")
        .track_id;
    let target_sample = reader
        .samples(video_track)
        .expect("video samples")
        .get(2)
        .expect("sample 2 should exist")
        .sample_id;
    let mut request = FrameDecodeWindowRequest::new(video_track, target_sample);
    request.after = 2;
    request.output_mode = DecodeOutputMode::Rgb24;
    let mut cached = CachedFrameDecoder::new(
        &mut reader,
        DecodedFrameCacheConfig {
            max_frames: 8,
            max_bytes: 32 * 1024 * 1024,
        },
    );
    let first = match cached.decode_sample_cached(request.clone()) {
        Ok(result) => result,
        Err(err) => {
            let error = format!("{err:#}");
            if error.contains("failed to create decoder session")
                || error.contains("auto backend selection failed")
                || error.contains("unsupported config")
            {
                eprintln!("skipping cached decode test: {error}");
                return;
            }
            panic!("cached decode failed unexpectedly: {error}");
        }
    };
    let second = cached
        .decode_sample_cached(request)
        .expect("second decode should be served from cache");

    assert_eq!(first.frames.len(), 1);
    assert_eq!(second.frames.len(), 1);
    assert_eq!(second.cache_stats_delta.hits, 1);
    assert_eq!(second.cache_stats_delta.misses, 0);
    assert_eq!(second.diagnostics.decoded_frame_count, 0);
    assert_eq!(second.frames[0].sample_id, Some(target_sample));
    assert_eq!(
        rgb24_bytes(&first.frames[0]),
        rgb24_bytes(&second.frames[0]),
        "cached target frame should match the original decoded target"
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
fn cached_frame_decoder_reuses_reverse_prefetch_window() {
    let (mut reader, video_track, samples) = open_foreman_reader_with_samples();
    let target = samples[8].sample_id;
    let previous = samples[7].sample_id;
    let mut cached = CachedFrameDecoder::new(
        &mut reader,
        DecodedFrameCacheConfig {
            max_frames: 16,
            max_bytes: 64 * 1024 * 1024,
        },
    );
    let mut first_request = FrameDecodeWindowRequest::new(video_track, target);
    first_request.before = 3;
    first_request.output_mode = DecodeOutputMode::Rgb24;
    if decode_sample_cached_or_skip(&mut cached, first_request).is_none() {
        return;
    }

    let mut second_request = FrameDecodeWindowRequest::new(video_track, previous);
    second_request.before = 3;
    second_request.output_mode = DecodeOutputMode::Rgb24;
    let second = cached
        .decode_sample_cached(second_request)
        .expect("reverse-neighbor decode should succeed");
    assert_eq!(second.cache_stats_delta.hits, 1);
    assert_eq!(second.cache_stats_delta.misses, 0);
    assert_eq!(second.diagnostics.decoded_frame_count, 0);
    assert_eq!(second.frames[0].sample_id, Some(previous));
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
fn cached_frame_decoder_after_prefetch_does_not_help_reverse_neighbor() {
    let (mut reader, video_track, samples) = open_foreman_reader_with_samples();
    let target = samples[8].sample_id;
    let previous = samples[7].sample_id;
    let mut cached = CachedFrameDecoder::new(
        &mut reader,
        DecodedFrameCacheConfig {
            max_frames: 16,
            max_bytes: 64 * 1024 * 1024,
        },
    );
    let mut first_request = FrameDecodeWindowRequest::new(video_track, target);
    first_request.after = 3;
    first_request.output_mode = DecodeOutputMode::Rgb24;
    if decode_sample_cached_or_skip(&mut cached, first_request).is_none() {
        return;
    }

    let mut second_request = FrameDecodeWindowRequest::new(video_track, previous);
    second_request.after = 3;
    second_request.output_mode = DecodeOutputMode::Rgb24;
    let second = cached
        .decode_sample_cached(second_request)
        .expect("reverse-neighbor decode should succeed");
    assert_eq!(second.cache_stats_delta.hits, 0);
    assert_eq!(second.cache_stats_delta.misses, 1);
    assert!(second.diagnostics.decoded_frame_count > 0);
    assert_eq!(second.frames[0].sample_id, Some(previous));
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
fn cached_frame_decoder_window_cached_requires_full_window() {
    let (mut reader, video_track, samples) = open_foreman_reader_with_samples();
    let target = samples[8].sample_id;
    let mut cached = CachedFrameDecoder::new(
        &mut reader,
        DecodedFrameCacheConfig {
            max_frames: 16,
            max_bytes: 64 * 1024 * 1024,
        },
    );

    let mut window_request = FrameDecodeWindowRequest::new(video_track, target);
    window_request.before = 1;
    window_request.after = 1;
    window_request.output_mode = DecodeOutputMode::Rgb24;
    if decode_window_cached_or_skip(&mut cached, window_request.clone()).is_none() {
        return;
    }
    let second = cached
        .decode_window_cached(window_request)
        .expect("cached window should resolve");
    assert_eq!(second.cache_stats_delta.hits, 3);
    assert_eq!(second.cache_stats_delta.misses, 0);
    assert_eq!(second.diagnostics.decoded_frame_count, 0);
    assert_eq!(second.frames.len(), 3);

    cached.clear_cache();
    let mut partial_seed = FrameDecodeWindowRequest::new(video_track, target);
    partial_seed.after = 1;
    partial_seed.output_mode = DecodeOutputMode::Rgb24;
    let _ = cached
        .decode_sample_cached(partial_seed)
        .expect("partial seed should decode");
    let mut wider_window = FrameDecodeWindowRequest::new(video_track, target);
    wider_window.after = 2;
    wider_window.output_mode = DecodeOutputMode::Rgb24;
    let partial = cached
        .decode_window_cached(wider_window)
        .expect("partial cached window should fallback to decode");
    assert!(partial.cache_stats_delta.hits >= 1);
    assert!(partial.cache_stats_delta.misses >= 1);
    assert!(partial.diagnostics.decoded_frame_count > 0);
    assert_eq!(partial.frames.len(), 3);
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
fn cached_frame_decoder_zero_capacity_disables_reuse() {
    let (mut reader, video_track, samples) = open_foreman_reader_with_samples();
    let target = samples[8].sample_id;
    let mut cached = CachedFrameDecoder::new(
        &mut reader,
        DecodedFrameCacheConfig {
            max_frames: 0,
            max_bytes: DecodedFrameCacheConfig::default().max_bytes,
        },
    );
    let mut request = FrameDecodeWindowRequest::new(video_track, target);
    request.after = 2;
    request.output_mode = DecodeOutputMode::Rgb24;
    if decode_sample_cached_or_skip(&mut cached, request.clone()).is_none() {
        return;
    }
    let second = cached
        .decode_sample_cached(request)
        .expect("second zero-capacity decode should succeed");
    assert_eq!(second.cache_stats_delta.hits, 0);
    assert_eq!(second.cache_stats_delta.misses, 1);
    assert_eq!(second.cache_stats_delta.inserts, 0);
    assert!(second.diagnostics.decoded_frame_count > 0);
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
fn open_foreman_reader_with_samples() -> (Fmp4Reader<SyncReading>, TrackId, Vec<SampleMeta>) {
    let input_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("foreman_cif.mp4");
    let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(input_path))
        .into_sync_session()
        .expect("sample should open");
    let video_track = reader
        .tracks()
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .expect("video track should exist")
        .track_id;
    let samples = reader.samples(video_track).expect("video samples").to_vec();
    (reader, video_track, samples)
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
fn decode_sample_cached_or_skip(
    cached: &mut CachedFrameDecoder<'_>,
    request: FrameDecodeWindowRequest,
) -> Option<CachedFrameDecodeResult> {
    match cached.decode_sample_cached(request) {
        Ok(result) => Some(result),
        Err(err) => {
            let error = format!("{err:#}");
            if error.contains("failed to create decoder session")
                || error.contains("auto backend selection failed")
                || error.contains("unsupported config")
            {
                eprintln!("skipping cached decode test: {error}");
                None
            } else {
                panic!("cached decode failed unexpectedly: {error}");
            }
        }
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
fn decode_window_cached_or_skip(
    cached: &mut CachedFrameDecoder<'_>,
    request: FrameDecodeWindowRequest,
) -> Option<CachedFrameDecodeResult> {
    match cached.decode_window_cached(request) {
        Ok(result) => Some(result),
        Err(err) => {
            let error = format!("{err:#}");
            if error.contains("failed to create decoder session")
                || error.contains("auto backend selection failed")
                || error.contains("unsupported config")
            {
                eprintln!("skipping cached window decode test: {error}");
                None
            } else {
                panic!("cached window decode failed unexpectedly: {error}");
            }
        }
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
fn rgb24_window_payload(frames: &[DecodedSampleFrame]) -> (u32, u32, Vec<u8>) {
    let mut dims = None;
    let mut raw = Vec::new();
    for frame in frames {
        let DecodedFrame::Rgb24 {
            dims: frame_dims,
            data,
            ..
        } = &frame.frame
        else {
            panic!("decode window should return RGB24 frames");
        };
        let frame_dims = *frame_dims;
        if let Some(expected) = dims {
            assert_eq!(frame_dims, expected);
        } else {
            dims = Some(frame_dims);
        }
        raw.extend_from_slice(data);
    }
    let dims = dims.expect("at least one frame");
    (dims.width.get(), dims.height.get(), raw)
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
fn rgb24_bytes(frame: &DecodedSampleFrame) -> &[u8] {
    let DecodedFrame::Rgb24 { data, .. } = &frame.frame else {
        panic!("expected RGB24 frame");
    };
    data
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
fn encode_marker_mp4(
    source_raw: &std::path::Path,
    output_mp4: &std::path::Path,
    width: u32,
    height: u32,
    bframes: u32,
) {
    run_ffmpeg(&[
        "-y",
        "-f",
        "rawvideo",
        "-pixel_format",
        "rgb24",
        "-video_size",
        &format!("{width}x{height}"),
        "-framerate",
        "30",
        "-i",
        source_raw.to_str().expect("utf-8 source path"),
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "18",
        "-g",
        "12",
        "-bf",
        &bframes.to_string(),
        "-pix_fmt",
        "yuv420p",
        output_mp4.to_str().expect("utf-8 output path"),
    ]);
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
fn assert_decode_window_matches_markers(
    input_path: &std::path::Path,
    width: u32,
    height: u32,
    total_frames: usize,
    window_start: usize,
    window_frames: usize,
    expect_reordered_samples: bool,
) {
    let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(input_path.to_path_buf()))
        .into_sync_session()
        .expect("synthetic sample should open");
    let video_track = reader
        .tracks()
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .expect("video track should exist")
        .track_id;
    let samples = reader.samples(video_track).expect("video samples").to_vec();
    let sample_id_order = samples
        .iter()
        .map(|sample| sample.sample_id)
        .collect::<Vec<_>>();
    let pts_order = ordered_sample_ids_by_pts(&samples);
    assert_eq!(samples.len(), total_frames);
    if expect_reordered_samples {
        assert_ne!(
            sample_id_order, pts_order,
            "synthetic B-frame input should have reordered PTS/sample order"
        );
    } else {
        assert_eq!(
            sample_id_order, pts_order,
            "no-B-frame control should keep sample order in presentation order"
        );
    }

    let mut request = FrameDecodeWindowRequest::new(video_track, pts_order[window_start]);
    request.after = u32::try_from(window_frames - 1).expect("frame count fits u32");
    request.output_mode = DecodeOutputMode::Rgb24;
    let mut decoder = FrameDecoder::new(&mut reader);
    let result = match decoder.decode_window(request) {
        Ok(result) => result,
        Err(err) => {
            let error = format!("{err:#}");
            if error.contains("failed to create decoder session")
                || error.contains("auto backend selection failed")
                || error.contains("unsupported config")
            {
                eprintln!("skipping synthetic marker alignment test: {error}");
                return;
            }
            panic!("decode_window failed unexpectedly: {error}");
        }
    };
    assert_eq!(result.frames.len(), window_frames);
    for (index, frame) in result.frames.iter().enumerate() {
        assert_eq!(frame.presentation_index, Some(index));
        assert_eq!(
            frame.sample_meta.as_ref().map(|sample| sample.sample_id),
            frame.sample_id
        );
        assert_eq!(
            frame.sample_meta.as_ref().map(|sample| sample.pts),
            samples
                .iter()
                .find(|sample| sample.sample_id == pts_order[window_start + index])
                .map(|sample| sample.pts)
        );
    }

    let (decoded_width, decoded_height, decoded_rgb) = rgb24_window_payload(&result.frames);
    assert_eq!((decoded_width, decoded_height), (width, height));
    let reference_raw = input_path.with_extension("reference.rgb");
    run_ffmpeg(&[
        "-y",
        "-i",
        input_path.to_str().expect("utf-8 input path"),
        "-frames:v",
        &total_frames.to_string(),
        "-pix_fmt",
        "rgb24",
        "-f",
        "rawvideo",
        reference_raw.to_str().expect("utf-8 reference path"),
    ]);
    let reference_rgb = fs::read(&reference_raw).expect("reference raw should exist");
    let frame_len = width as usize * height as usize * 3;
    let reference_window =
        &reference_rgb[window_start * frame_len..(window_start + window_frames) * frame_len];
    assert_diagonal_best_match(reference_window, &decoded_rgb, frame_len, window_frames);
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
fn make_marker_rgb(width: usize, height: usize, frames: usize) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(width * height * 3 * frames);
    for frame in 0..frames {
        let stripe_start = frame * width / frames;
        let stripe_end = (frame + 1) * width / frames;
        let base = [
            (frame * 19 + 20) as u8,
            235_u8.saturating_sub((frame * 17) as u8),
            (frame * 11 + 40) as u8,
        ];
        for y in 0..height {
            for x in 0..width {
                let in_stripe = x >= stripe_start && x < stripe_end;
                let bit_cell = (x / 16).saturating_add((y / 12) * 8);
                let bit = ((frame >> (bit_cell % 4)) & 1) != 0;
                let checker = ((x / 8) + (y / 8) + frame) % 2 == 0;
                let pixel = if in_stripe {
                    [255, 255, 255]
                } else if bit {
                    [base[0], 20, base[2]]
                } else if checker {
                    base
                } else {
                    [base[0] / 2, base[1] / 2, base[2] / 2]
                };
                rgb.extend_from_slice(&pixel);
            }
        }
    }
    rgb
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
fn run_ffmpeg(args: &[&str]) {
    let output = Command::new("ffmpeg")
        .args(args)
        .output()
        .expect("ffmpeg should launch");
    assert!(
        output.status.success(),
        "ffmpeg failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
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
fn assert_diagonal_best_match(source: &[u8], roundtrip: &[u8], frame_len: usize, frames: usize) {
    assert_eq!(source.len(), frame_len * frames);
    assert_eq!(roundtrip.len(), frame_len * frames);
    for source_index in 0..frames {
        let source_frame =
            &source[source_index * frame_len..source_index.saturating_add(1) * frame_len];
        let mut best_index = 0;
        let mut best_mse = f64::INFINITY;
        let mut diagonal_mse = f64::INFINITY;
        for roundtrip_index in 0..frames {
            let roundtrip_frame = &roundtrip
                [roundtrip_index * frame_len..roundtrip_index.saturating_add(1) * frame_len];
            let mse = frame_mse(source_frame, roundtrip_frame);
            if roundtrip_index == source_index {
                diagonal_mse = mse;
            }
            if mse < best_mse {
                best_mse = mse;
                best_index = roundtrip_index;
            }
        }
        assert!(
            best_index == source_index || diagonal_mse <= best_mse * 1.02 + 1.0,
            "source frame {source_index} matched roundtrip frame {best_index}; diagonal mse={diagonal_mse:.4}, best mse={best_mse:.4}"
        );
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
fn frame_mse(left: &[u8], right: &[u8]) -> f64 {
    assert_eq!(left.len(), right.len());
    let sum = left
        .iter()
        .zip(right)
        .map(|(left, right)| {
            let delta = f64::from(*left) - f64::from(*right);
            delta * delta
        })
        .sum::<f64>();
    sum / left.len() as f64
}

#[cfg(feature = "serde")]
#[test]
fn serde_roundtrips_decode_diagnostics() {
    let diagnostics = DecodeDiagnostics {
        requested_backend: Backend::Auto,
        resolved_backend: BackendKind::Nvidia,
        require_hardware: false,
        output_mode: DecodeOutputMode::Rgb24,
        fps: 30,
        fallback_used: true,
        fallback_reason: Some("primary backend failed".to_string()),
        requested_sample_count: 3,
        decoded_frame_count: 4,
        returned_sample_count: 2,
        frames_with_sample_metadata_count: 2,
        dropped_or_unmatched_frame_count: 2,
        ambiguous_sample_association_count: 1,
        returned_frame_order: ReturnedFrameOrder::Presentation,
        missing_sample_ids: vec![SampleId(42)],
    };
    let json = serde_json::to_string(&diagnostics).expect("serialize diagnostics");
    assert!(json.contains("\"requested_backend\":\"auto\""));
    assert!(json.contains("\"output_mode\":\"rgb24\""));
    assert!(json.contains("\"returned_frame_order\":\"presentation\""));
    let roundtrip: DecodeDiagnostics =
        serde_json::from_str(&json).expect("deserialize diagnostics");
    assert_eq!(roundtrip.requested_backend, diagnostics.requested_backend);
    assert_eq!(roundtrip.resolved_backend, diagnostics.resolved_backend);
    assert_eq!(roundtrip.output_mode, diagnostics.output_mode);
    assert_eq!(roundtrip.fallback_used, diagnostics.fallback_used);
    assert_eq!(roundtrip.fallback_reason, diagnostics.fallback_reason);
    assert!(matches!(
        roundtrip.returned_frame_order,
        ReturnedFrameOrder::Presentation
    ));
    assert_eq!(
        roundtrip.ambiguous_sample_association_count,
        diagnostics.ambiguous_sample_association_count
    );
    assert_eq!(roundtrip.missing_sample_ids, diagnostics.missing_sample_ids);
}
