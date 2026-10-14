use anyhow::{Context, Result};
use tracing::info;
use video_hw::Backend;

use super::{decode::decode_sample_index_sync, media::LoadedVideo};

pub(super) fn run_smoke_test(
    video: &LoadedVideo,
    backend: Backend,
    require_hardware: bool,
    strict_backend: bool,
) -> Result<()> {
    println!(
        "smoke_test input={} track={} samples={} duration_s={:.3} timeline_start_ticks={}",
        video.input_path.display(),
        video.track.track_id,
        video.sample_count(),
        video.total_duration_seconds(),
        video.timeline_start_ticks
    );
    let mut checkpoints = vec![
        0usize,
        video.sample_count() / 2,
        video.sample_count().saturating_sub(1),
    ];
    checkpoints.sort_unstable();
    checkpoints.dedup();
    for &target in &checkpoints {
        let sample = video
            .sample(target)
            .context("smoke-test checkpoint is outside the presentation timeline")?;
        println!(
            "smoke_seek sample={} pts={} position_s={:.6} dur={} keyframe={} keyframe_start={}",
            target,
            sample.pts.ticks,
            video.sample_timestamp_seconds(target),
            sample.duration,
            sample.keyframe,
            video.keyframe_start_for(target)
        );
    }
    info!(
        backend = %backend,
        require_hardware,
        strict_backend,
        samples = video.sample_count(),
        "starting smoke test"
    );

    anyhow::ensure!(
        video
            .samples()
            .windows(2)
            .all(|window| window[0].pts.ticks <= window[1].pts.ticks),
        "presentation timeline PTS values are not monotonic"
    );
    anyhow::ensure!(
        video.sample_timestamp_seconds(0) == 0.0,
        "presentation timeline is not normalized to a zero-based player clock"
    );
    let last_sample_index = video.sample_count().saturating_sub(1);
    anyhow::ensure!(
        video.sample_index_for_seconds(video.total_duration_seconds()) == last_sample_index,
        "end-of-stream seek did not resolve to the final presentation sample"
    );
    let final_sample_end_seconds = video.sample_timestamp_seconds(last_sample_index)
        + video.sample_duration_seconds(last_sample_index);
    let tick_tolerance = 1.0 / f64::from(video.track.timescale.get());
    anyhow::ensure!(
        final_sample_end_seconds <= video.total_duration_seconds() + tick_tolerance,
        "final sample ends after the advertised player duration"
    );

    for &target in &checkpoints {
        let attempt =
            decode_sample_index_sync(video, backend, require_hardware, target, strict_backend)
                .with_context(|| format!("failed to decode checkpoint sample {target}"))?;
        let resolved_backend = attempt.resolved_backend.to_string();
        let output_mode = attempt.output_mode.to_string();
        let fallback_reason = attempt.fallback_reason.as_deref().unwrap_or("none");
        let preview = attempt.preview.with_context(|| {
            format!("decoder returned no displayable preview for checkpoint sample {target}")
        })?;
        println!(
            "smoke_decode sample={} requested_backend={} decode_backend={} resolved_backend={} mode={} fallback_used={} fallback_reason={} frame={}x{} frames={}",
            target,
            backend,
            attempt.decode_backend,
            resolved_backend,
            output_mode,
            attempt.fallback_used,
            fallback_reason,
            preview.width,
            preview.height,
            attempt.frame_count
        );
        anyhow::ensure!(
            attempt.frame_count == 1,
            "decoder returned {} exact-target frames at sample {} for backend {}; expected 1",
            attempt.frame_count,
            target,
            resolved_backend
        );
    }

    println!("smoke_result=ok");
    info!("smoke test completed");
    Ok(())
}
