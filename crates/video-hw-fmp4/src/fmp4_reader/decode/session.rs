//! Hardware decoder selection and track-specific session configuration.

use super::*;

pub(super) fn create_decode_session(
    requested_backend: Backend,
    config: DecoderConfig,
) -> Result<(AnyDecodeSession, DecodeDiagnostics)> {
    let resolved_backend = requested_backend.resolve_decoder(&config).with_context(|| {
        format!(
            "failed to resolve decoder backend: requested={}, codec={}, fps={}, require_hardware={}",
            requested_backend, config.codec, config.fps, config.require_hardware
        )
    })?;
    match AnyDecodeSession::with_backend_kind(resolved_backend, config.clone()) {
        Ok(session) => Ok((
            session,
            DecodeDiagnostics {
                requested_backend,
                resolved_backend,
                require_hardware: config.require_hardware,
                output_mode: config.output_mode,
                fps: config.fps,
                fallback_used: false,
                fallback_reason: None,
                requested_sample_count: 0,
                decoded_frame_count: 0,
                returned_sample_count: 0,
                frames_with_sample_metadata_count: 0,
                dropped_or_unmatched_frame_count: 0,
                ambiguous_sample_association_count: 0,
                returned_frame_order: ReturnedFrameOrder::Unknown,
                missing_sample_ids: Vec::new(),
            },
        )),
        Err(primary_err) if requested_backend != Backend::Auto && !config.require_hardware => {
            let fallback_backend = Backend::Auto
                .resolve_decoder(&config)
                .context("failed to resolve fallback auto decoder backend")?;
            let fallback_reason =
                format!("requested backend {resolved_backend} failed: {primary_err}");
            let session = AnyDecodeSession::with_backend_kind(fallback_backend, config.clone())
                .with_context(|| {
                    format!(
                        "fallback backend {fallback_backend} also failed after {resolved_backend}"
                    )
                })?;
            Ok((
                session,
                DecodeDiagnostics {
                    requested_backend,
                    resolved_backend: fallback_backend,
                    require_hardware: config.require_hardware,
                    output_mode: config.output_mode,
                    fps: config.fps,
                    fallback_used: fallback_backend != resolved_backend,
                    fallback_reason: Some(fallback_reason),
                    requested_sample_count: 0,
                    decoded_frame_count: 0,
                    returned_sample_count: 0,
                    frames_with_sample_metadata_count: 0,
                    dropped_or_unmatched_frame_count: 0,
                    ambiguous_sample_association_count: 0,
                    returned_frame_order: ReturnedFrameOrder::Unknown,
                    missing_sample_ids: Vec::new(),
                },
            ))
        }
        Err(err) => Err(anyhow!(err).context(format!(
            "failed to create decoder session with {resolved_backend}"
        ))),
    }
}

pub(super) fn apply_track_decoder_options(config: &mut DecoderConfig, track: &Fmp4Track) {
    if config.codec != Codec::Av1 {
        return;
    }
    let Some(SampleEntry::Av01(av01)) = track.sample_entry.as_ref() else {
        return;
    };
    let av1c_box = &av01.av1c_box;
    config.backend_options = BackendDecoderOptions::VideoToolbox(VtDecoderOptions {
        video_width: Some(av01.visual.width),
        video_height: Some(av01.visual.height),
        av1c_record: Some(av1c_record_from_sample_entry(av01)),
        av1_config_obus: Some(av1c_box.config_obus.clone()),
        ..Default::default()
    });
}

fn av1c_record_from_sample_entry(av01: &shiguredo_mp4::boxes::Av01Box) -> Vec<u8> {
    let av1c = &av01.av1c_box;
    let mut out = Vec::with_capacity(4 + av1c.config_obus.len());
    out.push(0x80 | 1);
    out.push((av1c.seq_profile.get() << 5) | av1c.seq_level_idx_0.get());
    out.push(
        (av1c.seq_tier_0.get() << 7)
            | (av1c.high_bitdepth.get() << 6)
            | (av1c.twelve_bit.get() << 5)
            | (av1c.monochrome.get() << 4)
            | (av1c.chroma_subsampling_x.get() << 3)
            | (av1c.chroma_subsampling_y.get() << 2)
            | av1c.chroma_sample_position.get(),
    );
    out.push(
        av1c.initial_presentation_delay_minus_one
            .as_ref()
            .map(|value| 0x10 | value.get())
            .unwrap_or(0),
    );
    out.extend_from_slice(&av1c.config_obus);
    out
}
