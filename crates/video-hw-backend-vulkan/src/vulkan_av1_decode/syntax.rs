//! AV1 low-overhead OBU and sequence-header parsing.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedAv1SequenceHeader {
    pub seq_profile: u8,
    pub still_picture: bool,
    pub reduced_still_picture_header: bool,
    pub frame_width_bits_minus_1: u8,
    pub frame_height_bits_minus_1: u8,
    pub max_frame_width_minus_1: u32,
    pub max_frame_height_minus_1: u32,
    pub use_128x128_superblock: bool,
    pub enable_filter_intra: bool,
    pub enable_intra_edge_filter: bool,
    pub enable_interintra_compound: bool,
    pub enable_masked_compound: bool,
    pub enable_warped_motion: bool,
    pub enable_dual_filter: bool,
    pub enable_order_hint: bool,
    pub enable_jnt_comp: bool,
    pub enable_ref_frame_mvs: bool,
    pub frame_id_numbers_present_flag: bool,
    pub enable_superres: bool,
    pub enable_cdef: bool,
    pub enable_restoration: bool,
    pub film_grain_params_present: bool,
    pub separate_uv_delta_q: bool,
    pub timing_info_present_flag: bool,
    pub initial_display_delay_present_flag: bool,
    pub order_hint_bits_minus_1: u8,
    pub seq_force_screen_content_tools: u8,
    pub seq_force_integer_mv: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Av1ObuType {
    SequenceHeader,
    TemporalDelimiter,
    FrameHeader,
    TileGroup,
    Metadata,
    Frame,
    RedundantFrameHeader,
    TileList,
    Padding,
    Unknown(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Av1ObuRecord {
    pub(super) obu_type: Av1ObuType,
    pub(super) obu_range: Range<usize>,
    pub(super) payload_range: Range<usize>,
    pub(super) temporal_unit_index: usize,
}

pub(super) fn parse_av1_low_overhead_obus(bitstream: &[u8]) -> Result<Vec<Av1ObuRecord>, String> {
    if bitstream.starts_with(b"DKIF") {
        return Err(
            "AV1 IVF container input is not a low-overhead OBU elementary stream".to_string(),
        );
    }

    let mut records = Vec::new();
    let mut cursor = 0usize;
    let mut temporal_unit_index = 0usize;
    let mut current_temporal_unit_has_payload = false;

    while cursor < bitstream.len() {
        let obu_start = cursor;
        let header = bitstream[cursor];
        cursor += 1;

        if header & 0x80 != 0 {
            return Err(format!(
                "AV1 OBU at offset {obu_start} sets obu_forbidden_bit"
            ));
        }
        if header & 0x01 != 0 {
            return Err(format!("AV1 OBU at offset {obu_start} sets reserved bit"));
        }

        let obu_type = av1_obu_type((header >> 3) & 0x0f);
        let has_extension = header & 0x04 != 0;
        let has_size_field = header & 0x02 != 0;
        if has_extension {
            if cursor >= bitstream.len() {
                return Err(format!(
                    "AV1 OBU at offset {obu_start} is truncated before extension header"
                ));
            }
            cursor += 1;
        }
        if !has_size_field {
            return Err(format!(
                "AV1 OBU at offset {obu_start} lacks the low-overhead size field"
            ));
        }

        let (payload_len, leb_len) = read_av1_leb128(&bitstream[cursor..])
            .map_err(|err| format!("AV1 OBU at offset {obu_start} has invalid size: {err}"))?;
        cursor += leb_len;
        let payload_start = cursor;
        let payload_end = payload_start
            .checked_add(payload_len)
            .ok_or_else(|| format!("AV1 OBU at offset {obu_start} size overflows usize"))?;
        if payload_end > bitstream.len() {
            return Err(format!(
                "AV1 OBU at offset {obu_start} is truncated: payload_end={payload_end}, len={}",
                bitstream.len()
            ));
        }

        if obu_type == Av1ObuType::TemporalDelimiter && current_temporal_unit_has_payload {
            temporal_unit_index = temporal_unit_index.saturating_add(1);
            current_temporal_unit_has_payload = false;
        }
        records.push(Av1ObuRecord {
            obu_type,
            obu_range: obu_start..payload_end,
            payload_range: payload_start..payload_end,
            temporal_unit_index,
        });
        if obu_type != Av1ObuType::TemporalDelimiter {
            current_temporal_unit_has_payload = true;
        }
        cursor = payload_end;
    }

    if records.is_empty() {
        return Err("AV1 bitstream contains no OBUs".to_string());
    }

    Ok(records)
}

pub(super) fn parse_av1_sequence_header_payload(
    payload: &[u8],
) -> Result<ParsedAv1SequenceHeader, String> {
    let mut bits = BitReader::new(payload);
    let seq_profile = bits.read_bits_u8(3, "seq_profile")?;
    let still_picture = bits.read_bool("still_picture")?;
    let reduced_still_picture_header = bits.read_bool("reduced_still_picture_header")?;

    let mut timing_info_present_flag = false;
    let mut initial_display_delay_present_flag = false;
    if reduced_still_picture_header {
        let _seq_level_idx_0 = bits.read_bits_u8(5, "seq_level_idx_0")?;
    } else {
        let flags = skip_av1_operating_points(&mut bits)?;
        timing_info_present_flag = flags.timing_info_present_flag;
        initial_display_delay_present_flag = flags.initial_display_delay_present_flag;
    }

    let frame_width_bits_minus_1 = bits.read_bits_u8(4, "frame_width_bits_minus_1")?;
    let frame_height_bits_minus_1 = bits.read_bits_u8(4, "frame_height_bits_minus_1")?;
    let max_frame_width_minus_1 = bits.read_bits_u32(
        usize::from(frame_width_bits_minus_1) + 1,
        "max_frame_width_minus_1",
    )?;
    let max_frame_height_minus_1 = bits.read_bits_u32(
        usize::from(frame_height_bits_minus_1) + 1,
        "max_frame_height_minus_1",
    )?;

    let mut frame_id_numbers_present_flag = false;
    if !reduced_still_picture_header {
        frame_id_numbers_present_flag = bits.read_bool("frame_id_numbers_present_flag")?;
        if frame_id_numbers_present_flag {
            let _delta_frame_id_length_minus_2 =
                bits.read_bits_u8(4, "delta_frame_id_length_minus_2")?;
            let _additional_frame_id_length_minus_1 =
                bits.read_bits_u8(3, "additional_frame_id_length_minus_1")?;
        }
    }

    let use_128x128_superblock = bits.read_bool("use_128x128_superblock")?;
    let enable_filter_intra = bits.read_bool("enable_filter_intra")?;
    let enable_intra_edge_filter = bits.read_bool("enable_intra_edge_filter")?;
    let (
        enable_interintra_compound,
        enable_masked_compound,
        enable_warped_motion,
        enable_dual_filter,
        enable_order_hint,
        enable_jnt_comp,
        enable_ref_frame_mvs,
        order_hint_bits_minus_1,
        seq_force_screen_content_tools,
        seq_force_integer_mv,
        enable_superres,
        enable_cdef,
        enable_restoration,
    ) = if reduced_still_picture_header {
        (
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            0,
            AV1_SELECT_SCREEN_CONTENT_TOOLS,
            AV1_SELECT_INTEGER_MV,
            false,
            false,
            false,
        )
    } else {
        let enable_interintra_compound = bits.read_bool("enable_interintra_compound")?;
        let enable_masked_compound = bits.read_bool("enable_masked_compound")?;
        let enable_warped_motion = bits.read_bool("enable_warped_motion")?;
        let enable_dual_filter = bits.read_bool("enable_dual_filter")?;
        let enable_order_hint = bits.read_bool("enable_order_hint")?;
        let (enable_jnt_comp, enable_ref_frame_mvs) = if enable_order_hint {
            (
                bits.read_bool("enable_jnt_comp")?,
                bits.read_bool("enable_ref_frame_mvs")?,
            )
        } else {
            (false, false)
        };
        let seq_force_screen_content_tools = if bits.read_bool("seq_choose_screen_content_tools")? {
            AV1_SELECT_SCREEN_CONTENT_TOOLS
        } else {
            bits.read_bits_u8(1, "seq_force_screen_content_tools")?
        };
        let seq_force_integer_mv =
            if seq_force_screen_content_tools > 0 && bits.read_bool("seq_choose_integer_mv")? {
                AV1_SELECT_INTEGER_MV
            } else if seq_force_screen_content_tools > 0 {
                bits.read_bits_u8(1, "seq_force_integer_mv")?
            } else {
                0
            };
        let order_hint_bits_minus_1 = if enable_order_hint {
            bits.read_bits_u8(3, "order_hint_bits_minus_1")?
        } else {
            0
        };
        let enable_superres = bits.read_bool("enable_superres")?;
        let enable_cdef = bits.read_bool("enable_cdef")?;
        let enable_restoration = bits.read_bool("enable_restoration")?;
        (
            enable_interintra_compound,
            enable_masked_compound,
            enable_warped_motion,
            enable_dual_filter,
            enable_order_hint,
            enable_jnt_comp,
            enable_ref_frame_mvs,
            order_hint_bits_minus_1,
            seq_force_screen_content_tools,
            seq_force_integer_mv,
            enable_superres,
            enable_cdef,
            enable_restoration,
        )
    };
    let color_config = if !reduced_still_picture_header {
        skip_av1_color_config_and_read_film_grain_flag(&mut bits, seq_profile)?
    } else {
        Av1ColorConfigParse {
            film_grain_params_present: false,
            separate_uv_delta_q: false,
        }
    };

    Ok(ParsedAv1SequenceHeader {
        seq_profile,
        still_picture,
        reduced_still_picture_header,
        frame_width_bits_minus_1,
        frame_height_bits_minus_1,
        max_frame_width_minus_1,
        max_frame_height_minus_1,
        use_128x128_superblock,
        enable_filter_intra,
        enable_intra_edge_filter,
        enable_interintra_compound,
        enable_masked_compound,
        enable_warped_motion,
        enable_dual_filter,
        enable_order_hint,
        enable_jnt_comp,
        enable_ref_frame_mvs,
        frame_id_numbers_present_flag,
        enable_superres,
        enable_cdef,
        enable_restoration,
        film_grain_params_present: color_config.film_grain_params_present,
        separate_uv_delta_q: color_config.separate_uv_delta_q,
        timing_info_present_flag,
        initial_display_delay_present_flag,
        order_hint_bits_minus_1,
        seq_force_screen_content_tools,
        seq_force_integer_mv,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Av1OperatingPointFlags {
    pub(super) timing_info_present_flag: bool,
    pub(super) initial_display_delay_present_flag: bool,
}

pub(super) fn skip_av1_operating_points(
    bits: &mut BitReader<'_>,
) -> Result<Av1OperatingPointFlags, String> {
    let timing_info_present_flag = bits.read_bool("timing_info_present_flag")?;
    if timing_info_present_flag {
        let _num_units_in_display_tick = bits.read_bits_u32(32, "num_units_in_display_tick")?;
        let _time_scale = bits.read_bits_u32(32, "time_scale")?;
        let equal_picture_interval = bits.read_bool("equal_picture_interval")?;
        if equal_picture_interval {
            let _num_ticks_per_picture_minus_1 = bits.read_uvlc("num_ticks_per_picture_minus_1")?;
        }
        let decoder_model_info_present_flag = bits.read_bool("decoder_model_info_present_flag")?;
        if decoder_model_info_present_flag {
            let _buffer_delay_length_minus_1 =
                bits.read_bits_u8(5, "buffer_delay_length_minus_1")?;
            let _num_units_in_decoding_tick =
                bits.read_bits_u32(32, "num_units_in_decoding_tick")?;
            let _buffer_removal_time_length_minus_1 =
                bits.read_bits_u8(5, "buffer_removal_time_length_minus_1")?;
            let _frame_presentation_time_length_minus_1 =
                bits.read_bits_u8(5, "frame_presentation_time_length_minus_1")?;
        }
    }

    let initial_display_delay_present_flag =
        bits.read_bool("initial_display_delay_present_flag")?;
    let operating_points_cnt_minus_1 = bits.read_bits_u8(5, "operating_points_cnt_minus_1")?;
    for _ in 0..=operating_points_cnt_minus_1 {
        let _operating_point_idc = bits.read_bits_u16(12, "operating_point_idc")?;
        let seq_level_idx = bits.read_bits_u8(5, "seq_level_idx")?;
        if seq_level_idx > 7 {
            let _seq_tier = bits.read_bool("seq_tier")?;
        }
        if initial_display_delay_present_flag {
            let initial_display_delay_present_for_this_op =
                bits.read_bool("initial_display_delay_present_for_this_op")?;
            if initial_display_delay_present_for_this_op {
                let _initial_display_delay_minus_1 =
                    bits.read_bits_u8(4, "initial_display_delay_minus_1")?;
            }
        }
    }

    Ok(Av1OperatingPointFlags {
        timing_info_present_flag,
        initial_display_delay_present_flag,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Av1ColorConfigParse {
    pub(super) film_grain_params_present: bool,
    pub(super) separate_uv_delta_q: bool,
}

pub(super) fn skip_av1_color_config_and_read_film_grain_flag(
    bits: &mut BitReader<'_>,
    seq_profile: u8,
) -> Result<Av1ColorConfigParse, String> {
    let high_bitdepth = bits.read_bool("high_bitdepth")?;
    if seq_profile == 2 && high_bitdepth {
        let _twelve_bit = bits.read_bool("twelve_bit")?;
    }
    let mono_chrome = if seq_profile == 1 {
        false
    } else {
        bits.read_bool("mono_chrome")?
    };
    let color_description_present_flag = bits.read_bool("color_description_present_flag")?;
    if color_description_present_flag {
        let _color_primaries = bits.read_bits_u8(8, "color_primaries")?;
        let _transfer_characteristics = bits.read_bits_u8(8, "transfer_characteristics")?;
        let _matrix_coefficients = bits.read_bits_u8(8, "matrix_coefficients")?;
    }
    let _color_range = bits.read_bool("color_range")?;
    let mut separate_uv_delta_q = false;
    if !mono_chrome {
        if seq_profile == 0 {
            let _chroma_sample_position = bits.read_bits_u8(2, "chroma_sample_position")?;
        }
        separate_uv_delta_q = bits.read_bool("separate_uv_delta_q")?;
    }
    let film_grain_params_present = bits.read_bool("film_grain_params_present")?;
    Ok(Av1ColorConfigParse {
        film_grain_params_present,
        separate_uv_delta_q,
    })
}

pub(super) fn read_av1_leb128(bytes: &[u8]) -> Result<(usize, usize), String> {
    let mut value = 0u64;
    for (index, byte) in bytes.iter().copied().take(8).enumerate() {
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            let value = usize::try_from(value)
                .map_err(|_| "LEB128 value does not fit in usize".to_string())?;
            return Ok((value, index + 1));
        }
    }
    if bytes.len() < 8 {
        Err("truncated LEB128 value".to_string())
    } else {
        Err("LEB128 value exceeds AV1 8-byte limit".to_string())
    }
}

pub(super) fn av1_obu_type(raw: u8) -> Av1ObuType {
    match raw {
        1 => Av1ObuType::SequenceHeader,
        2 => Av1ObuType::TemporalDelimiter,
        3 => Av1ObuType::FrameHeader,
        4 => Av1ObuType::TileGroup,
        5 => Av1ObuType::Metadata,
        6 => Av1ObuType::Frame,
        7 => Av1ObuType::RedundantFrameHeader,
        8 => Av1ObuType::TileList,
        15 => Av1ObuType::Padding,
        other => Av1ObuType::Unknown(other),
    }
}

pub(super) struct BitReader<'a> {
    pub(super) data: &'a [u8],
    pub(super) bit_offset: usize,
}

impl<'a> BitReader<'a> {
    pub(super) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            bit_offset: 0,
        }
    }

    pub(super) fn read_bool(&mut self, field_name: &str) -> Result<bool, String> {
        Ok(self.read_bits_u8(1, field_name)? != 0)
    }

    pub(super) fn read_bits_u8(&mut self, count: usize, field_name: &str) -> Result<u8, String> {
        u8::try_from(self.read_bits(count, field_name)?)
            .map_err(|_| format!("{field_name} does not fit in u8"))
    }

    pub(super) fn read_bits_u16(&mut self, count: usize, field_name: &str) -> Result<u16, String> {
        u16::try_from(self.read_bits(count, field_name)?)
            .map_err(|_| format!("{field_name} does not fit in u16"))
    }

    pub(super) fn read_bits_u32(&mut self, count: usize, field_name: &str) -> Result<u32, String> {
        u32::try_from(self.read_bits(count, field_name)?)
            .map_err(|_| format!("{field_name} does not fit in u32"))
    }

    pub(super) fn read_bits(&mut self, count: usize, field_name: &str) -> Result<u64, String> {
        if count > 64 {
            return Err(format!("{field_name} bit count {count} exceeds 64"));
        }
        let mut value = 0u64;
        for _ in 0..count {
            let byte = self
                .data
                .get(self.bit_offset / 8)
                .ok_or_else(|| format!("sequence header ended while reading {field_name}"))?;
            let bit = (byte >> (7 - (self.bit_offset % 8))) & 1;
            value = (value << 1) | u64::from(bit);
            self.bit_offset = self
                .bit_offset
                .checked_add(1)
                .ok_or_else(|| "bit offset overflow".to_string())?;
        }
        Ok(value)
    }

    pub(super) fn read_uvlc(&mut self, field_name: &str) -> Result<u64, String> {
        let mut leading_zeroes = 0usize;
        while !self.read_bool(field_name)? {
            leading_zeroes = leading_zeroes
                .checked_add(1)
                .ok_or_else(|| format!("{field_name} leading-zero count overflow"))?;
        }
        if leading_zeroes >= 63 {
            return Err(format!("{field_name} UVLC value exceeds u64 range"));
        }
        let suffix = if leading_zeroes == 0 {
            0
        } else {
            self.read_bits(leading_zeroes, field_name)?
        };
        Ok((1u64 << leading_zeroes) - 1 + suffix)
    }

    pub(super) fn byte_offset(&self) -> usize {
        self.bit_offset.div_ceil(8)
    }

    pub(super) fn align_to_next_byte(&mut self, field_name: &str) -> Result<(), String> {
        while !self.bit_offset.is_multiple_of(8) {
            let _ = self.read_bool(field_name)?;
        }
        Ok(())
    }
}
