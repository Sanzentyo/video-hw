//! HEVC parameter-set storage construction and RBSP syntax parsing.

use super::*;

pub(crate) fn build_hevc_std_parameter_set_storage(
    parameter_sets: &HevcParameterSets,
) -> Result<HevcStdParameterSetStorage, String> {
    let parsed_vps = parse_hevc_vps(&parameter_sets.vps)?;
    let parsed_pps = parse_hevc_pps(&parameter_sets.pps)?;
    let sps = &parameter_sets.parsed_sps;

    let profile = &sps.profile_tier_level.general_profile;
    let mut profile_tier_flags = empty_profile_tier_level_flags();
    profile_tier_flags.set_general_tier_flag(bool_to_u32(profile.tier_flag));
    profile_tier_flags
        .set_general_progressive_source_flag(bool_to_u32(profile.progressive_source_flag));
    profile_tier_flags
        .set_general_interlaced_source_flag(bool_to_u32(profile.interlaced_source_flag));
    profile_tier_flags
        .set_general_non_packed_constraint_flag(bool_to_u32(profile.non_packed_constraint_flag));
    profile_tier_flags
        .set_general_frame_only_constraint_flag(bool_to_u32(profile.frame_only_constraint_flag));

    let profile_tier_level = StdVideoH265ProfileTierLevel {
        flags: profile_tier_flags,
        general_profile_idc: StdVideoH265ProfileIdc::from(profile.profile_idc),
        general_level_idc: StdVideoH265LevelIdc::from(profile.level_idc.unwrap_or_default()),
    };

    let mut dec_pic_buf_mgr = StdVideoH265DecPicBufMgr {
        max_latency_increase_plus1: [0; 7],
        max_dec_pic_buffering_minus1: [0; 7],
        max_num_reorder_pics: [0; 7],
    };
    for (index, &value) in sps
        .sub_layer_ordering_info
        .sps_max_latency_increase_plus1
        .iter()
        .enumerate()
        .take(7)
    {
        dec_pic_buf_mgr.max_latency_increase_plus1[index] = value;
    }
    for (index, &value) in sps
        .sub_layer_ordering_info
        .sps_max_dec_pic_buffering_minus1
        .iter()
        .enumerate()
        .take(7)
    {
        dec_pic_buf_mgr.max_dec_pic_buffering_minus1[index] =
            narrow_u64_to_u8(value, "sps_max_dec_pic_buffering_minus1")?;
    }
    for (index, &value) in sps
        .sub_layer_ordering_info
        .sps_max_num_reorder_pics
        .iter()
        .enumerate()
        .take(7)
    {
        dec_pic_buf_mgr.max_num_reorder_pics[index] =
            narrow_u64_to_u8(value, "sps_max_num_reorder_pics")?;
    }

    let short_term_ref_pic_sets = build_std_short_term_ref_pic_sets(&parameter_sets.sps, sps)?;
    let long_term_ref_pics_sps = build_std_long_term_ref_pics_sps(sps)?;
    let sequence_parameter_set_vui = build_std_sequence_parameter_set_vui(sps)?;
    let sequence_parameter_set_vui_hrd = sequence_parameter_set_vui
        .as_ref()
        .map(|_| Box::new(empty_hrd_parameters()));

    let has_conformance_window = sps.conformance_window.conf_win_left_offset != 0
        || sps.conformance_window.conf_win_right_offset != 0
        || sps.conformance_window.conf_win_top_offset != 0
        || sps.conformance_window.conf_win_bottom_offset != 0;
    let inferred_sub_layer_ordering_present = infer_sps_sub_layer_ordering_info_present_flag(sps);

    let mut sps_flags = empty_sps_flags();
    sps_flags.set_sps_temporal_id_nesting_flag(bool_to_u32(sps.sps_temporal_id_nesting_flag));
    sps_flags.set_separate_colour_plane_flag(bool_to_u32(sps.separate_colour_plane_flag));
    sps_flags.set_conformance_window_flag(bool_to_u32(has_conformance_window));
    sps_flags.set_sps_sub_layer_ordering_info_present_flag(bool_to_u32(
        inferred_sub_layer_ordering_present,
    ));
    // Do not advertise scaling lists until `pScalingLists` is populated below.
    sps_flags.set_scaling_list_enabled_flag(0);
    sps_flags.set_sps_scaling_list_data_present_flag(0);
    sps_flags.set_amp_enabled_flag(bool_to_u32(sps.amp_enabled_flag));
    sps_flags.set_sample_adaptive_offset_enabled_flag(bool_to_u32(
        sps.sample_adaptive_offset_enabled_flag,
    ));
    sps_flags.set_pcm_enabled_flag(bool_to_u32(sps.pcm.is_some()));
    sps_flags.set_pcm_loop_filter_disabled_flag(bool_to_u32(
        sps.pcm
            .as_ref()
            .is_some_and(|pcm| pcm.pcm_loop_filter_disabled_flag),
    ));
    sps_flags.set_long_term_ref_pics_present_flag(bool_to_u32(sps.long_term_ref_pics.is_some()));
    sps_flags.set_sps_temporal_mvp_enabled_flag(bool_to_u32(sps.sps_temporal_mvp_enabled_flag));
    sps_flags.set_strong_intra_smoothing_enabled_flag(bool_to_u32(
        sps.strong_intra_smoothing_enabled_flag,
    ));
    sps_flags.set_vui_parameters_present_flag(bool_to_u32(sps.vui_parameters.is_some()));
    let sps_extension_present = sps.range_extension.is_some()
        || sps.multilayer_extension.is_some()
        || sps.sps_3d_extension.is_some()
        || sps.scc_extension.is_some();
    sps_flags.set_sps_extension_present_flag(bool_to_u32(sps_extension_present));
    sps_flags.set_sps_range_extension_flag(bool_to_u32(sps.range_extension.is_some()));
    if let Some(range_extension) = &sps.range_extension {
        sps_flags.set_transform_skip_rotation_enabled_flag(bool_to_u32(
            range_extension.transform_skip_rotation_enabled_flag,
        ));
        sps_flags.set_transform_skip_context_enabled_flag(bool_to_u32(
            range_extension.transform_skip_context_enabled_flag,
        ));
        sps_flags.set_implicit_rdpcm_enabled_flag(bool_to_u32(
            range_extension.implicit_rdpcm_enabled_flag,
        ));
        sps_flags.set_explicit_rdpcm_enabled_flag(bool_to_u32(
            range_extension.explicit_rdpcm_enabled_flag,
        ));
        sps_flags.set_extended_precision_processing_flag(bool_to_u32(
            range_extension.extended_precision_processing_flag,
        ));
        sps_flags.set_intra_smoothing_disabled_flag(bool_to_u32(
            range_extension.intra_smoothing_disabled_flag,
        ));
        sps_flags.set_high_precision_offsets_enabled_flag(bool_to_u32(
            range_extension.high_precision_offsets_enabled_flag,
        ));
        sps_flags.set_persistent_rice_adaptation_enabled_flag(bool_to_u32(
            range_extension.persistent_rice_adaptation_enabled_flag,
        ));
        sps_flags.set_cabac_bypass_alignment_enabled_flag(bool_to_u32(
            range_extension.cabac_bypass_alignment_enabled_flag,
        ));
    }

    let chroma_format_idc = map_h265_chroma_format_idc(sps.chroma_format_idc)?;
    let short_term_count =
        narrow_usize_to_u8(short_term_ref_pic_sets.len(), "num_short_term_ref_pic_sets")?;
    let long_term_count = if let Some(long_term_ref_pics) = &sps.long_term_ref_pics {
        narrow_usize_to_u8(
            long_term_ref_pics.lt_ref_pic_poc_lsb_sps.len(),
            "num_long_term_ref_pics_sps",
        )?
    } else {
        0
    };

    let std_sps = StdVideoH265SequenceParameterSet {
        flags: sps_flags,
        chroma_format_idc,
        pic_width_in_luma_samples: narrow_u64_to_u32(
            sps.pic_width_in_luma_samples.get(),
            "pic_width_in_luma_samples",
        )?,
        pic_height_in_luma_samples: narrow_u64_to_u32(
            sps.pic_height_in_luma_samples.get(),
            "pic_height_in_luma_samples",
        )?,
        sps_video_parameter_set_id: sps.sps_video_parameter_set_id,
        sps_max_sub_layers_minus1: sps.sps_max_sub_layers_minus1,
        sps_seq_parameter_set_id: narrow_u64_to_u8(
            sps.sps_seq_parameter_set_id,
            "sps_seq_parameter_set_id",
        )?,
        bit_depth_luma_minus8: sps.bit_depth_luma_minus8,
        bit_depth_chroma_minus8: sps.bit_depth_chroma_minus8,
        log2_max_pic_order_cnt_lsb_minus4: sps.log2_max_pic_order_cnt_lsb_minus4,
        log2_min_luma_coding_block_size_minus3: narrow_u64_to_u8(
            sps.log2_min_luma_coding_block_size_minus3,
            "log2_min_luma_coding_block_size_minus3",
        )?,
        log2_diff_max_min_luma_coding_block_size: narrow_u64_to_u8(
            sps.log2_diff_max_min_luma_coding_block_size,
            "log2_diff_max_min_luma_coding_block_size",
        )?,
        log2_min_luma_transform_block_size_minus2: narrow_u64_to_u8(
            sps.log2_min_luma_transform_block_size_minus2,
            "log2_min_luma_transform_block_size_minus2",
        )?,
        log2_diff_max_min_luma_transform_block_size: narrow_u64_to_u8(
            sps.log2_diff_max_min_luma_transform_block_size,
            "log2_diff_max_min_luma_transform_block_size",
        )?,
        max_transform_hierarchy_depth_inter: narrow_u64_to_u8(
            sps.max_transform_hierarchy_depth_inter,
            "max_transform_hierarchy_depth_inter",
        )?,
        max_transform_hierarchy_depth_intra: narrow_u64_to_u8(
            sps.max_transform_hierarchy_depth_intra,
            "max_transform_hierarchy_depth_intra",
        )?,
        num_short_term_ref_pic_sets: short_term_count,
        num_long_term_ref_pics_sps: long_term_count,
        pcm_sample_bit_depth_luma_minus1: sps
            .pcm
            .as_ref()
            .map_or(0, |pcm| pcm.pcm_sample_bit_depth_luma_minus1),
        pcm_sample_bit_depth_chroma_minus1: sps
            .pcm
            .as_ref()
            .map_or(0, |pcm| pcm.pcm_sample_bit_depth_chroma_minus1),
        log2_min_pcm_luma_coding_block_size_minus3: if let Some(pcm) = &sps.pcm {
            narrow_u64_to_u8(
                pcm.log2_min_pcm_luma_coding_block_size_minus3,
                "log2_min_pcm_luma_coding_block_size_minus3",
            )?
        } else {
            0
        },
        log2_diff_max_min_pcm_luma_coding_block_size: if let Some(pcm) = &sps.pcm {
            narrow_u64_to_u8(
                pcm.log2_diff_max_min_pcm_luma_coding_block_size,
                "log2_diff_max_min_pcm_luma_coding_block_size",
            )?
        } else {
            0
        },
        reserved1: 0,
        reserved2: 0,
        palette_max_size: 0,
        delta_palette_max_predictor_size: 0,
        motion_vector_resolution_control_idc: 0,
        sps_num_palette_predictor_initializers_minus1: 0,
        conf_win_left_offset: narrow_u64_to_u32(
            sps.conformance_window.conf_win_left_offset,
            "conf_win_left_offset",
        )?,
        conf_win_right_offset: narrow_u64_to_u32(
            sps.conformance_window.conf_win_right_offset,
            "conf_win_right_offset",
        )?,
        conf_win_top_offset: narrow_u64_to_u32(
            sps.conformance_window.conf_win_top_offset,
            "conf_win_top_offset",
        )?,
        conf_win_bottom_offset: narrow_u64_to_u32(
            sps.conformance_window.conf_win_bottom_offset,
            "conf_win_bottom_offset",
        )?,
        pProfileTierLevel: std::ptr::null(),
        pDecPicBufMgr: std::ptr::null(),
        pScalingLists: std::ptr::null(),
        pShortTermRefPicSet: std::ptr::null(),
        pLongTermRefPicsSps: std::ptr::null(),
        pSequenceParameterSetVui: std::ptr::null(),
        pPredictorPaletteEntries: std::ptr::null(),
    };

    let mut vps_flags = empty_vps_flags();
    vps_flags
        .set_vps_temporal_id_nesting_flag(bool_to_u32(parsed_vps.vps_temporal_id_nesting_flag));
    vps_flags.set_vps_sub_layer_ordering_info_present_flag(bool_to_u32(
        parsed_vps.vps_sub_layer_ordering_info_present_flag,
    ));
    vps_flags
        .set_vps_timing_info_present_flag(bool_to_u32(parsed_vps.vps_timing_info_present_flag));
    vps_flags.set_vps_poc_proportional_to_timing_flag(bool_to_u32(
        parsed_vps.vps_poc_proportional_to_timing_flag,
    ));

    let std_vps = StdVideoH265VideoParameterSet {
        flags: vps_flags,
        vps_video_parameter_set_id: parsed_vps.vps_video_parameter_set_id,
        vps_max_sub_layers_minus1: parsed_vps.vps_max_sub_layers_minus1,
        reserved1: 0,
        reserved2: 0,
        vps_num_units_in_tick: parsed_vps.vps_num_units_in_tick,
        vps_time_scale: parsed_vps.vps_time_scale,
        vps_num_ticks_poc_diff_one_minus1: parsed_vps.vps_num_ticks_poc_diff_one_minus1,
        reserved3: 0,
        pDecPicBufMgr: std::ptr::null(),
        pHrdParameters: std::ptr::null(),
        pProfileTierLevel: std::ptr::null(),
    };

    let mut pps_flags = empty_pps_flags();
    pps_flags.set_dependent_slice_segments_enabled_flag(bool_to_u32(
        parsed_pps.dependent_slice_segments_enabled_flag,
    ));
    pps_flags.set_output_flag_present_flag(bool_to_u32(parsed_pps.output_flag_present_flag));
    pps_flags
        .set_sign_data_hiding_enabled_flag(bool_to_u32(parsed_pps.sign_data_hiding_enabled_flag));
    pps_flags.set_cabac_init_present_flag(bool_to_u32(parsed_pps.cabac_init_present_flag));
    pps_flags.set_constrained_intra_pred_flag(bool_to_u32(parsed_pps.constrained_intra_pred_flag));
    pps_flags.set_transform_skip_enabled_flag(bool_to_u32(parsed_pps.transform_skip_enabled_flag));
    pps_flags.set_cu_qp_delta_enabled_flag(bool_to_u32(parsed_pps.cu_qp_delta_enabled_flag));
    pps_flags.set_pps_slice_chroma_qp_offsets_present_flag(bool_to_u32(
        parsed_pps.pps_slice_chroma_qp_offsets_present_flag,
    ));
    pps_flags.set_weighted_pred_flag(bool_to_u32(parsed_pps.weighted_pred_flag));
    pps_flags.set_weighted_bipred_flag(bool_to_u32(parsed_pps.weighted_bipred_flag));
    pps_flags
        .set_transquant_bypass_enabled_flag(bool_to_u32(parsed_pps.transquant_bypass_enabled_flag));
    pps_flags.set_tiles_enabled_flag(bool_to_u32(parsed_pps.tiles_enabled_flag));
    pps_flags.set_entropy_coding_sync_enabled_flag(bool_to_u32(
        parsed_pps.entropy_coding_sync_enabled_flag,
    ));
    pps_flags.set_uniform_spacing_flag(bool_to_u32(parsed_pps.uniform_spacing_flag));
    pps_flags.set_loop_filter_across_tiles_enabled_flag(bool_to_u32(
        parsed_pps.loop_filter_across_tiles_enabled_flag,
    ));
    pps_flags.set_pps_loop_filter_across_slices_enabled_flag(bool_to_u32(
        parsed_pps.pps_loop_filter_across_slices_enabled_flag,
    ));
    pps_flags.set_deblocking_filter_control_present_flag(bool_to_u32(
        parsed_pps.deblocking_filter_control_present_flag,
    ));
    pps_flags.set_deblocking_filter_override_enabled_flag(bool_to_u32(
        parsed_pps.deblocking_filter_override_enabled_flag,
    ));
    pps_flags.set_pps_deblocking_filter_disabled_flag(bool_to_u32(
        parsed_pps.pps_deblocking_filter_disabled_flag,
    ));
    pps_flags.set_lists_modification_present_flag(bool_to_u32(
        parsed_pps.lists_modification_present_flag,
    ));
    pps_flags.set_slice_segment_header_extension_present_flag(bool_to_u32(
        parsed_pps.slice_segment_header_extension_present_flag,
    ));
    let std_pps = StdVideoH265PictureParameterSet {
        flags: pps_flags,
        pps_pic_parameter_set_id: parsed_pps.pps_pic_parameter_set_id,
        pps_seq_parameter_set_id: parsed_pps.pps_seq_parameter_set_id,
        sps_video_parameter_set_id: sps.sps_video_parameter_set_id,
        num_extra_slice_header_bits: parsed_pps.num_extra_slice_header_bits,
        num_ref_idx_l0_default_active_minus1: parsed_pps.num_ref_idx_l0_default_active_minus1,
        num_ref_idx_l1_default_active_minus1: parsed_pps.num_ref_idx_l1_default_active_minus1,
        init_qp_minus26: parsed_pps.init_qp_minus26,
        diff_cu_qp_delta_depth: parsed_pps.diff_cu_qp_delta_depth,
        pps_cb_qp_offset: parsed_pps.pps_cb_qp_offset,
        pps_cr_qp_offset: parsed_pps.pps_cr_qp_offset,
        pps_beta_offset_div2: parsed_pps.pps_beta_offset_div2,
        pps_tc_offset_div2: parsed_pps.pps_tc_offset_div2,
        log2_parallel_merge_level_minus2: parsed_pps.log2_parallel_merge_level_minus2,
        log2_max_transform_skip_block_size_minus2: 0,
        diff_cu_chroma_qp_offset_depth: 0,
        chroma_qp_offset_list_len_minus1: 0,
        cb_qp_offset_list: [0; 6],
        cr_qp_offset_list: [0; 6],
        log2_sao_offset_scale_luma: 0,
        log2_sao_offset_scale_chroma: 0,
        pps_act_y_qp_offset_plus5: 0,
        pps_act_cb_qp_offset_plus5: 0,
        pps_act_cr_qp_offset_plus3: 0,
        pps_num_palette_predictor_initializers: 0,
        luma_bit_depth_entry_minus8: 0,
        chroma_bit_depth_entry_minus8: 0,
        num_tile_columns_minus1: parsed_pps.num_tile_columns_minus1,
        num_tile_rows_minus1: parsed_pps.num_tile_rows_minus1,
        reserved1: 0,
        reserved2: 0,
        column_width_minus1: parsed_pps.column_width_minus1,
        row_height_minus1: parsed_pps.row_height_minus1,
        reserved3: 0,
        pScalingLists: std::ptr::null(),
        pPredictorPaletteEntries: std::ptr::null(),
    };

    let mut storage = HevcStdParameterSetStorage {
        vps: [std_vps],
        sps: [std_sps],
        pps: [std_pps],
        profile_tier_level: Box::new(profile_tier_level),
        vps_profile_tier_level: Box::new(parsed_vps.profile_tier_level),
        dec_pic_buf_mgr: Box::new(dec_pic_buf_mgr),
        short_term_ref_pic_sets: short_term_ref_pic_sets.into_boxed_slice(),
        long_term_ref_pics_sps: long_term_ref_pics_sps.map(Box::new),
        sequence_parameter_set_vui: sequence_parameter_set_vui.map(Box::new),
        sequence_parameter_set_vui_hrd,
        vps_dec_pic_buf_mgr: Box::new(parsed_vps.vps_dec_pic_buf_mgr),
        vps_hrd_parameters: Box::new(empty_hrd_parameters()),
    };

    let profile_ptr = storage.profile_tier_level.as_ref() as *const StdVideoH265ProfileTierLevel;
    let vps_profile_ptr =
        storage.vps_profile_tier_level.as_ref() as *const StdVideoH265ProfileTierLevel;
    let dec_pic_buf_mgr_ptr = storage.dec_pic_buf_mgr.as_ref() as *const StdVideoH265DecPicBufMgr;
    let vps_dec_pic_buf_mgr_ptr =
        storage.vps_dec_pic_buf_mgr.as_ref() as *const StdVideoH265DecPicBufMgr;
    storage.vps[0].pProfileTierLevel = vps_profile_ptr;
    storage.vps[0].pDecPicBufMgr = vps_dec_pic_buf_mgr_ptr;
    storage.vps[0].pHrdParameters =
        storage.vps_hrd_parameters.as_ref() as *const StdVideoH265HrdParameters;
    storage.sps[0].pProfileTierLevel = profile_ptr;
    storage.sps[0].pDecPicBufMgr = dec_pic_buf_mgr_ptr;

    storage.sps[0].pShortTermRefPicSet = if storage.short_term_ref_pic_sets.is_empty() {
        std::ptr::null()
    } else {
        storage.short_term_ref_pic_sets.as_ptr()
    };
    storage.sps[0].pLongTermRefPicsSps = storage
        .long_term_ref_pics_sps
        .as_ref()
        .map_or(std::ptr::null(), |long_term| {
            long_term.as_ref() as *const StdVideoH265LongTermRefPicsSps
        });
    storage.sps[0].pSequenceParameterSetVui = storage
        .sequence_parameter_set_vui
        .as_ref()
        .map_or(std::ptr::null(), |vui| {
            vui.as_ref() as *const StdVideoH265SequenceParameterSetVui
        });
    if let (Some(vui), Some(hrd)) = (
        storage.sequence_parameter_set_vui.as_mut(),
        storage.sequence_parameter_set_vui_hrd.as_ref(),
    ) {
        vui.pHrdParameters = hrd.as_ref() as *const StdVideoH265HrdParameters;
    }

    Ok(storage)
}

fn build_std_sequence_parameter_set_vui(
    sps: &SpsRbsp,
) -> Result<Option<StdVideoH265SequenceParameterSetVui>, String> {
    let Some(vui) = &sps.vui_parameters else {
        return Ok(None);
    };

    let mut flags = empty_sps_vui_flags();
    let (aspect_ratio_idc, sar_width, sar_height, aspect_ratio_present) =
        match &vui.aspect_ratio_info {
            AspectRatioInfo::Predefined(aspect_ratio_idc) => {
                let present = *aspect_ratio_idc != AspectRatioIdc::Unspecified;
                (u32::from(aspect_ratio_idc.0), 0, 0, present)
            }
            AspectRatioInfo::ExtendedSar {
                sar_width,
                sar_height,
            } => (
                u32::from(AspectRatioIdc::ExtendedSar.0),
                *sar_width,
                *sar_height,
                true,
            ),
        };
    flags.set_aspect_ratio_info_present_flag(bool_to_u32(aspect_ratio_present));
    flags.set_overscan_info_present_flag(bool_to_u32(vui.overscan_appropriate_flag.is_some()));
    flags
        .set_overscan_appropriate_flag(bool_to_u32(vui.overscan_appropriate_flag.unwrap_or(false)));

    let video_signal_present = vui.video_signal_type.video_format.0 != 5
        || vui.video_signal_type.video_full_range_flag
        || vui.video_signal_type.colour_primaries != 2
        || vui.video_signal_type.transfer_characteristics != 2
        || vui.video_signal_type.matrix_coeffs != 2;
    flags.set_video_signal_type_present_flag(bool_to_u32(video_signal_present));
    flags.set_video_full_range_flag(bool_to_u32(vui.video_signal_type.video_full_range_flag));
    flags.set_colour_description_present_flag(bool_to_u32(
        video_signal_present
            && (vui.video_signal_type.colour_primaries != 2
                || vui.video_signal_type.transfer_characteristics != 2
                || vui.video_signal_type.matrix_coeffs != 2),
    ));

    flags.set_chroma_loc_info_present_flag(bool_to_u32(vui.chroma_loc_info.is_some()));
    flags.set_neutral_chroma_indication_flag(bool_to_u32(vui.neutral_chroma_indication_flag));
    flags.set_field_seq_flag(bool_to_u32(vui.field_seq_flag));
    flags.set_frame_field_info_present_flag(bool_to_u32(vui.frame_field_info_present_flag));
    let default_display_window_present = vui.default_display_window.def_disp_win_left_offset != 0
        || vui.default_display_window.def_disp_win_right_offset != 0
        || vui.default_display_window.def_disp_win_top_offset != 0
        || vui.default_display_window.def_disp_win_bottom_offset != 0;
    flags.set_default_display_window_flag(bool_to_u32(default_display_window_present));

    let timing_info = vui.vui_timing_info.as_ref();
    flags.set_vui_timing_info_present_flag(bool_to_u32(timing_info.is_some()));
    flags.set_vui_poc_proportional_to_timing_flag(bool_to_u32(
        timing_info.is_some_and(|timing| timing.poc_proportional_to_timing_flag),
    ));
    // Do not advertise HRD parameters until `pHrdParameters` is populated below.
    flags.set_vui_hrd_parameters_present_flag(0);

    let bitstream_restriction_present = vui
        .bitstream_restriction
        .restricted_ref_pic_lists_flag
        .is_some();
    flags.set_bitstream_restriction_flag(bool_to_u32(bitstream_restriction_present));
    flags.set_tiles_fixed_structure_flag(bool_to_u32(
        vui.bitstream_restriction.tiles_fixed_structure_flag,
    ));
    flags.set_motion_vectors_over_pic_boundaries_flag(bool_to_u32(
        vui.bitstream_restriction
            .motion_vectors_over_pic_boundaries_flag,
    ));
    flags.set_restricted_ref_pic_lists_flag(bool_to_u32(
        vui.bitstream_restriction
            .restricted_ref_pic_lists_flag
            .unwrap_or(false),
    ));

    Ok(Some(StdVideoH265SequenceParameterSetVui {
        flags,
        aspect_ratio_idc,
        sar_width,
        sar_height,
        video_format: vui.video_signal_type.video_format.0,
        colour_primaries: vui.video_signal_type.colour_primaries,
        transfer_characteristics: vui.video_signal_type.transfer_characteristics,
        matrix_coeffs: vui.video_signal_type.matrix_coeffs,
        chroma_sample_loc_type_top_field: if let Some(chroma) = &vui.chroma_loc_info {
            narrow_u64_to_u8(chroma.top_field, "chroma_sample_loc_type_top_field")?
        } else {
            0
        },
        chroma_sample_loc_type_bottom_field: if let Some(chroma) = &vui.chroma_loc_info {
            narrow_u64_to_u8(chroma.bottom_field, "chroma_sample_loc_type_bottom_field")?
        } else {
            0
        },
        reserved1: 0,
        reserved2: 0,
        def_disp_win_left_offset: narrow_u64_to_u16(
            vui.default_display_window.def_disp_win_left_offset,
            "def_disp_win_left_offset",
        )?,
        def_disp_win_right_offset: narrow_u64_to_u16(
            vui.default_display_window.def_disp_win_right_offset,
            "def_disp_win_right_offset",
        )?,
        def_disp_win_top_offset: narrow_u64_to_u16(
            vui.default_display_window.def_disp_win_top_offset,
            "def_disp_win_top_offset",
        )?,
        def_disp_win_bottom_offset: narrow_u64_to_u16(
            vui.default_display_window.def_disp_win_bottom_offset,
            "def_disp_win_bottom_offset",
        )?,
        vui_num_units_in_tick: timing_info.map_or(0, |timing| timing.num_units_in_tick.get()),
        vui_time_scale: timing_info.map_or(0, |timing| timing.time_scale.get()),
        vui_num_ticks_poc_diff_one_minus1: timing_info
            .and_then(|timing| timing.num_ticks_poc_diff_one_minus1)
            .unwrap_or(0),
        min_spatial_segmentation_idc: vui.bitstream_restriction.min_spatial_segmentation_idc,
        reserved3: 0,
        max_bytes_per_pic_denom: vui.bitstream_restriction.max_bytes_per_pic_denom,
        max_bits_per_min_cu_denom: vui.bitstream_restriction.max_bits_per_min_cu_denom,
        log2_max_mv_length_horizontal: vui.bitstream_restriction.log2_max_mv_length_horizontal,
        log2_max_mv_length_vertical: vui.bitstream_restriction.log2_max_mv_length_vertical,
        pHrdParameters: std::ptr::null(),
    }))
}

fn build_std_short_term_ref_pic_sets(
    sps_nalu: &[u8],
    sps: &SpsRbsp,
) -> Result<Vec<StdVideoH265ShortTermRefPicSet>, String> {
    let short_term = &sps.short_term_ref_pic_sets;
    let set_count = short_term.num_delta_pocs.len();
    let prediction_metadata = parse_hevc_sps_short_term_ref_pic_set_prediction_metadata(
        sps_nalu, sps,
    )
    .unwrap_or_else(|_| vec![HevcSpsShortTermRefPicSetPredictionMetadata::default(); set_count]);
    let mut sets = Vec::with_capacity(set_count);

    for set_index in 0..set_count {
        let delta_s0 = short_term
            .delta_poc_s0
            .get(set_index)
            .ok_or_else(|| format!("short_term_ref_pic_sets[{set_index}] missing delta_poc_s0"))?;
        let delta_s1 = short_term
            .delta_poc_s1
            .get(set_index)
            .ok_or_else(|| format!("short_term_ref_pic_sets[{set_index}] missing delta_poc_s1"))?;
        if delta_s0.len() > 16 {
            return Err(format!(
                "short_term_ref_pic_sets[{set_index}] has {} negative pics (max 16)",
                delta_s0.len()
            ));
        }
        if delta_s1.len() > 16 {
            return Err(format!(
                "short_term_ref_pic_sets[{set_index}] has {} positive pics (max 16)",
                delta_s1.len()
            ));
        }

        let mut delta_poc_s0_minus1 = [0_u16; 16];
        let mut delta_poc_s1_minus1 = [0_u16; 16];
        for (index, &delta) in delta_s0.iter().enumerate() {
            if delta >= 0 {
                return Err(format!(
                    "short_term_ref_pic_sets[{set_index}].delta_poc_s0[{index}] must be negative (got {delta})"
                ));
            }
            let minus1 = (-delta)
                .checked_sub(1)
                .ok_or_else(|| {
                    format!(
                        "short_term_ref_pic_sets[{set_index}].delta_poc_s0[{index}] conversion underflow"
                    )
                })?;
            delta_poc_s0_minus1[index] = narrow_i64_to_u16(minus1, "delta_poc_s0_minus1")?;
        }
        for (index, &delta) in delta_s1.iter().enumerate() {
            if delta <= 0 {
                return Err(format!(
                    "short_term_ref_pic_sets[{set_index}].delta_poc_s1[{index}] must be positive (got {delta})"
                ));
            }
            let minus1 = delta.checked_sub(1).ok_or_else(|| {
                format!(
                    "short_term_ref_pic_sets[{set_index}].delta_poc_s1[{index}] conversion underflow"
                )
            })?;
            delta_poc_s1_minus1[index] = narrow_i64_to_u16(minus1, "delta_poc_s1_minus1")?;
        }

        let used_s0 = short_term
            .used_by_curr_pic_s0
            .get(set_index)
            .ok_or_else(|| {
                format!("short_term_ref_pic_sets[{set_index}] missing used_by_curr_pic_s0")
            })?;
        let used_s1 = short_term
            .used_by_curr_pic_s1
            .get(set_index)
            .ok_or_else(|| {
                format!("short_term_ref_pic_sets[{set_index}] missing used_by_curr_pic_s1")
            })?;
        let used_by_curr_pic_s0_flag = bools_to_u16_mask(used_s0, "used_by_curr_pic_s0")?;
        let used_by_curr_pic_s1_flag = bools_to_u16_mask(used_s1, "used_by_curr_pic_s1")?;

        let metadata = prediction_metadata
            .get(set_index)
            .copied()
            .unwrap_or_default();
        let mut flags = empty_short_term_ref_pic_set_flags();
        flags.set_inter_ref_pic_set_prediction_flag(bool_to_u32(
            metadata.inter_ref_pic_set_prediction_flag,
        ));
        flags.set_delta_rps_sign(bool_to_u32(metadata.delta_rps_sign));

        sets.push(StdVideoH265ShortTermRefPicSet {
            flags,
            delta_idx_minus1: metadata.delta_idx_minus1,
            use_delta_flag: metadata.use_delta_flag,
            abs_delta_rps_minus1: narrow_u32_to_u16(
                metadata.abs_delta_rps_minus1,
                "abs_delta_rps_minus1",
            )?,
            used_by_curr_pic_flag: metadata.used_by_curr_pic_flag,
            used_by_curr_pic_s0_flag,
            used_by_curr_pic_s1_flag,
            reserved1: 0,
            reserved2: 0,
            reserved3: 0,
            num_negative_pics: narrow_usize_to_u8(delta_s0.len(), "num_negative_pics")?,
            num_positive_pics: narrow_usize_to_u8(delta_s1.len(), "num_positive_pics")?,
            delta_poc_s0_minus1,
            delta_poc_s1_minus1,
        });
    }

    Ok(sets)
}

fn parse_hevc_sps_short_term_ref_pic_set_prediction_metadata(
    sps_nalu: &[u8],
    sps: &SpsRbsp,
) -> Result<Vec<HevcSpsShortTermRefPicSetPredictionMetadata>, String> {
    let rbsp = nalu_payload_to_rbsp(sps_nalu)?;
    let mut reader = RbspBitReader::new(&rbsp);

    let _sps_video_parameter_set_id = reader.read_bits(4)?;
    let sps_max_sub_layers_minus1 = usize::try_from(reader.read_bits(3)?)
        .map_err(|_| "sps_max_sub_layers_minus1 conversion failed".to_string())?;
    let _sps_temporal_id_nesting_flag = reader.read_flag()?;
    skip_hevc_profile_tier_level(&mut reader, sps_max_sub_layers_minus1)?;

    let _sps_seq_parameter_set_id = reader.read_ue()?;
    let chroma_format_idc = reader.read_ue()?;
    if chroma_format_idc == 3 {
        let _separate_colour_plane_flag = reader.read_flag()?;
    }
    let _pic_width_in_luma_samples = reader.read_ue()?;
    let _pic_height_in_luma_samples = reader.read_ue()?;
    if reader.read_flag()? {
        let _conf_win_left_offset = reader.read_ue()?;
        let _conf_win_right_offset = reader.read_ue()?;
        let _conf_win_top_offset = reader.read_ue()?;
        let _conf_win_bottom_offset = reader.read_ue()?;
    }
    let _bit_depth_luma_minus8 = reader.read_ue()?;
    let _bit_depth_chroma_minus8 = reader.read_ue()?;
    let _log2_max_pic_order_cnt_lsb_minus4 = reader.read_ue()?;

    let sps_sub_layer_ordering_info_present_flag = reader.read_flag()?;
    let ordering_start = if sps_sub_layer_ordering_info_present_flag {
        0
    } else {
        sps_max_sub_layers_minus1
    };
    for _ in ordering_start..=sps_max_sub_layers_minus1 {
        let _sps_max_dec_pic_buffering_minus1 = reader.read_ue()?;
        let _sps_max_num_reorder_pics = reader.read_ue()?;
        let _sps_max_latency_increase_plus1 = reader.read_ue()?;
    }

    let _log2_min_luma_coding_block_size_minus3 = reader.read_ue()?;
    let _log2_diff_max_min_luma_coding_block_size = reader.read_ue()?;
    let _log2_min_luma_transform_block_size_minus2 = reader.read_ue()?;
    let _log2_diff_max_min_luma_transform_block_size = reader.read_ue()?;
    let _max_transform_hierarchy_depth_inter = reader.read_ue()?;
    let _max_transform_hierarchy_depth_intra = reader.read_ue()?;

    if reader.read_flag()? {
        let sps_scaling_list_data_present_flag = reader.read_flag()?;
        if sps_scaling_list_data_present_flag {
            skip_hevc_scaling_list_data(&mut reader)?;
        }
    }

    let _amp_enabled_flag = reader.read_flag()?;
    let _sample_adaptive_offset_enabled_flag = reader.read_flag()?;
    if reader.read_flag()? {
        let _pcm_sample_bit_depth_luma_minus1 = reader.read_bits(4)?;
        let _pcm_sample_bit_depth_chroma_minus1 = reader.read_bits(4)?;
        let _log2_min_pcm_luma_coding_block_size_minus3 = reader.read_ue()?;
        let _log2_diff_max_min_pcm_luma_coding_block_size = reader.read_ue()?;
        let _pcm_loop_filter_disabled_flag = reader.read_flag()?;
    }

    let num_short_term_ref_pic_sets = usize::try_from(reader.read_ue()?)
        .map_err(|_| "num_short_term_ref_pic_sets conversion failed".to_string())?;
    let expected_count = sps.short_term_ref_pic_sets.num_delta_pocs.len();
    if num_short_term_ref_pic_sets != expected_count {
        return Err(format!(
            "SPS ST-RPS count mismatch: raw={num_short_term_ref_pic_sets}, parsed={expected_count}"
        ));
    }

    let mut metadata = Vec::with_capacity(num_short_term_ref_pic_sets);
    for st_rps_idx in 0..num_short_term_ref_pic_sets {
        metadata.push(parse_hevc_sps_st_ref_pic_set_prediction_metadata(
            &mut reader,
            st_rps_idx,
            num_short_term_ref_pic_sets,
            sps,
        )?);
    }
    Ok(metadata)
}

fn skip_hevc_profile_tier_level(
    reader: &mut RbspBitReader<'_>,
    max_sub_layers_minus1: usize,
) -> Result<(), String> {
    let _general_profile_space_tier_profile_idc = reader.read_bits(8)?;
    let _general_profile_compatibility_flags = reader.read_bits(32)?;
    let _general_constraint_indicator_flags_hi = reader.read_bits(32)?;
    let _general_constraint_indicator_flags_lo = reader.read_bits(16)?;
    let _general_level_idc = reader.read_bits(8)?;

    let mut sub_layer_profile_present = [false; 7];
    let mut sub_layer_level_present = [false; 7];
    for i in 0..max_sub_layers_minus1 {
        sub_layer_profile_present[i] = reader.read_flag()?;
        sub_layer_level_present[i] = reader.read_flag()?;
    }
    if max_sub_layers_minus1 > 0 {
        for _ in max_sub_layers_minus1..8 {
            let _reserved_zero_2bits = reader.read_bits(2)?;
        }
    }
    for i in 0..max_sub_layers_minus1 {
        if sub_layer_profile_present[i] {
            let _sub_layer_profile_space_tier_profile_idc = reader.read_bits(8)?;
            let _sub_layer_profile_compatibility_flags = reader.read_bits(32)?;
            let _sub_layer_constraint_indicator_flags_hi = reader.read_bits(32)?;
            let _sub_layer_constraint_indicator_flags_lo = reader.read_bits(16)?;
        }
        if sub_layer_level_present[i] {
            let _sub_layer_level_idc = reader.read_bits(8)?;
        }
    }
    Ok(())
}

fn parse_hevc_profile_tier_level(
    reader: &mut RbspBitReader<'_>,
    max_sub_layers_minus1: usize,
) -> Result<StdVideoH265ProfileTierLevel, String> {
    let profile_space_tier_profile_idc = reader.read_bits(8)?;
    let tier_flag = ((profile_space_tier_profile_idc >> 5) & 1) != 0;
    let profile_idc = profile_space_tier_profile_idc & 0x1f;
    let _general_profile_compatibility_flags = reader.read_bits(32)?;
    let constraint_indicator_hi = reader.read_bits(32)?;
    let _constraint_indicator_lo = reader.read_bits(16)?;
    let general_level_idc = narrow_u32_to_u8(reader.read_bits(8)?, "general_level_idc")?;

    let mut profile_tier_flags = empty_profile_tier_level_flags();
    profile_tier_flags.set_general_tier_flag(bool_to_u32(tier_flag));
    profile_tier_flags.set_general_progressive_source_flag((constraint_indicator_hi >> 31) & 1);
    profile_tier_flags.set_general_interlaced_source_flag((constraint_indicator_hi >> 30) & 1);
    profile_tier_flags.set_general_non_packed_constraint_flag((constraint_indicator_hi >> 29) & 1);
    profile_tier_flags.set_general_frame_only_constraint_flag((constraint_indicator_hi >> 28) & 1);

    let mut sub_layer_profile_present = [false; 7];
    let mut sub_layer_level_present = [false; 7];
    for i in 0..max_sub_layers_minus1 {
        sub_layer_profile_present[i] = reader.read_flag()?;
        sub_layer_level_present[i] = reader.read_flag()?;
    }
    if max_sub_layers_minus1 > 0 {
        for _ in max_sub_layers_minus1..8 {
            let _reserved_zero_2bits = reader.read_bits(2)?;
        }
    }
    for i in 0..max_sub_layers_minus1 {
        if sub_layer_profile_present[i] {
            let _sub_layer_profile_space_tier_profile_idc = reader.read_bits(8)?;
            let _sub_layer_profile_compatibility_flags = reader.read_bits(32)?;
            let _sub_layer_constraint_indicator_flags_hi = reader.read_bits(32)?;
            let _sub_layer_constraint_indicator_flags_lo = reader.read_bits(16)?;
        }
        if sub_layer_level_present[i] {
            let _sub_layer_level_idc = reader.read_bits(8)?;
        }
    }

    Ok(StdVideoH265ProfileTierLevel {
        flags: profile_tier_flags,
        general_profile_idc: StdVideoH265ProfileIdc::from(profile_idc),
        general_level_idc: StdVideoH265LevelIdc::from(general_level_idc),
    })
}

fn skip_hevc_scaling_list_data(reader: &mut RbspBitReader<'_>) -> Result<(), String> {
    for size_id in 0..4 {
        let matrix_count = if size_id == 3 { 2 } else { 6 };
        for _ in 0..matrix_count {
            let scaling_list_pred_mode_flag = reader.read_flag()?;
            if !scaling_list_pred_mode_flag {
                let _scaling_list_pred_matrix_id_delta = reader.read_ue()?;
                continue;
            }
            let coef_num = 64_usize.min(1_usize << (4 + (size_id << 1)));
            if size_id > 1 {
                let _scaling_list_dc_coef_minus8 = reader.read_se()?;
            }
            for _ in 0..coef_num {
                let _scaling_list_delta_coef = reader.read_se()?;
            }
        }
    }
    Ok(())
}

fn parse_hevc_sps_st_ref_pic_set_prediction_metadata(
    reader: &mut RbspBitReader<'_>,
    st_rps_idx: usize,
    num_short_term_ref_pic_sets: usize,
    sps: &SpsRbsp,
) -> Result<HevcSpsShortTermRefPicSetPredictionMetadata, String> {
    let inter_ref_pic_set_prediction_flag = if st_rps_idx == 0 {
        false
    } else {
        reader.read_flag()?
    };
    if !inter_ref_pic_set_prediction_flag {
        let num_negative_pics = usize::try_from(reader.read_ue()?)
            .map_err(|_| "num_negative_pics conversion failed".to_string())?;
        let num_positive_pics = usize::try_from(reader.read_ue()?)
            .map_err(|_| "num_positive_pics conversion failed".to_string())?;
        for _ in 0..num_negative_pics {
            let _delta_poc_s0_minus1 = reader.read_ue()?;
            let _used_by_curr_pic_s0_flag = reader.read_flag()?;
        }
        for _ in 0..num_positive_pics {
            let _delta_poc_s1_minus1 = reader.read_ue()?;
            let _used_by_curr_pic_s1_flag = reader.read_flag()?;
        }
        return Ok(HevcSpsShortTermRefPicSetPredictionMetadata::default());
    }

    let delta_idx_minus1 = if st_rps_idx == num_short_term_ref_pic_sets {
        reader.read_ue()?
    } else {
        0
    };
    let ref_rps_idx = st_rps_idx
        .checked_sub(usize::try_from(delta_idx_minus1).unwrap_or(usize::MAX).saturating_add(1))
        .ok_or_else(|| {
            format!(
                "SPS predicted short-term RPS index underflow: st_rps_idx={st_rps_idx}, delta_idx_minus1={delta_idx_minus1}"
            )
        })?;
    let delta_rps_sign = reader.read_flag()?;
    let abs_delta_rps_minus1 = reader.read_ue()?;
    let ref_num_delta_pocs = sps
        .short_term_ref_pic_sets
        .num_delta_pocs
        .get(ref_rps_idx)
        .copied()
        .ok_or_else(|| format!("SPS predicted ST-RPS ref index {ref_rps_idx} missing"))?;
    let ref_num_delta_pocs = usize::try_from(ref_num_delta_pocs)
        .map_err(|_| "ref_num_delta_pocs conversion failed".to_string())?;

    let mut used_by_curr_pic_flag = 0_u16;
    let mut use_delta_flag = 0_u16;
    for index in 0..=ref_num_delta_pocs {
        let used = reader.read_flag()?;
        if used {
            used_by_curr_pic_flag |= 1_u16
                .checked_shl(u32::try_from(index).unwrap_or(u32::MAX))
                .unwrap_or(0);
            use_delta_flag |= 1_u16
                .checked_shl(u32::try_from(index).unwrap_or(u32::MAX))
                .unwrap_or(0);
        } else {
            let use_delta = reader.read_flag()?;
            if use_delta {
                use_delta_flag |= 1_u16
                    .checked_shl(u32::try_from(index).unwrap_or(u32::MAX))
                    .unwrap_or(0);
            }
        }
    }

    Ok(HevcSpsShortTermRefPicSetPredictionMetadata {
        inter_ref_pic_set_prediction_flag,
        delta_idx_minus1,
        delta_rps_sign,
        abs_delta_rps_minus1,
        used_by_curr_pic_flag,
        use_delta_flag,
    })
}

fn build_std_long_term_ref_pics_sps(
    sps: &SpsRbsp,
) -> Result<Option<StdVideoH265LongTermRefPicsSps>, String> {
    let Some(long_term_ref_pics) = &sps.long_term_ref_pics else {
        return Ok(None);
    };
    if long_term_ref_pics.lt_ref_pic_poc_lsb_sps.len() > 32 {
        return Err(format!(
            "long_term_ref_pics has {} entries (max 32)",
            long_term_ref_pics.lt_ref_pic_poc_lsb_sps.len()
        ));
    }
    let mut used_by_curr_pic_lt_sps_flag = 0_u32;
    let mut lt_ref_pic_poc_lsb_sps = [0_u32; 32];

    for (index, &poc_lsb) in long_term_ref_pics.lt_ref_pic_poc_lsb_sps.iter().enumerate() {
        lt_ref_pic_poc_lsb_sps[index] = narrow_u64_to_u32(poc_lsb, "lt_ref_pic_poc_lsb_sps")?;
    }
    for (index, &used) in long_term_ref_pics
        .used_by_curr_pic_lt_sps_flag
        .iter()
        .enumerate()
    {
        if used {
            used_by_curr_pic_lt_sps_flag |= 1_u32 << index;
        }
    }

    Ok(Some(StdVideoH265LongTermRefPicsSps {
        used_by_curr_pic_lt_sps_flag,
        lt_ref_pic_poc_lsb_sps,
    }))
}

fn infer_sps_sub_layer_ordering_info_present_flag(sps: &SpsRbsp) -> bool {
    if sps.sps_max_sub_layers_minus1 == 0 {
        return true;
    }
    let dec_pic_buf = &sps.sub_layer_ordering_info.sps_max_dec_pic_buffering_minus1;
    let reorder_pics = &sps.sub_layer_ordering_info.sps_max_num_reorder_pics;
    let latency = &sps.sub_layer_ordering_info.sps_max_latency_increase_plus1;
    !all_values_equal(dec_pic_buf) || !all_values_equal(reorder_pics) || !all_values_equal(latency)
}

fn all_values_equal<T: PartialEq>(values: &[T]) -> bool {
    match values.split_first() {
        None => true,
        Some((head, tail)) => tail.iter().all(|value| value == head),
    }
}

pub(super) fn parse_hevc_vps(vps_nalu: &[u8]) -> Result<ParsedHevcVps, String> {
    match hevc_nal_type(vps_nalu) {
        Some(NALUnitType::VpsNut) => {}
        Some(other) => return Err(format!("expected VPS NALU but found {other:?}")),
        None => return Err("truncated VPS NALU header".to_string()),
    }

    let rbsp = nalu_payload_to_rbsp(vps_nalu)?;
    let mut reader = RbspBitReader::new(&rbsp);
    let vps_video_parameter_set_id =
        narrow_u32_to_u8(reader.read_bits(4)?, "vps_video_parameter_set_id")?;
    let _vps_base_layer_internal_flag = reader.read_flag()?;
    let _vps_base_layer_available_flag = reader.read_flag()?;
    let _vps_max_layers_minus1 = reader.read_bits(6)?;
    let vps_max_sub_layers_minus1 =
        narrow_u32_to_u8(reader.read_bits(3)?, "vps_max_sub_layers_minus1")?;
    let vps_temporal_id_nesting_flag = reader.read_flag()?;
    let _vps_reserved_0xffff_16bits = reader.read_bits(16)?;
    let profile_tier_level =
        parse_hevc_profile_tier_level(&mut reader, usize::from(vps_max_sub_layers_minus1))?;

    let vps_sub_layer_ordering_info_present_flag = reader.read_flag()?;
    let mut vps_dec_pic_buf_mgr = StdVideoH265DecPicBufMgr {
        max_latency_increase_plus1: [0; 7],
        max_dec_pic_buffering_minus1: [0; 7],
        max_num_reorder_pics: [0; 7],
    };
    let first_ordering_index = if vps_sub_layer_ordering_info_present_flag {
        0
    } else {
        usize::from(vps_max_sub_layers_minus1)
    };
    for index in first_ordering_index..=usize::from(vps_max_sub_layers_minus1) {
        vps_dec_pic_buf_mgr.max_dec_pic_buffering_minus1[index] =
            narrow_u32_to_u8(reader.read_ue()?, "vps_max_dec_pic_buffering_minus1")?;
        vps_dec_pic_buf_mgr.max_num_reorder_pics[index] =
            narrow_u32_to_u8(reader.read_ue()?, "vps_max_num_reorder_pics")?;
        vps_dec_pic_buf_mgr.max_latency_increase_plus1[index] = reader.read_ue()?;
    }
    if !vps_sub_layer_ordering_info_present_flag {
        let inferred_index = usize::from(vps_max_sub_layers_minus1);
        for index in 0..inferred_index {
            vps_dec_pic_buf_mgr.max_dec_pic_buffering_minus1[index] =
                vps_dec_pic_buf_mgr.max_dec_pic_buffering_minus1[inferred_index];
            vps_dec_pic_buf_mgr.max_num_reorder_pics[index] =
                vps_dec_pic_buf_mgr.max_num_reorder_pics[inferred_index];
            vps_dec_pic_buf_mgr.max_latency_increase_plus1[index] =
                vps_dec_pic_buf_mgr.max_latency_increase_plus1[inferred_index];
        }
    }

    let vps_max_layer_id = reader.read_bits(6)?;
    let vps_num_layer_sets_minus1 = reader.read_ue()?;
    for _ in 1..=vps_num_layer_sets_minus1 {
        for _ in 0..=vps_max_layer_id {
            let _layer_id_included_flag = reader.read_flag()?;
        }
    }

    let vps_timing_info_present_flag = reader.read_flag()?;
    let mut vps_num_units_in_tick = 0;
    let mut vps_time_scale = 0;
    let mut vps_poc_proportional_to_timing_flag = false;
    let mut vps_num_ticks_poc_diff_one_minus1 = 0;
    if vps_timing_info_present_flag {
        vps_num_units_in_tick = reader.read_bits(32)?;
        vps_time_scale = reader.read_bits(32)?;
        vps_poc_proportional_to_timing_flag = reader.read_flag()?;
        if vps_poc_proportional_to_timing_flag {
            vps_num_ticks_poc_diff_one_minus1 = reader.read_ue()?;
        }
    }

    Ok(ParsedHevcVps {
        vps_video_parameter_set_id,
        vps_max_sub_layers_minus1,
        vps_temporal_id_nesting_flag,
        profile_tier_level,
        vps_sub_layer_ordering_info_present_flag,
        vps_dec_pic_buf_mgr,
        vps_timing_info_present_flag,
        vps_num_units_in_tick,
        vps_time_scale,
        vps_poc_proportional_to_timing_flag,
        vps_num_ticks_poc_diff_one_minus1,
    })
}

pub(super) fn parse_hevc_pps(pps_nalu: &[u8]) -> Result<ParsedHevcPps, String> {
    match hevc_nal_type(pps_nalu) {
        Some(NALUnitType::PpsNut) => {}
        Some(other) => return Err(format!("expected PPS NALU but found {other:?}")),
        None => return Err("truncated PPS NALU header".to_string()),
    }

    let rbsp = nalu_payload_to_rbsp(pps_nalu)?;
    let mut reader = RbspBitReader::new(&rbsp);

    let pps_pic_parameter_set_id = narrow_u32_to_u8(reader.read_ue()?, "pps_pic_parameter_set_id")?;
    let pps_seq_parameter_set_id = narrow_u32_to_u8(reader.read_ue()?, "pps_seq_parameter_set_id")?;
    let dependent_slice_segments_enabled_flag = reader.read_flag()?;
    let output_flag_present_flag = reader.read_flag()?;
    let num_extra_slice_header_bits =
        narrow_u32_to_u8(reader.read_bits(3)?, "num_extra_slice_header_bits")?;
    let sign_data_hiding_enabled_flag = reader.read_flag()?;
    let cabac_init_present_flag = reader.read_flag()?;
    let num_ref_idx_l0_default_active_minus1 =
        narrow_u32_to_u8(reader.read_ue()?, "num_ref_idx_l0_default_active_minus1")?;
    let num_ref_idx_l1_default_active_minus1 =
        narrow_u32_to_u8(reader.read_ue()?, "num_ref_idx_l1_default_active_minus1")?;
    let init_qp_minus26 = narrow_i32_to_i8(reader.read_se()?, "init_qp_minus26")?;
    let constrained_intra_pred_flag = reader.read_flag()?;
    let transform_skip_enabled_flag = reader.read_flag()?;
    let cu_qp_delta_enabled_flag = reader.read_flag()?;
    let diff_cu_qp_delta_depth = if cu_qp_delta_enabled_flag {
        narrow_u32_to_u8(reader.read_ue()?, "diff_cu_qp_delta_depth")?
    } else {
        0
    };
    let pps_cb_qp_offset = narrow_i32_to_i8(reader.read_se()?, "pps_cb_qp_offset")?;
    let pps_cr_qp_offset = narrow_i32_to_i8(reader.read_se()?, "pps_cr_qp_offset")?;
    let pps_slice_chroma_qp_offsets_present_flag = reader.read_flag()?;
    let weighted_pred_flag = reader.read_flag()?;
    let weighted_bipred_flag = reader.read_flag()?;
    let transquant_bypass_enabled_flag = reader.read_flag()?;
    let tiles_enabled_flag = reader.read_flag()?;
    let entropy_coding_sync_enabled_flag = reader.read_flag()?;

    let mut num_tile_columns_minus1 = 0_u8;
    let mut num_tile_rows_minus1 = 0_u8;
    let mut uniform_spacing_flag = false;
    let mut loop_filter_across_tiles_enabled_flag = false;
    let mut column_width_minus1 = [0_u16; 19];
    let mut row_height_minus1 = [0_u16; 21];

    if tiles_enabled_flag {
        let num_tile_columns = reader
            .read_ue()?
            .checked_add(1)
            .ok_or_else(|| "num_tile_columns overflows u32".to_string())?;
        let num_tile_rows = reader
            .read_ue()?
            .checked_add(1)
            .ok_or_else(|| "num_tile_rows overflows u32".to_string())?;
        if num_tile_columns == 0 || num_tile_columns > 19 {
            return Err(format!(
                "num_tile_columns={num_tile_columns} is outside supported range 1..=19"
            ));
        }
        if num_tile_rows == 0 || num_tile_rows > 21 {
            return Err(format!(
                "num_tile_rows={num_tile_rows} is outside supported range 1..=21"
            ));
        }

        num_tile_columns_minus1 =
            narrow_u32_to_u8(num_tile_columns - 1, "num_tile_columns_minus1")?;
        num_tile_rows_minus1 = narrow_u32_to_u8(num_tile_rows - 1, "num_tile_rows_minus1")?;
        uniform_spacing_flag = reader.read_flag()?;
        if !uniform_spacing_flag {
            for width in column_width_minus1
                .iter_mut()
                .take(usize::from(num_tile_columns_minus1))
            {
                *width = narrow_u32_to_u16(reader.read_ue()?, "column_width_minus1")?;
            }
            for height in row_height_minus1
                .iter_mut()
                .take(usize::from(num_tile_rows_minus1))
            {
                *height = narrow_u32_to_u16(reader.read_ue()?, "row_height_minus1")?;
            }
        }
        loop_filter_across_tiles_enabled_flag = reader.read_flag()?;
    }

    let pps_loop_filter_across_slices_enabled_flag = reader.read_flag()?;
    let deblocking_filter_control_present_flag = reader.read_flag()?;
    let mut deblocking_filter_override_enabled_flag = false;
    let mut pps_deblocking_filter_disabled_flag = false;
    let mut pps_beta_offset_div2 = 0_i8;
    let mut pps_tc_offset_div2 = 0_i8;
    if deblocking_filter_control_present_flag {
        deblocking_filter_override_enabled_flag = reader.read_flag()?;
        pps_deblocking_filter_disabled_flag = reader.read_flag()?;
        if !pps_deblocking_filter_disabled_flag {
            pps_beta_offset_div2 = narrow_i32_to_i8(reader.read_se()?, "pps_beta_offset_div2")?;
            pps_tc_offset_div2 = narrow_i32_to_i8(reader.read_se()?, "pps_tc_offset_div2")?;
        }
    }

    let pps_scaling_list_data_present_flag = reader.read_flag()?;
    if pps_scaling_list_data_present_flag {
        return Err("pps_scaling_list_data_present_flag=1 is not yet supported".to_string());
    }
    let lists_modification_present_flag = reader.read_flag()?;
    let log2_parallel_merge_level_minus2 =
        narrow_u32_to_u8(reader.read_ue()?, "log2_parallel_merge_level_minus2")?;
    let slice_segment_header_extension_present_flag = reader.read_flag()?;
    let pps_extension_present_flag = reader.read_flag()?;
    if pps_extension_present_flag {
        return Err("pps_extension_present_flag=1 is not yet supported".to_string());
    }

    Ok(ParsedHevcPps {
        pps_pic_parameter_set_id,
        pps_seq_parameter_set_id,
        num_extra_slice_header_bits,
        num_ref_idx_l0_default_active_minus1,
        num_ref_idx_l1_default_active_minus1,
        init_qp_minus26,
        diff_cu_qp_delta_depth,
        pps_cb_qp_offset,
        pps_cr_qp_offset,
        pps_beta_offset_div2,
        pps_tc_offset_div2,
        log2_parallel_merge_level_minus2,
        num_tile_columns_minus1,
        num_tile_rows_minus1,
        column_width_minus1,
        row_height_minus1,
        dependent_slice_segments_enabled_flag,
        output_flag_present_flag,
        sign_data_hiding_enabled_flag,
        cabac_init_present_flag,
        constrained_intra_pred_flag,
        transform_skip_enabled_flag,
        cu_qp_delta_enabled_flag,
        pps_slice_chroma_qp_offsets_present_flag,
        weighted_pred_flag,
        weighted_bipred_flag,
        transquant_bypass_enabled_flag,
        tiles_enabled_flag,
        entropy_coding_sync_enabled_flag,
        uniform_spacing_flag,
        loop_filter_across_tiles_enabled_flag,
        pps_loop_filter_across_slices_enabled_flag,
        deblocking_filter_control_present_flag,
        deblocking_filter_override_enabled_flag,
        pps_deblocking_filter_disabled_flag,
        lists_modification_present_flag,
        slice_segment_header_extension_present_flag,
    })
}

pub(super) fn parse_hevc_slice_header(
    slice_nalu: &[u8],
    parsed_pps: &ParsedHevcPps,
    sps: &SpsRbsp,
) -> Result<ParsedHevcSliceHeader, String> {
    let nal_unit_type =
        hevc_nal_type_raw(slice_nalu).ok_or_else(|| "truncated slice NALU header".to_string())?;
    if nal_unit_type > 31 {
        return Err(format!(
            "NALU type {nal_unit_type} is not a VCL slice NAL unit"
        ));
    }

    let rbsp = nalu_payload_to_rbsp(slice_nalu)?;
    let mut reader = RbspBitReader::new(&rbsp);
    let first_slice_segment_in_pic_flag = reader.read_flag()?;
    if (16..=23).contains(&nal_unit_type) {
        let _no_output_of_prior_pics_flag = reader.read_flag()?;
    }
    let pps_id = narrow_u32_to_u8(reader.read_ue()?, "slice_pic_parameter_set_id")?;

    if !first_slice_segment_in_pic_flag {
        return Ok(ParsedHevcSliceHeader {
            nal_unit_type,
            pps_id,
            is_first_slice_segment: false,
            pic_order_cnt_lsb: None,
            slice_type: None,
            short_term_ref_pic_set_idx: None,
            inline_short_term_ref_pic_set_pocs: None,
            num_bits_for_st_ref_pic_set_in_slice: 0,
            num_delta_pocs_of_ref_rps_idx: 0,
        });
    }

    for _ in 0..parsed_pps.num_extra_slice_header_bits {
        let _ = reader.read_flag()?;
    }
    let slice_type = narrow_u32_to_u8(reader.read_ue()?, "slice_type")?;
    if parsed_pps.output_flag_present_flag {
        let _pic_output_flag = reader.read_flag()?;
    }
    if sps.separate_colour_plane_flag {
        let _colour_plane_id = reader.read_bits(2)?;
    }

    let pic_order_cnt_lsb = if is_hevc_idr_nal_type(nal_unit_type) {
        None
    } else {
        let poc_bits = usize::from(sps.log2_max_pic_order_cnt_lsb_minus4) + 4;
        Some(narrow_u32_to_u16(
            reader.read_bits(poc_bits)?,
            "slice_pic_order_cnt_lsb",
        )?)
    };
    let (
        short_term_ref_pic_set_idx,
        inline_short_term_ref_pic_set_pocs,
        num_bits_for_st_ref_pic_set_in_slice,
        num_delta_pocs_of_ref_rps_idx,
    ) = if is_hevc_idr_nal_type(nal_unit_type) {
        (None, None, 0_u16, 0_u8)
    } else {
        let short_term_ref_pic_set_count = sps.short_term_ref_pic_sets.num_delta_pocs.len();
        // Per HEVC spec 7.4.7.1: NumBitsForSTRefPicSetInSlice counts only the bits of
        // st_ref_pic_set() (sps_flag=0) or short_term_ref_pic_set_idx (sps_flag=1, count>1).
        // The 1-bit short_term_ref_pic_set_sps_flag is NOT included.  Capture bits_before
        // AFTER reading the sps_flag so the measured delta excludes it.
        let current_poc = pic_order_cnt_lsb.map(i32::from).unwrap_or(0);
        if short_term_ref_pic_set_count == 0 {
            // num_short_term_ref_pic_sets==0: sps_flag is still transmitted (always 0).
            let _sps_flag_always_zero = reader.read_flag()?;
            let bits_before = reader.bit_offset;
            let result =
                parse_hevc_inline_short_term_ref_pic_set_usage(&mut reader, 0, sps, current_poc)?;
            let bits_after = reader.bit_offset;
            let num_bits =
                u16::try_from(bits_after.saturating_sub(bits_before)).unwrap_or(u16::MAX);
            match result {
                Some(pocs) => (
                    None,
                    Some(pocs),
                    num_bits,
                    u8::try_from(pocs.num_delta_pocs_of_ref_rps_idx).unwrap_or(u8::MAX),
                ),
                None => (None, None, 0, 0),
            }
        } else {
            let short_term_ref_pic_set_sps_flag = reader.read_flag()?;
            if !short_term_ref_pic_set_sps_flag {
                let bits_before = reader.bit_offset;
                let result = parse_hevc_inline_short_term_ref_pic_set_usage(
                    &mut reader,
                    short_term_ref_pic_set_count,
                    sps,
                    current_poc,
                )?;
                let bits_after = reader.bit_offset;
                let num_bits =
                    u16::try_from(bits_after.saturating_sub(bits_before)).unwrap_or(u16::MAX);
                match result {
                    Some(pocs) => (
                        None,
                        Some(pocs),
                        num_bits,
                        u8::try_from(pocs.num_delta_pocs_of_ref_rps_idx).unwrap_or(u8::MAX),
                    ),
                    None => (None, None, 0, 0),
                }
            } else if short_term_ref_pic_set_count == 1 {
                // sps_flag=1, exactly one SPS RPS → no index field → NumBitsForSTRefPicSetInSlice=0.
                (Some(0), None, 0, 0)
            } else {
                let bits_before = reader.bit_offset;
                let index_bit_count =
                    bit_width_for_index_range(short_term_ref_pic_set_count.saturating_sub(1));
                let parsed_index =
                    usize::try_from(reader.read_bits(index_bit_count)?).map_err(|_| {
                        "short_term_ref_pic_set_idx conversion to usize failed".to_string()
                    })?;
                if parsed_index >= short_term_ref_pic_set_count {
                    return Err(format!(
                        "short_term_ref_pic_set_idx {parsed_index} exceeds SPS short-term set count {short_term_ref_pic_set_count}"
                    ));
                }
                let bits_after = reader.bit_offset;
                let num_bits =
                    u16::try_from(bits_after.saturating_sub(bits_before)).unwrap_or(u16::MAX);
                (Some(parsed_index), None, num_bits, 0)
            }
        }
    };

    Ok(ParsedHevcSliceHeader {
        nal_unit_type,
        pps_id,
        is_first_slice_segment: true,
        pic_order_cnt_lsb,
        slice_type: Some(slice_type),
        short_term_ref_pic_set_idx,
        inline_short_term_ref_pic_set_pocs,
        num_bits_for_st_ref_pic_set_in_slice,
        num_delta_pocs_of_ref_rps_idx,
    })
}

pub(super) fn bit_width_for_index_range(max_index: usize) -> usize {
    if max_index == 0 {
        return 0;
    }
    usize::BITS
        .saturating_sub(max_index.leading_zeros())
        .try_into()
        .unwrap_or(usize::BITS as usize)
}

pub(super) fn parse_hevc_inline_short_term_ref_pic_set_usage(
    reader: &mut RbspBitReader<'_>,
    st_rps_idx: usize,
    sps: &SpsRbsp,
    current_poc: i32,
) -> Result<Option<HevcInlineRefPicSetPocs>, String> {
    let max_poc_lsb = i32::try_from(sps.max_pic_order_cnt_lsb()).unwrap_or(i32::MAX / 2);
    let inter_ref_pic_set_prediction_flag = if st_rps_idx == 0 {
        false
    } else {
        reader.read_flag()?
    };
    if inter_ref_pic_set_prediction_flag {
        let short_term_ref_pic_sets = &sps.short_term_ref_pic_sets;
        let short_term_ref_pic_set_count = short_term_ref_pic_sets.num_delta_pocs.len();
        let delta_idx_minus1 = if st_rps_idx == short_term_ref_pic_set_count {
            usize::try_from(reader.read_ue()?)
                .map_err(|_| "delta_idx_minus1 conversion to usize failed".to_string())?
        } else {
            0
        };
        let ref_rps_idx = st_rps_idx
            .checked_sub(delta_idx_minus1.saturating_add(1))
            .ok_or_else(|| {
                format!(
                    "predicted inline short-term RPS index underflow: st_rps_idx={st_rps_idx}, delta_idx_minus1={delta_idx_minus1}"
                )
            })?;
        let ref_delta_poc_s0 = short_term_ref_pic_sets
            .delta_poc_s0
            .get(ref_rps_idx)
            .ok_or_else(|| format!("missing delta_poc_s0 for ref_rps_idx={ref_rps_idx}"))?;
        let ref_delta_poc_s1 = short_term_ref_pic_sets
            .delta_poc_s1
            .get(ref_rps_idx)
            .ok_or_else(|| format!("missing delta_poc_s1 for ref_rps_idx={ref_rps_idx}"))?;
        let ref_num_delta_pocs = ref_delta_poc_s0
            .len()
            .saturating_add(ref_delta_poc_s1.len());
        let delta_rps_sign = reader.read_flag()?;
        let abs_delta_rps_minus1 = i64::from(reader.read_ue()?);
        let delta_rps_magnitude = abs_delta_rps_minus1.saturating_add(1);
        let delta_rps = if delta_rps_sign {
            -delta_rps_magnitude
        } else {
            delta_rps_magnitude
        };

        let mut before_deltas: Vec<i64> = Vec::new();
        let mut after_deltas: Vec<i64> = Vec::new();
        for delta_index in 0..ref_num_delta_pocs {
            let used_by_curr_pic_flag = reader.read_flag()?;
            if !used_by_curr_pic_flag {
                let _use_delta_flag = reader.read_flag()?;
                continue;
            }
            let base_delta = if delta_index < ref_delta_poc_s0.len() {
                ref_delta_poc_s0[delta_index]
            } else {
                ref_delta_poc_s1[delta_index - ref_delta_poc_s0.len()]
            };
            let candidate_delta = base_delta.saturating_add(delta_rps);
            if candidate_delta < 0 {
                before_deltas.push(candidate_delta);
            } else if candidate_delta > 0 {
                after_deltas.push(candidate_delta);
            }
        }
        // RefPicSetStCurrBefore: most-recent first (largest poc < current → delta closest to 0 → descending).
        before_deltas.sort_by(|a, b| b.cmp(a));
        // RefPicSetStCurrAfter: nearest first (smallest poc > current → delta closest to 0 → ascending).
        after_deltas.sort();
        let mut pocs = HevcInlineRefPicSetPocs {
            num_delta_pocs_of_ref_rps_idx: ref_num_delta_pocs,
            ..Default::default()
        };
        for delta in before_deltas.iter().take(HEVC_REF_PIC_SET_LIST_SIZE) {
            let abs_poc =
                (i64::from(current_poc).saturating_add(*delta)).rem_euclid(i64::from(max_poc_lsb));
            pocs.before[pocs.before_count] = i32::try_from(abs_poc).unwrap_or(i32::MIN);
            pocs.before_count += 1;
        }
        for delta in after_deltas.iter().take(HEVC_REF_PIC_SET_LIST_SIZE) {
            let abs_poc =
                (i64::from(current_poc).saturating_add(*delta)).rem_euclid(i64::from(max_poc_lsb));
            pocs.after[pocs.after_count] = i32::try_from(abs_poc).unwrap_or(i32::MIN);
            pocs.after_count += 1;
        }
        return Ok(Some(pocs));
    }

    // Non-predicted inline RPS.  num_negative_pics and num_positive_pics are read first, then
    // the per-picture (delta, used) pairs are interleaved within each set.
    // Delta POC values accumulate: DeltaPocS0[i] = DeltaPocS0[i-1] - (delta_poc_s0_minus1[i] + 1),
    // with DeltaPocS0[0] = -(delta_poc_s0_minus1[0] + 1).  Entry 0 is the most recent before-ref
    // (smallest magnitude negative delta), matching RefPicSetStCurrBefore ordering.
    // DeltaPocS1 accumulates positively; entry 0 is the nearest after-ref.
    let num_negative_pics = usize::try_from(reader.read_ue()?)
        .map_err(|_| "num_negative_pics conversion to usize failed".to_string())?;
    let num_positive_pics = usize::try_from(reader.read_ue()?)
        .map_err(|_| "num_positive_pics conversion to usize failed".to_string())?;
    let mut pocs = HevcInlineRefPicSetPocs::default();
    let mut cumulative_s0: i64 = 0;
    for _ in 0..num_negative_pics {
        let delta_minus1 = i64::from(reader.read_ue()?);
        cumulative_s0 -= delta_minus1 + 1;
        let used_by_curr_pic_s0_flag = reader.read_flag()?;
        if used_by_curr_pic_s0_flag && pocs.before_count < HEVC_REF_PIC_SET_LIST_SIZE {
            let abs_poc =
                (i64::from(current_poc) + cumulative_s0).rem_euclid(i64::from(max_poc_lsb));
            pocs.before[pocs.before_count] = i32::try_from(abs_poc).unwrap_or(i32::MIN);
            pocs.before_count += 1;
        }
    }
    let mut cumulative_s1: i64 = 0;
    for _ in 0..num_positive_pics {
        let delta_minus1 = i64::from(reader.read_ue()?);
        cumulative_s1 += delta_minus1 + 1;
        let used_by_curr_pic_s1_flag = reader.read_flag()?;
        if used_by_curr_pic_s1_flag && pocs.after_count < HEVC_REF_PIC_SET_LIST_SIZE {
            let abs_poc =
                (i64::from(current_poc) + cumulative_s1).rem_euclid(i64::from(max_poc_lsb));
            pocs.after[pocs.after_count] = i32::try_from(abs_poc).unwrap_or(i32::MIN);
            pocs.after_count += 1;
        }
    }
    // Non-predicted inline RPS: NumDeltaPocsOfRefRpsIdx = 0 (already default).
    Ok(Some(pocs))
}

/// Returns the H.265 temporal identifier (0-based) from byte 1 of the NAL unit header.
///
/// Per spec: `nuh_temporal_id_plus1 = nalu[1] & 0x07`; temporal_id = nuh_temporal_id_plus1 - 1.
/// VCL NAL units always have `nuh_temporal_id_plus1 >= 1`, so saturating_sub is safe.
pub(super) fn hevc_nalu_temporal_id(nalu: &[u8]) -> u8 {
    nalu.get(1).map_or(0, |&b| (b & 0x07).saturating_sub(1))
}

/// POC unwrapper implementing H.265 spec section 8.3.1.
///
/// Maintains the `prevTid0PicOrderCntMsb` / `prevTid0PicOrderCntLsb` state variables and
/// computes the full (unwrapped) `PicOrderCntVal` for each picture in decode order.
fn nalu_payload_to_rbsp(nalu: &[u8]) -> Result<Vec<u8>, String> {
    if nalu.len() < 2 {
        return Err("HEVC NALU is truncated before RBSP payload".to_string());
    }
    let payload = &nalu[2..];
    let mut rbsp = Vec::with_capacity(payload.len());
    let mut index = 0_usize;
    while index < payload.len() {
        if index + 2 < payload.len()
            && payload[index] == 0
            && payload[index + 1] == 0
            && payload[index + 2] == 0x03
        {
            rbsp.push(0);
            rbsp.push(0);
            index += 3;
            continue;
        }
        rbsp.push(payload[index]);
        index += 1;
    }
    Ok(rbsp)
}

pub(super) const fn bool_to_u32(value: bool) -> u32 {
    if value { 1 } else { 0 }
}

fn map_h265_chroma_format_idc(
    chroma_format_idc: u8,
) -> Result<StdVideoH265ChromaFormatIdc, String> {
    match chroma_format_idc {
        0 => Ok(StdVideoH265ChromaFormatIdc_STD_VIDEO_H265_CHROMA_FORMAT_IDC_MONOCHROME),
        1 => Ok(StdVideoH265ChromaFormatIdc_STD_VIDEO_H265_CHROMA_FORMAT_IDC_420),
        2 => Ok(StdVideoH265ChromaFormatIdc_STD_VIDEO_H265_CHROMA_FORMAT_IDC_422),
        3 => Ok(StdVideoH265ChromaFormatIdc_STD_VIDEO_H265_CHROMA_FORMAT_IDC_444),
        _ => Err(format!(
            "unsupported SPS chroma_format_idc={chroma_format_idc} (expected 0..=3)"
        )),
    }
}

fn bools_to_u16_mask(values: &[bool], field_name: &str) -> Result<u16, String> {
    if values.len() > 16 {
        return Err(format!(
            "{field_name} length {} exceeds 16-bit mask capacity",
            values.len()
        ));
    }
    let mut mask = 0_u16;
    for (index, &value) in values.iter().enumerate() {
        if value {
            mask |= 1_u16 << index;
        }
    }
    Ok(mask)
}

fn empty_vps_flags() -> StdVideoH265VpsFlags {
    StdVideoH265VpsFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
        __bindgen_padding_0: [0; 3],
    }
}

fn empty_profile_tier_level_flags() -> StdVideoH265ProfileTierLevelFlags {
    StdVideoH265ProfileTierLevelFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
        __bindgen_padding_0: [0; 3],
    }
}

fn empty_sps_flags() -> StdVideoH265SpsFlags {
    StdVideoH265SpsFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
    }
}

fn empty_sps_vui_flags() -> StdVideoH265SpsVuiFlags {
    StdVideoH265SpsVuiFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
        __bindgen_padding_0: 0,
    }
}

fn empty_hrd_flags() -> StdVideoH265HrdFlags {
    StdVideoH265HrdFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
    }
}

fn empty_hrd_parameters() -> StdVideoH265HrdParameters {
    StdVideoH265HrdParameters {
        flags: empty_hrd_flags(),
        tick_divisor_minus2: 0,
        du_cpb_removal_delay_increment_length_minus1: 0,
        dpb_output_delay_du_length_minus1: 0,
        bit_rate_scale: 0,
        cpb_size_scale: 0,
        cpb_size_du_scale: 0,
        initial_cpb_removal_delay_length_minus1: 0,
        au_cpb_removal_delay_length_minus1: 0,
        dpb_output_delay_length_minus1: 0,
        cpb_cnt_minus1: [0; 7],
        elemental_duration_in_tc_minus1: [0; 7],
        reserved: [0; 3],
        pSubLayerHrdParametersNal: std::ptr::null(),
        pSubLayerHrdParametersVcl: std::ptr::null(),
    }
}

fn empty_pps_flags() -> StdVideoH265PpsFlags {
    StdVideoH265PpsFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
    }
}

fn empty_short_term_ref_pic_set_flags() -> StdVideoH265ShortTermRefPicSetFlags {
    StdVideoH265ShortTermRefPicSetFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
        __bindgen_padding_0: [0; 3],
    }
}

pub(super) fn empty_decode_h265_picture_info_flags() -> StdVideoDecodeH265PictureInfoFlags {
    StdVideoDecodeH265PictureInfoFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
        __bindgen_padding_0: [0; 3],
    }
}

pub(super) fn empty_decode_h265_reference_info_flags() -> StdVideoDecodeH265ReferenceInfoFlags {
    StdVideoDecodeH265ReferenceInfoFlags {
        _bitfield_align_1: [],
        _bitfield_1: Default::default(),
        __bindgen_padding_0: [0; 3],
    }
}

pub(super) fn narrow_u64_to_u8(value: u64, field_name: &str) -> Result<u8, String> {
    u8::try_from(value).map_err(|_| format!("{field_name}={value} exceeds u8 range"))
}

fn narrow_u64_to_u32(value: u64, field_name: &str) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("{field_name}={value} exceeds u32 range"))
}

fn narrow_u64_to_u16(value: u64, field_name: &str) -> Result<u16, String> {
    u16::try_from(value).map_err(|_| format!("{field_name}={value} exceeds u16 range"))
}

fn narrow_u32_to_u8(value: u32, field_name: &str) -> Result<u8, String> {
    u8::try_from(value).map_err(|_| format!("{field_name}={value} exceeds u8 range"))
}

fn narrow_u32_to_u16(value: u32, field_name: &str) -> Result<u16, String> {
    u16::try_from(value).map_err(|_| format!("{field_name}={value} exceeds u16 range"))
}

fn narrow_i32_to_i8(value: i32, field_name: &str) -> Result<i8, String> {
    i8::try_from(value).map_err(|_| format!("{field_name}={value} exceeds i8 range"))
}

fn narrow_i64_to_u16(value: i64, field_name: &str) -> Result<u16, String> {
    u16::try_from(value).map_err(|_| format!("{field_name}={value} exceeds u16 range"))
}

fn narrow_usize_to_u8(value: usize, field_name: &str) -> Result<u8, String> {
    u8::try_from(value).map_err(|_| format!("{field_name}={value} exceeds u8 range"))
}
