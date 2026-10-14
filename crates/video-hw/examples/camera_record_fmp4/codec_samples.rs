//! H.264/HEVC sample-entry construction, packet repacking, and init timing repair.

use std::{num::NonZeroU32, time::Duration};

use anyhow::{Context, Result};
use shiguredo_mp4::{
    Decode, Encode, Mp4FileTime, Uint,
    boxes::{
        Avc1Box, AvccBox, FtypBox, Hvc1Box, HvccBox, HvccNalUintArray, MoovBox, SampleEntry,
        VisualSampleEntryFields,
    },
};

pub(super) fn create_h264_sample_entry(
    width: u16,
    height: u16,
    sps: &[u8],
    pps: &[u8],
) -> SampleEntry {
    let avc_profile_indication = sps.get(1).copied().unwrap_or(66);
    let profile_compatibility = sps.get(2).copied().unwrap_or(0);
    let avc_level_indication = sps.get(3).copied().unwrap_or(30);

    SampleEntry::Avc1(Avc1Box {
        visual: VisualSampleEntryFields {
            data_reference_index: VisualSampleEntryFields::DEFAULT_DATA_REFERENCE_INDEX,
            width,
            height,
            horizresolution: VisualSampleEntryFields::DEFAULT_HORIZRESOLUTION,
            vertresolution: VisualSampleEntryFields::DEFAULT_VERTRESOLUTION,
            frame_count: VisualSampleEntryFields::DEFAULT_FRAME_COUNT,
            compressorname: VisualSampleEntryFields::NULL_COMPRESSORNAME,
            depth: VisualSampleEntryFields::DEFAULT_DEPTH,
        },
        avcc_box: AvccBox {
            avc_profile_indication,
            profile_compatibility,
            avc_level_indication,
            length_size_minus_one: Uint::new(3),
            sps_list: vec![sps.to_vec()],
            pps_list: vec![pps.to_vec()],
            chroma_format: Some(Uint::new(1)),
            bit_depth_luma_minus8: Some(Uint::new(0)),
            bit_depth_chroma_minus8: Some(Uint::new(0)),
            sps_ext_list: vec![],
        },
        unknown_boxes: vec![],
    })
}

pub(super) fn create_hevc_sample_entry(
    width: u16,
    height: u16,
    vps: &[u8],
    sps: &[u8],
    pps: &[u8],
) -> SampleEntry {
    // Conservative baseline values. The VPS/SPS/PPS arrays carry decoder-critical details.
    let nalu_arrays = vec![
        HvccNalUintArray {
            array_completeness: Uint::new(1),
            nal_unit_type: Uint::new(32),
            nalus: vec![vps.to_vec()],
        },
        HvccNalUintArray {
            array_completeness: Uint::new(1),
            nal_unit_type: Uint::new(33),
            nalus: vec![sps.to_vec()],
        },
        HvccNalUintArray {
            array_completeness: Uint::new(1),
            nal_unit_type: Uint::new(34),
            nalus: vec![pps.to_vec()],
        },
    ];

    SampleEntry::Hvc1(Hvc1Box {
        visual: VisualSampleEntryFields {
            data_reference_index: VisualSampleEntryFields::DEFAULT_DATA_REFERENCE_INDEX,
            width,
            height,
            horizresolution: VisualSampleEntryFields::DEFAULT_HORIZRESOLUTION,
            vertresolution: VisualSampleEntryFields::DEFAULT_VERTRESOLUTION,
            frame_count: VisualSampleEntryFields::DEFAULT_FRAME_COUNT,
            compressorname: VisualSampleEntryFields::NULL_COMPRESSORNAME,
            depth: VisualSampleEntryFields::DEFAULT_DEPTH,
        },
        hvcc_box: HvccBox {
            general_profile_space: Uint::new(0),
            general_tier_flag: Uint::new(0),
            general_profile_idc: Uint::new(1),
            general_profile_compatibility_flags: 0,
            general_constraint_indicator_flags: Uint::new(0),
            general_level_idc: 120,
            min_spatial_segmentation_idc: Uint::new(0),
            parallelism_type: Uint::new(0),
            chroma_format_idc: Uint::new(1),
            bit_depth_luma_minus8: Uint::new(0),
            bit_depth_chroma_minus8: Uint::new(0),
            avg_frame_rate: 0,
            constant_frame_rate: Uint::new(0),
            num_temporal_layers: Uint::new(0),
            temporal_id_nested: Uint::new(1),
            length_size_minus_one: Uint::new(3),
            nalu_arrays,
        },
        unknown_boxes: vec![],
    })
}

pub(super) fn annexb_chunk_to_avcc_sample(
    annexb: &[u8],
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus: Vec<&[u8]> = Vec::new();
    for nalu in split_annexb_nalus(annexb) {
        if nalu.is_empty() {
            continue;
        }
        match h264_nal_type(nalu) {
            7 => *sps_out = Some(nalu.to_vec()),
            8 => *pps_out = Some(nalu.to_vec()),
            _ => sample_nalus.push(nalu),
        }
    }

    if sample_nalus.is_empty() {
        return Ok(None);
    }

    let mut avcc = Vec::new();
    for nalu in sample_nalus {
        let len = u32::try_from(nalu.len()).context("NAL too large for AVCC")?;
        avcc.extend_from_slice(&len.to_be_bytes());
        avcc.extend_from_slice(nalu);
    }
    Ok(Some(avcc))
}

pub(super) fn annexb_chunk_to_hvcc_sample(
    annexb: &[u8],
    vps_out: &mut Option<Vec<u8>>,
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus: Vec<&[u8]> = Vec::new();
    for nalu in split_annexb_nalus(annexb) {
        if nalu.is_empty() {
            continue;
        }
        match hevc_nal_type(nalu) {
            32 => *vps_out = Some(nalu.to_vec()),
            33 => *sps_out = Some(nalu.to_vec()),
            34 => *pps_out = Some(nalu.to_vec()),
            _ => sample_nalus.push(nalu),
        }
    }

    if sample_nalus.is_empty() {
        return Ok(None);
    }

    let mut hvcc = Vec::new();
    for nalu in sample_nalus {
        let len = u32::try_from(nalu.len()).context("NAL too large for hvcC sample")?;
        hvcc.extend_from_slice(&len.to_be_bytes());
        hvcc.extend_from_slice(nalu);
    }
    Ok(Some(hvcc))
}

pub(super) fn avcc_chunk_to_avcc_sample(
    avcc: &[u8],
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus: Vec<&[u8]> = Vec::new();
    for nalu in split_length_prefixed_nalus(avcc)? {
        if nalu.is_empty() {
            continue;
        }
        match h264_nal_type(nalu) {
            7 => *sps_out = Some(nalu.to_vec()),
            8 => *pps_out = Some(nalu.to_vec()),
            _ => sample_nalus.push(nalu),
        }
    }

    if sample_nalus.is_empty() {
        return Ok(None);
    }

    Ok(Some(repack_length_prefixed_nalus(&sample_nalus)?))
}

pub(super) fn hvcc_chunk_to_hvcc_sample(
    hvcc: &[u8],
    vps_out: &mut Option<Vec<u8>>,
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus: Vec<&[u8]> = Vec::new();
    for nalu in split_length_prefixed_nalus(hvcc)? {
        if nalu.is_empty() {
            continue;
        }
        match hevc_nal_type(nalu) {
            32 => *vps_out = Some(nalu.to_vec()),
            33 => *sps_out = Some(nalu.to_vec()),
            34 => *pps_out = Some(nalu.to_vec()),
            _ => sample_nalus.push(nalu),
        }
    }

    if sample_nalus.is_empty() {
        return Ok(None);
    }

    Ok(Some(repack_length_prefixed_nalus(&sample_nalus)?))
}

pub(super) fn split_annexb_nalus(data: &[u8]) -> Vec<&[u8]> {
    let mut nalus = Vec::new();
    let mut cursor = 0usize;

    while let Some((start, start_code_len)) = find_start_code(data, cursor) {
        let nalu_start = start + start_code_len;
        let nalu_end = find_start_code(data, nalu_start)
            .map(|(pos, _)| pos)
            .unwrap_or(data.len());
        if nalu_start < nalu_end {
            nalus.push(&data[nalu_start..nalu_end]);
        }
        cursor = nalu_end;
    }

    if nalus.is_empty() && !data.is_empty() {
        nalus.push(data);
    }
    nalus
}

pub(super) fn split_length_prefixed_nalus(data: &[u8]) -> Result<Vec<&[u8]>> {
    let mut nalus = Vec::new();
    let mut cursor = 0usize;
    while cursor < data.len() {
        let len_bytes = data
            .get(cursor..cursor + 4)
            .context("length-prefixed sample is truncated before NAL length")?;
        let nalu_len =
            u32::from_be_bytes(len_bytes.try_into().expect("length slice size")) as usize;
        cursor = cursor.saturating_add(4);
        let nalu = data
            .get(cursor..cursor + nalu_len)
            .context("length-prefixed sample is truncated inside NAL payload")?;
        nalus.push(nalu);
        cursor = cursor.saturating_add(nalu_len);
    }
    Ok(nalus)
}

pub(super) fn repack_length_prefixed_nalus(nalus: &[&[u8]]) -> Result<Vec<u8>> {
    let mut sample = Vec::new();
    for nalu in nalus {
        let len = u32::try_from(nalu.len()).context("NAL too large for length-prefixed sample")?;
        sample.extend_from_slice(&len.to_be_bytes());
        sample.extend_from_slice(nalu);
    }
    Ok(sample)
}

pub(super) fn find_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    if data.len() < 3 || from >= data.len() {
        return None;
    }
    let mut i = from;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            if data.get(i + 2) == Some(&1) {
                return Some((i, 3));
            }
            if i + 3 < data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                return Some((i, 4));
            }
        }
        i += 1;
    }
    None
}

pub(super) fn h264_nal_type(nalu: &[u8]) -> u8 {
    nalu[0] & 0x1f
}

pub(super) fn hevc_nal_type(nalu: &[u8]) -> u8 {
    (nalu[0] >> 1) & 0x3f
}

pub(super) fn duration_90k_to_seconds(duration_90k: u64) -> f64 {
    duration_90k as f64 / 90_000.0
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

pub(super) fn patch_fmp4_init_segment_timing(
    init_segment: &[u8],
    creation_timestamp: Duration,
    duration_in_input_timescale: u64,
    input_timescale: NonZeroU32,
) -> Result<Vec<u8>> {
    let (ftyp_box, ftyp_size) =
        FtypBox::decode(init_segment).context("failed to decode ftyp from init segment")?;
    let moov_input = init_segment
        .get(ftyp_size..)
        .context("init segment is missing moov payload")?;
    let (mut moov_box, moov_size) =
        MoovBox::decode(moov_input).context("failed to decode moov from init segment")?;
    let creation_time = Mp4FileTime::from_unix_time(creation_timestamp);

    let movie_duration = convert_duration_timescale(
        duration_in_input_timescale,
        input_timescale,
        moov_box.mvhd_box.timescale,
    );
    moov_box.mvhd_box.creation_time = creation_time;
    moov_box.mvhd_box.modification_time = creation_time;
    // For fragmented MP4, QuickTime/Finder showed about double duration if both the header boxes
    // and `mehd` carried the finalized length. Keep the real duration only in `mehd`.
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

    let mut patched = ftyp_box
        .encode_to_vec()
        .context("failed to encode ftyp for patched init segment")?;
    patched.extend_from_slice(
        &moov_box
            .encode_to_vec()
            .context("failed to encode moov for patched init segment")?,
    );

    let consumed = ftyp_size.saturating_add(moov_size);
    if consumed < init_segment.len() {
        patched.extend_from_slice(&init_segment[consumed..]);
    }
    Ok(patched)
}
