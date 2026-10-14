//! Presentation timestamp arithmetic and HEVC display-order bookkeeping.

use super::*;

pub(super) fn decode_pts_step(fps: i32) -> i64 {
    if fps <= 0 {
        3_000
    } else {
        i64::from((90_000 / fps).max(1))
    }
}

pub(super) fn usize_to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

pub(super) fn sort_hevc_display_order_frames(
    mut frames_with_poc: Vec<(i32, Frame)>,
    start_pts_90k: i64,
    pts_step: i64,
) -> Vec<Frame> {
    frames_with_poc.sort_by_key(|(poc, _)| *poc);
    frames_with_poc
        .into_iter()
        .enumerate()
        .map(|(display_idx, (_, mut frame))| {
            frame.pts_90k = Some(
                start_pts_90k.saturating_add(usize_to_i64(display_idx).saturating_mul(pts_step)),
            );
            frame
        })
        .collect()
}
