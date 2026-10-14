//! Unit tests for Surface timestamp normalization.

use super::normalize_encoder_pts_90k;

#[test]
fn normalizes_large_surface_timestamps_to_relative_90k() {
    let mut origin = None;
    let mut last = None;

    let first = normalize_encoder_pts_90k(&mut origin, &mut last, 1_780_746_839_834_000);
    let second = normalize_encoder_pts_90k(&mut origin, &mut last, 1_780_746_839_867_333);

    assert_eq!(first, 0);
    assert_eq!(second, 2_999);
}

#[test]
fn keeps_repeated_or_backwards_timestamps_monotonic() {
    let mut origin = None;
    let mut last = None;

    assert_eq!(
        normalize_encoder_pts_90k(&mut origin, &mut last, 100_000),
        0
    );
    assert_eq!(
        normalize_encoder_pts_90k(&mut origin, &mut last, 100_000),
        1
    );
    assert_eq!(normalize_encoder_pts_90k(&mut origin, &mut last, 90_000), 2);
}
