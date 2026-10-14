//! HEVC encode queue submission, command recording, and feedback extraction.

use super::*;

pub(super) fn probe_hevc_encode_submit_execution(
    device: &ash::Device,
    instance: &ash::Instance,
    config: HevcEncodeSubmitExecutionConfig,
) -> HevcEncodeSubmitExecutionProbe {
    let video_queue_device = ash::khr::video_queue::Device::new(instance, device);
    let video_encode_device = ash::khr::video_encode_queue::Device::new(instance, device);
    let mut command_pool = vk::CommandPool::null();
    let mut fence = vk::Fence::null();
    let mut source_image = vk::Image::null();
    let mut source_image_memory = vk::DeviceMemory::null();
    let mut source_image_view = vk::ImageView::null();
    let mut source_image_dedicated_allocation = false;
    let mut source_upload_buffer = vk::Buffer::null();
    let mut source_upload_buffer_memory = vk::DeviceMemory::null();
    let mut dpb_image = vk::Image::null();
    let mut dpb_image_memory = vk::DeviceMemory::null();
    let mut dpb_image_view = vk::ImageView::null();
    let mut dpb_image_dedicated_allocation = false;
    let mut ycbcr_conversion = vk::SamplerYcbcrConversion::null();
    let mut dst_buffer = vk::Buffer::null();
    let mut dst_buffer_memory = vk::DeviceMemory::null();
    let mut query_pool = vk::QueryPool::null();
    let mut query_result = HevcEncodeFeedbackQueryResult::default();
    let mut dst_head16 = String::new();
    let mut dst_buffer_offset = 0_u64;
    let mut bitstream_memory_offset = 0_u64;
    let mut output_bitstream = Vec::new();

    let probe_result = (|| -> Result<(), String> {
        if !config
            .supported_encode_feedback_flags
            .contains(vk::VideoEncodeFeedbackFlagsKHR::BITSTREAM_BYTES_WRITTEN)
        {
            return Err(
                "encode submit feedback does not support BITSTREAM_BYTES_WRITTEN".to_string(),
            );
        }

        // SAFETY: queue family index and queue 0 were requested during device creation.
        let queue = unsafe { device.get_device_queue(config.queue_family_index.0, 0) };
        if queue == vk::Queue::null() {
            return Err(format!(
                "vkGetDeviceQueue returned null for queue family {}",
                config.queue_family_index.0
            ));
        }

        let command_pool_info =
            vk::CommandPoolCreateInfo::default().queue_family_index(config.queue_family_index.0);
        // SAFETY: command pool create info references only stack data.
        command_pool = unsafe { device.create_command_pool(&command_pool_info, None) }
            .map_err(|err| format!("vkCreateCommandPool failed: {err}"))?;

        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(5);
        // SAFETY: allocation info references valid command pool handle.
        let command_buffers = unsafe { device.allocate_command_buffers(&allocate_info) }
            .map_err(|err| format!("vkAllocateCommandBuffers failed: {err}"))?;
        let command_buffer = *command_buffers.first().ok_or_else(|| {
            "vkAllocateCommandBuffers returned no command buffers for encode submit probe"
                .to_string()
        })?;
        let coding_scope_probe_command_buffer = *command_buffers.get(1).ok_or_else(|| {
            "vkAllocateCommandBuffers returned no coding-scope probe command buffer".to_string()
        })?;
        let pre_encode_scope_probe_command_buffer = *command_buffers.get(2).ok_or_else(|| {
            "vkAllocateCommandBuffers returned no pre-encode scope-only probe command buffer"
                .to_string()
        })?;
        let pre_encode_probe_command_buffer = *command_buffers.get(3).ok_or_else(|| {
            "vkAllocateCommandBuffers returned no pre-encode probe command buffer".to_string()
        })?;
        let pre_encode_minimal_probe_command_buffer = *command_buffers.get(4).ok_or_else(|| {
            "vkAllocateCommandBuffers returned no pre-encode minimal probe command buffer"
                .to_string()
        })?;
        let image_width = u32::try_from(align_up(
            u64::from(config.coded_width),
            u64::from(config.picture_access_granularity.width.max(1)),
        ))
        .map_err(|_| "aligned encode source image width exceeds u32 range".to_string())?;
        let image_height = u32::try_from(align_up(
            u64::from(config.coded_height),
            u64::from(config.picture_access_granularity.height.max(1)),
        ))
        .map_err(|_| "aligned encode source image height exceeds u32 range".to_string())?;
        let image_view_mode = resolve_hevc_encode_probe_image_view_mode();
        let image_view_mode_label = hevc_encode_probe_image_view_mode_label(image_view_mode);
        if matches!(
            image_view_mode,
            HevcEncodeProbeImageViewMode::YcbcrConversion
        ) {
            ycbcr_conversion =
                create_hevc_encode_probe_ycbcr_conversion(device, config.picture_format)?;
        }
        create_hevc_encode_probe_image(
            device,
            instance,
            ycbcr_conversion,
            HevcEncodeProbeImageConfig {
                physical_device: config.physical_device,
                queue_family_index: config.queue_family_index,
                image_width,
                image_height,
                image_view_mode,
                picture_format: config.picture_format,
                usage: vk::ImageUsageFlags::VIDEO_ENCODE_SRC_KHR
                    | vk::ImageUsageFlags::TRANSFER_DST,
            },
            HevcEncodeProbeImageResources {
                image: &mut source_image,
                memory: &mut source_image_memory,
                view: &mut source_image_view,
                dedicated_allocation: &mut source_image_dedicated_allocation,
            },
        )?;
        let (source_upload_buffer_size, source_upload_regions) =
            build_hevc_encode_source_upload_regions(
                config.picture_format,
                image_width,
                image_height,
            )?;
        create_hevc_encode_probe_transfer_buffer(
            device,
            instance,
            config.physical_device,
            source_upload_buffer_size,
            &mut source_upload_buffer,
            &mut source_upload_buffer_memory,
        )?;
        write_hevc_encode_source_upload(
            device,
            source_upload_buffer_memory,
            source_upload_buffer_size,
            &source_upload_regions,
            config.source_nv12.as_deref(),
            config.coded_width,
            config.coded_height,
        )?;
        create_hevc_encode_probe_image(
            device,
            instance,
            ycbcr_conversion,
            HevcEncodeProbeImageConfig {
                physical_device: config.physical_device,
                queue_family_index: config.queue_family_index,
                image_width,
                image_height,
                image_view_mode,
                picture_format: config.reference_picture_format,
                usage: vk::ImageUsageFlags::VIDEO_ENCODE_DPB_KHR,
            },
            HevcEncodeProbeImageResources {
                image: &mut dpb_image,
                memory: &mut dpb_image_memory,
                view: &mut dpb_image_view,
                dedicated_allocation: &mut dpb_image_dedicated_allocation,
            },
        )?;
        let base_dst_size = u64::from(config.coded_width)
            .saturating_mul(u64::from(config.coded_height))
            .saturating_mul(4)
            .max(1_048_576);
        let dst_offset_alignment = config.min_bitstream_buffer_offset_alignment.max(1);
        let synthetic_prefix_bytes = align_up(
            resolve_hevc_encode_probe_dst_prefix_bytes(),
            dst_offset_alignment,
        );
        dst_buffer_offset = synthetic_prefix_bytes;
        if !dst_buffer_offset.is_multiple_of(dst_offset_alignment) {
            return Err(format!(
                "encode dst buffer offset {dst_buffer_offset} is not aligned to {dst_offset_alignment}"
            ));
        }
        let dst_size_alignment = config.min_bitstream_buffer_size_alignment.max(1);
        let dst_buffer_alloc_range = align_up(base_dst_size, dst_size_alignment);
        let dst_range_mode = resolve_hevc_encode_probe_dst_range_mode();
        let dst_range_mode_label = hevc_encode_probe_dst_range_mode_label(dst_range_mode);
        let dst_buffer_range = hevc_encode_probe_dst_buffer_range(
            dst_buffer_alloc_range,
            dst_size_alignment,
            dst_range_mode,
        );
        if dst_buffer_range == 0 {
            return Err("encode dst buffer range resolved to zero".to_string());
        }
        let dst_buffer_size = dst_buffer_offset
            .checked_add(dst_buffer_alloc_range)
            .ok_or_else(|| "encode dst buffer size overflow".to_string())?;
        create_hevc_encode_probe_dst_buffer(
            device,
            instance,
            config.physical_device,
            dst_buffer_size,
            &mut dst_buffer,
            &mut dst_buffer_memory,
        )?;
        let dst_prefix_mode = resolve_hevc_encode_probe_dst_prefix_mode();
        let dst_prefix_mode_label = hevc_encode_probe_dst_prefix_mode_label(dst_prefix_mode);
        let dst_prefix_written = write_hevc_encode_probe_dst_prefix(
            device,
            dst_buffer_memory,
            dst_buffer_offset,
            dst_prefix_mode,
        )?;
        query_pool =
            create_hevc_encode_feedback_query_pool(device, config.supported_encode_feedback_flags)?;
        let requested_rate_control_mode = resolve_hevc_encode_probe_rate_control_mode();
        let selected_rate_control_mode = resolve_hevc_encode_probe_selected_rate_control_mode(
            requested_rate_control_mode,
            config.rate_control_modes,
            config.max_rate_control_layers,
        );
        let reference_list_mode = resolve_hevc_encode_probe_reference_list_mode();
        let reference_index_mode = resolve_hevc_encode_probe_reference_index_mode();
        let rps_mode = resolve_hevc_encode_probe_rps_mode();
        let begin_reference_slot_mode = resolve_hevc_encode_probe_begin_reference_slot_mode();
        let setup_reference_slot_mode = resolve_hevc_encode_probe_setup_reference_slot_mode();
        let dpb_barrier_mode = resolve_hevc_encode_probe_dpb_barrier_mode();
        let encode_reference_slot_pointer_mode =
            resolve_hevc_encode_probe_encode_reference_slot_pointer_mode();
        let control_mode = resolve_hevc_encode_probe_control_mode();
        let nalu_mode = resolve_hevc_encode_probe_nalu_mode();
        let codec_info_mode = resolve_hevc_encode_probe_codec_info_mode();
        let picture_flags_mode = resolve_hevc_encode_probe_picture_flags_mode();
        let picture_info_mode = resolve_hevc_encode_probe_picture_info_mode();
        let picture_type = hevc_encode_probe_picture_type(picture_info_mode);
        let slice_type = hevc_encode_probe_slice_type(picture_info_mode);
        let temporal_id = hevc_encode_probe_temporal_id(picture_info_mode);
        let pic_order_cnt_val = hevc_encode_probe_pic_order_cnt_val(picture_info_mode);
        let reference_idx_minus1 = hevc_encode_probe_reference_idx_minus1(reference_index_mode);
        let selected_quality_level =
            resolve_hevc_encode_probe_quality_level(config.max_quality_levels);
        let control_config = HevcEncodeProbeControlConfig {
            selected_rate_control_mode,
            selected_quality_level,
            enable_quality_level_control: config.max_quality_levels > 0,
            control_mode,
        };
        let selected_rate_control_mode_label = selected_rate_control_mode
            .map(|mode| format!("{mode:?}"))
            .unwrap_or_else(|| "none".to_string());
        let requested_rate_control_mode_label =
            hevc_encode_probe_rate_control_mode_label(requested_rate_control_mode);
        let parameter_mode_label = hevc_encode_parameter_mode_label(config.parameter_mode);
        let parameter_size_mode_label =
            hevc_encode_probe_parameter_size_mode_label(config.parameter_size_mode);
        let parameter_vui_safety_mode_label =
            hevc_encode_probe_parameter_vui_safety_mode_label(config.parameter_vui_safety_mode);
        let constant_qp = resolve_hevc_encode_probe_constant_qp(selected_rate_control_mode);
        let slice_qp_delta =
            hevc_encode_probe_slice_qp_delta(constant_qp, config.parameter_set_pps_init_qp_minus26)
                .map_err(|err| format!("{err} for submit probe"))?;
        let parameter_feedback_probe = config
            .parameter_feedback_probe_summary
            .clone()
            .unwrap_or_else(|| "none".to_string());
        let reference_list_mode_label =
            hevc_encode_probe_reference_list_mode_label(reference_list_mode);
        let reference_index_mode_label =
            hevc_encode_probe_reference_index_mode_label(reference_index_mode);
        let rps_mode_label = hevc_encode_probe_rps_mode_label(rps_mode);
        let begin_reference_slot_mode_label =
            hevc_encode_probe_begin_reference_slot_mode_label(begin_reference_slot_mode);
        let setup_reference_slot_mode_label =
            hevc_encode_probe_setup_reference_slot_mode_label(setup_reference_slot_mode);
        let dpb_barrier_mode_label = hevc_encode_probe_dpb_barrier_mode_label(dpb_barrier_mode);
        let encode_reference_slot_pointer_mode_label =
            hevc_encode_probe_encode_reference_slot_pointer_mode_label(
                encode_reference_slot_pointer_mode,
            );
        let begin_session_parameters_mode =
            resolve_hevc_encode_probe_begin_session_parameters_mode();
        let begin_pnext_mode = resolve_hevc_encode_probe_begin_pnext_mode();
        let begin_session_parameters_mode_label =
            hevc_encode_probe_begin_session_parameters_mode_label(begin_session_parameters_mode);
        let begin_pnext_mode_label = hevc_encode_probe_begin_pnext_mode_label(begin_pnext_mode);
        let control_mode_label = hevc_encode_probe_control_mode_label(control_mode);
        let nalu_mode_label = hevc_encode_probe_nalu_mode_label(nalu_mode);
        let codec_info_mode_label = hevc_encode_probe_codec_info_mode_label(codec_info_mode);
        let primary_probe_mode = resolve_hevc_encode_probe_primary_mode();
        let primary_probe_mode_label = hevc_encode_primary_probe_mode_label(primary_probe_mode);
        let picture_flags_mode_label =
            hevc_encode_probe_picture_flags_mode_label(picture_flags_mode);
        let picture_info_mode_label = hevc_encode_probe_picture_info_mode_label(picture_info_mode);
        let session_dpb_mode_label =
            hevc_encode_probe_session_dpb_mode_label(config.session_dpb_mode);
        let session_h265_create_info_mode_label =
            hevc_encode_probe_session_h265_create_info_mode_label(
                config.session_h265_create_info_mode,
            );
        let picture_resource_extent_mode = resolve_hevc_encode_probe_picture_resource_extent_mode();
        let (picture_resource_coded_width, picture_resource_coded_height) =
            match picture_resource_extent_mode {
                HevcEncodeProbePictureResourceExtentMode::Coded => {
                    (config.coded_width, config.coded_height)
                }
                HevcEncodeProbePictureResourceExtentMode::ImageAligned => {
                    (image_width, image_height)
                }
            };
        let picture_resource_extent_mode_label =
            hevc_encode_probe_picture_resource_extent_mode_label(picture_resource_extent_mode);
        let source_picture_resource_extent_mode =
            resolve_hevc_encode_probe_source_picture_resource_extent_mode();
        let (source_picture_resource_width, source_picture_resource_height) =
            match source_picture_resource_extent_mode {
                HevcEncodeProbePictureResourceExtentMode::Coded => {
                    (config.coded_width, config.coded_height)
                }
                HevcEncodeProbePictureResourceExtentMode::ImageAligned => {
                    (image_width, image_height)
                }
            };
        let source_picture_resource_extent_mode_label =
            hevc_encode_probe_picture_resource_extent_mode_label(
                source_picture_resource_extent_mode,
            );
        let encode_probe_context = format!(
            "encode_probe_inputs(coded={}x{}, image={}x{}, src_picture_resource={}x{}, src_picture_resource_extent_mode={}, picture_resource_coded={}x{}, picture_resource_extent_mode={}, picture_format={:?}, reference_picture_format={:?}, source_init=nv12-upload, image_view_mode={}, image_memory_dedicated=src:{}|dpb:{}, dst_offset={}, dst_range={}, dst_alloc_range={}, dst_prefix={}, dst_prefix_mode={}, dst_prefix_written={}, dst_offset_align={}, dst_size_align={}, dst_range_mode={}, parameter_mode={}, parameter_size_mode={}, parameter_vui_safety={}, parameter_set_ids=vps:{}|sps:{}|pps:{}, parameter_set_coded={}x{}, parameter_set_coded_match={}, parameter_set_pps_init_qp_minus26={}, parameter_set_sao={}, parameter_set_temporal_mvp={}, h265_std_syntax_flags={:?}, parameter_feedback_probe={}, reference_list_mode={}, reference_idx_mode={}, reference_idx_minus1={}, rps_mode={}, begin_reference_slot_mode={}, setup_reference_slot_mode={}, encode_reference_slot_pointer_mode={}, dpb_barrier_mode={}, begin_session_parameters_mode={}, begin_pnext_mode={}, control_mode={}, nalu_mode={}, codec_info_mode={}, primary_mode={}, picture_flags_mode={}, picture_info_mode={}, pic_order_cnt_val={}, temporal_id={}, constant_qp={}, slice_qp_delta={}, requested_rate_control_mode={}, rate_control_mode={}, max_rate_control_layers={}, quality_level={}, quality_level_control={}, maintenance1_mode={}, maintenance1_feature_enabled={}, session_dpb_mode={}, session_h265_create_info_mode={}, session_max_dpb_slots={}, session_max_active_refs={})",
            config.coded_width,
            config.coded_height,
            image_width,
            image_height,
            source_picture_resource_width,
            source_picture_resource_height,
            source_picture_resource_extent_mode_label,
            picture_resource_coded_width,
            picture_resource_coded_height,
            picture_resource_extent_mode_label,
            config.picture_format,
            config.reference_picture_format,
            image_view_mode_label,
            source_image_dedicated_allocation,
            dpb_image_dedicated_allocation,
            dst_buffer_offset,
            dst_buffer_range,
            dst_buffer_alloc_range,
            synthetic_prefix_bytes,
            dst_prefix_mode_label,
            dst_prefix_written,
            config.min_bitstream_buffer_offset_alignment.max(1),
            dst_size_alignment,
            dst_range_mode_label,
            parameter_mode_label,
            parameter_size_mode_label,
            parameter_vui_safety_mode_label,
            config.parameter_set_ids.vps_id,
            config.parameter_set_ids.sps_id,
            config.parameter_set_ids.pps_id,
            config.parameter_set_coded_width,
            config.parameter_set_coded_height,
            config.parameter_set_coded_width == config.coded_width
                && config.parameter_set_coded_height == config.coded_height,
            config.parameter_set_pps_init_qp_minus26,
            config.parameter_set_sample_adaptive_offset_enabled,
            config.parameter_set_sps_temporal_mvp_enabled,
            config.encode_h265_std_syntax_flags,
            parameter_feedback_probe,
            reference_list_mode_label,
            reference_index_mode_label,
            reference_idx_minus1,
            rps_mode_label,
            begin_reference_slot_mode_label,
            setup_reference_slot_mode_label,
            encode_reference_slot_pointer_mode_label,
            dpb_barrier_mode_label,
            begin_session_parameters_mode_label,
            begin_pnext_mode_label,
            control_mode_label,
            nalu_mode_label,
            codec_info_mode_label,
            primary_probe_mode_label,
            picture_flags_mode_label,
            picture_info_mode_label,
            pic_order_cnt_val,
            temporal_id,
            constant_qp,
            slice_qp_delta,
            requested_rate_control_mode_label,
            selected_rate_control_mode_label,
            config.max_rate_control_layers,
            selected_quality_level,
            control_config.enable_quality_level_control,
            hevc_encode_probe_maintenance1_mode_label(config.maintenance1_mode),
            config.maintenance1_feature_enabled,
            session_dpb_mode_label,
            session_h265_create_info_mode_label,
            config.session_max_dpb_slots,
            config.session_max_active_reference_pictures,
        );

        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: command buffer is valid and not recording.
        unsafe { device.begin_command_buffer(command_buffer, &begin_info) }
            .map_err(|err| format!("vkBeginCommandBuffer failed: {err}"))?;

        let source_image_prepare_barrier = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::NONE)
            .src_access_mask(vk::AccessFlags2::NONE)
            .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(source_image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let source_upload_buffer_barrier = vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::HOST)
            .src_access_mask(vk::AccessFlags2::HOST_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
            .buffer(source_upload_buffer)
            .offset(0)
            .size(source_upload_buffer_size);
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
            .image(source_image)
            .subresource_range(clear_range);
        let dpb_image_encode_barrier = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::NONE)
            .src_access_mask(vk::AccessFlags2::NONE)
            .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_ENCODE_KHR)
            .dst_access_mask(
                vk::AccessFlags2::VIDEO_ENCODE_READ_KHR | vk::AccessFlags2::VIDEO_ENCODE_WRITE_KHR,
            )
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::VIDEO_ENCODE_DPB_KHR)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(dpb_image)
            .subresource_range(clear_range);
        let dst_buffer_barrier = vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::NONE)
            .src_access_mask(vk::AccessFlags2::NONE)
            .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_ENCODE_KHR)
            .dst_access_mask(vk::AccessFlags2::VIDEO_ENCODE_WRITE_KHR)
            .buffer(dst_buffer)
            .offset(0)
            .size(dst_buffer_size);
        let encode_image_barriers_with_dpb =
            [source_image_encode_barrier, dpb_image_encode_barrier];
        let encode_image_barriers_without_dpb = [source_image_encode_barrier];
        let encode_image_barriers = match dpb_barrier_mode {
            HevcEncodeProbeDpbBarrierMode::With => encode_image_barriers_with_dpb.as_slice(),
            HevcEncodeProbeDpbBarrierMode::None => encode_image_barriers_without_dpb.as_slice(),
        };
        let dependency_info = vk::DependencyInfo::default()
            .image_memory_barriers(encode_image_barriers)
            .buffer_memory_barriers(std::slice::from_ref(&dst_buffer_barrier));
        // SAFETY: barriers reference resources created above and command buffer is recording.
        unsafe {
            device.cmd_reset_query_pool(command_buffer, query_pool, 0, 1);
            device.cmd_pipeline_barrier2(command_buffer, &source_prepare_dependency);
            device.cmd_copy_buffer_to_image(
                command_buffer,
                source_upload_buffer,
                source_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &source_upload_regions,
            );
            device.cmd_pipeline_barrier2(command_buffer, &dependency_info);
        }

        let reconstructed_picture_resource = vk::VideoPictureResourceInfoKHR::default()
            .coded_offset(vk::Offset2D::default())
            .coded_extent(vk::Extent2D {
                width: picture_resource_coded_width,
                height: picture_resource_coded_height,
            })
            .base_array_layer(0)
            .image_view_binding(dpb_image_view);
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
        let begin_reference_info = StdVideoEncodeH265ReferenceInfo {
            flags: StdVideoEncodeH265ReferenceInfoFlags {
                _bitfield_align_1: [],
                _bitfield_1: Default::default(),
            },
            pic_type: StdVideoH265PictureType_STD_VIDEO_H265_PICTURE_TYPE_IDR,
            PicOrderCntVal: 0,
            TemporalId: 0,
        };
        let mut begin_dpb_slot_info =
            vk::VideoEncodeH265DpbSlotInfoKHR::default().std_reference_info(&begin_reference_info);
        let begin_reference_slots = [vk::VideoReferenceSlotInfoKHR::default()
            .slot_index(-1)
            .picture_resource(&reconstructed_picture_resource)
            .push_next(&mut begin_dpb_slot_info)];
        let begin_reference_slots_no_codec_info = [vk::VideoReferenceSlotInfoKHR::default()
            .slot_index(-1)
            .picture_resource(&reconstructed_picture_resource)];

        let begin_coding_info_base = vk::VideoBeginCodingInfoKHR::default()
            .video_session(config.video_session)
            .video_session_parameters(config.video_session_parameters);
        let begin_coding_info_with_ref_slots = match begin_reference_slot_mode {
            HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOne => {
                begin_coding_info_base.reference_slots(&begin_reference_slots)
            }
            HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOneNoCodecInfo => {
                begin_coding_info_base.reference_slots(&begin_reference_slots_no_codec_info)
            }
            HevcEncodeProbeBeginReferenceSlotMode::None => begin_coding_info_base,
        };
        let begin_coding_info_without_session_parameters =
            vk::VideoBeginCodingInfoKHR::default().video_session(config.video_session);
        let begin_coding_info = match begin_session_parameters_mode {
            HevcEncodeProbeBeginSessionParametersMode::With => begin_coding_info_with_ref_slots,
            HevcEncodeProbeBeginSessionParametersMode::Without => match begin_reference_slot_mode {
                HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOne => {
                    begin_coding_info_without_session_parameters
                        .reference_slots(&begin_reference_slots)
                }
                HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOneNoCodecInfo => {
                    begin_coding_info_without_session_parameters
                        .reference_slots(&begin_reference_slots_no_codec_info)
                }
                HevcEncodeProbeBeginReferenceSlotMode::None => {
                    begin_coding_info_without_session_parameters
                }
            },
        };
        if matches!(primary_probe_mode, HevcEncodePrimaryProbeMode::ScopeOnly) {
            let scope_only_result = run_hevc_encode_coding_scope_probe(
                device,
                &video_queue_device,
                command_buffer,
                config.video_session,
                config.video_session_parameters,
                begin_session_parameters_mode,
                control_config,
            )
            .map_err(|err| {
                format!("scope-only primary probe failed: {err}; {encode_probe_context}")
            });
            return scope_only_result;
        }

        let pre_encode_resources = HevcEncodePreEncodeProbeResources {
            source_image,
            source_image_view,
            source_upload_buffer,
            source_upload_buffer_size,
            source_upload_width: image_width,
            source_upload_height: image_height,
            dpb_image,
            dpb_image_view,
            source_picture_resource_width,
            source_picture_resource_height,
            picture_resource_coded_width,
            picture_resource_coded_height,
            dst_buffer,
            dst_buffer_offset,
            dst_buffer_range,
            dst_buffer_size,
        };
        match primary_probe_mode {
            HevcEncodePrimaryProbeMode::PreEncodeScopeOnly => {
                let pre_encode_scope_result = run_hevc_encode_pre_encode_probe(
                    device,
                    &video_queue_device,
                    &video_encode_device,
                    HevcEncodePreEncodeProbeConfig {
                        command_buffer,
                        video_session: config.video_session,
                        video_session_parameters: config.video_session_parameters,
                        control_config,
                        mode: HevcEncodePreEncodeProbeMode::ScopeOnly,
                        begin_session_parameters_mode,
                        nalu_mode,
                        codec_info_mode,
                        sample_adaptive_offset_enabled: config
                            .parameter_set_sample_adaptive_offset_enabled,
                        sps_temporal_mvp_enabled: config.parameter_set_sps_temporal_mvp_enabled,
                        resources: pre_encode_resources,
                    },
                )
                .map_err(|err| {
                    format!(
                        "pre-encode-scope-only primary probe failed: {err}; {encode_probe_context}"
                    )
                });
                return pre_encode_scope_result;
            }
            HevcEncodePrimaryProbeMode::PreEncode => {
                let pre_encode_result = run_hevc_encode_pre_encode_probe(
                    device,
                    &video_queue_device,
                    &video_encode_device,
                    HevcEncodePreEncodeProbeConfig {
                        command_buffer,
                        video_session: config.video_session,
                        video_session_parameters: config.video_session_parameters,
                        control_config,
                        mode: HevcEncodePreEncodeProbeMode::WithEncode,
                        begin_session_parameters_mode,
                        nalu_mode,
                        codec_info_mode,
                        sample_adaptive_offset_enabled: config
                            .parameter_set_sample_adaptive_offset_enabled,
                        sps_temporal_mvp_enabled: config.parameter_set_sps_temporal_mvp_enabled,
                        resources: pre_encode_resources,
                    },
                )
                .map_err(|err| {
                    format!("pre-encode primary probe failed: {err}; {encode_probe_context}")
                });
                return pre_encode_result;
            }
            HevcEncodePrimaryProbeMode::PreEncodeMinimal => {
                let pre_encode_minimal_result = run_hevc_encode_pre_encode_probe(
                    device,
                    &video_queue_device,
                    &video_encode_device,
                    HevcEncodePreEncodeProbeConfig {
                        command_buffer,
                        video_session: config.video_session,
                        video_session_parameters: config.video_session_parameters,
                        control_config,
                        mode: HevcEncodePreEncodeProbeMode::WithEncodeMinimal,
                        begin_session_parameters_mode,
                        nalu_mode,
                        codec_info_mode,
                        sample_adaptive_offset_enabled: config
                            .parameter_set_sample_adaptive_offset_enabled,
                        sps_temporal_mvp_enabled: config.parameter_set_sps_temporal_mvp_enabled,
                        resources: pre_encode_resources,
                    },
                )
                .map_err(|err| {
                    format!(
                        "pre-encode-minimal primary probe failed: {err}; {encode_probe_context}"
                    )
                });
                return pre_encode_minimal_result;
            }
            HevcEncodePrimaryProbeMode::Submit => {}
            HevcEncodePrimaryProbeMode::ScopeOnly => unreachable!(),
        }

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
        let mut coding_control_info =
            vk::VideoCodingControlInfoKHR::default().flags(vk::VideoCodingControlFlagsKHR::RESET);
        let mut quality_level_info = vk::VideoEncodeQualityLevelInfoKHR::default()
            .quality_level(control_config.selected_quality_level);
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
        let h265_begin_rate_control_info = hevc_encode_h265_rate_control_info();
        let mut begin_rate_control_layer_h265 =
            vk::VideoEncodeH265RateControlLayerInfoKHR::default();
        let begin_rate_control_layer = vk::VideoEncodeRateControlLayerInfoKHR::default()
            .average_bitrate(8_000_000)
            .max_bitrate(8_000_000)
            .frame_rate_numerator(30)
            .frame_rate_denominator(1)
            .push_next(&mut begin_rate_control_layer_h265);
        let mut begin_rate_control_info =
            vk::VideoEncodeRateControlInfoKHR::default().rate_control_mode(rate_control_mode);
        let mut begin_coding_info = begin_coding_info;
        if matches!(begin_pnext_mode, HevcEncodeProbeBeginPnextMode::RateControl) {
            begin_rate_control_info.p_next =
                std::ptr::from_ref(&h265_begin_rate_control_info).cast();
            if rate_control_mode != vk::VideoEncodeRateControlModeFlagsKHR::DISABLED {
                begin_rate_control_info = begin_rate_control_info
                    .layers(std::slice::from_ref(&begin_rate_control_layer))
                    .virtual_buffer_size_in_ms(1000)
                    .initial_virtual_buffer_size_in_ms(1000);
            }
            begin_coding_info = begin_coding_info.push_next(&mut begin_rate_control_info);
        }
        // SAFETY: command buffer is recording and session handles are valid.
        unsafe {
            (video_queue_device.fp().cmd_begin_video_coding_khr)(
                command_buffer,
                &begin_coding_info,
            );
            if issue_coding_control {
                (video_queue_device.fp().cmd_control_video_coding_khr)(
                    command_buffer,
                    &coding_control_info,
                );
            }
        }

        let std_slice_flags = hevc_encode_std_slice_segment_header_flags(
            config.parameter_set_sample_adaptive_offset_enabled,
        );
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
        let mut std_reference_list_flags =
            ash::vk::native::StdVideoEncodeH265ReferenceListsInfoFlags {
                _bitfield_align_1: [],
                _bitfield_1: Default::default(),
            };
        std_reference_list_flags.set_ref_pic_list_modification_flag_l0(0);
        std_reference_list_flags.set_ref_pic_list_modification_flag_l1(0);
        let reference_list_sentinel = match reference_list_mode {
            HevcEncodeProbeReferenceListMode::Sentinel => HEVC_NO_REFERENCE_PICTURE,
            HevcEncodeProbeReferenceListMode::Zero => 0,
            HevcEncodeProbeReferenceListMode::NullPointers => HEVC_NO_REFERENCE_PICTURE,
        };
        let std_reference_lists = StdVideoEncodeH265ReferenceListsInfo {
            flags: std_reference_list_flags,
            num_ref_idx_l0_active_minus1: reference_idx_minus1,
            num_ref_idx_l1_active_minus1: reference_idx_minus1,
            RefPicList0: [reference_list_sentinel; 15],
            RefPicList1: [reference_list_sentinel; 15],
            list_entry_l0: [0; 15],
            list_entry_l1: [0; 15],
        };
        let reference_lists_ptr = match reference_list_mode {
            HevcEncodeProbeReferenceListMode::NullPointers => std::ptr::null(),
            HevcEncodeProbeReferenceListMode::Sentinel | HevcEncodeProbeReferenceListMode::Zero => {
                &std_reference_lists as *const StdVideoEncodeH265ReferenceListsInfo
            }
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
        std_picture_flags.set_slice_temporal_mvp_enabled_flag(u32::from(
            config.parameter_set_sps_temporal_mvp_enabled,
        ));
        std_picture_flags.set_used_for_long_term_reference(0);
        std_picture_flags.set_discardable_flag(0);
        std_picture_flags.set_cross_layer_bla_flag(0);
        std_picture_flags.set_no_output_of_prior_pics_flag(0);
        let std_picture_info = StdVideoEncodeH265PictureInfo {
            flags: std_picture_flags,
            pic_type: picture_type,
            sps_video_parameter_set_id: config.parameter_set_ids.vps_id,
            pps_seq_parameter_set_id: config.parameter_set_ids.sps_id,
            pps_pic_parameter_set_id: config.parameter_set_ids.pps_id,
            short_term_ref_pic_set_idx: 0,
            PicOrderCntVal: pic_order_cnt_val,
            TemporalId: temporal_id,
            reserved1: [0; 7],
            pRefLists: reference_lists_ptr,
            pShortTermRefPicSet: short_term_ref_pic_set_ptr,
            pLongTermRefPics: long_term_ref_pics_ptr,
        };
        let mut h265_encode_info = vk::VideoEncodeH265PictureInfoKHR::default()
            .nalu_slice_segment_entries(nalu_entries)
            .std_picture_info(&std_picture_info);
        let src_picture_resource = vk::VideoPictureResourceInfoKHR::default()
            .coded_offset(vk::Offset2D::default())
            .coded_extent(vk::Extent2D {
                width: source_picture_resource_width,
                height: source_picture_resource_height,
            })
            .base_array_layer(0)
            .image_view_binding(source_image_view);
        let encode_info = vk::VideoEncodeInfoKHR::default()
            .dst_buffer(dst_buffer)
            .dst_buffer_offset(dst_buffer_offset)
            .dst_buffer_range(dst_buffer_range)
            .src_picture_resource(src_picture_resource)
            .reference_slots(&[])
            .push_next(&mut h265_encode_info);
        let encode_info = match encode_reference_slot_pointer_mode {
            HevcEncodeProbeEncodeReferenceSlotPointerMode::EmptySlice => encode_info,
            HevcEncodeProbeEncodeReferenceSlotPointerMode::FfmpegNonNull => {
                let reference_slots_ptr = match begin_reference_slot_mode {
                    HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOne => {
                        begin_reference_slots.as_ptr()
                    }
                    HevcEncodeProbeBeginReferenceSlotMode::SlotMinusOneNoCodecInfo => {
                        begin_reference_slots_no_codec_info.as_ptr()
                    }
                    HevcEncodeProbeBeginReferenceSlotMode::None => std::ptr::null(),
                };
                vk::VideoEncodeInfoKHR {
                    reference_slot_count: 0,
                    p_reference_slots: reference_slots_ptr,
                    ..encode_info
                }
            }
        };
        let encode_info = match setup_reference_slot_mode {
            HevcEncodeProbeSetupReferenceSlotMode::SlotZero => {
                encode_info.setup_reference_slot(&setup_reference_slot)
            }
            HevcEncodeProbeSetupReferenceSlotMode::None => encode_info,
        };
        // SAFETY: encode info references local data that remains valid through the call.
        unsafe {
            device.cmd_begin_query(
                command_buffer,
                query_pool,
                0,
                vk::QueryControlFlags::empty(),
            );
            (video_encode_device.fp().cmd_encode_video_khr)(command_buffer, &encode_info);
            device.cmd_end_query(command_buffer, query_pool, 0);
        }

        let end_coding_info = vk::VideoEndCodingInfoKHR::default();
        // SAFETY: command buffer is recording and video coding scope was opened.
        unsafe {
            (video_queue_device.fp().cmd_end_video_coding_khr)(command_buffer, &end_coding_info);
        }

        // SAFETY: command buffer is valid and currently recording.
        if let Err(err) = unsafe { device.end_command_buffer(command_buffer) } {
            let coding_scope_probe = run_hevc_encode_coding_scope_probe(
                device,
                &video_queue_device,
                coding_scope_probe_command_buffer,
                config.video_session,
                config.video_session_parameters,
                begin_session_parameters_mode,
                control_config,
            )
            .map(|()| "ok".to_string())
            .unwrap_or_else(|probe_err| format!("failed ({probe_err})"));
            let pre_encode_scope_probe = run_hevc_encode_pre_encode_probe(
                device,
                &video_queue_device,
                &video_encode_device,
                HevcEncodePreEncodeProbeConfig {
                    command_buffer: pre_encode_scope_probe_command_buffer,
                    video_session: config.video_session,
                    video_session_parameters: config.video_session_parameters,
                    control_config,
                    mode: HevcEncodePreEncodeProbeMode::ScopeOnly,
                    begin_session_parameters_mode,
                    nalu_mode,
                    codec_info_mode,
                    sample_adaptive_offset_enabled: config
                        .parameter_set_sample_adaptive_offset_enabled,
                    sps_temporal_mvp_enabled: config.parameter_set_sps_temporal_mvp_enabled,
                    resources: HevcEncodePreEncodeProbeResources {
                        source_image,
                        source_image_view,
                        source_upload_buffer,
                        source_upload_buffer_size,
                        source_upload_width: image_width,
                        source_upload_height: image_height,
                        dpb_image,
                        dpb_image_view,
                        source_picture_resource_width,
                        source_picture_resource_height,
                        picture_resource_coded_width,
                        picture_resource_coded_height,
                        dst_buffer,
                        dst_buffer_offset,
                        dst_buffer_range,
                        dst_buffer_size,
                    },
                },
            )
            .map(|()| "ok".to_string())
            .unwrap_or_else(|probe_err| format!("failed ({probe_err})"));
            let pre_encode_probe = run_hevc_encode_pre_encode_probe(
                device,
                &video_queue_device,
                &video_encode_device,
                HevcEncodePreEncodeProbeConfig {
                    command_buffer: pre_encode_probe_command_buffer,
                    video_session: config.video_session,
                    video_session_parameters: config.video_session_parameters,
                    control_config,
                    mode: HevcEncodePreEncodeProbeMode::WithEncode,
                    begin_session_parameters_mode,
                    nalu_mode,
                    codec_info_mode,
                    sample_adaptive_offset_enabled: config
                        .parameter_set_sample_adaptive_offset_enabled,
                    sps_temporal_mvp_enabled: config.parameter_set_sps_temporal_mvp_enabled,
                    resources: HevcEncodePreEncodeProbeResources {
                        source_image,
                        source_image_view,
                        source_upload_buffer,
                        source_upload_buffer_size,
                        source_upload_width: image_width,
                        source_upload_height: image_height,
                        dpb_image,
                        dpb_image_view,
                        source_picture_resource_width,
                        source_picture_resource_height,
                        picture_resource_coded_width,
                        picture_resource_coded_height,
                        dst_buffer,
                        dst_buffer_offset,
                        dst_buffer_range,
                        dst_buffer_size,
                    },
                },
            )
            .map(|()| "ok".to_string())
            .unwrap_or_else(|probe_err| format!("failed ({probe_err})"));
            let pre_encode_minimal_probe = run_hevc_encode_pre_encode_probe(
                device,
                &video_queue_device,
                &video_encode_device,
                HevcEncodePreEncodeProbeConfig {
                    command_buffer: pre_encode_minimal_probe_command_buffer,
                    video_session: config.video_session,
                    video_session_parameters: config.video_session_parameters,
                    control_config,
                    mode: HevcEncodePreEncodeProbeMode::WithEncodeMinimal,
                    begin_session_parameters_mode,
                    nalu_mode,
                    codec_info_mode,
                    sample_adaptive_offset_enabled: config
                        .parameter_set_sample_adaptive_offset_enabled,
                    sps_temporal_mvp_enabled: config.parameter_set_sps_temporal_mvp_enabled,
                    resources: HevcEncodePreEncodeProbeResources {
                        source_image,
                        source_image_view,
                        source_upload_buffer,
                        source_upload_buffer_size,
                        source_upload_width: image_width,
                        source_upload_height: image_height,
                        dpb_image,
                        dpb_image_view,
                        source_picture_resource_width,
                        source_picture_resource_height,
                        picture_resource_coded_width,
                        picture_resource_coded_height,
                        dst_buffer,
                        dst_buffer_offset,
                        dst_buffer_range,
                        dst_buffer_size,
                    },
                },
            )
            .map(|()| "ok".to_string())
            .unwrap_or_else(|probe_err| format!("failed ({probe_err})"));
            return Err(format!(
                "vkEndCommandBuffer failed: {err}; {encode_probe_context}; coding_scope_probe={coding_scope_probe}; pre_encode_scope_probe={pre_encode_scope_probe}; pre_encode_probe={pre_encode_probe}; pre_encode_minimal_probe={pre_encode_minimal_probe}"
            ));
        }

        // SAFETY: default fence create info is valid.
        fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }
            .map_err(|err| format!("vkCreateFence failed: {err}"))?;

        let command_buffers = [command_buffer];
        let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers);
        // SAFETY: submit info references command buffers that stay alive through the call.
        unsafe { device.queue_submit(queue, std::slice::from_ref(&submit_info), fence) }
            .map_err(|err| format!("vkQueueSubmit failed: {err}"))?;
        // SAFETY: fence is valid and associated with this queue submit.
        unsafe { device.wait_for_fences(std::slice::from_ref(&fence), true, 1_000_000_000) }
            .map_err(|err| format!("vkWaitForFences failed: {err}"))?;
        let feedback = read_hevc_encode_feedback_output(
            device,
            query_pool,
            dst_buffer_memory,
            dst_buffer_offset,
        )?;
        query_result = feedback.query_result;
        bitstream_memory_offset = feedback.bitstream_memory_offset;
        dst_head16 = feedback.head16;
        output_bitstream = feedback.bitstream;
        Ok(())
    })();

    // SAFETY: synchronization primitives are valid if they were created and are no longer used.
    unsafe {
        if source_image_view != vk::ImageView::null() {
            device.destroy_image_view(source_image_view, None);
        }
        if source_image != vk::Image::null() {
            device.destroy_image(source_image, None);
        }
        if source_image_memory != vk::DeviceMemory::null() {
            device.free_memory(source_image_memory, None);
        }
        if source_upload_buffer != vk::Buffer::null() {
            device.destroy_buffer(source_upload_buffer, None);
        }
        if source_upload_buffer_memory != vk::DeviceMemory::null() {
            device.free_memory(source_upload_buffer_memory, None);
        }
        if dpb_image_view != vk::ImageView::null() {
            device.destroy_image_view(dpb_image_view, None);
        }
        if dpb_image != vk::Image::null() {
            device.destroy_image(dpb_image, None);
        }
        if dpb_image_memory != vk::DeviceMemory::null() {
            device.free_memory(dpb_image_memory, None);
        }
        if ycbcr_conversion != vk::SamplerYcbcrConversion::null() {
            device.destroy_sampler_ycbcr_conversion(ycbcr_conversion, None);
        }
        if dst_buffer != vk::Buffer::null() {
            device.destroy_buffer(dst_buffer, None);
        }
        if dst_buffer_memory != vk::DeviceMemory::null() {
            device.free_memory(dst_buffer_memory, None);
        }
        if query_pool != vk::QueryPool::null() {
            device.destroy_query_pool(query_pool, None);
        }
        if fence != vk::Fence::null() {
            device.destroy_fence(fence, None);
        }
        if command_pool != vk::CommandPool::null() {
            device.destroy_command_pool(command_pool, None);
        }
    }

    match probe_result {
        Ok(()) => HevcEncodeSubmitExecutionProbe::Ready {
            queue_family_index: config.queue_family_index.0,
            bitstream_buffer_offset: query_result.offset,
            dst_buffer_offset,
            bitstream_memory_offset,
            bytes_written: query_result.bytes_written,
            head16: dst_head16,
            bitstream: output_bitstream,
            bitstreams: Vec::new(),
        },
        Err(err) => {
            HevcEncodeSubmitExecutionProbe::Failed(append_hevc_encode_validation_messages(err))
        }
    }
}
