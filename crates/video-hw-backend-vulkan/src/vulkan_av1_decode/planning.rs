//! AV1 decode planning and standard-picture translation.

use super::*;

pub(crate) fn build_av1_decode_submit_skeleton(
    bitstream: &[u8],
) -> Result<Av1DecodeSubmitSkeleton, String> {
    build_av1_decode_submit_skeletons(bitstream)?
        .into_iter()
        .next()
        .ok_or_else(|| "missing AV1 frame or tile-group OBU for decode submit".to_string())
}

pub(crate) fn build_av1_decode_submit_skeletons(
    bitstream: &[u8],
) -> Result<Vec<Av1DecodeSubmitSkeleton>, String> {
    let records = parse_av1_low_overhead_obus(bitstream)?;
    let sequence_header = records
        .iter()
        .find(|record| record.obu_type == Av1ObuType::SequenceHeader)
        .map(|record| parse_av1_sequence_header_payload(&bitstream[record.payload_range.clone()]))
        .transpose()?;
    let oxideav_sequence_header = records
        .iter()
        .find(|record| record.obu_type == Av1ObuType::SequenceHeader)
        .map(|record| {
            oxideav_av1::parse_sequence_header(&bitstream[record.payload_range.clone()])
                .map_err(|err| format!("oxideav AV1 sequence-header parse failed: {err}"))
        })
        .transpose()
        .ok()
        .flatten();
    let mut submits = Vec::new();
    let mut consumed_tile_groups = vec![false; records.len()];
    let mut oxideav_dpb = OxideAv1Dpb::new();

    for (index, record) in records.iter().enumerate() {
        match record.obu_type {
            Av1ObuType::Frame => {
                submits.push(build_av1_frame_obu_submit_skeleton(
                    bitstream,
                    record,
                    sequence_header.as_ref(),
                    oxideav_sequence_header.as_ref(),
                    &mut oxideav_dpb,
                )?);
            }
            Av1ObuType::FrameHeader => {
                let tile_groups: Vec<(usize, &Av1ObuRecord)> = records
                    .iter()
                    .enumerate()
                    .skip(index + 1)
                    .take_while(|(_, next)| next.temporal_unit_index == record.temporal_unit_index)
                    .take_while(|(_, next)| next.obu_type != Av1ObuType::FrameHeader)
                    .filter(|(_, next)| next.obu_type == Av1ObuType::TileGroup)
                    .collect();
                if !tile_groups.is_empty() {
                    for (tile_index, _) in &tile_groups {
                        consumed_tile_groups[*tile_index] = true;
                    }
                    submits.push(build_av1_tile_group_submit_skeleton(
                        bitstream,
                        record,
                        &tile_groups,
                        sequence_header.as_ref(),
                        oxideav_sequence_header.as_ref(),
                        &mut oxideav_dpb,
                    )?);
                }
            }
            Av1ObuType::TileGroup if !consumed_tile_groups[index] => {
                return Err(
                    "AV1 tile-group OBU is missing a preceding frame-header OBU".to_string()
                );
            }
            _ => {}
        }
    }

    if submits.is_empty() {
        return Err("missing AV1 frame or tile-group OBU for decode submit".to_string());
    }

    Ok(submits)
}

pub(crate) fn build_av1_decode_info_skeletons(
    bitstream: &[u8],
) -> Result<Vec<Av1DecodeInfoSkeleton>, String> {
    ensure_av1_sequence_header_before_first_frame(bitstream)?;
    let std_sequence_header = extract_av1_std_sequence_header(bitstream)?;
    let coded_width = u32::from(std_sequence_header.max_frame_width_minus_1) + 1;
    let coded_height = u32::from(std_sequence_header.max_frame_height_minus_1) + 1;

    build_av1_decode_submit_skeletons(bitstream)?
        .into_iter()
        .map(|submit| {
            build_av1_decode_info_skeleton_from_submit(
                bitstream.len(),
                coded_width,
                coded_height,
                submit,
            )
        })
        .collect::<Result<Vec<_>, String>>()
}

pub(super) fn apply_av1_reference_name_slot_replay(
    decodes: &mut [Av1DecodeInfoSkeleton],
    max_dpb_slots: u32,
) -> Result<(), String> {
    if max_dpb_slots == 0 {
        return Err("AV1 reference replay requires at least one DPB slot".to_string());
    }
    let max_dpb_slots = usize::try_from(max_dpb_slots)
        .map_err(|_| "AV1 max_dpb_slots does not fit usize".to_string())?;
    #[derive(Clone, Copy)]
    struct ReferenceFrameReplayState {
        slot_index: i32,
        base_array_layer: u32,
        order_hint: u8,
        reference_info: StdVideoDecodeAV1ReferenceInfo,
    }

    let mut reference_frame_slots: [Option<ReferenceFrameReplayState>; 8] = [None; 8];

    for (frame_index, decode) in decodes.iter_mut().enumerate() {
        let setup_slot_index = i32::try_from(frame_index % max_dpb_slots)
            .map_err(|_| "AV1 replay frame index exceeds i32 slot range".to_string())?;
        let setup_base_array_layer = u32::try_from(frame_index)
            .map_err(|_| "AV1 replay frame index exceeds u32 layer range".to_string())?;
        if let Some(summary) = decode.picture_info.frame_header_summary {
            let mut ref_frame_sign_bias = 0_u8;
            if summary.is_key_frame() {
                decode.picture_info.reference_name_slot_indices = [-1; 7];
                decode.picture_info.std_picture_info.OrderHints = [0; 8];
                if let Some(overrides) = decode.picture_info.std_overrides.as_mut() {
                    overrides.order_hints = [0; 8];
                }
                decode.reference_slot_infos.clear();
                decode.reference_base_array_layers.clear();
            } else {
                let mut order_hints = [0_u8; 8];
                let mut reference_slots = Vec::new();
                for (name_index, ref_frame_idx) in summary.ref_frame_idx.iter().enumerate() {
                    let ref_frame_idx = usize::try_from(*ref_frame_idx).map_err(|_| {
                        format!("AV1 ref_frame_idx[{name_index}] is negative: {ref_frame_idx}")
                    })?;
                    let state = reference_frame_slots
                        .get(ref_frame_idx)
                        .copied()
                        .flatten()
                        .ok_or_else(|| {
                            format!(
                                "AV1 reference frame state {ref_frame_idx} is unavailable for reference name {name_index}"
                            )
                        })?;
                    decode.picture_info.reference_name_slot_indices[name_index] = state.slot_index;
                    if let Some((_, existing_layer, _)) = reference_slots
                        .iter()
                        .find(|(slot_index, _, _)| *slot_index == state.slot_index)
                    {
                        if *existing_layer != state.base_array_layer {
                            return Err(format!(
                                "AV1 reference frame state aliases Vulkan DPB slot {} with multiple image layers ({} and {}) in one decode; stream requires more distinct active references than the reported max_dpb_slots={max_dpb_slots}",
                                state.slot_index, existing_layer, state.base_array_layer
                            ));
                        }
                    } else {
                        reference_slots.push((
                            state.slot_index,
                            state.base_array_layer,
                            state.reference_info,
                        ));
                    }
                    let reference_name = name_index + 1;
                    order_hints[reference_name] = state.order_hint;
                    if summary.enable_order_hint
                        && av1_relative_dist(
                            u32::from(state.order_hint),
                            u32::from(summary.order_hint),
                            summary.order_hint_bits,
                        ) > 0
                    {
                        ref_frame_sign_bias |= 1_u8 << reference_name;
                    }
                }
                decode.picture_info.std_picture_info.OrderHints = order_hints;
                if let Some(overrides) = decode.picture_info.std_overrides.as_mut() {
                    overrides.order_hints = order_hints;
                }
                decode.reference_slot_infos = reference_slots
                    .iter()
                    .map(|(_, _, reference_info)| *reference_info)
                    .collect();
                decode.reference_base_array_layers = reference_slots
                    .iter()
                    .map(|(_, base_array_layer, _)| *base_array_layer)
                    .collect();
            }

            let mut setup_reference_info = av1_std_reference_info(&decode.picture_info);
            setup_reference_info.SavedOrderHints = decode.picture_info.std_picture_info.OrderHints;
            setup_reference_info.RefFrameSignBias = ref_frame_sign_bias;
            for (ref_index, reference_frame_slot) in reference_frame_slots.iter_mut().enumerate() {
                if (summary.refresh_frame_flags & (1_u8 << ref_index)) != 0 {
                    *reference_frame_slot = Some(ReferenceFrameReplayState {
                        slot_index: setup_slot_index,
                        base_array_layer: setup_base_array_layer,
                        order_hint: decode.picture_info.std_picture_info.OrderHint,
                        reference_info: setup_reference_info,
                    });
                }
            }
            decode.setup_reference_info = setup_reference_info;
        } else if decode
            .picture_info
            .frame_header_summary
            .is_none_or(Av1ParsedFrameHeaderSummary::is_key_frame)
        {
            let mut setup_reference_info = av1_std_reference_info(&decode.picture_info);
            setup_reference_info.SavedOrderHints =
                [decode.picture_info.std_picture_info.OrderHint; 8];
            reference_frame_slots.fill(Some(ReferenceFrameReplayState {
                slot_index: setup_slot_index,
                base_array_layer: setup_base_array_layer,
                order_hint: decode.picture_info.std_picture_info.OrderHint,
                reference_info: setup_reference_info,
            }));
            decode.reference_slot_infos.clear();
            decode.reference_base_array_layers.clear();
            decode.setup_reference_info = setup_reference_info;
        }
    }

    Ok(())
}

pub(super) fn build_av1_decode_info_skeleton_from_submit(
    bitstream_len: usize,
    coded_width: u32,
    coded_height: u32,
    submit: Av1DecodeSubmitSkeleton,
) -> Result<Av1DecodeInfoSkeleton, String> {
    let picture_info = build_av1_decode_picture_info_skeleton(&submit)?;
    let (src_buffer_offset, src_buffer_range) =
        av1_decode_source_range(bitstream_len, &picture_info)?;
    let rebased_picture_info = rebase_av1_picture_info_offsets(picture_info, src_buffer_offset)?;

    Ok(Av1DecodeInfoSkeleton {
        temporal_unit_index: submit.temporal_unit_index,
        src_buffer_offset: u64::from(src_buffer_offset),
        src_buffer_range: u64::from(src_buffer_range),
        coded_width,
        coded_height,
        setup_reference_info: av1_std_reference_info(&rebased_picture_info),
        reference_slot_infos: Vec::new(),
        reference_base_array_layers: Vec::new(),
        picture_info: rebased_picture_info,
    })
}

pub(crate) fn build_av1_decode_info_skeleton(
    bitstream: &[u8],
) -> Result<Av1DecodeInfoSkeleton, String> {
    build_av1_decode_info_skeletons(bitstream)?
        .into_iter()
        .next()
        .ok_or_else(|| "missing AV1 frame or tile-group OBU for decode info".to_string())
}

pub(crate) fn build_av1_key_frame_decode_command_skeleton(
    bitstream: &[u8],
    max_dpb_slots: u32,
) -> Result<Av1DecodeCommandSkeleton, String> {
    let mut decodes = build_av1_decode_info_skeletons(bitstream)?;
    apply_av1_reference_name_slot_replay(&mut decodes, max_dpb_slots)?;
    build_av1_key_frame_decode_command_skeleton_from_decodes(&decodes, max_dpb_slots)
}

pub(crate) fn build_av1_aligned_decode_bitstream_upload_plan(
    bitstream: &[u8],
    offset_alignment: u64,
    size_alignment: u64,
    max_dpb_slots: u32,
) -> Result<Av1DecodeBitstreamUploadPlan, String> {
    let mut bytes = Vec::new();
    let mut aligned_decodes = Vec::new();

    let mut decodes = build_av1_decode_info_skeletons(bitstream)?;
    apply_av1_reference_name_slot_replay(&mut decodes, max_dpb_slots)?;

    for decode in decodes {
        let current_len =
            u64::try_from(bytes.len()).map_err(|_| "AV1 upload buffer length exceeds u64")?;
        let aligned_offset = align_up_av1_decode_value(current_len, offset_alignment);
        let padding = usize::try_from(aligned_offset.saturating_sub(current_len))
            .map_err(|_| "AV1 upload offset padding exceeds usize")?;
        bytes.resize(bytes.len() + padding, 0);

        let source_start = usize::try_from(decode.src_buffer_offset)
            .map_err(|_| "AV1 decode source offset exceeds usize")?;
        let source_len = usize::try_from(decode.src_buffer_range)
            .map_err(|_| "AV1 decode source range exceeds usize")?;
        let source_end = source_start
            .checked_add(source_len)
            .ok_or_else(|| "AV1 decode source range overflows usize".to_string())?;
        let source = bitstream
            .get(source_start..source_end)
            .ok_or_else(|| "AV1 decode source range exceeds bitstream length".to_string())?;
        bytes.extend_from_slice(source);

        let aligned_range = align_up_av1_decode_value(decode.src_buffer_range, size_alignment);
        let required_len = usize::try_from(aligned_offset.saturating_add(aligned_range))
            .map_err(|_| "AV1 aligned upload range exceeds usize")?;
        if bytes.len() < required_len {
            bytes.resize(required_len, 0);
        }

        let mut aligned_decode = decode;
        aligned_decode.src_buffer_offset = aligned_offset;
        aligned_decode.src_buffer_range = aligned_range;
        aligned_decodes.push(aligned_decode);
    }

    if aligned_decodes.is_empty() {
        return Err("missing AV1 frames for aligned decode upload plan".to_string());
    }

    Ok(Av1DecodeBitstreamUploadPlan {
        bytes,
        decodes: aligned_decodes,
    })
}

pub(crate) fn build_av1_aligned_key_frame_decode_command_skeleton(
    bitstream: &[u8],
    max_dpb_slots: u32,
    offset_alignment: u64,
    size_alignment: u64,
) -> Result<(Av1DecodeBitstreamUploadPlan, Av1DecodeCommandSkeleton), String> {
    let upload_plan = build_av1_aligned_decode_bitstream_upload_plan(
        bitstream,
        offset_alignment,
        size_alignment,
        max_dpb_slots,
    )?;
    let command = build_av1_key_frame_decode_command_skeleton_from_decodes(
        &upload_plan.decodes,
        max_dpb_slots,
    )?;

    Ok((upload_plan, command))
}

pub(super) fn build_av1_key_frame_decode_command_skeleton_from_decodes(
    decodes: &[Av1DecodeInfoSkeleton],
    max_dpb_slots: u32,
) -> Result<Av1DecodeCommandSkeleton, String> {
    if max_dpb_slots == 0 {
        return Err("AV1 decode command skeleton requires at least one DPB slot".to_string());
    }
    let max_dpb_slots = usize::try_from(max_dpb_slots)
        .map_err(|_| "AV1 max_dpb_slots does not fit usize".to_string())?;
    let first = decodes
        .first()
        .ok_or_else(|| "missing AV1 frames for decode command skeleton".to_string())?;

    let required_reference_slot_count = decodes
        .iter()
        .flat_map(Av1DecodeInfoSkeleton::reference_slot_indices)
        .filter_map(|slot_index| usize::try_from(slot_index).ok())
        .map(|slot_index| slot_index.saturating_add(1))
        .max()
        .unwrap_or(0);
    let planned_slot_count = decodes
        .len()
        .max(required_reference_slot_count)
        .min(max_dpb_slots)
        .max(2.min(max_dpb_slots));
    if required_reference_slot_count > planned_slot_count {
        return Err(format!(
            "AV1 reference slot index exceeds available DPB slots: required={}, available={}",
            required_reference_slot_count, planned_slot_count
        ));
    }
    let begin_slots = (0..planned_slot_count)
        .map(|slot| {
            let slot_index = i32::try_from(slot)
                .map_err(|_| "AV1 begin-coding slot index exceeds i32 range".to_string())?;
            let base_array_layer = u32::try_from(slot)
                .map_err(|_| "AV1 begin-coding slot index exceeds u32 range".to_string())?;
            Ok(Av1BeginCodingSlotSkeleton {
                slot_index,
                base_array_layer,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let frames = decodes
        .iter()
        .enumerate()
        .map(|(frame_index, decode)| {
            let setup_slot_index = i32::try_from(frame_index % planned_slot_count)
                .map_err(|_| "AV1 setup slot index exceeds i32 range".to_string())?;
            Ok(Av1DecodeFrameCommandSkeleton {
                frame_index,
                temporal_unit_index: decode.temporal_unit_index,
                setup_slot_index,
                dst_base_array_layer: u32::try_from(frame_index)
                    .map_err(|_| "AV1 dst base array layer exceeds u32 range".to_string())?,
                reference_slot_indices: decode.reference_slot_indices(),
                reference_base_array_layers: decode.reference_base_array_layers.clone(),
                src_buffer_offset: decode.src_buffer_offset,
                src_buffer_range: decode.src_buffer_range,
                tile_count: decode.tile_count(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(Av1DecodeCommandSkeleton {
        coded_width: first.coded_width,
        coded_height: first.coded_height,
        begin_slots,
        frames,
    })
}

pub(super) fn align_up_av1_decode_value(value: u64, alignment: u64) -> u64 {
    if alignment <= 1 {
        return value;
    }
    let remainder = value % alignment;
    if remainder == 0 {
        value
    } else {
        value.saturating_add(alignment.saturating_sub(remainder))
    }
}

pub(crate) fn build_av1_decode_picture_info_skeleton(
    submit: &Av1DecodeSubmitSkeleton,
) -> Result<Av1DecodePictureInfoSkeleton, String> {
    if submit.tile_offsets.is_empty() {
        return Err("AV1 decode picture info requires at least one tile offset".to_string());
    }
    if submit.tile_offsets.len() != submit.tile_sizes.len() {
        return Err(format!(
            "AV1 decode picture info tile offset/size count mismatch: offsets={}, sizes={}",
            submit.tile_offsets.len(),
            submit.tile_sizes.len()
        ));
    }
    if submit.tile_sizes.contains(&0) {
        return Err("AV1 decode picture info contains an empty tile payload".to_string());
    }

    Ok(Av1DecodePictureInfoSkeleton {
        std_picture_info: av1_std_picture_info_for_submit(submit)?,
        reference_name_slot_indices: if let Some(summary) = submit.frame_header_summary
            && !summary.is_key_frame()
        {
            summary.ref_frame_idx
        } else {
            [-1; vk::MAX_VIDEO_AV1_REFERENCES_PER_FRAME_KHR]
        },
        frame_header_offset: submit.frame_header_offset,
        tile_offsets: submit.tile_offsets.clone(),
        tile_sizes: submit.tile_sizes.clone(),
        use_128x128_superblock: submit.use_128x128_superblock,
        key_frame_header: submit.key_frame_header,
        frame_header_summary: submit.frame_header_summary,
        std_overrides: submit.std_overrides,
    })
}

pub(super) fn av1_std_picture_info_for_submit(
    submit: &Av1DecodeSubmitSkeleton,
) -> Result<StdVideoDecodeAV1PictureInfo, String> {
    let mut picture_info = key_frame_std_picture_info();
    if let Some(summary) = submit.frame_header_summary {
        picture_info.frame_type = av1_std_frame_type(summary.frame_type)?;
        picture_info.OrderHint = summary.order_hint;
        picture_info.primary_ref_frame = summary.primary_ref_frame;
        picture_info.refresh_frame_flags = summary.refresh_frame_flags;
        picture_info
            .flags
            .set_error_resilient_mode(u32::from(summary.error_resilient_mode));
        if let Some(overrides) = submit.std_overrides {
            picture_info.current_frame_id = overrides.current_frame_id;
            picture_info.interpolation_filter = overrides.interpolation_filter;
            picture_info.TxMode = overrides.tx_mode;
            picture_info.delta_q_res = overrides.delta_q_res;
            picture_info.delta_lf_res = overrides.delta_lf_res;
            picture_info.SkipModeFrame = overrides.skip_mode_frame;
            picture_info.coded_denom = overrides.coded_denom;
            picture_info.OrderHints = overrides.order_hints;
            picture_info.expectedFrameId = overrides.expected_frame_id;
            picture_info
                .flags
                .set_disable_cdf_update(bool_to_u32(overrides.disable_cdf_update));
            picture_info
                .flags
                .set_use_superres(bool_to_u32(overrides.use_superres));
            picture_info
                .flags
                .set_render_and_frame_size_different(bool_to_u32(
                    overrides.render_and_frame_size_different,
                ));
            picture_info
                .flags
                .set_allow_screen_content_tools(bool_to_u32(overrides.allow_screen_content_tools));
            picture_info
                .flags
                .set_force_integer_mv(bool_to_u32(overrides.force_integer_mv));
            picture_info
                .flags
                .set_frame_size_override_flag(bool_to_u32(overrides.frame_size_override_flag));
            picture_info
                .flags
                .set_allow_intrabc(bool_to_u32(overrides.allow_intrabc));
            picture_info
                .flags
                .set_allow_high_precision_mv(bool_to_u32(overrides.allow_high_precision_mv));
            picture_info
                .flags
                .set_is_filter_switchable(bool_to_u32(overrides.is_filter_switchable));
            picture_info
                .flags
                .set_is_motion_mode_switchable(bool_to_u32(overrides.is_motion_mode_switchable));
            picture_info
                .flags
                .set_use_ref_frame_mvs(bool_to_u32(overrides.use_ref_frame_mvs));
            picture_info
                .flags
                .set_disable_frame_end_update_cdf(bool_to_u32(
                    overrides.disable_frame_end_update_cdf,
                ));
            picture_info
                .flags
                .set_allow_warped_motion(bool_to_u32(overrides.allow_warped_motion));
            picture_info
                .flags
                .set_reduced_tx_set(bool_to_u32(overrides.reduced_tx_set));
            picture_info
                .flags
                .set_reference_select(bool_to_u32(overrides.reference_select));
            picture_info
                .flags
                .set_skip_mode_present(bool_to_u32(overrides.skip_mode_present));
            picture_info
                .flags
                .set_delta_q_present(bool_to_u32(overrides.delta_q_present));
            picture_info
                .flags
                .set_delta_lf_present(bool_to_u32(overrides.delta_lf_present));
            picture_info
                .flags
                .set_delta_lf_multi(bool_to_u32(overrides.delta_lf_multi));
            picture_info
                .flags
                .set_segmentation_enabled(bool_to_u32(overrides.segmentation_enabled));
            picture_info
                .flags
                .set_segmentation_update_map(bool_to_u32(overrides.segmentation_update_map));
            picture_info
                .flags
                .set_segmentation_temporal_update(bool_to_u32(
                    overrides.segmentation_temporal_update,
                ));
            picture_info
                .flags
                .set_segmentation_update_data(bool_to_u32(overrides.segmentation_update_data));
            picture_info
                .flags
                .set_UsesLr(bool_to_u32(overrides.uses_lr));
            picture_info
                .flags
                .set_usesChromaLr(bool_to_u32(overrides.uses_chroma_lr));
            picture_info
                .flags
                .set_apply_grain(bool_to_u32(overrides.apply_grain));
        }
    }
    Ok(picture_info)
}

pub(super) fn av1_std_frame_type(
    frame_type: u8,
) -> Result<ash::vk::native::StdVideoAV1FrameType, String> {
    match frame_type {
        0 => Ok(StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_KEY),
        1 => Ok(StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_INTER),
        2 => Ok(StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_INTRA_ONLY),
        3 => Ok(StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_SWITCH),
        other => Err(format!("unsupported AV1 frame_type {other}")),
    }
}

pub(super) fn parse_av1_frame_obu_summary_with_dpb(
    sequence_header: &OxideAv1SequenceHeader,
    payload: &[u8],
    dpb: &mut OxideAv1Dpb,
) -> Result<Av1ParsedFrameHeaderSummary, String> {
    let (frame_header, tile_payload) = parse_frame_obu_with_dpb(sequence_header, payload, dpb)
        .map_err(|err| format!("oxideav AV1 frame OBU parse failed: {err}"))?;
    let tile_payload_offset = payload
        .len()
        .checked_sub(tile_payload.len())
        .ok_or_else(|| "oxideav AV1 frame OBU tile payload exceeds payload length".to_string())?;
    update_oxideav_dpb_from_frame_header(dpb, &frame_header);
    av1_frame_header_summary_from_oxideav(sequence_header, frame_header, tile_payload_offset)
}

pub(super) fn reject_show_existing_av1_frame(
    frame_header_summary: Option<Av1ParsedFrameHeaderSummary>,
) -> Result<(), String> {
    if frame_header_summary.is_some_and(|summary| summary.show_existing_frame) {
        return Err(
            "AV1 frame-header parsing is not implemented: show_existing_frame has no tile payload"
                .to_string(),
        );
    }
    Ok(())
}

pub(super) fn parse_av1_frame_header_summary_with_dpb(
    sequence_header: &OxideAv1SequenceHeader,
    payload: &[u8],
    dpb: &mut OxideAv1Dpb,
) -> Result<Av1ParsedFrameHeaderSummary, String> {
    let frame_header = parse_frame_header_with_dpb(sequence_header, payload, dpb)
        .map_err(|err| format!("oxideav AV1 frame-header parse failed: {err}"))?;
    update_oxideav_dpb_from_frame_header(dpb, &frame_header);
    av1_frame_header_summary_from_oxideav(sequence_header, frame_header, 0)
}

pub(super) fn update_oxideav_dpb_from_frame_header(
    dpb: &mut OxideAv1Dpb,
    frame_header: &OxideAv1FrameHeader,
) {
    if frame_header.frame_type == OxideAv1FrameType::Key {
        dpb.reset();
    }
    if frame_header.refresh_frame_flags != 0 {
        dpb.refresh_with_gm(
            frame_header.refresh_frame_flags,
            frame_header.order_hint,
            None,
            &frame_header.gm_params,
        );
    }
}

pub(super) fn av1_frame_header_summary_from_oxideav(
    sequence_header: &OxideAv1SequenceHeader,
    frame_header: OxideAv1FrameHeader,
    tile_payload_offset: usize,
) -> Result<Av1ParsedFrameHeaderSummary, String> {
    let frame_type = match frame_header.frame_type {
        OxideAv1FrameType::Key => 0,
        OxideAv1FrameType::Inter => 1,
        OxideAv1FrameType::IntraOnly => 2,
        OxideAv1FrameType::Switch => 3,
    };
    let order_hint = u8::try_from(frame_header.order_hint)
        .map_err(|_| format!("AV1 order_hint exceeds u8: {}", frame_header.order_hint))?;
    let primary_ref_frame = u8::try_from(frame_header.primary_ref_frame).map_err(|_| {
        format!(
            "AV1 primary_ref_frame exceeds u8: {}",
            frame_header.primary_ref_frame
        )
    })?;
    let mut ref_frame_idx = [-1_i32; vk::MAX_VIDEO_AV1_REFERENCES_PER_FRAME_KHR];
    for (dst, src) in ref_frame_idx
        .iter_mut()
        .zip(frame_header.ref_frame_idx.iter())
    {
        *dst = i32::try_from(*src).map_err(|_| format!("AV1 ref_frame_idx exceeds i32: {src}"))?;
    }

    Ok(Av1ParsedFrameHeaderSummary {
        frame_type,
        show_existing_frame: frame_header.show_existing_frame,
        error_resilient_mode: frame_header.error_resilient_mode,
        order_hint,
        enable_order_hint: sequence_header.enable_order_hint,
        order_hint_bits: sequence_header.order_hint_bits,
        primary_ref_frame,
        refresh_frame_flags: frame_header.refresh_frame_flags,
        ref_frame_idx,
        tile_payload_offset,
        std_overrides: Some(av1_std_picture_overrides_from_oxideav(&frame_header)?),
    })
}

pub(super) fn av1_relative_dist(a: u32, b: u32, order_hint_bits: u32) -> i32 {
    if order_hint_bits == 0 {
        return 0;
    }
    let diff = a.wrapping_sub(b);
    let m = 1_u32 << (order_hint_bits - 1);
    ((diff & (m - 1)) as i32) - ((diff & m) as i32)
}

pub(super) fn av1_std_picture_overrides_from_oxideav(
    frame_header: &OxideAv1FrameHeader,
) -> Result<Av1StdPictureOverrides, String> {
    Ok(Av1StdPictureOverrides {
        disable_cdf_update: frame_header.disable_cdf_update,
        current_frame_id: frame_header.current_frame_id,
        use_superres: frame_header.use_superres,
        render_and_frame_size_different: frame_header.render_and_frame_size_different,
        allow_screen_content_tools: frame_header.allow_screen_content_tools != 0,
        force_integer_mv: frame_header.force_integer_mv != 0,
        frame_size_override_flag: frame_header.frame_size_override_flag,
        allow_intrabc: frame_header.allow_intrabc,
        allow_high_precision_mv: frame_header.allow_high_precision_mv,
        is_filter_switchable: frame_header.is_filter_switchable,
        interpolation_filter: av1_std_interpolation_filter(frame_header.interpolation_filter)?,
        is_motion_mode_switchable: frame_header.is_motion_mode_switchable,
        use_ref_frame_mvs: frame_header.use_ref_frame_mvs,
        disable_frame_end_update_cdf: frame_header.disable_frame_end_update_cdf,
        allow_warped_motion: frame_header.allow_warped_motion,
        reduced_tx_set: frame_header.reduced_tx_set,
        reference_select: frame_header.reference_select,
        skip_mode_present: frame_header.skip_mode_present,
        delta_q_present: frame_header.delta_q_present,
        delta_q_res: frame_header.delta_q_res,
        delta_lf_present: frame_header.delta_lf_present,
        delta_lf_res: frame_header.delta_lf_res,
        delta_lf_multi: frame_header.delta_lf_multi,
        segmentation_enabled: frame_header.segmentation.enabled,
        segmentation_update_map: frame_header.segmentation.update_map,
        segmentation_temporal_update: frame_header.segmentation.temporal_update,
        segmentation_update_data: frame_header.segmentation.update_data,
        uses_lr: frame_header.lr.uses_lr,
        uses_chroma_lr: frame_header.lr.uses_chroma_lr,
        apply_grain: frame_header.film_grain.apply_grain,
        skip_mode_frame: frame_header.skip_mode_frame,
        coded_denom: u8::try_from(frame_header.superres_denom).map_err(|_| {
            format!(
                "AV1 superres denom exceeds u8: {}",
                frame_header.superres_denom
            )
        })?,
        order_hints: av1_u32_array_to_u8(frame_header.ref_order_hint, "AV1 ref_order_hint")?,
        expected_frame_id: [0; 8],
        tx_mode: av1_std_tx_mode(frame_header.tx_mode),
        quantization: av1_std_quantization_from_oxideav(&frame_header.quant),
        segmentation: av1_std_segmentation_from_oxideav(&frame_header.segmentation),
        loop_filter: av1_std_loop_filter_from_oxideav(&frame_header.loop_filter),
        cdef: av1_std_cdef_from_oxideav(&frame_header.cdef),
        loop_restoration: av1_std_loop_restoration_from_oxideav(&frame_header.lr)?,
        global_motion: av1_std_global_motion_from_oxideav(frame_header)?,
    })
}

pub(super) fn av1_u32_array_to_u8(values: [u32; 8], field_name: &str) -> Result<[u8; 8], String> {
    let mut out = [0_u8; 8];
    for (dst, src) in out.iter_mut().zip(values) {
        *dst = u8::try_from(src).map_err(|_| format!("{field_name} exceeds u8: {src}"))?;
    }
    Ok(out)
}

pub(super) fn av1_std_interpolation_filter(
    filter: u32,
) -> Result<ash::vk::native::StdVideoAV1InterpolationFilter, String> {
    match filter {
        0 => Ok(StdVideoAV1InterpolationFilter_STD_VIDEO_AV1_INTERPOLATION_FILTER_EIGHTTAP),
        1 => Ok(StdVideoAV1InterpolationFilter_STD_VIDEO_AV1_INTERPOLATION_FILTER_EIGHTTAP_SMOOTH),
        2 => Ok(StdVideoAV1InterpolationFilter_STD_VIDEO_AV1_INTERPOLATION_FILTER_EIGHTTAP_SHARP),
        3 => Ok(StdVideoAV1InterpolationFilter_STD_VIDEO_AV1_INTERPOLATION_FILTER_BILINEAR),
        4 => Ok(StdVideoAV1InterpolationFilter_STD_VIDEO_AV1_INTERPOLATION_FILTER_SWITCHABLE),
        other => Err(format!("unsupported AV1 interpolation_filter {other}")),
    }
}

pub(super) fn av1_std_tx_mode(tx_mode: OxideAv1TxMode) -> ash::vk::native::StdVideoAV1TxMode {
    match tx_mode {
        OxideAv1TxMode::Only4x4 => StdVideoAV1TxMode_STD_VIDEO_AV1_TX_MODE_ONLY_4X4,
        OxideAv1TxMode::Largest => StdVideoAV1TxMode_STD_VIDEO_AV1_TX_MODE_LARGEST,
        OxideAv1TxMode::Select => StdVideoAV1TxMode_STD_VIDEO_AV1_TX_MODE_SELECT,
    }
}

pub(super) fn av1_std_quantization_from_oxideav(
    quant: &oxideav_av1::frame_header_tail::QuantizationParams,
) -> StdVideoAV1Quantization {
    StdVideoAV1Quantization {
        flags: StdVideoAV1QuantizationFlags {
            _bitfield_align_1: [],
            _bitfield_1: StdVideoAV1QuantizationFlags::new_bitfield_1(
                bool_to_u32(quant.using_qmatrix),
                bool_to_u32(quant.diff_uv_delta),
                0,
            ),
        },
        base_q_idx: quant.base_q_idx,
        DeltaQYDc: quant.delta_q_y_dc,
        DeltaQUDc: quant.delta_q_u_dc,
        DeltaQUAc: quant.delta_q_u_ac,
        DeltaQVDc: quant.delta_q_v_dc,
        DeltaQVAc: quant.delta_q_v_ac,
        qm_y: quant.qm_y,
        qm_u: quant.qm_u,
        qm_v: quant.qm_v,
    }
}

pub(super) fn av1_std_segmentation_from_oxideav(
    segmentation: &oxideav_av1::frame_header_tail::SegmentationParams,
) -> StdVideoAV1Segmentation {
    let mut feature_enabled = [0_u8; 8];
    for (segment_index, enabled) in segmentation.feature_enabled.iter().enumerate() {
        let mut bits = 0_u8;
        for (feature_index, active) in enabled.iter().enumerate() {
            if *active {
                bits |= 1_u8 << feature_index;
            }
        }
        feature_enabled[segment_index] = bits;
    }

    StdVideoAV1Segmentation {
        FeatureEnabled: feature_enabled,
        FeatureData: segmentation.feature_data,
    }
}

pub(super) fn av1_std_loop_filter_from_oxideav(
    loop_filter: &oxideav_av1::frame_header_tail::LoopFilterParams,
) -> StdVideoAV1LoopFilter {
    StdVideoAV1LoopFilter {
        flags: StdVideoAV1LoopFilterFlags {
            _bitfield_align_1: [],
            _bitfield_1: StdVideoAV1LoopFilterFlags::new_bitfield_1(
                bool_to_u32(loop_filter.mode_ref_delta_enabled),
                bool_to_u32(loop_filter.mode_ref_delta_update),
                0,
            ),
        },
        loop_filter_level: [
            loop_filter.level_y0,
            loop_filter.level_y1,
            loop_filter.level_u,
            loop_filter.level_v,
        ],
        loop_filter_sharpness: loop_filter.sharpness,
        update_ref_delta: bool_to_u8(loop_filter.mode_ref_delta_update),
        loop_filter_ref_deltas: loop_filter.ref_deltas,
        update_mode_delta: bool_to_u8(loop_filter.mode_ref_delta_update),
        loop_filter_mode_deltas: loop_filter.mode_deltas,
    }
}

pub(super) fn av1_std_cdef_from_oxideav(
    cdef: &oxideav_av1::frame_header_tail::CdefParams,
) -> StdVideoAV1CDEF {
    StdVideoAV1CDEF {
        cdef_damping_minus_3: cdef.cdef_damping_minus3,
        cdef_bits: cdef.cdef_bits,
        cdef_y_pri_strength: cdef.y_pri_strengths,
        cdef_y_sec_strength: cdef.y_sec_strengths,
        cdef_uv_pri_strength: cdef.uv_pri_strengths,
        cdef_uv_sec_strength: cdef.uv_sec_strengths,
    }
}

pub(super) fn av1_std_loop_restoration_from_oxideav(
    lr: &OxideAv1LoopRestorationParams,
) -> Result<StdVideoAV1LoopRestoration, String> {
    Ok(StdVideoAV1LoopRestoration {
        FrameRestorationType: [
            av1_std_restoration_type(lr.frame_restoration_type[0])?,
            av1_std_restoration_type(lr.frame_restoration_type[1])?,
            av1_std_restoration_type(lr.frame_restoration_type[2])?,
        ],
        LoopRestorationSize: [
            av1_restoration_size(lr.log2_restoration_unit_size[0]),
            av1_restoration_size(lr.log2_restoration_unit_size[1]),
            av1_restoration_size(lr.log2_restoration_unit_size[2]),
        ],
    })
}

pub(super) fn av1_std_restoration_type(
    value: u8,
) -> Result<ash::vk::native::StdVideoAV1FrameRestorationType, String> {
    match value {
        0 => Ok(StdVideoAV1FrameRestorationType_STD_VIDEO_AV1_FRAME_RESTORATION_TYPE_NONE),
        RESTORATION_WIENER => Ok(
            ash::vk::native::StdVideoAV1FrameRestorationType_STD_VIDEO_AV1_FRAME_RESTORATION_TYPE_WIENER,
        ),
        RESTORATION_SGR => Ok(
            ash::vk::native::StdVideoAV1FrameRestorationType_STD_VIDEO_AV1_FRAME_RESTORATION_TYPE_SGRPROJ,
        ),
        RESTORATION_SWITCHABLE => Ok(
            ash::vk::native::StdVideoAV1FrameRestorationType_STD_VIDEO_AV1_FRAME_RESTORATION_TYPE_SWITCHABLE,
        ),
        other => Err(format!("unsupported AV1 restoration type {other}")),
    }
}

pub(super) fn av1_restoration_size(log2_size: u8) -> u16 {
    if log2_size == 0 {
        1
    } else {
        u16::from(log2_size.saturating_sub(5))
    }
}

pub(super) fn av1_std_global_motion_from_oxideav(
    frame_header: &OxideAv1FrameHeader,
) -> Result<StdVideoAV1GlobalMotion, String> {
    let mut gm_type = [0_u8; 8];
    for (dst, src) in gm_type.iter_mut().zip(frame_header.gm_type) {
        *dst = av1_std_gm_type(src);
    }
    Ok(StdVideoAV1GlobalMotion {
        GmType: gm_type,
        gm_params: frame_header.gm_params,
    })
}

pub(super) fn av1_std_gm_type(gm_type: OxideAv1GmType) -> u8 {
    match gm_type {
        OxideAv1GmType::Identity => 0,
        OxideAv1GmType::Translation => 1,
        OxideAv1GmType::RotZoom => 2,
        OxideAv1GmType::Affine => 3,
    }
}

pub(super) fn ensure_av1_sequence_header_before_first_frame(
    bitstream: &[u8],
) -> Result<(), String> {
    let records = parse_av1_low_overhead_obus(bitstream)?;
    let sequence_header_start = records
        .iter()
        .find(|record| record.obu_type == Av1ObuType::SequenceHeader)
        .map(|record| record.obu_range.start)
        .ok_or_else(|| "missing AV1 sequence header OBU before first frame".to_string())?;
    let first_frame_start = records
        .iter()
        .find(|record| {
            matches!(
                record.obu_type,
                Av1ObuType::Frame | Av1ObuType::FrameHeader | Av1ObuType::TileGroup
            )
        })
        .map(|record| record.obu_range.start);

    if first_frame_start.is_some_and(|frame_start| sequence_header_start > frame_start) {
        return Err("AV1 sequence header OBU appears after the first frame payload".to_string());
    }

    Ok(())
}

impl ParsedAv1SequenceHeader {
    pub(super) fn coded_width(&self) -> u32 {
        self.max_frame_width_minus_1 + 1
    }

    pub(super) fn coded_height(&self) -> u32 {
        self.max_frame_height_minus_1 + 1
    }
}

pub(super) fn av1_decode_source_range(
    bitstream_len: usize,
    picture_info: &Av1DecodePictureInfoSkeleton,
) -> Result<(u32, u32), String> {
    let frame_header_offset = picture_info.frame_header_offset;
    let mut range_start = frame_header_offset;
    let mut range_end = frame_header_offset
        .checked_add(1)
        .ok_or_else(|| "AV1 frame header offset overflows u32".to_string())?;

    for (&tile_offset, &tile_size) in picture_info
        .tile_offsets
        .iter()
        .zip(picture_info.tile_sizes.iter())
    {
        if tile_size == 0 {
            return Err("AV1 decode info contains an empty tile payload".to_string());
        }
        let tile_end = tile_offset
            .checked_add(tile_size)
            .ok_or_else(|| "AV1 tile payload range overflows u32".to_string())?;
        range_start = range_start.min(tile_offset);
        range_end = range_end.max(tile_end);
    }

    let bitstream_len = u32::try_from(bitstream_len)
        .map_err(|_| "AV1 bitstream is too large for Vulkan u32 offsets".to_string())?;
    if range_end > bitstream_len {
        return Err(format!(
            "AV1 decode source range exceeds bitstream length: end={range_end}, len={bitstream_len}"
        ));
    }
    let range_len = range_end
        .checked_sub(range_start)
        .ok_or_else(|| "AV1 decode source range is inverted".to_string())?;
    if range_len == 0 {
        return Err("AV1 decode source range is empty".to_string());
    }

    Ok((range_start, range_len))
}

pub(super) fn rebase_av1_picture_info_offsets(
    mut picture_info: Av1DecodePictureInfoSkeleton,
    src_buffer_offset: u32,
) -> Result<Av1DecodePictureInfoSkeleton, String> {
    picture_info.frame_header_offset = picture_info
        .frame_header_offset
        .checked_sub(src_buffer_offset)
        .ok_or_else(|| "AV1 frame header offset precedes source buffer range".to_string())?;
    for tile_offset in &mut picture_info.tile_offsets {
        *tile_offset = tile_offset
            .checked_sub(src_buffer_offset)
            .ok_or_else(|| "AV1 tile offset precedes source buffer range".to_string())?;
    }
    Ok(picture_info)
}

pub(super) fn key_frame_std_reference_info(
    picture_info: &Av1DecodePictureInfoSkeleton,
) -> StdVideoDecodeAV1ReferenceInfo {
    key_frame_std_reference_info_for_order_hint(picture_info.std_picture_info.OrderHint)
}

pub(super) fn av1_std_reference_info(
    picture_info: &Av1DecodePictureInfoSkeleton,
) -> StdVideoDecodeAV1ReferenceInfo {
    if picture_info
        .frame_header_summary
        .is_none_or(Av1ParsedFrameHeaderSummary::is_key_frame)
    {
        return key_frame_std_reference_info(picture_info);
    }
    StdVideoDecodeAV1ReferenceInfo {
        flags: StdVideoDecodeAV1ReferenceInfoFlags {
            _bitfield_align_1: [],
            _bitfield_1: StdVideoDecodeAV1ReferenceInfoFlags::new_bitfield_1(
                picture_info
                    .std_picture_info
                    .flags
                    .disable_frame_end_update_cdf(),
                picture_info.std_picture_info.flags.segmentation_enabled(),
                0,
            ),
        },
        frame_type: picture_info.std_picture_info.frame_type as u8,
        RefFrameSignBias: 0,
        OrderHint: picture_info.std_picture_info.OrderHint,
        SavedOrderHints: picture_info.std_picture_info.OrderHints,
    }
}

pub(super) fn key_frame_std_reference_info_for_order_hint(
    order_hint: u8,
) -> StdVideoDecodeAV1ReferenceInfo {
    StdVideoDecodeAV1ReferenceInfo {
        flags: StdVideoDecodeAV1ReferenceInfoFlags {
            _bitfield_align_1: [],
            _bitfield_1: StdVideoDecodeAV1ReferenceInfoFlags::new_bitfield_1(0, 0, 0),
        },
        frame_type: StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_KEY as u8,
        RefFrameSignBias: 0,
        OrderHint: order_hint,
        SavedOrderHints: [0; 8],
    }
}

pub(super) fn key_frame_std_picture_info() -> StdVideoDecodeAV1PictureInfo {
    StdVideoDecodeAV1PictureInfo {
        flags: StdVideoDecodeAV1PictureInfoFlags {
            _bitfield_align_1: [],
            _bitfield_1: StdVideoDecodeAV1PictureInfoFlags::new_bitfield_1(
                1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0,
            ),
        },
        frame_type: StdVideoAV1FrameType_STD_VIDEO_AV1_FRAME_TYPE_KEY,
        current_frame_id: 0,
        OrderHint: 0,
        primary_ref_frame: 7,
        refresh_frame_flags: 0xff,
        reserved1: 0,
        interpolation_filter:
            StdVideoAV1InterpolationFilter_STD_VIDEO_AV1_INTERPOLATION_FILTER_SWITCHABLE,
        TxMode: StdVideoAV1TxMode_STD_VIDEO_AV1_TX_MODE_SELECT,
        delta_q_res: 0,
        delta_lf_res: 0,
        SkipModeFrame: [0; 2],
        coded_denom: 0,
        reserved2: [0; 3],
        OrderHints: [0; 8],
        expectedFrameId: [0; 8],
        pTileInfo: std::ptr::null(),
        pQuantization: std::ptr::null(),
        pSegmentation: std::ptr::null(),
        pLoopFilter: std::ptr::null(),
        pCDEF: std::ptr::null(),
        pLoopRestoration: std::ptr::null(),
        pGlobalMotion: std::ptr::null(),
        pFilmGrain: std::ptr::null(),
    }
}

pub(super) fn av1_identity_global_motion_params() -> [[i32; 6]; 8] {
    let mut params = [[0; 6]; 8];
    for ref_params in &mut params {
        ref_params[2] = AV1_WARPEDMODEL_PRECISION_IDENTITY;
        ref_params[5] = AV1_WARPEDMODEL_PRECISION_IDENTITY;
    }
    params
}

pub(super) fn build_av1_frame_obu_submit_skeleton(
    bitstream: &[u8],
    frame_record: &Av1ObuRecord,
    sequence_header: Option<&ParsedAv1SequenceHeader>,
    oxideav_sequence_header: Option<&OxideAv1SequenceHeader>,
    oxideav_dpb: &mut OxideAv1Dpb,
) -> Result<Av1DecodeSubmitSkeleton, String> {
    let frame_header_offset = u32::try_from(frame_record.obu_range.start)
        .map_err(|_| "AV1 frame header offset exceeds u32 range".to_string())?;
    let frame_payload_offset = u32::try_from(frame_record.payload_range.start)
        .map_err(|_| "AV1 frame OBU payload offset exceeds u32 range".to_string())?;
    let payload = bitstream
        .get(frame_record.payload_range.clone())
        .ok_or_else(|| "AV1 frame OBU payload range exceeds bitstream".to_string())?;
    let frame_header_summary = oxideav_sequence_header.and_then(|sequence_header| {
        parse_av1_frame_obu_summary_with_dpb(sequence_header, payload, oxideav_dpb).ok()
    });
    reject_show_existing_av1_frame(frame_header_summary)?;
    let key_frame_header =
        if frame_header_summary.is_none_or(Av1ParsedFrameHeaderSummary::is_key_frame) {
            handle_av1_key_frame_header_parse_result(
                parse_av1_key_frame_obu_header(payload, sequence_header),
                sequence_header,
            )?
        } else {
            None
        };
    let tile_payload_offset = frame_header_summary
        .map(|summary| summary.tile_payload_offset)
        .or_else(|| key_frame_header.map(|header| header.tile_payload_offset))
        .unwrap_or(0);
    let tile_offset = frame_payload_offset
        .checked_add(
            u32::try_from(tile_payload_offset)
                .map_err(|_| "AV1 frame OBU tile payload offset exceeds u32 range".to_string())?,
        )
        .ok_or_else(|| "AV1 frame OBU tile payload offset overflows u32".to_string())?;
    let tile_size = u32::try_from(
        frame_record
            .payload_range
            .len()
            .checked_sub(tile_payload_offset)
            .ok_or_else(|| "AV1 frame OBU tile payload offset exceeds payload size".to_string())?,
    )
    .map_err(|_| "AV1 frame OBU payload size exceeds u32 range".to_string())?;
    if tile_size == 0 {
        return Err("AV1 frame OBU payload is empty".to_string());
    }
    Ok(Av1DecodeSubmitSkeleton {
        temporal_unit_index: frame_record.temporal_unit_index,
        frame_header_offset,
        tile_offsets: vec![tile_offset],
        tile_sizes: vec![tile_size],
        use_128x128_superblock: sequence_header.is_some_and(|header| header.use_128x128_superblock),
        key_frame_header,
        frame_header_summary,
        std_overrides: frame_header_summary.and_then(|summary| summary.std_overrides),
    })
}

pub(super) fn handle_av1_key_frame_header_parse_result(
    result: Result<Av1ParsedKeyFrameHeader, String>,
    sequence_header: Option<&ParsedAv1SequenceHeader>,
) -> Result<Option<Av1ParsedKeyFrameHeader>, String> {
    result.map(Some).or_else(|err| {
        if sequence_header.is_some_and(|header| !header.reduced_still_picture_header) {
            Err(if err.contains("got frame_type=") {
                format!("AV1 inter-frame decode is not implemented: {err}")
            } else {
                format!("AV1 frame-header parsing is not implemented: {err}")
            })
        } else {
            Ok(None)
        }
    })
}

pub(super) fn build_av1_tile_group_submit_skeleton(
    bitstream: &[u8],
    frame_header_record: &Av1ObuRecord,
    tile_group_records: &[(usize, &Av1ObuRecord)],
    sequence_header: Option<&ParsedAv1SequenceHeader>,
    oxideav_sequence_header: Option<&OxideAv1SequenceHeader>,
    oxideav_dpb: &mut OxideAv1Dpb,
) -> Result<Av1DecodeSubmitSkeleton, String> {
    let frame_header_offset = u32::try_from(frame_header_record.obu_range.start)
        .map_err(|_| "AV1 frame header offset exceeds u32 range".to_string())?;
    let frame_header_payload = bitstream
        .get(frame_header_record.payload_range.clone())
        .ok_or_else(|| "AV1 frame-header OBU payload range exceeds bitstream".to_string())?;
    let frame_header_summary = oxideav_sequence_header.and_then(|sequence_header| {
        parse_av1_frame_header_summary_with_dpb(sequence_header, frame_header_payload, oxideav_dpb)
            .ok()
    });
    reject_show_existing_av1_frame(frame_header_summary)?;
    let key_frame_header =
        if frame_header_summary.is_none_or(Av1ParsedFrameHeaderSummary::is_key_frame) {
            handle_av1_key_frame_header_parse_result(
                parse_av1_key_frame_obu_header(frame_header_payload, sequence_header),
                sequence_header,
            )?
        } else {
            None
        };

    let mut tile_offsets = Vec::with_capacity(tile_group_records.len());
    let mut tile_sizes = Vec::with_capacity(tile_group_records.len());
    for (_, tile_group_record) in tile_group_records {
        let tile_offset = u32::try_from(tile_group_record.payload_range.start)
            .map_err(|_| "AV1 tile offset exceeds u32 range".to_string())?;
        let tile_size = u32::try_from(tile_group_record.payload_range.len())
            .map_err(|_| "AV1 tile group payload size exceeds u32 range".to_string())?;
        if tile_size == 0 {
            return Err("AV1 tile-group OBU payload is empty".to_string());
        }
        tile_offsets.push(tile_offset);
        tile_sizes.push(tile_size);
    }

    Ok(Av1DecodeSubmitSkeleton {
        temporal_unit_index: frame_header_record.temporal_unit_index,
        frame_header_offset,
        tile_offsets,
        tile_sizes,
        use_128x128_superblock: sequence_header.is_some_and(|header| header.use_128x128_superblock),
        key_frame_header,
        frame_header_summary,
        std_overrides: frame_header_summary.and_then(|summary| summary.std_overrides),
    })
}

pub(super) fn parse_av1_key_frame_obu_header(
    payload: &[u8],
    sequence_header: Option<&ParsedAv1SequenceHeader>,
) -> Result<Av1ParsedKeyFrameHeader, String> {
    let mut bits = BitReader::new(payload);
    let show_existing_frame = bits.read_bool("show_existing_frame")?;
    if show_existing_frame {
        return Err("show_existing_frame has no tile payload".to_string());
    }
    let frame_type = bits.read_bits_u8(2, "frame_type")?;
    if frame_type != 0 {
        return Err(format!(
            "only AV1 key-frame OBU tile offset parsing is implemented, got frame_type={frame_type}"
        ));
    }
    let show_frame = bits.read_bool("show_frame")?;
    if !show_frame {
        let _showable_frame = bits.read_bool("showable_frame")?;
    }
    let disable_cdf_update = bits.read_bool("disable_cdf_update")?;
    let allow_screen_content_tools = bits.read_bool("allow_screen_content_tools")?;
    let force_integer_mv = bits.read_bool("force_integer_mv")?;
    let frame_size_override_flag = bits.read_bool("frame_size_override_flag")?;
    let order_hint = bits.read_bits_u8(7, "order_hint")?;
    let render_and_frame_size_different = bits.read_bool("render_and_frame_size_different")?;
    if render_and_frame_size_different {
        return Err(
            "render_and_frame_size_different AV1 key-frame parsing is not implemented".to_string(),
        );
    }
    let allow_intrabc = bits.read_bool("allow_intrabc")?;
    let disable_frame_end_update_cdf = bits.read_bool("disable_frame_end_update_cdf")?;
    let uniform_tile_spacing_flag = bits.read_bool("uniform_tile_spacing_flag")?;
    if !uniform_tile_spacing_flag {
        return Err("non-uniform AV1 tile spacing parsing is not implemented".to_string());
    }
    while bits.read_bool("tile_cols_log2")? {}
    while bits.read_bool("tile_rows_log2")? {}

    let quantization = parse_av1_quantization_params(
        &mut bits,
        sequence_header.is_some_and(|sequence| sequence.separate_uv_delta_q),
    )?;

    let segmentation = parse_av1_key_frame_segmentation_params(&mut bits)?;
    let delta_q_present = bits.read_bool("delta_q_present")?;
    let mut delta_q_res = 0;
    if delta_q_present {
        delta_q_res = bits.read_bits_u8(2, "delta_q_res")?;
    }

    let loop_filter_level = [
        bits.read_bits_u8(6, "loop_filter_level[0]")?,
        bits.read_bits_u8(6, "loop_filter_level[1]")?,
        bits.read_bits_u8(6, "loop_filter_level[2]")?,
        bits.read_bits_u8(6, "loop_filter_level[3]")?,
    ];
    let mut loop_filter_sharpness = 0;
    let mut loop_filter_delta_enabled = false;
    let mut loop_filter_delta_update = false;
    if loop_filter_level.iter().any(|&level| level != 0) {
        loop_filter_sharpness = bits.read_bits_u8(3, "loop_filter_sharpness")?;
        loop_filter_delta_enabled = bits.read_bool("loop_filter_delta_enabled")?;
        if loop_filter_delta_enabled {
            loop_filter_delta_update = bits.read_bool("loop_filter_delta_update")?;
            if loop_filter_delta_update {
                skip_av1_loop_filter_delta_updates(&mut bits)?;
            }
        }
    }

    let cdef_damping_minus_3 = bits.read_bits_u8(2, "cdef_damping_minus_3")?;
    let cdef_bits = bits.read_bits_u8(2, "cdef_bits")?;
    let mut cdef_y_pri_strength = [0_u8; 8];
    let mut cdef_y_sec_strength = [0_u8; 8];
    let mut cdef_uv_pri_strength = [0_u8; 8];
    let mut cdef_uv_sec_strength = [0_u8; 8];
    for index in 0..(1_u8 << cdef_bits) {
        let index = usize::from(index);
        cdef_y_pri_strength[index] = bits.read_bits_u8(4, "cdef_y_pri_strength")?;
        cdef_y_sec_strength[index] = bits.read_bits_u8(2, "cdef_y_sec_strength")?;
        cdef_uv_pri_strength[index] = bits.read_bits_u8(4, "cdef_uv_pri_strength")?;
        cdef_uv_sec_strength[index] = bits.read_bits_u8(2, "cdef_uv_sec_strength")?;
    }

    let tx_mode_select = bits.read_bool("tx_mode_select")?;
    let tx_mode = if tx_mode_select {
        StdVideoAV1TxMode_STD_VIDEO_AV1_TX_MODE_SELECT
    } else {
        StdVideoAV1TxMode_STD_VIDEO_AV1_TX_MODE_LARGEST
    };
    let reduced_tx_set = bits.read_bool("reduced_tx_set")?;
    bits.align_to_next_byte("frame_header_obu_byte_alignment")?;
    Ok(Av1ParsedKeyFrameHeader {
        tile_payload_offset: bits.byte_offset(),
        disable_cdf_update,
        allow_screen_content_tools,
        force_integer_mv,
        frame_size_override_flag,
        order_hint,
        allow_intrabc,
        disable_frame_end_update_cdf,
        base_q_idx: quantization.base_q_idx,
        delta_q_y_dc: quantization.delta_q_y_dc,
        diff_uv_delta: quantization.diff_uv_delta,
        delta_q_u_dc: quantization.delta_q_u_dc,
        delta_q_u_ac: quantization.delta_q_u_ac,
        delta_q_v_dc: quantization.delta_q_v_dc,
        delta_q_v_ac: quantization.delta_q_v_ac,
        using_qmatrix: quantization.using_qmatrix,
        qm_y: quantization.qm_y,
        qm_u: quantization.qm_u,
        qm_v: quantization.qm_v,
        segmentation_enabled: segmentation.enabled,
        segmentation_update_map: segmentation.update_map,
        segmentation_temporal_update: segmentation.temporal_update,
        segmentation_update_data: segmentation.update_data,
        segmentation: segmentation.std_segmentation,
        delta_q_present,
        delta_q_res,
        loop_filter_level,
        loop_filter_sharpness,
        loop_filter_delta_enabled,
        loop_filter_delta_update,
        cdef_damping_minus_3,
        cdef_bits,
        cdef_y_pri_strength,
        cdef_y_sec_strength,
        cdef_uv_pri_strength,
        cdef_uv_sec_strength,
        tx_mode,
        reduced_tx_set,
    })
}

pub(super) fn parse_av1_quantization_params(
    bits: &mut BitReader<'_>,
    separate_uv_delta_q: bool,
) -> Result<Av1ParsedQuantization, String> {
    let base_q_idx = bits.read_bits_u8(8, "base_q_idx")?;
    let delta_q_y_dc = read_av1_delta_q(bits, "delta_q_y_dc")?;
    let diff_uv_delta = separate_uv_delta_q && bits.read_bool("diff_uv_delta")?;
    let delta_q_u_dc = read_av1_delta_q(bits, "delta_q_u_dc")?;
    let delta_q_u_ac = read_av1_delta_q(bits, "delta_q_u_ac")?;
    let (delta_q_v_dc, delta_q_v_ac) = if diff_uv_delta {
        (
            read_av1_delta_q(bits, "delta_q_v_dc")?,
            read_av1_delta_q(bits, "delta_q_v_ac")?,
        )
    } else {
        (delta_q_u_dc, delta_q_u_ac)
    };
    let using_qmatrix = bits.read_bool("using_qmatrix")?;
    let qm_y = if using_qmatrix {
        bits.read_bits_u8(4, "qm_y")?
    } else {
        0
    };
    let qm_u = if using_qmatrix {
        bits.read_bits_u8(4, "qm_u")?
    } else {
        0
    };
    let qm_v = if using_qmatrix && separate_uv_delta_q {
        bits.read_bits_u8(4, "qm_v")?
    } else {
        qm_u
    };
    Ok(Av1ParsedQuantization {
        base_q_idx,
        delta_q_y_dc,
        diff_uv_delta,
        delta_q_u_dc,
        delta_q_u_ac,
        delta_q_v_dc,
        delta_q_v_ac,
        using_qmatrix,
        qm_y,
        qm_u,
        qm_v,
    })
}

pub(super) fn read_av1_delta_q(bits: &mut BitReader<'_>, field_name: &str) -> Result<i8, String> {
    let delta_coded = bits.read_bool(field_name)?;
    if delta_coded {
        read_av1_signed_literal_i8(bits, 7, field_name)
    } else {
        Ok(0)
    }
}

pub(super) fn read_av1_signed_literal_i8(
    bits: &mut BitReader<'_>,
    count: usize,
    field_name: &str,
) -> Result<i8, String> {
    if count == 0 || count > 8 {
        return Err(format!(
            "{field_name} signed literal bit count {count} is unsupported"
        ));
    }
    let raw = bits.read_bits_u8(count, field_name)?;
    let sign_bit = 1_u8 << (count - 1);
    if raw & sign_bit == 0 {
        i8::try_from(raw).map_err(|_| format!("{field_name} does not fit in i8"))
    } else {
        let signed = i16::from(raw) - (1_i16 << count);
        i8::try_from(signed).map_err(|_| format!("{field_name} does not fit in i8"))
    }
}

pub(super) fn parse_av1_key_frame_segmentation_params(
    bits: &mut BitReader<'_>,
) -> Result<Av1ParsedSegmentation, String> {
    let enabled = bits.read_bool("segmentation_enabled")?;
    let mut parsed = Av1ParsedSegmentation {
        enabled,
        update_map: false,
        temporal_update: false,
        update_data: false,
        std_segmentation: StdVideoAV1Segmentation {
            FeatureEnabled: [0; AV1_MAX_SEGMENTS],
            FeatureData: [[0; AV1_SEG_LVL_MAX]; AV1_MAX_SEGMENTS],
        },
    };
    if !enabled {
        return Ok(parsed);
    }

    parsed.update_map = true;
    parsed.update_data = true;
    for segment_index in 0..AV1_MAX_SEGMENTS {
        let mut feature_enabled_bits = 0_u8;
        for feature_index in 0..AV1_SEG_LVL_MAX {
            if bits.read_bool("segmentation_feature_enabled")? {
                feature_enabled_bits |= 1_u8 << feature_index;
                let feature_bits = AV1_SEG_FEATURE_BITS[feature_index];
                if feature_bits > 0 {
                    parsed.std_segmentation.FeatureData[segment_index][feature_index] =
                        if AV1_SEG_FEATURE_SIGNED[feature_index] {
                            read_av1_signed_literal_i16(
                                bits,
                                feature_bits + 1,
                                "segmentation_feature_data",
                            )?
                        } else {
                            i16::try_from(
                                bits.read_bits_u16(feature_bits, "segmentation_feature_data")?,
                            )
                            .map_err(|_| {
                                "segmentation_feature_data does not fit in i16".to_string()
                            })?
                        };
                }
            }
        }
        parsed.std_segmentation.FeatureEnabled[segment_index] = feature_enabled_bits;
    }

    Ok(parsed)
}

pub(super) fn read_av1_signed_literal_i16(
    bits: &mut BitReader<'_>,
    count: usize,
    field_name: &str,
) -> Result<i16, String> {
    if count == 0 || count > 16 {
        return Err(format!(
            "{field_name} signed literal bit count {count} is unsupported"
        ));
    }
    let raw = bits.read_bits_u16(count, field_name)?;
    let sign_bit = 1_u16 << (count - 1);
    if raw & sign_bit == 0 {
        i16::try_from(raw).map_err(|_| format!("{field_name} does not fit in i16"))
    } else {
        let signed = i32::from(raw) - (1_i32 << count);
        i16::try_from(signed).map_err(|_| format!("{field_name} does not fit in i16"))
    }
}

pub(super) fn skip_av1_loop_filter_delta_updates(bits: &mut BitReader<'_>) -> Result<(), String> {
    for _ in 0..8 {
        if bits.read_bool("loop_filter_ref_delta_update")? {
            skip_av1_loop_filter_signed_delta(bits, "loop_filter_ref_delta")?;
        }
    }
    for _ in 0..2 {
        if bits.read_bool("loop_filter_mode_delta_update")? {
            skip_av1_loop_filter_signed_delta(bits, "loop_filter_mode_delta")?;
        }
    }
    Ok(())
}

pub(super) fn skip_av1_loop_filter_signed_delta(
    bits: &mut BitReader<'_>,
    field_name: &str,
) -> Result<(), String> {
    let _magnitude = bits.read_bits_u8(6, field_name)?;
    let _sign = bits.read_bool(field_name)?;
    Ok(())
}

pub(super) fn build_av1_std_sequence_header(
    parsed: &ParsedAv1SequenceHeader,
) -> Result<StdVideoAV1SequenceHeader, String> {
    let seq_profile = match parsed.seq_profile {
        0 => StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_MAIN,
        1 => StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_HIGH,
        2 => StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_PROFESSIONAL,
        other => return Err(format!("unsupported AV1 seq_profile {other}")),
    };
    let max_frame_width_minus_1 = u16::try_from(parsed.max_frame_width_minus_1)
        .map_err(|_| "max_frame_width_minus_1 exceeds Vulkan std u16 range".to_string())?;
    let max_frame_height_minus_1 = u16::try_from(parsed.max_frame_height_minus_1)
        .map_err(|_| "max_frame_height_minus_1 exceeds Vulkan std u16 range".to_string())?;

    Ok(StdVideoAV1SequenceHeader {
        flags: StdVideoAV1SequenceHeaderFlags {
            _bitfield_align_1: [],
            _bitfield_1: StdVideoAV1SequenceHeaderFlags::new_bitfield_1(
                parsed.still_picture as u32,
                parsed.reduced_still_picture_header as u32,
                parsed.use_128x128_superblock as u32,
                parsed.enable_filter_intra as u32,
                parsed.enable_intra_edge_filter as u32,
                parsed.enable_interintra_compound as u32,
                parsed.enable_masked_compound as u32,
                parsed.enable_warped_motion as u32,
                parsed.enable_dual_filter as u32,
                parsed.enable_order_hint as u32,
                parsed.enable_jnt_comp as u32,
                parsed.enable_ref_frame_mvs as u32,
                parsed.frame_id_numbers_present_flag as u32,
                parsed.enable_superres as u32,
                parsed.enable_cdef as u32,
                parsed.enable_restoration as u32,
                parsed.film_grain_params_present as u32,
                parsed.timing_info_present_flag as u32,
                parsed.initial_display_delay_present_flag as u32,
                0,
            ),
        },
        seq_profile,
        frame_width_bits_minus_1: parsed.frame_width_bits_minus_1,
        frame_height_bits_minus_1: parsed.frame_height_bits_minus_1,
        max_frame_width_minus_1,
        max_frame_height_minus_1,
        delta_frame_id_length_minus_2: 0,
        additional_frame_id_length_minus_1: 0,
        order_hint_bits_minus_1: parsed.order_hint_bits_minus_1,
        seq_force_integer_mv: parsed.seq_force_integer_mv,
        seq_force_screen_content_tools: parsed.seq_force_screen_content_tools,
        reserved1: [0; 5],
        pColorConfig: std::ptr::null(),
        pTimingInfo: std::ptr::null(),
    })
}
