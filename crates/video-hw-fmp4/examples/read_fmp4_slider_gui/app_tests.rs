use std::path::PathBuf;

use anyhow::Result;

use super::*;
use crate::media::load_video_samples;

fn repository_sample_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sample-videos")
        .join("sample-10s.mp4")
}

#[test]
fn reader_app_represents_end_of_stream_without_snapping_back() -> Result<()> {
    let loaded = load_video_samples(repository_sample_path())?;
    let duration = loaded.total_duration_seconds();
    let last_index = loaded.sample_count() - 1;
    let mut app = ReaderApp::new(loaded, false, Backend::Auto, false, false);

    app.seek_to_seconds(duration);
    assert!(app.at_end_of_stream);
    assert_eq!(app.current_index, last_index);
    assert_eq!(app.slider_seconds, duration);

    app.seek_to_index(0);
    assert!(!app.at_end_of_stream);
    assert_eq!(app.slider_seconds, 0.0);
    app.playing = true;
    app.apply_cached_decode_status(&CachedDecodeOutcome {
        preview: None,
        status: "cached decode failure".to_string(),
        level: DecodeStatusLevel::Error,
    });
    assert!(
        app.playing,
        "decode status must not alter the playback clock"
    );
    assert_eq!(app.decode_status, "cached decode failure");

    let active = DecodeWorkerRequest {
        request_id: 10,
        generation: app.decode_generation,
        sample_index: 0,
        backend: Backend::Auto,
        require_hardware: false,
        strict_backend: false,
    };
    assert!(app.decode_scheduler.try_activate(active));
    let previous_generation = app.decode_generation;
    app.reset_decoder_state("test reset");
    assert_eq!(app.decode_generation, previous_generation.saturating_add(1));
    assert!(
        app.decode_scheduler.is_busy(),
        "reset cannot cancel or forget physical worker activity"
    );
    Ok(())
}

#[test]
fn decode_scheduler_skips_stale_positions_and_starts_latest_after_completion() {
    fn request(request_id: u64, sample_index: usize) -> DecodeWorkerRequest {
        DecodeWorkerRequest {
            request_id,
            generation: 0,
            sample_index,
            backend: Backend::Auto,
            require_hardware: false,
            strict_backend: false,
        }
    }

    let mut scheduler = DecodeScheduler::default();
    assert!(scheduler.try_activate(request(1, 56)));
    for sample_index in 57..=101 {
        assert!(!scheduler.try_activate(request(2, sample_index)));
    }
    assert_eq!(scheduler.active_key().map(|key| key.sample_index), Some(56));
    assert!(scheduler.complete(999).is_none());
    assert!(scheduler.is_busy());
    assert_eq!(
        scheduler.complete(1).map(|done| done.sample_index),
        Some(56)
    );
    assert!(!scheduler.is_busy());

    assert!(scheduler.try_activate(request(2, 101)));
    assert_eq!(
        scheduler.active_key().map(|key| key.sample_index),
        Some(101)
    );
}

#[test]
fn preview_cache_obeys_its_byte_budget() {
    let mut app = ReaderApp::new(
        load_video_samples(repository_sample_path()).expect("fixture loads"),
        false,
        Backend::Auto,
        false,
        false,
    );
    let frame_bytes = ReaderApp::DECODE_CACHE_MAX_BYTES / 2 + 1;
    for sample_index in 0..3 {
        let key = PreviewCacheKey {
            sample_index,
            backend: Backend::Auto,
            require_hardware: false,
            strict_backend: false,
        };
        app.store_decode_cache_entry(
            key,
            CachedDecodeOutcome {
                preview: Some(DecodedPreview {
                    width: frame_bytes,
                    height: 1,
                    rgba: vec![0; frame_bytes].into(),
                }),
                status: "cached".to_string(),
                level: DecodeStatusLevel::Success,
            },
        );
    }
    assert!(app.decode_cache_resident_bytes <= ReaderApp::DECODE_CACHE_MAX_BYTES);
    assert!(app.decode_cache.len() < 3);
}
