//! Vulkan image, buffer, upload, and encode-resource management.

use super::*;

pub(super) fn create_hevc_encode_probe_ycbcr_conversion(
    device: &ash::Device,
    picture_format: vk::Format,
) -> Result<vk::SamplerYcbcrConversion, String> {
    let create_info = vk::SamplerYcbcrConversionCreateInfo::default()
        .format(picture_format)
        .ycbcr_model(vk::SamplerYcbcrModelConversion::RGB_IDENTITY)
        .ycbcr_range(vk::SamplerYcbcrRange::ITU_FULL)
        .components(vk::ComponentMapping {
            r: vk::ComponentSwizzle::IDENTITY,
            g: vk::ComponentSwizzle::IDENTITY,
            b: vk::ComponentSwizzle::IDENTITY,
            a: vk::ComponentSwizzle::IDENTITY,
        })
        .x_chroma_offset(vk::ChromaLocation::COSITED_EVEN)
        .y_chroma_offset(vk::ChromaLocation::COSITED_EVEN)
        .chroma_filter(vk::Filter::NEAREST)
        .force_explicit_reconstruction(false);
    // SAFETY: create info references only stack data and uses the encode picture format.
    unsafe { device.create_sampler_ycbcr_conversion(&create_info, None) }.map_err(|err| {
        format!("vkCreateSamplerYcbcrConversion for encode image views failed: {err}")
    })
}

pub(super) struct HevcEncodeProbeImageResources<'a> {
    pub(super) image: &'a mut vk::Image,
    pub(super) memory: &'a mut vk::DeviceMemory,
    pub(super) view: &'a mut vk::ImageView,
    pub(super) dedicated_allocation: &'a mut bool,
}

pub(super) fn create_hevc_encode_probe_image(
    device: &ash::Device,
    instance: &ash::Instance,
    ycbcr_conversion: vk::SamplerYcbcrConversion,
    config: HevcEncodeProbeImageConfig,
    resources: HevcEncodeProbeImageResources<'_>,
) -> Result<(), String> {
    let HevcEncodeProbeImageResources {
        image,
        memory: image_memory,
        view: image_view,
        dedicated_allocation,
    } = resources;
    let mut encode_h265_profile = vk::VideoEncodeH265ProfileInfoKHR::default()
        .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
    let mut encode_usage = vk::VideoEncodeUsageInfoKHR::default()
        .video_usage_hints(vk::VideoEncodeUsageFlagsKHR::DEFAULT);
    let profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(vk::VideoCodecOperationFlagsKHR::ENCODE_H265)
        .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
        .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .push_next(&mut encode_usage)
        .push_next(&mut encode_h265_profile);
    let profiles = [profile];
    let mut profile_list = vk::VideoProfileListInfoKHR::default().profiles(&profiles);
    let create_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(config.picture_format)
        .extent(vk::Extent3D {
            width: config.image_width,
            height: config.image_height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(config.usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .queue_family_indices(std::slice::from_ref(&config.queue_family_index.0))
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .push_next(&mut profile_list);
    // SAFETY: image create info references local data valid for this call.
    *image = unsafe { device.create_image(&create_info, None) }
        .map_err(|err| format!("vkCreateImage for encode submit probe failed: {err}"))?;
    let image_memory_info = vk::ImageMemoryRequirementsInfo2::default().image(*image);
    let mut dedicated_requirements = vk::MemoryDedicatedRequirements::default();
    let mut memory_requirements =
        vk::MemoryRequirements2::default().push_next(&mut dedicated_requirements);
    // SAFETY: image handle is valid and owned by `device`.
    unsafe {
        device.get_image_memory_requirements2(&image_memory_info, &mut memory_requirements);
    }
    let memory_requirements = memory_requirements.memory_requirements;
    let memory_type_index = find_memory_type_index(
        config.physical_device,
        instance,
        memory_requirements.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    )
    .or_else(|| {
        find_memory_type_index(
            config.physical_device,
            instance,
            memory_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::empty(),
        )
    })
    .ok_or_else(|| "no compatible memory type for encode submit source image".to_string())?;
    let mut dedicated_allocate_info = vk::MemoryDedicatedAllocateInfo::default().image(*image);
    let allocate_info = vk::MemoryAllocateInfo::default()
        .allocation_size(memory_requirements.size)
        .memory_type_index(memory_type_index);
    *dedicated_allocation = dedicated_requirements.requires_dedicated_allocation != 0
        || dedicated_requirements.prefers_dedicated_allocation != 0;
    let allocate_info = if *dedicated_allocation {
        allocate_info.push_next(&mut dedicated_allocate_info)
    } else {
        allocate_info
    };
    // SAFETY: allocation info references local data and selected memory type index is valid.
    *image_memory = unsafe { device.allocate_memory(&allocate_info, None) }
        .map_err(|err| format!("vkAllocateMemory for encode submit source image failed: {err}"))?;
    // SAFETY: image and memory were created from this device.
    unsafe { device.bind_image_memory(*image, *image_memory, 0) }
        .map_err(|err| format!("vkBindImageMemory for encode submit source image failed: {err}"))?;

    let view_usage = if config
        .usage
        .contains(vk::ImageUsageFlags::VIDEO_ENCODE_DPB_KHR)
    {
        vk::ImageUsageFlags::VIDEO_ENCODE_DPB_KHR
    } else if config
        .usage
        .contains(vk::ImageUsageFlags::VIDEO_ENCODE_SRC_KHR)
    {
        vk::ImageUsageFlags::VIDEO_ENCODE_SRC_KHR
    } else {
        config.usage
    };
    let mut ycbcr_conversion_info =
        vk::SamplerYcbcrConversionInfo::default().conversion(ycbcr_conversion);
    let mut image_view_usage = vk::ImageViewUsageCreateInfo::default().usage(view_usage);
    let view_create_info = vk::ImageViewCreateInfo::default()
        .image(*image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(config.picture_format)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        })
        .push_next(&mut image_view_usage);
    let view_create_info = match config.image_view_mode {
        HevcEncodeProbeImageViewMode::YcbcrConversion => {
            view_create_info.push_next(&mut ycbcr_conversion_info)
        }
        HevcEncodeProbeImageViewMode::NoYcbcrConversion => view_create_info,
    };
    // SAFETY: image view create info references valid image and stack data.
    *image_view = unsafe { device.create_image_view(&view_create_info, None) }
        .map_err(|err| format!("vkCreateImageView for encode submit source image failed: {err}"))?;
    Ok(())
}

pub(super) fn run_hevc_encode_coding_scope_probe(
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    command_buffer: vk::CommandBuffer,
    video_session: vk::VideoSessionKHR,
    video_session_parameters: vk::VideoSessionParametersKHR,
    begin_session_parameters_mode: HevcEncodeProbeBeginSessionParametersMode,
    control_config: HevcEncodeProbeControlConfig,
) -> Result<(), String> {
    let begin_info =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: command buffer is valid and not recording.
    unsafe { device.begin_command_buffer(command_buffer, &begin_info) }
        .map_err(|err| format!("coding-scope probe vkBeginCommandBuffer failed: {err}"))?;

    let begin_coding_info = match begin_session_parameters_mode {
        HevcEncodeProbeBeginSessionParametersMode::With => vk::VideoBeginCodingInfoKHR::default()
            .video_session(video_session)
            .video_session_parameters(video_session_parameters),
        HevcEncodeProbeBeginSessionParametersMode::Without => {
            vk::VideoBeginCodingInfoKHR::default().video_session(video_session)
        }
    };
    let rate_control_mode = control_config
        .selected_rate_control_mode
        .unwrap_or(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED);
    let mut rate_control_layer_h265 = vk::VideoEncodeH265RateControlLayerInfoKHR::default();
    let rate_control_layer = vk::VideoEncodeRateControlLayerInfoKHR::default()
        .average_bitrate(8_000_000)
        .max_bitrate(8_000_000)
        .frame_rate_numerator(30)
        .frame_rate_denominator(1)
        .push_next(&mut rate_control_layer_h265);
    let mut rate_control_info =
        vk::VideoEncodeRateControlInfoKHR::default().rate_control_mode(rate_control_mode);
    let mut quality_level_info = vk::VideoEncodeQualityLevelInfoKHR::default()
        .quality_level(control_config.selected_quality_level);
    let mut coding_control_info =
        vk::VideoCodingControlInfoKHR::default().flags(vk::VideoCodingControlFlagsKHR::RESET);
    let mut issue_coding_control = true;
    let h265_rate_control_info = hevc_encode_h265_rate_control_info();
    match control_config.control_mode {
        HevcEncodeProbeControlMode::Default => {
            if rate_control_mode != vk::VideoEncodeRateControlModeFlagsKHR::DISABLED {
                rate_control_info.p_next = std::ptr::from_ref(&h265_rate_control_info).cast();
                rate_control_info = rate_control_info
                    .layers(std::slice::from_ref(&rate_control_layer))
                    .virtual_buffer_size_in_ms(1000)
                    .initial_virtual_buffer_size_in_ms(1000);
                coding_control_info = coding_control_info
                    .flags(
                        vk::VideoCodingControlFlagsKHR::RESET
                            | vk::VideoCodingControlFlagsKHR::ENCODE_RATE_CONTROL,
                    )
                    .push_next(&mut rate_control_info);
            }
            if control_config.enable_quality_level_control {
                coding_control_info = coding_control_info
                    .flags(
                        coding_control_info.flags
                            | vk::VideoCodingControlFlagsKHR::ENCODE_QUALITY_LEVEL,
                    )
                    .push_next(&mut quality_level_info);
            }
        }
        HevcEncodeProbeControlMode::Ffmpeg => {
            rate_control_info.p_next = std::ptr::from_ref(&h265_rate_control_info).cast();
            if rate_control_mode != vk::VideoEncodeRateControlModeFlagsKHR::DISABLED {
                rate_control_info = rate_control_info
                    .layers(std::slice::from_ref(&rate_control_layer))
                    .virtual_buffer_size_in_ms(1000)
                    .initial_virtual_buffer_size_in_ms(1000);
            }
            coding_control_info = coding_control_info
                .flags(
                    vk::VideoCodingControlFlagsKHR::RESET
                        | vk::VideoCodingControlFlagsKHR::ENCODE_RATE_CONTROL,
                )
                .push_next(&mut rate_control_info);
            if control_config.enable_quality_level_control {
                coding_control_info = coding_control_info
                    .flags(
                        coding_control_info.flags
                            | vk::VideoCodingControlFlagsKHR::ENCODE_QUALITY_LEVEL,
                    )
                    .push_next(&mut quality_level_info);
            }
        }
        HevcEncodeProbeControlMode::None => {
            issue_coding_control = false;
        }
    }
    // SAFETY: command buffer is recording and video session handles are valid.
    unsafe {
        (video_queue_device.fp().cmd_begin_video_coding_khr)(command_buffer, &begin_coding_info);
        if issue_coding_control {
            (video_queue_device.fp().cmd_control_video_coding_khr)(
                command_buffer,
                &coding_control_info,
            );
        }
    }
    let end_coding_info = vk::VideoEndCodingInfoKHR::default();
    // SAFETY: command buffer is recording and coding scope was opened above.
    unsafe {
        (video_queue_device.fp().cmd_end_video_coding_khr)(command_buffer, &end_coding_info);
    }
    // SAFETY: command buffer is valid and recording.
    unsafe { device.end_command_buffer(command_buffer) }
        .map_err(|err| format!("coding-scope probe vkEndCommandBuffer failed: {err}"))?;
    Ok(())
}

pub(super) fn run_hevc_encode_pre_encode_probe(
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    video_encode_device: &ash::khr::video_encode_queue::Device,
    config: HevcEncodePreEncodeProbeConfig,
) -> Result<(), String> {
    let command_buffer = config.command_buffer;
    let video_session = config.video_session;
    let video_session_parameters = config.video_session_parameters;
    let begin_session_parameters_mode = config.begin_session_parameters_mode;
    let control_config = config.control_config;
    let mode = config.mode;
    let mode_label = hevc_encode_pre_encode_probe_mode_label(mode);
    let nalu_mode = config.nalu_mode;
    let codec_info_mode = config.codec_info_mode;
    let picture_flags_mode = resolve_hevc_encode_probe_picture_flags_mode();
    let picture_info_mode = resolve_hevc_encode_probe_picture_info_mode();
    let picture_type = hevc_encode_probe_picture_type(picture_info_mode);
    let slice_type = hevc_encode_probe_slice_type(picture_info_mode);
    let temporal_id = hevc_encode_probe_temporal_id(picture_info_mode);
    let pic_order_cnt_val = hevc_encode_probe_pic_order_cnt_val(picture_info_mode);
    let dpb_barrier_mode = resolve_hevc_encode_probe_dpb_barrier_mode();
    let encode_reference_slot_pointer_mode =
        resolve_hevc_encode_probe_encode_reference_slot_pointer_mode();
    let resources = config.resources;
    let begin_info =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: command buffer is valid and not recording.
    unsafe { device.begin_command_buffer(command_buffer, &begin_info) }.map_err(|err| {
        format!("pre-encode probe ({mode_label}) vkBeginCommandBuffer failed: {err}")
    })?;

    let source_image_prepare_barrier = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::NONE)
        .src_access_mask(vk::AccessFlags2::NONE)
        .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(resources.source_image)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });
    let source_upload_regions = build_hevc_encode_source_upload_regions(
        vk::Format::G8_B8R8_2PLANE_420_UNORM,
        resources.source_upload_width,
        resources.source_upload_height,
    )?
    .1;
    let source_upload_buffer_barrier = vk::BufferMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::HOST)
        .src_access_mask(vk::AccessFlags2::HOST_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
        .buffer(resources.source_upload_buffer)
        .offset(0)
        .size(resources.source_upload_buffer_size);
    let source_prepare_dependency = vk::DependencyInfo::default()
        .image_memory_barriers(std::slice::from_ref(&source_image_prepare_barrier))
        .buffer_memory_barriers(std::slice::from_ref(&source_upload_buffer_barrier));
    let clear_range = vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    };
    let source_image_encode_barrier = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_ENCODE_KHR)
        .dst_access_mask(vk::AccessFlags2::VIDEO_ENCODE_READ_KHR)
        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .new_layout(vk::ImageLayout::VIDEO_ENCODE_SRC_KHR)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(resources.source_image)
        .subresource_range(clear_range);
    let dpb_image_encode_barrier = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::NONE)
        .src_access_mask(vk::AccessFlags2::NONE)
        .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_ENCODE_KHR)
        .dst_access_mask(vk::AccessFlags2::VIDEO_ENCODE_WRITE_KHR)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::VIDEO_ENCODE_DPB_KHR)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(resources.dpb_image)
        .subresource_range(clear_range);
    let dst_buffer_barrier = vk::BufferMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::NONE)
        .src_access_mask(vk::AccessFlags2::NONE)
        .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_ENCODE_KHR)
        .dst_access_mask(vk::AccessFlags2::VIDEO_ENCODE_WRITE_KHR)
        .buffer(resources.dst_buffer)
        .offset(0)
        .size(resources.dst_buffer_size);
    let pre_encode_image_barriers_with_dpb =
        [source_image_encode_barrier, dpb_image_encode_barrier];
    let pre_encode_image_barriers_without_dpb = [source_image_encode_barrier];
    let pre_encode_image_barriers = match dpb_barrier_mode {
        HevcEncodeProbeDpbBarrierMode::With => pre_encode_image_barriers_with_dpb.as_slice(),
        HevcEncodeProbeDpbBarrierMode::None => pre_encode_image_barriers_without_dpb.as_slice(),
    };
    let dependency_info = vk::DependencyInfo::default()
        .image_memory_barriers(pre_encode_image_barriers)
        .buffer_memory_barriers(std::slice::from_ref(&dst_buffer_barrier));
    // SAFETY: barriers reference resources passed by the caller and command buffer is recording.
    unsafe {
        device.cmd_pipeline_barrier2(command_buffer, &source_prepare_dependency);
        device.cmd_copy_buffer_to_image(
            command_buffer,
            resources.source_upload_buffer,
            resources.source_image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &source_upload_regions,
        );
        device.cmd_pipeline_barrier2(command_buffer, &dependency_info);
    }

    let begin_coding_info = match begin_session_parameters_mode {
        HevcEncodeProbeBeginSessionParametersMode::With => vk::VideoBeginCodingInfoKHR::default()
            .video_session(video_session)
            .video_session_parameters(video_session_parameters),
        HevcEncodeProbeBeginSessionParametersMode::Without => {
            vk::VideoBeginCodingInfoKHR::default().video_session(video_session)
        }
    };
    let rate_control_mode = control_config
        .selected_rate_control_mode
        .unwrap_or(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED);
    let mut rate_control_layer_h265 = vk::VideoEncodeH265RateControlLayerInfoKHR::default();
    let rate_control_layer = vk::VideoEncodeRateControlLayerInfoKHR::default()
        .average_bitrate(8_000_000)
        .max_bitrate(8_000_000)
        .frame_rate_numerator(30)
        .frame_rate_denominator(1)
        .push_next(&mut rate_control_layer_h265);
    let mut rate_control_info =
        vk::VideoEncodeRateControlInfoKHR::default().rate_control_mode(rate_control_mode);
    let mut quality_level_info = vk::VideoEncodeQualityLevelInfoKHR::default()
        .quality_level(control_config.selected_quality_level);
    let mut coding_control_info =
        vk::VideoCodingControlInfoKHR::default().flags(vk::VideoCodingControlFlagsKHR::RESET);
    let mut issue_coding_control = true;
    match control_config.control_mode {
        HevcEncodeProbeControlMode::Default => {
            if rate_control_mode != vk::VideoEncodeRateControlModeFlagsKHR::DISABLED {
                rate_control_info = rate_control_info
                    .layers(std::slice::from_ref(&rate_control_layer))
                    .virtual_buffer_size_in_ms(1000)
                    .initial_virtual_buffer_size_in_ms(1000);
                coding_control_info = coding_control_info
                    .flags(
                        vk::VideoCodingControlFlagsKHR::RESET
                            | vk::VideoCodingControlFlagsKHR::ENCODE_RATE_CONTROL,
                    )
                    .push_next(&mut rate_control_info);
            }
            if control_config.enable_quality_level_control {
                coding_control_info = coding_control_info
                    .flags(
                        coding_control_info.flags
                            | vk::VideoCodingControlFlagsKHR::ENCODE_QUALITY_LEVEL,
                    )
                    .push_next(&mut quality_level_info);
            }
        }
        HevcEncodeProbeControlMode::Ffmpeg => {
            if rate_control_mode != vk::VideoEncodeRateControlModeFlagsKHR::DISABLED {
                rate_control_info = rate_control_info
                    .layers(std::slice::from_ref(&rate_control_layer))
                    .virtual_buffer_size_in_ms(1000)
                    .initial_virtual_buffer_size_in_ms(1000);
            }
            coding_control_info = coding_control_info
                .flags(
                    vk::VideoCodingControlFlagsKHR::RESET
                        | vk::VideoCodingControlFlagsKHR::ENCODE_RATE_CONTROL,
                )
                .push_next(&mut rate_control_info);
            if control_config.enable_quality_level_control {
                coding_control_info = coding_control_info
                    .flags(
                        coding_control_info.flags
                            | vk::VideoCodingControlFlagsKHR::ENCODE_QUALITY_LEVEL,
                    )
                    .push_next(&mut quality_level_info);
            }
        }
        HevcEncodeProbeControlMode::None => {
            issue_coding_control = false;
        }
    }
    // SAFETY: command buffer is recording and video session handles are valid.
    unsafe {
        (video_queue_device.fp().cmd_begin_video_coding_khr)(command_buffer, &begin_coding_info);
        if issue_coding_control {
            (video_queue_device.fp().cmd_control_video_coding_khr)(
                command_buffer,
                &coding_control_info,
            );
        }
    }
    if matches!(
        mode,
        HevcEncodePreEncodeProbeMode::WithEncode | HevcEncodePreEncodeProbeMode::WithEncodeMinimal
    ) {
        let reference_index_mode = resolve_hevc_encode_probe_reference_index_mode();
        let rps_mode = resolve_hevc_encode_probe_rps_mode();
        let setup_reference_slot_mode = resolve_hevc_encode_probe_setup_reference_slot_mode();
        let reference_idx_minus1 = hevc_encode_probe_reference_idx_minus1(reference_index_mode);
        let reconstructed_picture_resource = vk::VideoPictureResourceInfoKHR::default()
            .coded_offset(vk::Offset2D::default())
            .coded_extent(vk::Extent2D {
                width: resources.picture_resource_coded_width,
                height: resources.picture_resource_coded_height,
            })
            .base_array_layer(0)
            .image_view_binding(resources.dpb_image_view);
        let setup_reference_info = StdVideoEncodeH265ReferenceInfo {
            flags: StdVideoEncodeH265ReferenceInfoFlags {
                _bitfield_align_1: [],
                _bitfield_1: Default::default(),
            },
            pic_type: StdVideoH265PictureType_STD_VIDEO_H265_PICTURE_TYPE_IDR,
            PicOrderCntVal: 0,
            TemporalId: 0,
        };
        let mut setup_dpb_slot_info =
            vk::VideoEncodeH265DpbSlotInfoKHR::default().std_reference_info(&setup_reference_info);
        let setup_reference_slot = vk::VideoReferenceSlotInfoKHR::default()
            .slot_index(0)
            .picture_resource(&reconstructed_picture_resource)
            .push_next(&mut setup_dpb_slot_info);
        let mut std_reference_list_flags = StdVideoEncodeH265ReferenceListsInfoFlags {
            _bitfield_align_1: [],
            _bitfield_1: Default::default(),
        };
        std_reference_list_flags.set_ref_pic_list_modification_flag_l0(0);
        std_reference_list_flags.set_ref_pic_list_modification_flag_l1(0);
        let std_reference_lists = StdVideoEncodeH265ReferenceListsInfo {
            flags: std_reference_list_flags,
            num_ref_idx_l0_active_minus1: reference_idx_minus1,
            num_ref_idx_l1_active_minus1: reference_idx_minus1,
            RefPicList0: [HEVC_NO_REFERENCE_PICTURE; 15],
            RefPicList1: [HEVC_NO_REFERENCE_PICTURE; 15],
            list_entry_l0: [0; 15],
            list_entry_l1: [0; 15],
        };
        let std_short_term_ref_pic_set = StdVideoH265ShortTermRefPicSet {
            flags: ash::vk::native::StdVideoH265ShortTermRefPicSetFlags {
                _bitfield_align_1: [],
                _bitfield_1: Default::default(),
                __bindgen_padding_0: [0; 3],
            },
            delta_idx_minus1: 0,
            use_delta_flag: 0,
            abs_delta_rps_minus1: 0,
            used_by_curr_pic_flag: 0,
            used_by_curr_pic_s0_flag: 0,
            used_by_curr_pic_s1_flag: 0,
            reserved1: 0,
            reserved2: 0,
            reserved3: 0,
            num_negative_pics: 0,
            num_positive_pics: 0,
            delta_poc_s0_minus1: [0; 16],
            delta_poc_s1_minus1: [0; 16],
        };
        let std_long_term_ref_pics = StdVideoEncodeH265LongTermRefPics {
            num_long_term_sps: 0,
            num_long_term_pics: 0,
            lt_idx_sps: [0; 32],
            poc_lsb_lt: [0; 16],
            used_by_curr_pic_lt_flag: 0,
            delta_poc_msb_present_flag: [0; 48],
            delta_poc_msb_cycle_lt: [0; 48],
        };
        let (short_term_ref_pic_set_ptr, long_term_ref_pics_ptr) = match rps_mode {
            HevcEncodeProbeRpsMode::EmptyStruct => (
                &std_short_term_ref_pic_set as *const StdVideoH265ShortTermRefPicSet,
                &std_long_term_ref_pics as *const StdVideoEncodeH265LongTermRefPics,
            ),
            HevcEncodeProbeRpsMode::NullPointers => (std::ptr::null(), std::ptr::null()),
        };
        let mut std_picture_flags = StdVideoEncodeH265PictureInfoFlags {
            _bitfield_align_1: [],
            _bitfield_1: Default::default(),
        };
        let is_reference = matches!(picture_flags_mode, HevcEncodeProbePictureFlagsMode::Default);
        std_picture_flags.set_is_reference(u32::from(is_reference));
        std_picture_flags.set_IrapPicFlag(u32::from(is_reference));
        std_picture_flags.set_pic_output_flag(1);
        std_picture_flags.set_short_term_ref_pic_set_sps_flag(0);
        std_picture_flags
            .set_slice_temporal_mvp_enabled_flag(u32::from(config.sps_temporal_mvp_enabled));
        std_picture_flags.set_used_for_long_term_reference(0);
        std_picture_flags.set_discardable_flag(0);
        std_picture_flags.set_cross_layer_bla_flag(0);
        std_picture_flags.set_no_output_of_prior_pics_flag(0);
        let (reference_lists_ptr, short_term_ref_pic_set_ptr, long_term_ref_pics_ptr) = match mode {
            HevcEncodePreEncodeProbeMode::WithEncodeMinimal => {
                (std::ptr::null(), std::ptr::null(), std::ptr::null())
            }
            HevcEncodePreEncodeProbeMode::WithEncode | HevcEncodePreEncodeProbeMode::ScopeOnly => (
                &std_reference_lists as *const StdVideoEncodeH265ReferenceListsInfo,
                short_term_ref_pic_set_ptr,
                long_term_ref_pics_ptr,
            ),
        };
        let std_picture_info = StdVideoEncodeH265PictureInfo {
            flags: std_picture_flags,
            pic_type: picture_type,
            sps_video_parameter_set_id: 0,
            pps_seq_parameter_set_id: 0,
            pps_pic_parameter_set_id: 0,
            short_term_ref_pic_set_idx: 0,
            PicOrderCntVal: pic_order_cnt_val,
            TemporalId: temporal_id,
            reserved1: [0; 7],
            pRefLists: reference_lists_ptr,
            pShortTermRefPicSet: short_term_ref_pic_set_ptr,
            pLongTermRefPics: long_term_ref_pics_ptr,
        };
        let std_picture_info_empty = StdVideoEncodeH265PictureInfo {
            flags: StdVideoEncodeH265PictureInfoFlags {
                _bitfield_align_1: [],
                _bitfield_1: Default::default(),
            },
            pic_type: StdVideoH265PictureType_STD_VIDEO_H265_PICTURE_TYPE_IDR,
            sps_video_parameter_set_id: 0,
            pps_seq_parameter_set_id: 0,
            pps_pic_parameter_set_id: 0,
            short_term_ref_pic_set_idx: 0,
            PicOrderCntVal: 0,
            TemporalId: 0,
            reserved1: [0; 7],
            pRefLists: std::ptr::null(),
            pShortTermRefPicSet: std::ptr::null(),
            pLongTermRefPics: std::ptr::null(),
        };
        let std_slice_flags =
            hevc_encode_std_slice_segment_header_flags(config.sample_adaptive_offset_enabled);
        let constant_qp =
            resolve_hevc_encode_probe_constant_qp(control_config.selected_rate_control_mode);
        let slice_qp_delta = hevc_encode_probe_slice_qp_delta(constant_qp, 0)
            .map_err(|err| format!("{err} for pre-encode probe"))?;
        let std_slice_header = StdVideoEncodeH265SliceSegmentHeader {
            flags: std_slice_flags,
            slice_type,
            slice_segment_address: 0,
            collocated_ref_idx: 0,
            MaxNumMergeCand: 5,
            slice_cb_qp_offset: 0,
            slice_cr_qp_offset: 0,
            slice_beta_offset_div2: 0,
            slice_tc_offset_div2: 0,
            slice_act_y_qp_offset: 0,
            slice_act_cb_qp_offset: 0,
            slice_act_cr_qp_offset: 0,
            slice_qp_delta,
            reserved1: 0,
            pWeightTable: std::ptr::null(),
        };
        let nalu_slice_entries = [vk::VideoEncodeH265NaluSliceSegmentInfoKHR::default()
            .constant_qp(constant_qp)
            .std_slice_segment_header(&std_slice_header)];
        let nalu_entries: &[vk::VideoEncodeH265NaluSliceSegmentInfoKHR] = match nalu_mode {
            HevcEncodeProbeNaluMode::SingleSlice => &nalu_slice_entries,
            HevcEncodeProbeNaluMode::Empty => &[],
        };
        let mut h265_encode_info = vk::VideoEncodeH265PictureInfoKHR::default()
            .nalu_slice_segment_entries(nalu_entries)
            .std_picture_info(&std_picture_info);
        let mut h265_encode_info_std_picture_only =
            vk::VideoEncodeH265PictureInfoKHR::default().std_picture_info(&std_picture_info);
        let mut h265_encode_info_empty_std_picture = vk::VideoEncodeH265PictureInfoKHR::default()
            .nalu_slice_segment_entries(nalu_entries)
            .std_picture_info(&std_picture_info_empty);
        let mut h265_encode_info_minimal = vk::VideoEncodeH265PictureInfoKHR::default();
        let mut h265_encode_info_no_std_picture =
            vk::VideoEncodeH265PictureInfoKHR::default().nalu_slice_segment_entries(nalu_entries);
        let src_picture_resource = vk::VideoPictureResourceInfoKHR::default()
            .coded_offset(vk::Offset2D::default())
            .coded_extent(vk::Extent2D {
                width: resources.source_picture_resource_width,
                height: resources.source_picture_resource_height,
            })
            .base_array_layer(0)
            .image_view_binding(resources.source_image_view);
        let encode_info = vk::VideoEncodeInfoKHR::default()
            .dst_buffer(resources.dst_buffer)
            .dst_buffer_offset(resources.dst_buffer_offset)
            .dst_buffer_range(resources.dst_buffer_range)
            .src_picture_resource(src_picture_resource)
            .reference_slots(&[]);
        let encode_info = match encode_reference_slot_pointer_mode {
            HevcEncodeProbeEncodeReferenceSlotPointerMode::EmptySlice => encode_info,
            HevcEncodeProbeEncodeReferenceSlotPointerMode::FfmpegNonNull => {
                vk::VideoEncodeInfoKHR {
                    reference_slot_count: 0,
                    p_reference_slots: std::ptr::from_ref(&setup_reference_slot),
                    ..encode_info
                }
            }
        };
        let encode_info = match codec_info_mode {
            HevcEncodeProbeCodecInfoMode::WithH265Info => {
                encode_info.push_next(&mut h265_encode_info)
            }
            HevcEncodeProbeCodecInfoMode::WithH265InfoStdPictureOnly => {
                encode_info.push_next(&mut h265_encode_info_std_picture_only)
            }
            HevcEncodeProbeCodecInfoMode::WithH265InfoEmptyStdPicture => {
                encode_info.push_next(&mut h265_encode_info_empty_std_picture)
            }
            HevcEncodeProbeCodecInfoMode::WithH265InfoMinimal => {
                encode_info.push_next(&mut h265_encode_info_minimal)
            }
            HevcEncodeProbeCodecInfoMode::WithH265InfoNoStdPicture => {
                encode_info.push_next(&mut h265_encode_info_no_std_picture)
            }
            HevcEncodeProbeCodecInfoMode::WithoutH265Info => encode_info,
        };
        let encode_info = match mode {
            HevcEncodePreEncodeProbeMode::WithEncodeMinimal => encode_info,
            HevcEncodePreEncodeProbeMode::WithEncode | HevcEncodePreEncodeProbeMode::ScopeOnly => {
                match setup_reference_slot_mode {
                    HevcEncodeProbeSetupReferenceSlotMode::SlotZero => {
                        encode_info.setup_reference_slot(&setup_reference_slot)
                    }
                    HevcEncodeProbeSetupReferenceSlotMode::None => encode_info,
                }
            }
        };
        // SAFETY: command buffer is recording and structures are valid for this call.
        unsafe {
            (video_encode_device.fp().cmd_encode_video_khr)(command_buffer, &encode_info);
        }
    }
    let end_coding_info = vk::VideoEndCodingInfoKHR::default();
    // SAFETY: command buffer is recording and coding scope was opened above.
    unsafe {
        (video_queue_device.fp().cmd_end_video_coding_khr)(command_buffer, &end_coding_info);
    }
    // SAFETY: command buffer is valid and recording.
    unsafe { device.end_command_buffer(command_buffer) }.map_err(|err| {
        let picture_info_mode_label = hevc_encode_probe_picture_info_mode_label(picture_info_mode);
        format!(
            "pre-encode probe ({mode_label}, picture_info_mode={picture_info_mode_label}, pic_order_cnt_val={pic_order_cnt_val}, temporal_id={temporal_id}) vkEndCommandBuffer failed: {err}"
        )
    })?;
    Ok(())
}

pub(super) fn build_hevc_encode_source_upload_regions(
    input_format: vk::Format,
    image_width: u32,
    image_height: u32,
) -> Result<(u64, Vec<vk::BufferImageCopy>), String> {
    if image_width == 0 || image_height == 0 {
        return Err(format!(
            "invalid encode source upload extent: {image_width}x{image_height}"
        ));
    }

    let mut regions = Vec::new();
    let mut next_offset = 0_u64;
    let mut push_region = |aspect_mask: vk::ImageAspectFlags,
                           plane_width: u32,
                           plane_height: u32,
                           bytes_per_texel: u64|
     -> Result<(), String> {
        let row_bytes = u64::from(plane_width)
            .checked_mul(bytes_per_texel)
            .ok_or_else(|| "encode source upload row size overflowed u64".to_string())?;
        let plane_bytes = row_bytes
            .checked_mul(u64::from(plane_height))
            .ok_or_else(|| "encode source upload plane size overflowed u64".to_string())?;
        let offset = align_up(next_offset, 4);
        regions.push(
            vk::BufferImageCopy::default()
                .buffer_offset(offset)
                .buffer_row_length(plane_width)
                .buffer_image_height(plane_height)
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_offset(vk::Offset3D::default())
                .image_extent(vk::Extent3D {
                    width: plane_width,
                    height: plane_height,
                    depth: 1,
                }),
        );
        next_offset = offset
            .checked_add(plane_bytes)
            .ok_or_else(|| "encode source upload buffer size overflowed u64".to_string())?;
        Ok(())
    };

    match input_format {
        vk::Format::G8_B8R8_2PLANE_420_UNORM => {
            push_region(vk::ImageAspectFlags::PLANE_0, image_width, image_height, 1)?;
            push_region(
                vk::ImageAspectFlags::PLANE_1,
                image_width.div_ceil(2),
                image_height.div_ceil(2),
                2,
            )?;
        }
        other => {
            return Err(format!(
                "unsupported encode source upload format for HEVC probe: {other:?}"
            ));
        }
    }

    Ok((next_offset, regions))
}

pub(super) fn write_hevc_encode_source_upload(
    device: &ash::Device,
    memory: vk::DeviceMemory,
    buffer_size: u64,
    regions: &[vk::BufferImageCopy],
    source_nv12: Option<&[u8]>,
    coded_width: u32,
    coded_height: u32,
) -> Result<(), String> {
    let buffer_size_usize = usize::try_from(buffer_size)
        .map_err(|_| "encode source upload buffer exceeds usize range".to_string())?;
    // SAFETY: memory belongs to `device`; mapped range is within the allocation.
    let mapped = unsafe { device.map_memory(memory, 0, buffer_size, vk::MemoryMapFlags::empty()) }
        .map_err(|err| format!("vkMapMemory for encode source upload buffer failed: {err}"))?;
    let result = (|| -> Result<(), String> {
        // SAFETY: mapped pointer is valid for `buffer_size_usize` bytes until unmap.
        let bytes =
            unsafe { std::slice::from_raw_parts_mut(mapped.cast::<u8>(), buffer_size_usize) };
        bytes.fill(0);
        for region in regions {
            let offset = usize::try_from(region.buffer_offset)
                .map_err(|_| "encode source upload region offset exceeds usize".to_string())?;
            let row_length = usize::try_from(region.buffer_row_length)
                .map_err(|_| "encode source upload region row length exceeds usize".to_string())?;
            let image_height = usize::try_from(region.buffer_image_height).map_err(|_| {
                "encode source upload region image height exceeds usize".to_string()
            })?;
            let (bytes_per_texel, fill_value) = if region
                .image_subresource
                .aspect_mask
                .contains(vk::ImageAspectFlags::PLANE_1)
            {
                (2_usize, 128_u8)
            } else {
                (1_usize, 16_u8)
            };
            let len = row_length
                .checked_mul(image_height)
                .and_then(|value| value.checked_mul(bytes_per_texel))
                .ok_or_else(|| "encode source upload region size overflowed usize".to_string())?;
            let end = offset
                .checked_add(len)
                .ok_or_else(|| "encode source upload region end overflowed usize".to_string())?;
            let span = bytes.get_mut(offset..end).ok_or_else(|| {
                "encode source upload region falls outside mapped buffer".to_string()
            })?;
            span.fill(fill_value);
        }
        if let Some(source_nv12) = source_nv12 {
            copy_hevc_encode_source_nv12(
                bytes,
                regions,
                source_nv12,
                usize::try_from(coded_width)
                    .map_err(|_| "coded width exceeds usize".to_string())?,
                usize::try_from(coded_height)
                    .map_err(|_| "coded height exceeds usize".to_string())?,
            )?;
        }
        Ok(())
    })();
    // SAFETY: pointer was returned by map_memory for this allocation.
    unsafe {
        device.unmap_memory(memory);
    }
    result
}

fn copy_hevc_encode_source_nv12(
    upload: &mut [u8],
    regions: &[vk::BufferImageCopy],
    source_nv12: &[u8],
    coded_width: usize,
    coded_height: usize,
) -> Result<(), String> {
    let y_size = coded_width
        .checked_mul(coded_height)
        .ok_or_else(|| "HEVC source NV12 Y size overflow".to_string())?;
    let uv_height = coded_height.div_ceil(2);
    let uv_size = coded_width
        .checked_mul(uv_height)
        .ok_or_else(|| "HEVC source NV12 UV size overflow".to_string())?;
    let expected_len = y_size
        .checked_add(uv_size)
        .ok_or_else(|| "HEVC source NV12 size overflow".to_string())?;
    if source_nv12.len() != expected_len {
        return Err(format!(
            "HEVC source NV12 size mismatch: expected {expected_len}, got {}",
            source_nv12.len()
        ));
    }
    let y_region = regions
        .iter()
        .find(|region| {
            region
                .image_subresource
                .aspect_mask
                .contains(vk::ImageAspectFlags::PLANE_0)
        })
        .ok_or_else(|| "HEVC source upload has no Y plane region".to_string())?;
    let uv_region = regions
        .iter()
        .find(|region| {
            region
                .image_subresource
                .aspect_mask
                .contains(vk::ImageAspectFlags::PLANE_1)
        })
        .ok_or_else(|| "HEVC source upload has no UV plane region".to_string())?;
    copy_hevc_encode_source_plane(
        upload,
        y_region,
        &source_nv12[..y_size],
        coded_width,
        coded_height,
        1,
    )?;
    copy_hevc_encode_source_plane(
        upload,
        uv_region,
        &source_nv12[y_size..],
        coded_width / 2,
        uv_height,
        2,
    )
}

fn copy_hevc_encode_source_plane(
    upload: &mut [u8],
    region: &vk::BufferImageCopy,
    source: &[u8],
    source_width_texels: usize,
    source_height: usize,
    bytes_per_texel: usize,
) -> Result<(), String> {
    let dst_offset = usize::try_from(region.buffer_offset)
        .map_err(|_| "HEVC source plane dst offset exceeds usize".to_string())?;
    let dst_width_texels = usize::try_from(region.buffer_row_length)
        .map_err(|_| "HEVC source plane row length exceeds usize".to_string())?;
    let dst_height = usize::try_from(region.buffer_image_height)
        .map_err(|_| "HEVC source plane image height exceeds usize".to_string())?;
    if source_width_texels > dst_width_texels || source_height > dst_height {
        return Err(format!(
            "HEVC source plane {}x{} does not fit upload plane {}x{}",
            source_width_texels, source_height, dst_width_texels, dst_height
        ));
    }
    let src_row_bytes = source_width_texels
        .checked_mul(bytes_per_texel)
        .ok_or_else(|| "HEVC source plane row size overflow".to_string())?;
    let dst_row_bytes = dst_width_texels
        .checked_mul(bytes_per_texel)
        .ok_or_else(|| "HEVC upload plane row size overflow".to_string())?;
    for row in 0..source_height {
        let src_start = row
            .checked_mul(src_row_bytes)
            .ok_or_else(|| "HEVC source plane row offset overflow".to_string())?;
        let dst_start = dst_offset
            .checked_add(
                row.checked_mul(dst_row_bytes)
                    .ok_or_else(|| "HEVC upload plane row offset overflow".to_string())?,
            )
            .ok_or_else(|| "HEVC upload plane row start overflow".to_string())?;
        let dst_end = dst_start
            .checked_add(src_row_bytes)
            .ok_or_else(|| "HEVC upload plane row end overflow".to_string())?;
        upload
            .get_mut(dst_start..dst_end)
            .ok_or_else(|| "HEVC upload plane row outside buffer".to_string())?
            .copy_from_slice(
                source
                    .get(src_start..src_start + src_row_bytes)
                    .ok_or_else(|| "HEVC source plane row outside buffer".to_string())?,
            );
    }
    Ok(())
}

pub(super) fn write_hevc_encode_probe_dst_prefix(
    device: &ash::Device,
    memory: vk::DeviceMemory,
    prefix_bytes: u64,
    mode: HevcEncodeProbeDstPrefixMode,
) -> Result<u64, String> {
    if prefix_bytes == 0 || matches!(mode, HevcEncodeProbeDstPrefixMode::None) {
        return Ok(0);
    }
    // SAFETY: `memory` is a host-visible destination allocation and the caller bounds the prefix
    // to the allocation-backed destination buffer range.
    let mapped = unsafe { device.map_memory(memory, 0, prefix_bytes, vk::MemoryMapFlags::empty()) }
        .map_err(|err| format!("vkMapMemory for encode dst prefix failed: {err}"))?;
    let result = (|| {
        let prefix_len = usize::try_from(prefix_bytes)
            .map_err(|_| "encode dst prefix size exceeds usize range".to_string())?;
        // SAFETY: `mapped` remains valid for the successfully mapped `prefix_len` byte range until
        // the unconditional unmap below.
        let bytes = unsafe { std::slice::from_raw_parts_mut(mapped.cast::<u8>(), prefix_len) };
        match mode {
            HevcEncodeProbeDstPrefixMode::None => {}
            HevcEncodeProbeDstPrefixMode::Zero => bytes.fill(0),
            HevcEncodeProbeDstPrefixMode::ParameterSample => {
                bytes.fill(0);
                let sample = load_hevc_encode_probe_parameter_sample()?;
                let copy_len = sample.len().min(bytes.len());
                bytes[..copy_len].copy_from_slice(&sample[..copy_len]);
                return u64::try_from(copy_len)
                    .map_err(|_| "encode dst prefix copy length exceeds u64 range".to_string());
            }
        }
        Ok(prefix_bytes)
    })();
    // SAFETY: this allocation was mapped above and the temporary slice is no longer used.
    unsafe {
        device.unmap_memory(memory);
    }
    result
}

pub(super) fn create_hevc_encode_probe_transfer_buffer(
    device: &ash::Device,
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    buffer_size: u64,
    buffer: &mut vk::Buffer,
    buffer_memory: &mut vk::DeviceMemory,
) -> Result<(), String> {
    let create_info = vk::BufferCreateInfo::default()
        .size(buffer_size)
        .usage(vk::BufferUsageFlags::TRANSFER_SRC)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: buffer create info references local stack data.
    *buffer = unsafe { device.create_buffer(&create_info, None) }
        .map_err(|err| format!("vkCreateBuffer for encode source upload failed: {err}"))?;
    // SAFETY: buffer handle is valid and owned by this device.
    let memory_requirements = unsafe { device.get_buffer_memory_requirements(*buffer) };
    let memory_type_index = find_memory_type_index(
        physical_device,
        instance,
        memory_requirements.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    )
    .ok_or_else(|| "no host-coherent memory type for encode source upload buffer".to_string())?;
    let allocate_info = vk::MemoryAllocateInfo::default()
        .allocation_size(memory_requirements.size)
        .memory_type_index(memory_type_index);
    // SAFETY: allocation info is valid for this device.
    *buffer_memory = unsafe { device.allocate_memory(&allocate_info, None) }
        .map_err(|err| format!("vkAllocateMemory for encode source upload failed: {err}"))?;
    // SAFETY: buffer and memory were created from this device.
    unsafe { device.bind_buffer_memory(*buffer, *buffer_memory, 0) }
        .map_err(|err| format!("vkBindBufferMemory for encode source upload failed: {err}"))?;
    Ok(())
}

pub(super) fn create_hevc_encode_probe_dst_buffer(
    device: &ash::Device,
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    buffer_size: u64,
    buffer: &mut vk::Buffer,
    buffer_memory: &mut vk::DeviceMemory,
) -> Result<(), String> {
    let mut encode_h265_profile = vk::VideoEncodeH265ProfileInfoKHR::default()
        .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
    let mut encode_usage = vk::VideoEncodeUsageInfoKHR::default()
        .video_usage_hints(vk::VideoEncodeUsageFlagsKHR::DEFAULT);
    let profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(vk::VideoCodecOperationFlagsKHR::ENCODE_H265)
        .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
        .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .push_next(&mut encode_usage)
        .push_next(&mut encode_h265_profile);
    let profiles = [profile];
    let mut profile_list = vk::VideoProfileListInfoKHR::default().profiles(&profiles);
    let create_info = vk::BufferCreateInfo::default()
        .size(buffer_size)
        .usage(vk::BufferUsageFlags::VIDEO_ENCODE_DST_KHR)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .push_next(&mut profile_list);
    // SAFETY: buffer create info references local stack data.
    *buffer = unsafe { device.create_buffer(&create_info, None) }
        .map_err(|err| format!("vkCreateBuffer for encode submit probe failed: {err}"))?;
    // SAFETY: buffer handle is valid and owned by this device.
    let memory_requirements = unsafe { device.get_buffer_memory_requirements(*buffer) };
    let memory_type_index = find_memory_type_index(
        physical_device,
        instance,
        memory_requirements.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    )
    .or_else(|| {
        find_memory_type_index(
            physical_device,
            instance,
            memory_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE,
        )
    })
    .or_else(|| {
        find_memory_type_index(
            physical_device,
            instance,
            memory_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
    })
    .or_else(|| {
        find_memory_type_index(
            physical_device,
            instance,
            memory_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::empty(),
        )
    })
    .ok_or_else(|| "no compatible memory type for encode submit destination buffer".to_string())?;
    let allocate_info = vk::MemoryAllocateInfo::default()
        .allocation_size(memory_requirements.size)
        .memory_type_index(memory_type_index);
    // SAFETY: allocation info is valid for this device.
    *buffer_memory = unsafe { device.allocate_memory(&allocate_info, None) }.map_err(|err| {
        format!("vkAllocateMemory for encode submit destination buffer failed: {err}")
    })?;
    // SAFETY: buffer and memory were created from this device.
    unsafe { device.bind_buffer_memory(*buffer, *buffer_memory, 0) }.map_err(|err| {
        format!("vkBindBufferMemory for encode submit destination buffer failed: {err}")
    })?;
    Ok(())
}

pub(super) fn select_hevc_encode_rate_control_mode(
    modes: vk::VideoEncodeRateControlModeFlagsKHR,
    max_rate_control_layers: u32,
) -> Option<vk::VideoEncodeRateControlModeFlagsKHR> {
    if modes.contains(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED) {
        return Some(vk::VideoEncodeRateControlModeFlagsKHR::DISABLED);
    }
    if max_rate_control_layers == 0 {
        return None;
    }
    if modes.contains(vk::VideoEncodeRateControlModeFlagsKHR::CBR) {
        return Some(vk::VideoEncodeRateControlModeFlagsKHR::CBR);
    }
    if modes.contains(vk::VideoEncodeRateControlModeFlagsKHR::VBR) {
        return Some(vk::VideoEncodeRateControlModeFlagsKHR::VBR);
    }
    None
}

pub(super) fn align_up(value: u64, alignment: u64) -> u64 {
    if alignment <= 1 {
        value
    } else {
        let remainder = value % alignment;
        if remainder == 0 {
            value
        } else {
            value.saturating_add(alignment - remainder)
        }
    }
}

pub(super) fn find_memory_type_index(
    physical_device: vk::PhysicalDevice,
    instance: &ash::Instance,
    memory_type_bits: u32,
    required_properties: vk::MemoryPropertyFlags,
) -> Option<u32> {
    // SAFETY: physical device handle is valid for this instance.
    let memory_properties =
        unsafe { instance.get_physical_device_memory_properties(physical_device) };
    memory_properties
        .memory_types
        .iter()
        .take(usize::try_from(memory_properties.memory_type_count).ok()?)
        .enumerate()
        .find_map(|(index, memory_type)| {
            let index_u32 = u32::try_from(index).ok()?;
            let supported = (memory_type_bits & (1_u32 << index_u32)) != 0;
            let properties_match = memory_type.property_flags.contains(required_properties);
            (supported && properties_match).then_some(index_u32)
        })
}
