//! Codec sample entries, packet repacking, and strict AV1 configuration parsing.

use super::*;

pub(super) fn create_h264_sample_entry(
    width: u16,
    height: u16,
    sps: &[u8],
    pps: &[u8],
) -> Result<SampleEntry> {
    let config = parse_h264_sps_config(sps)?;
    validate_h264_pps(pps)?;
    Ok(SampleEntry::Avc1(Avc1Box {
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
            avc_profile_indication: config.profile_idc,
            profile_compatibility: config.profile_compatibility,
            avc_level_indication: config.level_idc,
            length_size_minus_one: Uint::new(3),
            sps_list: vec![sps.to_vec()],
            pps_list: vec![pps.to_vec()],
            chroma_format: config.chroma_format_idc.map(Uint::new),
            bit_depth_luma_minus8: config.bit_depth_luma_minus8.map(Uint::new),
            bit_depth_chroma_minus8: config.bit_depth_chroma_minus8.map(Uint::new),
            sps_ext_list: vec![],
        },
        unknown_boxes: vec![],
    }))
}

pub(super) fn create_hevc_sample_entry(
    width: u16,
    height: u16,
    vps: &[u8],
    sps: &[u8],
    pps: &[u8],
) -> Result<SampleEntry> {
    let config = parse_hevc_sps_config(sps)?;
    validate_hevc_parameter_set(vps, 32, "VPS")?;
    validate_hevc_parameter_set(pps, 34, "PPS")?;
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
    Ok(SampleEntry::Hvc1(Hvc1Box {
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
            general_profile_space: Uint::new(config.profile_space),
            general_tier_flag: Uint::new(config.tier_flag),
            general_profile_idc: Uint::new(config.profile_idc),
            general_profile_compatibility_flags: config.profile_compatibility_flags,
            general_constraint_indicator_flags: Uint::new(config.constraint_indicator_flags),
            general_level_idc: config.level_idc,
            min_spatial_segmentation_idc: Uint::new(0),
            parallelism_type: Uint::new(0),
            chroma_format_idc: Uint::new(config.chroma_format_idc),
            bit_depth_luma_minus8: Uint::new(config.bit_depth_luma_minus8),
            bit_depth_chroma_minus8: Uint::new(config.bit_depth_chroma_minus8),
            avg_frame_rate: 0,
            constant_frame_rate: Uint::new(0),
            num_temporal_layers: Uint::new(config.num_temporal_layers),
            temporal_id_nested: Uint::new(u8::from(config.temporal_id_nested)),
            length_size_minus_one: Uint::new(3),
            nalu_arrays,
        },
        unknown_boxes: vec![],
    }))
}

#[derive(Debug, Clone, Copy)]
pub(super) struct H264ConfigSummary {
    profile_idc: u8,
    profile_compatibility: u8,
    level_idc: u8,
    chroma_format_idc: Option<u8>,
    bit_depth_luma_minus8: Option<u8>,
    bit_depth_chroma_minus8: Option<u8>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct HevcConfigSummary {
    profile_space: u8,
    tier_flag: u8,
    profile_idc: u8,
    profile_compatibility_flags: u32,
    constraint_indicator_flags: u64,
    level_idc: u8,
    chroma_format_idc: u8,
    bit_depth_luma_minus8: u8,
    bit_depth_chroma_minus8: u8,
    num_temporal_layers: u8,
    temporal_id_nested: bool,
}

pub(super) fn parse_h264_sps_config(sps: &[u8]) -> Result<H264ConfigSummary> {
    anyhow::ensure!(
        sps.first().is_some_and(|byte| byte & 0x1f == 7),
        "H.264 SPS does not have NAL unit type 7"
    );
    anyhow::ensure!(sps.len() >= 4, "H.264 SPS is too short");
    let profile_idc = sps[1];
    let mut config = H264ConfigSummary {
        profile_idc,
        profile_compatibility: sps[2],
        level_idc: sps[3],
        chroma_format_idc: None,
        bit_depth_luma_minus8: None,
        bit_depth_chroma_minus8: None,
    };
    let rbsp = rbsp_from_ebsp(&sps[4..]);
    let mut bits = BitReader::new(&rbsp);
    bits.read_uvlc()
        .context("H.264 SPS is missing seq_parameter_set_id")?;
    if matches!(
        profile_idc,
        44 | 83 | 86 | 100 | 110 | 118 | 122 | 128 | 134 | 135 | 138 | 139 | 144 | 244
    ) {
        let chroma_format_idc = u8::try_from(
            bits.read_uvlc()
                .context("H.264 SPS is missing chroma_format_idc")?,
        )
        .context("H.264 chroma_format_idc exceeds u8")?;
        anyhow::ensure!(chroma_format_idc <= 3, "invalid H.264 chroma_format_idc");
        if chroma_format_idc == 3 {
            bits.read_bits(1)
                .context("H.264 SPS is missing separate_colour_plane_flag")?;
        }
        let bit_depth_luma_minus8 = u8::try_from(
            bits.read_uvlc()
                .context("H.264 SPS is missing bit_depth_luma_minus8")?,
        )
        .context("H.264 bit_depth_luma_minus8 exceeds u8")?;
        let bit_depth_chroma_minus8 = u8::try_from(
            bits.read_uvlc()
                .context("H.264 SPS is missing bit_depth_chroma_minus8")?,
        )
        .context("H.264 bit_depth_chroma_minus8 exceeds u8")?;
        anyhow::ensure!(
            bit_depth_luma_minus8 <= 7 && bit_depth_chroma_minus8 <= 7,
            "H.264 bit depth does not fit avcC"
        );
        config.chroma_format_idc = Some(chroma_format_idc);
        config.bit_depth_luma_minus8 = Some(bit_depth_luma_minus8);
        config.bit_depth_chroma_minus8 = Some(bit_depth_chroma_minus8);
    }
    Ok(config)
}

pub(super) fn validate_h264_pps(pps: &[u8]) -> Result<()> {
    anyhow::ensure!(
        pps.first().is_some_and(|byte| byte & 0x1f == 8),
        "H.264 PPS does not have NAL unit type 8"
    );
    anyhow::ensure!(pps.len() >= 2, "H.264 PPS is too short");
    let rbsp = rbsp_from_ebsp(&pps[1..]);
    let mut bits = BitReader::new(&rbsp);
    bits.read_uvlc()
        .context("H.264 PPS is missing pic_parameter_set_id")?;
    bits.read_uvlc()
        .context("H.264 PPS is missing seq_parameter_set_id")?;
    Ok(())
}

pub(super) fn parse_hevc_sps_config(sps: &[u8]) -> Result<HevcConfigSummary> {
    validate_hevc_parameter_set(sps, 33, "SPS")?;
    let parsed = scuffle_h265::SpsNALUnit::parse(std::io::Cursor::new(sps))
        .context("failed to parse HEVC SPS")?;
    let rbsp = rbsp_from_ebsp(&sps[2..]);
    anyhow::ensure!(rbsp.len() >= 13, "HEVC SPS profile_tier_level is truncated");
    let profile_byte = rbsp[1];
    let profile_compatibility_flags =
        u32::from_be_bytes(rbsp[2..6].try_into().expect("four-byte SPS field"));
    let constraint_indicator_flags = rbsp[6..12]
        .iter()
        .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte));
    Ok(HevcConfigSummary {
        profile_space: profile_byte >> 6,
        tier_flag: (profile_byte >> 5) & 1,
        profile_idc: profile_byte & 0x1f,
        profile_compatibility_flags,
        constraint_indicator_flags,
        level_idc: rbsp[12],
        chroma_format_idc: parsed.rbsp.chroma_format_idc,
        bit_depth_luma_minus8: parsed.rbsp.bit_depth_luma_minus8,
        bit_depth_chroma_minus8: parsed.rbsp.bit_depth_chroma_minus8,
        num_temporal_layers: parsed.rbsp.sps_max_sub_layers_minus1 + 1,
        temporal_id_nested: parsed.rbsp.sps_temporal_id_nesting_flag,
    })
}

pub(super) fn validate_hevc_parameter_set(
    data: &[u8],
    expected_type: u8,
    label: &str,
) -> Result<()> {
    anyhow::ensure!(data.len() >= 3, "HEVC {label} is too short");
    anyhow::ensure!(
        hevc_nal_type(data) == expected_type,
        "HEVC {label} does not have NAL unit type {expected_type}"
    );
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Av1ConfigSummary {
    pub(super) seq_profile: u8,
    pub(super) seq_level_idx_0: u8,
    pub(super) seq_tier_0: u8,
    pub(super) high_bitdepth: u8,
    pub(super) twelve_bit: u8,
    pub(super) monochrome: u8,
    pub(super) chroma_subsampling_x: u8,
    pub(super) chroma_subsampling_y: u8,
    pub(super) chroma_sample_position: u8,
}

impl Default for Av1ConfigSummary {
    fn default() -> Self {
        Self {
            seq_profile: 0,
            seq_level_idx_0: 31,
            seq_tier_0: 0,
            high_bitdepth: 0,
            twelve_bit: 0,
            monochrome: 0,
            chroma_subsampling_x: 1,
            chroma_subsampling_y: 1,
            chroma_sample_position: 0,
        }
    }
}

pub(super) fn create_av1_sample_entry(
    width: u16,
    height: u16,
    sequence_header: &[u8],
    config: Av1ConfigSummary,
) -> SampleEntry {
    SampleEntry::Av01(Av01Box {
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
        av1c_box: Av1cBox {
            seq_profile: Uint::new(config.seq_profile),
            seq_level_idx_0: Uint::new(config.seq_level_idx_0),
            seq_tier_0: Uint::new(config.seq_tier_0),
            high_bitdepth: Uint::new(config.high_bitdepth),
            twelve_bit: Uint::new(config.twelve_bit),
            monochrome: Uint::new(config.monochrome),
            chroma_subsampling_x: Uint::new(config.chroma_subsampling_x),
            chroma_subsampling_y: Uint::new(config.chroma_subsampling_y),
            chroma_sample_position: Uint::new(config.chroma_sample_position),
            initial_presentation_delay_minus_one: None,
            config_obus: sequence_header.to_vec(),
        },
        unknown_boxes: vec![],
    })
}

pub(super) fn annexb_chunk_to_avcc_sample(
    annexb: &[u8],
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus = Vec::new();
    let mut saw_sps = false;
    let mut saw_pps = false;
    for nalu in split_annexb_nalus(annexb) {
        if nalu.is_empty() {
            continue;
        }
        match h264_nal_type(nalu) {
            7 => {
                parse_h264_sps_config(nalu)?;
                record_parameter_set(sps_out, &mut saw_sps, nalu, "H.264 SPS")?;
            }
            8 => {
                validate_h264_pps(nalu)?;
                record_parameter_set(pps_out, &mut saw_pps, nalu, "H.264 PPS")?;
            }
            _ => sample_nalus.push(nalu),
        }
    }
    if sample_nalus.is_empty() {
        return Ok(None);
    }
    repack_length_prefixed_nalus(&sample_nalus).map(Some)
}

pub(super) fn annexb_chunk_to_hvcc_sample(
    annexb: &[u8],
    vps_out: &mut Option<Vec<u8>>,
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus = Vec::new();
    let mut saw_vps = false;
    let mut saw_sps = false;
    let mut saw_pps = false;
    for nalu in split_annexb_nalus(annexb) {
        if nalu.is_empty() {
            continue;
        }
        match hevc_nal_type(nalu) {
            32 => {
                validate_hevc_parameter_set(nalu, 32, "VPS")?;
                record_parameter_set(vps_out, &mut saw_vps, nalu, "HEVC VPS")?;
            }
            33 => {
                parse_hevc_sps_config(nalu)?;
                record_parameter_set(sps_out, &mut saw_sps, nalu, "HEVC SPS")?;
            }
            34 => {
                validate_hevc_parameter_set(nalu, 34, "PPS")?;
                record_parameter_set(pps_out, &mut saw_pps, nalu, "HEVC PPS")?;
            }
            _ => sample_nalus.push(nalu),
        }
    }
    if sample_nalus.is_empty() {
        return Ok(None);
    }
    repack_length_prefixed_nalus(&sample_nalus).map(Some)
}

pub(super) fn avcc_chunk_to_avcc_sample(
    avcc: &[u8],
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus = Vec::new();
    let mut saw_sps = false;
    let mut saw_pps = false;
    for nalu in split_length_prefixed_nalus(avcc)? {
        if nalu.is_empty() {
            continue;
        }
        match h264_nal_type(nalu) {
            7 => {
                parse_h264_sps_config(nalu)?;
                record_parameter_set(sps_out, &mut saw_sps, nalu, "H.264 SPS")?;
            }
            8 => {
                validate_h264_pps(nalu)?;
                record_parameter_set(pps_out, &mut saw_pps, nalu, "H.264 PPS")?;
            }
            _ => sample_nalus.push(nalu),
        }
    }
    if sample_nalus.is_empty() {
        return Ok(None);
    }
    repack_length_prefixed_nalus(&sample_nalus).map(Some)
}

pub(super) fn hvcc_chunk_to_hvcc_sample(
    hvcc: &[u8],
    vps_out: &mut Option<Vec<u8>>,
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus = Vec::new();
    let mut saw_vps = false;
    let mut saw_sps = false;
    let mut saw_pps = false;
    for nalu in split_length_prefixed_nalus(hvcc)? {
        if nalu.is_empty() {
            continue;
        }
        match hevc_nal_type(nalu) {
            32 => {
                validate_hevc_parameter_set(nalu, 32, "VPS")?;
                record_parameter_set(vps_out, &mut saw_vps, nalu, "HEVC VPS")?;
            }
            33 => {
                parse_hevc_sps_config(nalu)?;
                record_parameter_set(sps_out, &mut saw_sps, nalu, "HEVC SPS")?;
            }
            34 => {
                validate_hevc_parameter_set(nalu, 34, "PPS")?;
                record_parameter_set(pps_out, &mut saw_pps, nalu, "HEVC PPS")?;
            }
            _ => sample_nalus.push(nalu),
        }
    }
    if sample_nalus.is_empty() {
        return Ok(None);
    }
    repack_length_prefixed_nalus(&sample_nalus).map(Some)
}

pub(super) fn av1_chunk_to_av1_sample(
    data: &[u8],
    sequence_header_out: &mut Option<Vec<u8>>,
    config_out: &mut Option<Av1ConfigSummary>,
) -> Result<Option<Vec<u8>>> {
    if data.is_empty() {
        return Ok(None);
    }
    if let Some((obu, payload)) = find_av1_sequence_header_obu(data) {
        let config = parse_av1_sequence_header(payload)
            .context("AV1 frame chunk has a malformed sequence header OBU")?;
        *sequence_header_out = Some(obu.to_vec());
        *config_out = Some(config);
    }
    Ok(Some(data.to_vec()))
}

pub(super) fn split_annexb_nalus(data: &[u8]) -> Vec<&[u8]> {
    let mut nalus = Vec::new();
    let mut cursor = 0usize;
    while let Some((start, start_code_len)) = find_start_code(data, cursor) {
        let nalu_start = start + start_code_len;
        let mut nalu_end = find_start_code(data, nalu_start)
            .map(|(pos, _)| pos)
            .unwrap_or(data.len());
        while nalu_end > nalu_start && data[nalu_end - 1] == 0 {
            nalu_end -= 1;
        }
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
            .context("truncated NAL length")?;
        let nalu_len = u32::from_be_bytes(len_bytes.try_into().expect("slice len")) as usize;
        cursor = cursor.saturating_add(4);
        let nalu = data
            .get(cursor..cursor + nalu_len)
            .context("truncated NAL payload")?;
        nalus.push(nalu);
        cursor = cursor.saturating_add(nalu_len);
    }
    Ok(nalus)
}

pub(super) fn repack_length_prefixed_nalus(nalus: &[&[u8]]) -> Result<Vec<u8>> {
    let mut sample = Vec::new();
    for nalu in nalus {
        let len = u32::try_from(nalu.len()).context("NAL too large")?;
        sample.extend_from_slice(&len.to_be_bytes());
        sample.extend_from_slice(nalu);
    }
    Ok(sample)
}

fn record_parameter_set(
    output: &mut Option<Vec<u8>>,
    seen_in_chunk: &mut bool,
    parameter_set: &[u8],
    label: &str,
) -> Result<()> {
    if *seen_in_chunk {
        anyhow::ensure!(
            output.as_deref() == Some(parameter_set),
            "fMP4 writer does not support multiple active {label} values"
        );
    }
    *output = Some(parameter_set.to_vec());
    *seen_in_chunk = true;
    Ok(())
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

pub(super) fn find_av1_sequence_header_obu(data: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut offset = 0usize;
    while offset < data.len() {
        let (obu_type, payload_start, end) = parse_av1_obu_bounds(data, offset)?;
        if obu_type == 1 {
            return Some((&data[offset..end], &data[payload_start..end]));
        }
        offset = end;
    }
    None
}

pub(super) fn parse_av1_obu_bounds(data: &[u8], offset: usize) -> Option<(u8, usize, usize)> {
    let header = *data.get(offset)?;
    let obu_type = (header >> 3) & 0x0f;
    let has_extension = (header & 0x04) != 0;
    let has_size_field = (header & 0x02) != 0;
    let mut cursor = offset.checked_add(1)?;
    if has_extension {
        cursor = cursor.checked_add(1)?;
        data.get(cursor - 1)?;
    }
    if !has_size_field {
        return None;
    }
    let (payload_size, leb_len) = read_av1_leb128(&data[cursor..])?;
    cursor = cursor.checked_add(leb_len)?;
    let payload_start = cursor;
    let payload_size = usize::try_from(payload_size).ok()?;
    let end = cursor.checked_add(payload_size)?;
    if end > data.len() {
        return None;
    }
    Some((obu_type, payload_start, end))
}

pub(super) fn read_av1_leb128(data: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (i, byte) in data.iter().copied().take(8).enumerate() {
        value |= u64::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

pub(super) fn parse_av1_sequence_header(payload: &[u8]) -> Option<Av1ConfigSummary> {
    let mut bits = BitReader::new(payload);
    let seq_profile = bits.read_bits(3)? as u8;
    let _still_picture = bits.read_bits(1)?;
    let reduced_still_picture_header = bits.read_bits(1)? != 0;
    let mut seq_level_idx_0 = 31;
    let mut seq_tier_0 = 0;
    if reduced_still_picture_header {
        seq_level_idx_0 = bits.read_bits(5)? as u8;
    } else {
        let timing_info_present_flag = bits.read_bits(1)? != 0;
        if timing_info_present_flag {
            let _num_units_in_display_tick = bits.read_bits(32)?;
            let _time_scale = bits.read_bits(32)?;
            let equal_picture_interval = bits.read_bits(1)? != 0;
            if equal_picture_interval {
                let _num_ticks_per_picture_minus_1 = bits.read_uvlc()?;
            }
            let decoder_model_info_present_flag = bits.read_bits(1)? != 0;
            if decoder_model_info_present_flag {
                let buffer_delay_length_minus_1 = bits.read_bits(5)?;
                let _num_units_in_decoding_tick = bits.read_bits(32)?;
                let _buffer_removal_time_length_minus_1 = bits.read_bits(5)?;
                let _frame_presentation_time_length_minus_1 = bits.read_bits(5)?;
                let _ = buffer_delay_length_minus_1;
            }
        }
        let initial_display_delay_present_flag = bits.read_bits(1)? != 0;
        let operating_points_cnt_minus_1 = bits.read_bits(5)?;
        for operating_point in 0..=operating_points_cnt_minus_1 {
            let _operating_point_idc = bits.read_bits(12)?;
            let seq_level_idx = bits.read_bits(5)? as u8;
            let seq_tier = if seq_level_idx > 7 {
                bits.read_bits(1)? as u8
            } else {
                0
            };
            if initial_display_delay_present_flag {
                let initial_display_delay_present_for_this_op = bits.read_bits(1)? != 0;
                if initial_display_delay_present_for_this_op {
                    let _initial_display_delay_minus_1 = bits.read_bits(4)?;
                }
            }
            if operating_point == 0 {
                seq_level_idx_0 = seq_level_idx;
                seq_tier_0 = seq_tier;
            }
        }
    }

    Some(Av1ConfigSummary {
        seq_profile,
        seq_level_idx_0,
        seq_tier_0,
        ..Av1ConfigSummary::default()
    })
}

fn rbsp_from_ebsp(payload: &[u8]) -> Vec<u8> {
    let mut rbsp = Vec::with_capacity(payload.len());
    let mut zero_run = 0_u8;
    for &byte in payload {
        if zero_run >= 2 && byte == 0x03 {
            zero_run = 0;
            continue;
        }
        rbsp.push(byte);
        if byte == 0 {
            zero_run = zero_run.saturating_add(1);
        } else {
            zero_run = 0;
        }
    }
    rbsp
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_offset: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            bit_offset: 0,
        }
    }

    fn read_bits(&mut self, count: usize) -> Option<u64> {
        let mut value = 0u64;
        for _ in 0..count {
            let byte = *self.data.get(self.bit_offset / 8)?;
            let bit = (byte >> (7 - (self.bit_offset % 8))) & 1;
            value = (value << 1) | u64::from(bit);
            self.bit_offset = self.bit_offset.checked_add(1)?;
        }
        Some(value)
    }

    fn read_uvlc(&mut self) -> Option<u64> {
        let mut leading_zeroes = 0usize;
        while self.read_bits(1)? == 0 {
            leading_zeroes = leading_zeroes.checked_add(1)?;
        }
        if leading_zeroes >= 63 {
            return None;
        }
        let suffix = if leading_zeroes == 0 {
            0
        } else {
            self.read_bits(leading_zeroes)?
        };
        Some((1u64 << leading_zeroes) - 1 + suffix)
    }
}
