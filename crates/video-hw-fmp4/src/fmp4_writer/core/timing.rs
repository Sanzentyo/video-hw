//! Final init-segment timestamps, movie duration, and raw RGBA adaptation.

use super::*;

pub(super) fn patch_fmp4_init_segment_timing(
    init_segment: &[u8],
    creation_timestamp: Duration,
    duration_in_input_timescale: u64,
    input_timescale: NonZeroU32,
) -> Result<Vec<u8>> {
    let (ftyp_box, ftyp_size) = FtypBox::decode(init_segment).context("failed to decode ftyp")?;
    let moov_input = init_segment
        .get(ftyp_size..)
        .context("init segment missing moov")?;
    let (mut moov_box, moov_size) = MoovBox::decode(moov_input).context("failed to decode moov")?;
    let creation_time = Mp4FileTime::from_unix_time(creation_timestamp);
    let movie_duration = convert_duration_timescale(
        duration_in_input_timescale,
        input_timescale,
        moov_box.mvhd_box.timescale,
    );
    moov_box.mvhd_box.creation_time = creation_time;
    moov_box.mvhd_box.modification_time = creation_time;
    // For fragmented MP4, QuickTime/Finder double-counted playback duration when both the movie/
    // track headers and `mehd` carried the finalized duration. Keeping header durations at 0 and
    // storing the final timeline only in `mehd` avoids that discrepancy while remaining readable
    // to ffprobe and players that follow the fragment timeline.
    moov_box.mvhd_box.duration = 0;
    for trak in &mut moov_box.trak_boxes {
        trak.tkhd_box.creation_time = creation_time;
        trak.tkhd_box.modification_time = creation_time;
        trak.tkhd_box.duration = 0;
        trak.mdia_box.mdhd_box.creation_time = creation_time;
        trak.mdia_box.mdhd_box.modification_time = creation_time;
        trak.mdia_box.mdhd_box.duration = 0;
    }
    if let Some(mvex_box) = moov_box.mvex_box.as_mut()
        && let Some(mehd_box) = mvex_box.mehd_box.as_mut()
    {
        mehd_box.fragment_duration = movie_duration;
    }
    let mut patched = ftyp_box.encode_to_vec().context("failed to encode ftyp")?;
    patched.extend_from_slice(&moov_box.encode_to_vec().context("failed to encode moov")?);
    let consumed = ftyp_size.saturating_add(moov_size);
    if consumed < init_segment.len() {
        patched.extend_from_slice(&init_segment[consumed..]);
    }
    Ok(patched)
}

pub(super) fn convert_duration_timescale(
    duration: u64,
    from_timescale: NonZeroU32,
    to_timescale: NonZeroU32,
) -> u64 {
    if duration == 0 || from_timescale == to_timescale {
        return duration;
    }
    let from = u128::from(from_timescale.get());
    let to = u128::from(to_timescale.get());
    let rounded = (u128::from(duration) * to + from / 2) / from;
    u64::try_from(rounded).unwrap_or(u64::MAX)
}

pub(super) fn rgba_to_argb_bytes(rgba: &[u8]) -> Vec<u8> {
    let mut argb = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        argb.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
    }
    argb
}
