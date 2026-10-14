//! Vulkan Video decode resource allocation, command recording, submission, and readback.
//!
//! Unsafe blocks remain colocated with the ash handles whose creation, binding, submission,
//! and destruction order they enforce.

use super::*;

pub(super) fn probe_hevc_decode_submit_execution(
    device: &ash::Device,
    context: HevcDecodeSubmitExecutionContext<'_>,
) -> HevcDecodeSubmitExecutionProbe {
    let HevcDecodeSubmitExecutionContext {
        instance,
        physical_device,
        queue_family_index,
        output_format,
        capability_snapshot,
        parameter_sets,
        bitstream,
        submit_probe_access_unit_limit,
    } = context;
    let video_queue_device = ash::khr::video_queue::Device::new(instance, device);
    let video_decode_device = ash::khr::video_decode_queue::Device::new(instance, device);
    // SAFETY: `physical_device` belongs to `instance`; this only reads immutable properties.
    let physical_device_properties =
        unsafe { instance.get_physical_device_properties(physical_device) };
    let slice_segment_offset_mode =
        hevc_slice_segment_offset_mode(physical_device_properties.vendor_id);
    let use_global_src_buffer = matches!(
        slice_segment_offset_mode,
        HevcSliceSegmentOffsetMode::GlobalRbspPayload
            | HevcSliceSegmentOffsetMode::MemoryBindingAbsolute
    );
    let mut video_session = vk::VideoSessionKHR::null();
    let mut video_session_parameters = vk::VideoSessionParametersKHR::null();
    let mut session_memories = Vec::new();
    let mut src_buffer_per_au: Vec<vk::Buffer> = Vec::new();
    let mut src_buffer_memory = vk::DeviceMemory::null();
    let mut src_buffer_memories: Vec<vk::DeviceMemory> = Vec::new();
    let mut readback_buffer = vk::Buffer::null();
    let mut readback_buffer_memory = vk::DeviceMemory::null();
    let mut decode_images: Vec<vk::Image> = Vec::new();
    let mut decode_image_memories: Vec<vk::DeviceMemory> = Vec::new();
    let mut decode_image_view = vk::ImageView::null();
    let mut decode_image_views: Vec<vk::ImageView> = Vec::new();
    let mut command_pool = vk::CommandPool::null();
    let mut fence = vk::Fence::null();
    let mut readback_non_zero = false;
    let mut readback_bytes = 0_usize;
    let mut readback_planes = 0_u32;
    let mut readback_sample_stride = 0_usize;
    let mut readback_sample_count = 0_u32;
    let mut readback_sample = Vec::new();
    let mut submitted_access_units = 0_u32;
    let mut experimental_dpb_enabled = false;
    let mut experimental_dpb_mode = HevcExperimentalDpbMode::Off;
    let mut experimental_dpb_status = "mode=off (experimental DPB disabled)".to_string();
    let mut experimental_dpb_marker_path = None;
    let use_per_slot_picture_views =
        std::env::var("VIDEO_HW_VULKAN_HEVC_PER_SLOT_VIEWS").as_deref() == Ok("1");
    let use_separate_src_memory =
        std::env::var("VIDEO_HW_VULKAN_HEVC_SEPARATE_SRC_MEMORY").as_deref() == Ok("1");
    let use_scope_per_access_unit =
        std::env::var("VIDEO_HW_VULKAN_HEVC_SCOPE_PER_AU").as_deref() == Ok("1");

    let submit_result = (|| -> Result<(), String> {
        let mut decode_h265_profile = vk::VideoDecodeH265ProfileInfoKHR::default()
            .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
        let mut decode_usage = vk::VideoDecodeUsageInfoKHR::default()
            .video_usage_hints(vk::VideoDecodeUsageFlagsKHR::DEFAULT);
        let profile = vk::VideoProfileInfoKHR::default()
            .video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_H265)
            .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
            .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
            .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
            .push_next(&mut decode_h265_profile)
            .push_next(&mut decode_usage);
        let session_max_dpb_slots = hevc_decode_session_limit_from_env(
            "VIDEO_HW_VULKAN_HEVC_DECODE_SESSION_DPB_SLOTS",
            capability_snapshot.max_dpb_slots,
        );
        let session_max_active_reference_pictures = hevc_decode_session_limit_from_env_allow_zero(
            "VIDEO_HW_VULKAN_HEVC_DECODE_SESSION_ACTIVE_REFS",
            capability_snapshot.max_active_reference_pictures,
        )
        .min(session_max_dpb_slots);
        let create_info = vk::VideoSessionCreateInfoKHR::default()
            .queue_family_index(queue_family_index.0)
            .video_profile(&profile)
            .picture_format(output_format)
            .max_coded_extent(vk::Extent2D {
                width: parameter_sets.coded_width,
                height: parameter_sets.coded_height,
            })
            .reference_picture_format(output_format)
            .max_dpb_slots(session_max_dpb_slots)
            .max_active_reference_pictures(session_max_active_reference_pictures)
            .std_header_version(&capability_snapshot.std_header_version);
        // SAFETY: All pointers in `create_info` reference stack data valid for this call.
        let session_create_result = unsafe {
            (video_queue_device.fp().create_video_session_khr)(
                device.handle(),
                &create_info,
                std::ptr::null(),
                &mut video_session,
            )
        };
        if session_create_result != vk::Result::SUCCESS {
            return Err(format!(
                "vkCreateVideoSessionKHR for submit execution probe failed: {session_create_result:?}"
            ));
        }

        // SAFETY: `physical_device` belongs to `instance`; we only read immutable memory properties.
        let memory_properties =
            unsafe { instance.get_physical_device_memory_properties(physical_device) };

        let mut session_requirement_count = 0_u32;
        // SAFETY: First query requests only the count for a valid video session handle.
        let session_requirement_count_result = unsafe {
            (video_queue_device
                .fp()
                .get_video_session_memory_requirements_khr)(
                device.handle(),
                video_session,
                &mut session_requirement_count,
                std::ptr::null_mut(),
            )
        };
        if session_requirement_count_result != vk::Result::SUCCESS {
            return Err(format!(
                "vkGetVideoSessionMemoryRequirementsKHR count query failed: {session_requirement_count_result:?}"
            ));
        }
        if session_requirement_count == 0 {
            return Err(
                "vkGetVideoSessionMemoryRequirementsKHR returned no memory requirements"
                    .to_string(),
            );
        }

        let mut session_requirements = vec![
            vk::VideoSessionMemoryRequirementsKHR::default();
            session_requirement_count as usize
        ];
        // SAFETY: `session_requirements` storage is sized from the count query above.
        let session_requirements_result = unsafe {
            (video_queue_device
                .fp()
                .get_video_session_memory_requirements_khr)(
                device.handle(),
                video_session,
                &mut session_requirement_count,
                session_requirements.as_mut_ptr(),
            )
        };
        if session_requirements_result != vk::Result::SUCCESS {
            return Err(format!(
                "vkGetVideoSessionMemoryRequirementsKHR query failed: {session_requirements_result:?}"
            ));
        }

        let mut session_bindings = Vec::with_capacity(session_requirements.len());
        for requirement in &session_requirements {
            let requirement_info = requirement.memory_requirements;
            let memory_type_index = select_memory_type_index(
                &memory_properties,
                requirement_info.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .or_else(|| {
                select_memory_type_index(
                    &memory_properties,
                    requirement_info.memory_type_bits,
                    vk::MemoryPropertyFlags::empty(),
                )
            })
            .ok_or_else(|| {
                format!(
                    "no compatible memory type for video session bind index {} (bits=0x{:X})",
                    requirement.memory_bind_index, requirement_info.memory_type_bits
                )
            })?;
            let allocation_size = requirement_info.size.max(1);
            let allocate_info = vk::MemoryAllocateInfo::default()
                .allocation_size(allocation_size)
                .memory_type_index(memory_type_index);
            // SAFETY: Allocation info references only POD values and `device` is valid.
            let memory = unsafe { device.allocate_memory(&allocate_info, None) }
                .map_err(|err| format!("vkAllocateMemory for video session bind failed: {err}"))?;
            session_memories.push(memory);
            session_bindings.push(
                vk::BindVideoSessionMemoryInfoKHR::default()
                    .memory_bind_index(requirement.memory_bind_index)
                    .memory(memory)
                    .memory_offset(0)
                    .memory_size(allocation_size),
            );
        }
        // SAFETY: bind infos reference memory allocations created above and live through the call.
        let bind_session_memory_result = unsafe {
            (video_queue_device.fp().bind_video_session_memory_khr)(
                device.handle(),
                video_session,
                u32::try_from(session_bindings.len())
                    .map_err(|_| "session binding count exceeds u32 range".to_string())?,
                session_bindings.as_ptr(),
            )
        };
        if bind_session_memory_result != vk::Result::SUCCESS {
            return Err(format!(
                "vkBindVideoSessionMemoryKHR failed: {bind_session_memory_result:?}"
            ));
        }

        video_session_parameters = create_video_session_parameters(
            device,
            &video_queue_device,
            video_session,
            parameter_sets,
        )?;

        let src_offset_alignment = capability_snapshot
            .min_bitstream_buffer_offset_alignment
            .max(1);
        let src_size_alignment = capability_snapshot
            .min_bitstream_buffer_size_alignment
            .max(1);
        let payload = build_hevc_submit_probe_bitstream_payload(
            parameter_sets,
            bitstream,
            submit_probe_access_unit_limit,
            src_offset_alignment,
            src_size_alignment,
        )?;
        // SAFETY: the device was created with this queue family and queue index zero.
        let queue = unsafe { device.get_device_queue(queue_family_index.0, 0) };
        if queue == vk::Queue::null() {
            return Err(format!(
                "vkGetDeviceQueue returned null queue for family {}",
                queue_family_index.0
            ));
        }

        if payload.bytes.is_empty() {
            return Err("submit probe payload produced an empty bitstream buffer".to_string());
        }

        // Create one VkBuffer per AU, each covering only that AU's bitstream data.
        //
        // NVIDIA (vendor 0x10DE) ignores `srcBufferOffset` and scans the buffer from byte 0 for
        // the first start code, regardless of `pSliceSegmentOffsets`. Splitting into per-AU
        // buffers ensures each buffer contains only one AU's bitstream so the driver cannot
        // accidentally decode a different AU. The per-AU buffers are all bound to a single
        // shared VkDeviceMemory allocation at their respective `buffer_offset` positions, which
        // preserves the existing memory layout without additional copies.
        //
        // `global` diagnostics use one buffer covering the whole prepared bitstream; normal mode
        // keeps one buffer per AU. VkVideoProfileListInfoKHR is required by spec for buffers with
        // VIDEO_DECODE_SRC_KHR.
        let src_buffer_sizes = if use_global_src_buffer {
            vec![
                u64::try_from(payload.bytes.len())
                    .map_err(|_| "submit probe payload size exceeds u64 range".to_string())?,
            ]
        } else {
            payload
                .access_units
                .iter()
                .map(|au| au.buffer_range)
                .collect::<Vec<_>>()
        };
        for src_buffer_size in src_buffer_sizes {
            let mut per_au_h265_profile = vk::VideoDecodeH265ProfileInfoKHR::default()
                .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
            let mut per_au_decode_usage = vk::VideoDecodeUsageInfoKHR::default()
                .video_usage_hints(vk::VideoDecodeUsageFlagsKHR::DEFAULT);
            let per_au_video_profile = vk::VideoProfileInfoKHR::default()
                .video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_H265)
                .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
                .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
                .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
                .push_next(&mut per_au_h265_profile)
                .push_next(&mut per_au_decode_usage);
            let per_au_video_profiles = [per_au_video_profile];
            let mut per_au_profile_list =
                vk::VideoProfileListInfoKHR::default().profiles(&per_au_video_profiles);
            let per_au_create_info = vk::BufferCreateInfo::default()
                .size(src_buffer_size)
                .usage(vk::BufferUsageFlags::VIDEO_DECODE_SRC_KHR)
                .sharing_mode(vk::SharingMode::EXCLUSIVE)
                .push_next(&mut per_au_profile_list);
            // SAFETY: `per_au_create_info` is fully initialized and `device` is valid.
            let per_au_buf =
                unsafe { device.create_buffer(&per_au_create_info, None) }.map_err(|err| {
                    format!("vkCreateBuffer for per-AU bitstream buffer failed: {err}")
                })?;
            src_buffer_per_au.push(per_au_buf);
        }

        // Gather memory requirements from all per-AU buffers so the allocation covers every
        // buffer's requirement. All VIDEO_DECODE_SRC_KHR buffers share the same memory type
        // bits and alignment on a given device, but we verify this explicitly.
        let per_au_requirements: Vec<vk::MemoryRequirements> = src_buffer_per_au
            .iter()
            // SAFETY: each buffer was just created from `device` and has not been bound yet.
            .map(|&buf| unsafe { device.get_buffer_memory_requirements(buf) })
            .collect();
        // Verify binding-offset alignment: au.buffer_offset must be aligned to both
        // minBitstreamBufferOffsetAlignment AND the buffer's memoryRequirements.alignment.
        // If they differ, the binding could be misaligned and NVIDIA would silently produce zeros.
        if !use_global_src_buffer && !use_separate_src_memory {
            for (i, (au, req)) in payload
                .access_units
                .iter()
                .zip(per_au_requirements.iter())
                .enumerate()
            {
                if req.alignment > 0 && au.buffer_offset % req.alignment != 0 {
                    return Err(format!(
                        "per-AU buffer[{i}] binding offset {} is not aligned to memoryRequirements.alignment {} — \
                         increase bitstream buffer offset alignment to fix P-frame decode failures",
                        au.buffer_offset, req.alignment
                    ));
                }
            }
        }
        let src_memory_type_bits = per_au_requirements
            .iter()
            .fold(u32::MAX, |acc, r| acc & r.memory_type_bits);
        if src_memory_type_bits == 0 {
            return Err("no common memory type across per-AU bitstream buffers".to_string());
        }
        let src_memory_type_index = select_memory_type_index(
            &memory_properties,
            src_memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or_else(|| {
            format!(
                "no HOST_VISIBLE|HOST_COHERENT memory type for per-AU bitstream buffers (bits=0x{src_memory_type_bits:X})"
            )
        })?;
        if use_separate_src_memory {
            for ((per_au_buf, au), req) in src_buffer_per_au
                .iter()
                .zip(payload.access_units.iter())
                .zip(per_au_requirements.iter())
            {
                let src_start = usize::try_from(au.buffer_offset)
                    .map_err(|_| "AU source offset exceeds usize".to_string())?;
                let src_len = usize::try_from(au.vcl_size)
                    .map_err(|_| "AU source size exceeds usize".to_string())?;
                let src_end = src_start
                    .checked_add(src_len)
                    .ok_or_else(|| "AU source end overflow".to_string())?;
                let source = payload.bytes.get(src_start..src_end).ok_or_else(|| {
                    "AU source range is outside prepared bitstream payload".to_string()
                })?;
                let allocate_info = vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size.max(au.buffer_range))
                    .memory_type_index(src_memory_type_index);
                // SAFETY: allocation info comes from this buffer's Vulkan memory requirements.
                let memory =
                    unsafe { device.allocate_memory(&allocate_info, None) }.map_err(|err| {
                        format!("vkAllocateMemory for separate HEVC bitstream buffer failed: {err}")
                    })?;
                // Track the allocation immediately so the common cleanup path owns it even when
                // binding or mapping fails.
                src_buffer_memories.push(memory);
                // SAFETY: buffer and fresh allocation belong to this device; offset zero satisfies
                // the allocation's alignment requirement.
                unsafe { device.bind_buffer_memory(*per_au_buf, memory, 0) }.map_err(|err| {
                    format!("vkBindBufferMemory for separate HEVC bitstream buffer failed: {err}")
                })?;
                // SAFETY: allocation is host-visible/coherent and `au.buffer_range` is no larger
                // than the requested allocation size.
                let mapped = unsafe {
                    device.map_memory(memory, 0, au.buffer_range, vk::MemoryMapFlags::empty())
                }
                .map_err(|err| {
                    format!("vkMapMemory for separate HEVC bitstream buffer failed: {err}")
                })?;
                // SAFETY: `source` is bounded above, and the mapped range is at least
                // `au.buffer_range >= source.len()` bytes. The regions do not overlap.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        source.as_ptr(),
                        mapped.cast::<u8>(),
                        source.len(),
                    );
                    device.unmap_memory(memory);
                }
            }
        } else {
            let src_payload_size = u64::try_from(payload.bytes.len())
                .map_err(|_| "submit probe payload size exceeds u64 range".to_string())?;
            let src_allocation_size = if use_global_src_buffer {
                per_au_requirements
                    .first()
                    .map(|req| req.size)
                    .unwrap_or(0)
                    .max(src_payload_size)
            } else {
                payload
                    .access_units
                    .iter()
                    .zip(per_au_requirements.iter())
                    .map(|(au, req)| au.buffer_offset.saturating_add(req.size))
                    .max()
                    .unwrap_or(0)
                    .max(src_payload_size)
            };
            let src_allocate_info = vk::MemoryAllocateInfo::default()
                .allocation_size(src_allocation_size)
                .memory_type_index(src_memory_type_index);
            // SAFETY: allocation info references only POD values and `device` is valid.
            src_buffer_memory = unsafe { device.allocate_memory(&src_allocate_info, None) }
                .map_err(|err| {
                    format!("vkAllocateMemory for per-AU bitstream buffers failed: {err}")
                })?;

            if use_global_src_buffer {
                // SAFETY: buffer and memory were created from the same logical device.
                unsafe { device.bind_buffer_memory(src_buffer_per_au[0], src_buffer_memory, 0) }
                    .map_err(|err| {
                        format!("vkBindBufferMemory for HEVC bitstream buffer failed: {err}")
                    })?;
            } else {
                for (per_au_buf, au) in src_buffer_per_au.iter().zip(payload.access_units.iter()) {
                    // SAFETY: buffer and memory were created from the same logical device.
                    unsafe {
                        device.bind_buffer_memory(*per_au_buf, src_buffer_memory, au.buffer_offset)
                    }
                    .map_err(|err| {
                        format!(
                            "vkBindBufferMemory for per-AU bitstream buffer (off={}) failed: {err}",
                            au.buffer_offset
                        )
                    })?;
                }
            }

            let src_alloc_map_size = u64::try_from(payload.bytes.len())
                .map_err(|_| "submit probe payload size exceeds u64 range".to_string())?;
            // SAFETY: mapping range is within the allocated per-AU source memory.
            let mapped = unsafe {
                device.map_memory(
                    src_buffer_memory,
                    0,
                    src_alloc_map_size,
                    vk::MemoryMapFlags::empty(),
                )
            }
            .map_err(|err| format!("vkMapMemory for per-AU bitstream buffers failed: {err}"))?;
            // SAFETY: destination pointer is valid for `payload.bytes.len()` bytes from `vkMapMemory`.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    payload.bytes.as_ptr(),
                    mapped.cast::<u8>(),
                    payload.bytes.len(),
                );
                device.unmap_memory(src_buffer_memory);
            }
        }
        let coded_extent = vk::Extent2D {
            width: parameter_sets.coded_width,
            height: parameter_sets.coded_height,
        };
        let (single_readback_buffer_size, readback_regions_template) =
            build_decode_readback_regions(output_format, coded_extent.width, coded_extent.height)?;
        readback_planes = u32::try_from(readback_regions_template.len()).unwrap_or(u32::MAX);
        readback_sample_stride = usize::try_from(single_readback_buffer_size)
            .map_err(|_| "decode readback sample size exceeds usize range".to_string())?;
        readback_sample_count = u32::try_from(payload.access_units.len()).unwrap_or(u32::MAX);
        let readback_buffer_size = single_readback_buffer_size
            .checked_mul(u64::from(readback_sample_count))
            .ok_or_else(|| "decode readback buffer size overflow".to_string())?;
        let dpb_configuration = configure_hevc_experimental_dpb();
        experimental_dpb_enabled = dpb_configuration.enabled;
        experimental_dpb_mode = dpb_configuration.mode;
        experimental_dpb_status = dpb_configuration.status;
        experimental_dpb_marker_path = dpb_configuration.marker_path;
        let dpb_slot_count = if experimental_dpb_enabled {
            usize::try_from(capability_snapshot.max_dpb_slots.max(1))
                .ok()
                .unwrap_or(1)
        } else {
            1
        };
        let image_extent_width = u32::try_from(align_up(
            u64::from(coded_extent.width),
            u64::from(capability_snapshot.picture_access_granularity.width.max(1)),
        ))
        .map_err(|_| "aligned decode image width exceeds u32 range".to_string())?;
        let image_extent_height = u32::try_from(align_up(
            u64::from(coded_extent.height),
            u64::from(capability_snapshot.picture_access_granularity.height.max(1)),
        ))
        .map_err(|_| "aligned decode image height exceeds u32 range".to_string())?;
        // All DPB slots share one VkImage with dpb_slot_count array layers. NVIDIA RTX 3080
        // (COINCIDE-only) requires all DPB slots to reside in a single image so the driver can
        // track inter-slot reference data correctly; using separate single-layer images causes all
        // non-IDR frames to produce all-zero output.
        // VkVideoProfileListInfoKHR is required in pNext for images with VIDEO_DECODE_DST/DPB usage.
        let mut image_decode_h265_profile_ext = vk::VideoDecodeH265ProfileInfoKHR::default()
            .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
        let mut image_decode_usage_ext = vk::VideoDecodeUsageInfoKHR::default()
            .video_usage_hints(vk::VideoDecodeUsageFlagsKHR::DEFAULT);
        let image_video_profile = vk::VideoProfileInfoKHR::default()
            .video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_H265)
            .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
            .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
            .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
            .push_next(&mut image_decode_h265_profile_ext)
            .push_next(&mut image_decode_usage_ext);
        let image_video_profiles = [image_video_profile];
        let mut image_profile_list =
            vk::VideoProfileListInfoKHR::default().profiles(&image_video_profiles);
        let decode_image_create_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(output_format)
            .extent(vk::Extent3D {
                width: image_extent_width,
                height: image_extent_height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(dpb_slot_count as u32)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(
                vk::ImageUsageFlags::VIDEO_DECODE_DST_KHR
                    | vk::ImageUsageFlags::VIDEO_DECODE_DPB_KHR
                    | vk::ImageUsageFlags::TRANSFER_SRC,
            )
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut image_profile_list);
        // SAFETY: image create info is fully initialized and device is valid.
        let dpb_image = unsafe { device.create_image(&decode_image_create_info, None) }
            .map_err(|err| format!("vkCreateImage for DPB image failed: {err}"))?;
        // SAFETY: image handle was created from `device`.
        let dpb_requirements = unsafe { device.get_image_memory_requirements(dpb_image) };
        let dpb_memory_type = select_memory_type_index(
            &memory_properties,
            dpb_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .or_else(|| {
            select_memory_type_index(
                &memory_properties,
                dpb_requirements.memory_type_bits,
                vk::MemoryPropertyFlags::empty(),
            )
        })
        .ok_or_else(|| {
            format!(
                "no compatible memory type for DPB image (bits=0x{:X})",
                dpb_requirements.memory_type_bits
            )
        })?;
        let dpb_alloc_info = vk::MemoryAllocateInfo::default()
            .allocation_size(dpb_requirements.size.max(1))
            .memory_type_index(dpb_memory_type);
        // SAFETY: allocation parameters are provided by Vulkan memory requirements.
        let dpb_memory = unsafe { device.allocate_memory(&dpb_alloc_info, None) }
            .map_err(|err| format!("vkAllocateMemory for DPB image failed: {err}"))?;
        // SAFETY: image and memory were created by the same logical device.
        unsafe { device.bind_image_memory(dpb_image, dpb_memory, 0) }
            .map_err(|err| format!("vkBindImageMemory for DPB image failed: {err}"))?;
        decode_images.push(dpb_image);
        decode_image_memories.push(dpb_memory);
        let decode_image_view_create_info = vk::ImageViewCreateInfo::default()
            .image(dpb_image)
            .view_type(vk::ImageViewType::TYPE_2D_ARRAY)
            .format(output_format)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: dpb_slot_count as u32,
            });
        // SAFETY: image view create info references a valid image and subresource range.
        decode_image_view =
            unsafe { device.create_image_view(&decode_image_view_create_info, None) }
                .map_err(|err| format!("vkCreateImageView for DPB slots failed: {err}"))?;
        if decode_image_view == vk::ImageView::null() {
            return Err(
                "submit execution probe could not create any decode image views".to_string(),
            );
        }
        if use_per_slot_picture_views {
            decode_image_views.reserve(dpb_slot_count);
            for slot in 0..dpb_slot_count {
                let view_create_info = vk::ImageViewCreateInfo::default()
                    .image(dpb_image)
                    .view_type(vk::ImageViewType::TYPE_2D_ARRAY)
                    .format(output_format)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: slot as u32,
                        layer_count: 1,
                    });
                // SAFETY: the image is live and `slot` is below its u32-derived array-layer count.
                let view = unsafe { device.create_image_view(&view_create_info, None) }.map_err(
                    |err| format!("vkCreateImageView for DPB slot {slot} failed: {err}"),
                )?;
                decode_image_views.push(view);
            }
        }
        let readback_buffer_create_info = vk::BufferCreateInfo::default()
            .size(readback_buffer_size)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: `readback_buffer_create_info` is fully initialized and `device` is valid.
        readback_buffer = unsafe { device.create_buffer(&readback_buffer_create_info, None) }
            .map_err(|err| format!("vkCreateBuffer for decode readback failed: {err}"))?;
        // SAFETY: readback buffer was created from the same logical device.
        let readback_buffer_requirements =
            unsafe { device.get_buffer_memory_requirements(readback_buffer) };
        let readback_memory_type_index = select_memory_type_index(
            &memory_properties,
            readback_buffer_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or_else(|| {
            format!(
                "no HOST_VISIBLE|HOST_COHERENT memory type for decode readback buffer (bits=0x{:X})",
                readback_buffer_requirements.memory_type_bits
            )
        })?;
        let readback_allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(readback_buffer_requirements.size.max(readback_buffer_size))
            .memory_type_index(readback_memory_type_index);
        // SAFETY: allocation info references only POD values and `device` is valid.
        readback_buffer_memory = unsafe { device.allocate_memory(&readback_allocate_info, None) }
            .map_err(|err| {
            format!("vkAllocateMemory for decode readback buffer failed: {err}")
        })?;
        // SAFETY: buffer and memory were created by the same logical device.
        unsafe { device.bind_buffer_memory(readback_buffer, readback_buffer_memory, 0) }.map_err(
            |err| format!("vkBindBufferMemory for decode readback buffer failed: {err}"),
        )?;

        let command_pool_info =
            vk::CommandPoolCreateInfo::default().queue_family_index(queue_family_index.0);
        // SAFETY: command pool create info is valid for the selected queue family.
        command_pool = unsafe { device.create_command_pool(&command_pool_info, None) }
            .map_err(|err| format!("vkCreateCommandPool failed: {err}"))?;
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: allocate info references a valid command pool.
        let command_buffers = unsafe { device.allocate_command_buffers(&allocate_info) }
            .map_err(|err| format!("vkAllocateCommandBuffers failed: {err}"))?;
        let command_buffer = command_buffers.first().copied().ok_or_else(|| {
            "vkAllocateCommandBuffers returned no command buffers for submit execution probe"
                .to_string()
        })?;

        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: command buffer is valid and not already recording.
        unsafe { device.begin_command_buffer(command_buffer, &begin_info) }
            .map_err(|err| format!("vkBeginCommandBuffer failed: {err}"))?;

        let source_memory_barrier = vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::HOST)
            .src_access_mask(vk::AccessFlags2::HOST_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
            .dst_access_mask(vk::AccessFlags2::VIDEO_DECODE_READ_KHR);
        // Transition all array layers of the single DPB image from UNDEFINED to
        // VIDEO_DECODE_DPB_KHR in one barrier. COINCIDE mode keeps all slots in DPB_KHR layout.
        // Use DST_KHR as the working layout for the single array-layer DPB image.  Although
        // the spec recommends DPB_KHR for COINCIDE mode, NVIDIA RTX 3080 requires DST_KHR
        // for the decode to produce non-zero output for non-IDR frames.
        let decode_working_layout =
            if std::env::var("VIDEO_HW_VULKAN_HEVC_USE_DPB_LAYOUT").as_deref() == Ok("1") {
                vk::ImageLayout::VIDEO_DECODE_DPB_KHR
            } else {
                vk::ImageLayout::VIDEO_DECODE_DST_KHR
            };
        let dpb_init_barrier = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::NONE)
            .src_access_mask(vk::AccessFlags2::NONE)
            .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
            .dst_access_mask(vk::AccessFlags2::VIDEO_DECODE_WRITE_KHR)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(decode_working_layout)
            .image(decode_images[0])
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: dpb_slot_count as u32,
            });
        let destination_image_barriers = [dpb_init_barrier];
        let dependency_info = vk::DependencyInfo::default()
            .memory_barriers(std::slice::from_ref(&source_memory_barrier))
            .image_memory_barriers(&destination_image_barriers);
        // SAFETY: barriers reference resources created above and the command buffer is recording.
        unsafe {
            device.cmd_pipeline_barrier2(command_buffer, &dependency_info);
        }

        let mut next_reference_slot = 0_usize;
        let mut active_reference_slots = Vec::new();
        // Tracks which DPB slots currently hold a reference picture (VIDEO_DECODE_DST_KHR layout).
        // All slots start in DST layout (true) when DPB is enabled; false = DST layout (non-COINCIDE).
        let mut slot_in_dpb_layout = vec![experimental_dpb_enabled; dpb_slot_count];
        // Per-slot deferred readback: tracks the BufferImageCopy regions for the reference frame
        // currently occupying each DPB slot.  Reference frame copies are deferred to just before
        // slot reuse (eviction-triggered, inside the scope) or until after cmd_end_video_coding_khr
        // (post-scope batch copy).  Deferring avoids VIDEO_DECODE_DST_KHR → TRANSFER_SRC_OPTIMAL
        // → VIDEO_DECODE_DST_KHR layout cycles on actively-referenced slots, which appear to
        // invalidate NVIDIA RTX 3080's internal DPB cache and cause subsequent P-frame decodes to
        // produce all-zero output.  Slot 0 (non-reference scratch) is never stored here since
        // non-reference frames are copied immediately in the non-reference branch below.
        let mut per_slot_deferred: Vec<Option<Vec<vk::BufferImageCopy>>> =
            vec![None; dpb_slot_count];
        // Cap active references at the SPS DPB limit.  The syntax element is a "minus1" value:
        // `sps_max_dec_pic_buffering_minus1 + 1` pictures may be held in the DPB.
        let max_active_reference_slots = if experimental_dpb_enabled {
            let highest_tid = usize::from(parameter_sets.parsed_sps.sps_max_sub_layers_minus1);
            let sps_max = parameter_sets
                .parsed_sps
                .sub_layer_ordering_info
                .sps_max_dec_pic_buffering_minus1
                .get(highest_tid)
                .copied()
                .unwrap_or(4);
            usize::try_from(sps_max).unwrap_or(4).saturating_add(1)
        } else {
            0
        };
        let max_poc_lsb = i32::try_from(parameter_sets.parsed_sps.max_pic_order_cnt_lsb())
            .unwrap_or(i32::MAX / 2);
        // Open the single video coding scope.  Vulkan spec requires RESET to be issued before the
        // first decode command in a new session.  We issue it unconditionally here.
        //
        // Vulkan spec VUID-07264 / VUID-07144: every DPB slot used as pSetupReferenceSlot or in
        // pReferenceSlots during vkCmdDecodeVideoKHR MUST be bound in vkBeginVideoCodingKHR.
        // NVIDIA RTX 3080 silently returns zero-filled output if slots are not bound here, even
        // though the spec permits mid-scope slot activation via pSetupReferenceSlot.  Binding all
        // dpb_slot_count slots upfront ensures every slot is usable throughout the scope.
        let begin_coding_picture_resources: Vec<vk::VideoPictureResourceInfoKHR<'_>> =
            if experimental_dpb_enabled {
                (0..dpb_slot_count)
                    .map(|s| {
                        let (view, layer) = if use_per_slot_picture_views {
                            (decode_image_views[s], 0)
                        } else {
                            (decode_image_view, s as u32)
                        };
                        vk::VideoPictureResourceInfoKHR::default()
                            .coded_offset(vk::Offset2D { x: 0, y: 0 })
                            .coded_extent(coded_extent)
                            .base_array_layer(layer)
                            .image_view_binding(view)
                    })
                    .collect()
            } else {
                Vec::new()
            };
        let mut begin_coding_reference_info_flags = empty_decode_h265_reference_info_flags();
        begin_coding_reference_info_flags.set_used_for_long_term_reference(0);
        begin_coding_reference_info_flags.set_unused_for_reference(1);
        let begin_coding_reference_info_values = vec![
            StdVideoDecodeH265ReferenceInfo {
                flags: begin_coding_reference_info_flags,
                PicOrderCntVal: 0,
            };
            begin_coding_picture_resources.len()
        ];
        let mut begin_coding_dpb_slot_infos = begin_coding_reference_info_values
            .iter()
            .map(|reference_info| {
                vk::VideoDecodeH265DpbSlotInfoKHR::default().std_reference_info(reference_info)
            })
            .collect::<Vec<_>>();
        let mut begin_coding_reference_slots =
            Vec::with_capacity(begin_coding_picture_resources.len());
        for ((s, resource), dpb_slot_info) in begin_coding_picture_resources
            .iter()
            .enumerate()
            .zip(begin_coding_dpb_slot_infos.iter_mut())
        {
            let slot = vk::VideoReferenceSlotInfoKHR::default()
                .slot_index(i32::try_from(s).unwrap_or(-1))
                .picture_resource(resource);
            begin_coding_reference_slots.push(slot.push_next(dpb_slot_info));
        }
        let begin_coding_info = vk::VideoBeginCodingInfoKHR::default()
            .video_session(video_session)
            .video_session_parameters(video_session_parameters)
            .reference_slots(&begin_coding_reference_slots);
        // SAFETY: command buffer is recording; session and parameters are valid.
        unsafe {
            (video_queue_device.fp().cmd_begin_video_coding_khr)(
                command_buffer,
                &begin_coding_info,
            );
        }
        // Issue mandatory codec reset before the first decode command in this fresh session.
        // The spec requires RESET to precede all decode commands in a newly-created session.
        let coding_control_reset =
            vk::VideoCodingControlInfoKHR::default().flags(vk::VideoCodingControlFlagsKHR::RESET);
        // SAFETY: command buffer is in a video coding scope.
        unsafe {
            (video_queue_device.fp().cmd_control_video_coding_khr)(
                command_buffer,
                &coding_control_reset,
            );
        }
        for (submitted_index, access_unit) in payload.access_units.iter().enumerate() {
            let is_irap = (16..=23).contains(&access_unit.header.nal_unit_type);
            // IDR (19,20) and BLA (16-18) always reset the DPB. CRA (21) and reserved IRAP
            // (22-23) preserve pre-IRAP references so that RASL pictures can use them when
            // decoding from stream start (NoRaslOutputFlag = 0 for CRA in this scenario).
            let is_dpb_reset = matches!(access_unit.header.nal_unit_type, 16..=20);
            if experimental_dpb_enabled && is_dpb_reset {
                active_reference_slots.clear();
            }
            let is_reference = experimental_dpb_enabled
                && is_hevc_reference_nal_type(access_unit.header.nal_unit_type);
            // Use the full (unwrapped) POC computed during payload building. Resolve the
            // references before choosing the destination slot so the allocator can avoid
            // overwriting a picture that this command is about to read.
            let current_pic_order_cnt_val = access_unit.header.poc_full;
            let selected_references = if experimental_dpb_enabled && !is_irap {
                if let Some(ref inline_pocs) = access_unit.inline_short_term_ref_pic_set_pocs {
                    select_hevc_references_by_inline_poc(&active_reference_slots, inline_pocs)
                } else {
                    let count_limits = resolve_hevc_ref_pic_set_usage_limits(
                        &parameter_sets.parsed_sps,
                        access_unit.short_term_ref_pic_set_idx,
                    )
                    .or_else(|| {
                        resolve_hevc_slice_type_reference_usage_limits(
                            &payload.parsed_pps,
                            access_unit.slice_type,
                        )
                    });
                    let candidates = active_reference_slots
                        .iter()
                        .rev()
                        .take(HEVC_REF_PIC_SET_LIST_SIZE)
                        .copied()
                        .collect::<Vec<_>>();
                    if let Some((max_before, max_after)) = count_limits {
                        select_hevc_references_for_ref_pic_set(
                            &candidates,
                            current_pic_order_cnt_val,
                            max_before.min(HEVC_REF_PIC_SET_LIST_SIZE),
                            max_after.min(HEVC_REF_PIC_SET_LIST_SIZE),
                        )
                    } else {
                        candidates
                    }
                }
            } else {
                Vec::new()
            };
            let slot = select_hevc_decode_dpb_slot(
                experimental_dpb_enabled,
                is_reference,
                dpb_slot_count,
                &mut next_reference_slot,
                &active_reference_slots,
                &selected_references,
            );
            if experimental_dpb_enabled {
                // Eviction-triggered copy: if this slot is currently occupied by a reference frame
                // whose readback was deferred, flush the copy now before the new decode overwrites
                // the slot.  The old occupant has been dropped from active_reference_slots (just
                // below), so no future decode will use it as a reference; the DST_KHR →
                // TRANSFER_SRC_OPTIMAL → DST_KHR cycle here cannot invalidate any live DPB entry.
                if let Some(evicted_regions) = per_slot_deferred[slot].take() {
                    let evict_to_transfer = vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
                        .src_access_mask(
                            vk::AccessFlags2::VIDEO_DECODE_WRITE_KHR
                                | vk::AccessFlags2::VIDEO_DECODE_READ_KHR,
                        )
                        .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                        .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                        .old_layout(decode_working_layout)
                        .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                        .image(decode_images[0])
                        .subresource_range(vk::ImageSubresourceRange {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            base_mip_level: 0,
                            level_count: 1,
                            base_array_layer: slot as u32,
                            layer_count: 1,
                        });
                    let evict_to_decode = vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                        .src_access_mask(vk::AccessFlags2::TRANSFER_READ)
                        .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
                        .dst_access_mask(vk::AccessFlags2::VIDEO_DECODE_WRITE_KHR)
                        .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                        .new_layout(decode_working_layout)
                        .image(decode_images[0])
                        .subresource_range(vk::ImageSubresourceRange {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            base_mip_level: 0,
                            level_count: 1,
                            base_array_layer: slot as u32,
                            layer_count: 1,
                        });
                    // SAFETY: barriers and copy use valid resources; the slot's old data is
                    // being evicted (no longer actively referenced) before the new decode.
                    unsafe {
                        device.cmd_pipeline_barrier2(
                            command_buffer,
                            &vk::DependencyInfo::default()
                                .image_memory_barriers(std::slice::from_ref(&evict_to_transfer)),
                        );
                        device.cmd_copy_image_to_buffer(
                            command_buffer,
                            decode_images[0],
                            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                            readback_buffer,
                            &evicted_regions,
                        );
                        device.cmd_pipeline_barrier2(
                            command_buffer,
                            &vk::DependencyInfo::default()
                                .image_memory_barriers(std::slice::from_ref(&evict_to_decode)),
                        );
                    }
                }
                active_reference_slots.retain(|entry| entry.slot != slot);
            }
            // Non-COINCIDE mode only: transition a reference slot back from DPB_KHR to DST_KHR
            // before decoding into it again.  In experimental (COINCIDE) mode, all images stay
            // in DPB_KHR; decoding into DPB_KHR is valid in COINCIDE (DPB = decode-dst), so no
            // layout transition is needed.
            if slot_in_dpb_layout[slot] && !experimental_dpb_enabled {
                let dpb_to_dst_barrier = vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
                    .src_access_mask(vk::AccessFlags2::VIDEO_DECODE_READ_KHR)
                    .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
                    .dst_access_mask(vk::AccessFlags2::VIDEO_DECODE_WRITE_KHR)
                    .old_layout(vk::ImageLayout::VIDEO_DECODE_DPB_KHR)
                    .new_layout(vk::ImageLayout::VIDEO_DECODE_DST_KHR)
                    .image(decode_images[0])
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: slot as u32,
                        layer_count: 1,
                    });
                let dpb_to_dst_dep = vk::DependencyInfo::default()
                    .image_memory_barriers(std::slice::from_ref(&dpb_to_dst_barrier));
                // SAFETY: image barrier transitions a subresource in DPB layout to DST layout.
                unsafe {
                    device.cmd_pipeline_barrier2(command_buffer, &dpb_to_dst_dep);
                }
                slot_in_dpb_layout[slot] = false;
            }
            // IRAP frames (IDR, BLA) reset DPB state; active_reference_slots was already
            // cleared before the loop body.
            let (destination_image_view, destination_base_array_layer) =
                if use_per_slot_picture_views {
                    (decode_image_views[slot], 0)
                } else {
                    (decode_image_view, slot as u32)
                };
            let destination_picture_resource = vk::VideoPictureResourceInfoKHR::default()
                .coded_offset(vk::Offset2D { x: 0, y: 0 })
                .coded_extent(coded_extent)
                .base_array_layer(destination_base_array_layer)
                .image_view_binding(destination_image_view);
            let mut std_picture_info_flags = empty_decode_h265_picture_info_flags();
            std_picture_info_flags.set_IrapPicFlag(bool_to_u32(is_irap));
            std_picture_info_flags.set_IdrPicFlag(bool_to_u32(is_hevc_idr_nal_type(
                access_unit.header.nal_unit_type,
            )));
            std_picture_info_flags.set_IsReference(bool_to_u32(is_reference));
            // current_pic_order_cnt_val is declared earlier in the loop (before begin_coding)
            // so that bc_std_ref_infos for the setup slot can carry the correct POC.
            std_picture_info_flags.set_short_term_ref_pic_set_sps_flag(bool_to_u32(
                access_unit.short_term_ref_pic_set_idx.is_some(),
            ));
            let (
                reference_pic_set_st_curr_before,
                reference_pic_set_st_curr_after,
                _num_delta_pocs_total,
            ) = build_hevc_ref_pic_set_lists(
                &selected_references,
                current_pic_order_cnt_val,
                max_poc_lsb,
            );
            let std_picture_info = StdVideoDecodeH265PictureInfo {
                flags: std_picture_info_flags,
                sps_video_parameter_set_id: parameter_sets.parsed_sps.sps_video_parameter_set_id,
                pps_seq_parameter_set_id: payload.parsed_pps.pps_seq_parameter_set_id,
                pps_pic_parameter_set_id: access_unit.header.pps_id,
                NumDeltaPocsOfRefRpsIdx: access_unit.num_delta_pocs_of_ref_rps_idx,
                PicOrderCntVal: current_pic_order_cnt_val,
                // Pass the actual parsed bit count so the driver can correctly skip over the
                // inline RPS bits when parsing the rest of the slice header. A value of 0 for
                // IDR frames (no inline RPS) is naturally correct.
                NumBitsForSTRefPicSetInSlice: access_unit.num_bits_for_st_ref_pic_set_in_slice,
                reserved: 0,
                RefPicSetStCurrBefore: reference_pic_set_st_curr_before,
                RefPicSetStCurrAfter: reference_pic_set_st_curr_after,
                RefPicSetLtCurr: [HEVC_NO_REFERENCE_PICTURE; HEVC_REF_PIC_SET_LIST_SIZE],
            };
            let slice_segment_offsets =
                [slice_segment_offset_mode.slice_segment_offset(access_unit)];
            let mut h265_picture_info = vk::VideoDecodeH265PictureInfoKHR::default()
                .std_picture_info(&std_picture_info)
                .slice_segment_offsets(&slice_segment_offsets);
            let mut reference_picture_resources = Vec::new();
            let mut reference_info_values = Vec::new();
            let mut reference_info_flags = empty_decode_h265_reference_info_flags();
            reference_info_flags.set_used_for_long_term_reference(0);
            reference_info_flags.set_unused_for_reference(0);
            let mut reference_slots = Vec::new();
            for reference in &selected_references {
                let (reference_image_view, reference_base_array_layer) =
                    if use_per_slot_picture_views {
                        (decode_image_views[reference.slot], 0)
                    } else {
                        (decode_image_view, reference.slot as u32)
                    };
                reference_picture_resources.push(
                    vk::VideoPictureResourceInfoKHR::default()
                        .coded_offset(vk::Offset2D { x: 0, y: 0 })
                        .coded_extent(coded_extent)
                        .base_array_layer(reference_base_array_layer)
                        .image_view_binding(reference_image_view),
                );
                reference_info_values.push(StdVideoDecodeH265ReferenceInfo {
                    flags: reference_info_flags,
                    PicOrderCntVal: reference.pic_order_cnt_val,
                });
            }
            let mut reference_dpb_slot_infos = reference_info_values
                .iter()
                .map(|reference_info| {
                    vk::VideoDecodeH265DpbSlotInfoKHR::default().std_reference_info(reference_info)
                })
                .collect::<Vec<_>>();
            for ((reference, reference_picture_resource), reference_dpb_slot_info) in
                selected_references
                    .iter()
                    .zip(reference_picture_resources.iter())
                    .zip(reference_dpb_slot_infos.iter_mut())
            {
                let reference_slot_index = i32::try_from(reference.slot)
                    .map_err(|_| "decode reference slot index exceeds i32 range".to_string())?;
                let slot_info =
                    vk::VideoReferenceSlotInfoKHR::default().slot_index(reference_slot_index);
                reference_slots.push(
                    slot_info
                        .picture_resource(reference_picture_resource)
                        .push_next(reference_dpb_slot_info),
                );
            }
            let effective_src_buf = if use_global_src_buffer {
                src_buffer_per_au[0]
            } else {
                src_buffer_per_au[submitted_index]
            };
            let effective_src_offset = if matches!(
                slice_segment_offset_mode,
                HevcSliceSegmentOffsetMode::GlobalRbspPayload
            ) {
                access_unit.buffer_offset
            } else {
                0
            };
            let effective_src_range = if matches!(
                slice_segment_offset_mode,
                HevcSliceSegmentOffsetMode::MemoryBindingAbsolute
            ) {
                u64::try_from(payload.bytes.len()).unwrap_or(u64::MAX)
            } else if std::env::var("VIDEO_HW_VULKAN_HEVC_EXACT_SRC_RANGE").as_deref() == Ok("1") {
                access_unit.vcl_size
            } else {
                access_unit.buffer_range
            };
            let decode_info_base = vk::VideoDecodeInfoKHR::default()
                .src_buffer(effective_src_buf)
                .src_buffer_offset(effective_src_offset)
                .src_buffer_range(effective_src_range)
                .dst_picture_resource(destination_picture_resource);
            let mut decode_info_builder = decode_info_base;
            if !reference_slots.is_empty() {
                decode_info_builder = decode_info_builder.reference_slots(&reference_slots);
            }
            // Explicit inter-decode memory barrier: ensure VIDEO_DECODE_WRITE from any prior decode
            // command is made visible to VIDEO_DECODE_READ in this command.  The Vulkan spec does
            // NOT guarantee implicit synchronization between consecutive vkCmdDecodeVideoKHR calls;
            // without this barrier a P-frame may observe stale (zero-initialised) data when reading
            // a reference slot that was written by an earlier decode in the same scope.
            if !reference_slots.is_empty() {
                let inter_decode_barrier = vk::MemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
                    .src_access_mask(vk::AccessFlags2::VIDEO_DECODE_WRITE_KHR)
                    .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
                    .dst_access_mask(vk::AccessFlags2::VIDEO_DECODE_READ_KHR);
                // SAFETY: barrier uses valid stage/access masks; command buffer is recording.
                unsafe {
                    device.cmd_pipeline_barrier2(
                        command_buffer,
                        &vk::DependencyInfo::default()
                            .memory_barriers(std::slice::from_ref(&inter_decode_barrier)),
                    );
                }
            }
            if is_reference {
                let mut setup_reference_info_flags = empty_decode_h265_reference_info_flags();
                setup_reference_info_flags.set_used_for_long_term_reference(0);
                setup_reference_info_flags.set_unused_for_reference(0);
                let setup_reference_info_value = StdVideoDecodeH265ReferenceInfo {
                    flags: setup_reference_info_flags,
                    PicOrderCntVal: current_pic_order_cnt_val,
                };
                let mut setup_reference_info = vk::VideoDecodeH265DpbSlotInfoKHR::default()
                    .std_reference_info(&setup_reference_info_value);
                let slot_index = i32::try_from(slot).map_err(|_| {
                    "decode setup reference slot index exceeds i32 range".to_string()
                })?;
                let setup_reference_slot = vk::VideoReferenceSlotInfoKHR::default()
                    .slot_index(slot_index)
                    // VUID-07170 (COINCIDE): pSetupReferenceSlot->pPictureResource MUST identify
                    // the same image subresource as dstPictureResource — reuse the same struct.
                    .picture_resource(&destination_picture_resource)
                    .push_next(&mut setup_reference_info);
                let decode_info = decode_info_builder
                    .setup_reference_slot(&setup_reference_slot)
                    .push_next(&mut h265_picture_info);
                // SAFETY: command buffer is recording and decode info references live local data.
                unsafe {
                    (video_decode_device.fp().cmd_decode_video_khr)(command_buffer, &decode_info);
                }
            } else {
                // On COINCIDE-only hardware (e.g. RTX 3080), pSetupReferenceSlot MUST NOT be
                // NULL even for non-reference frames (VUID-vkCmdDecodeVideoKHR-pDecodeInfo-07137).
                // Use the scratch slot (slot 0) with unused_for_reference=1 so the driver knows
                // this output picture will not be used as a future reference.
                let mut scratch_setup_info_flags = empty_decode_h265_reference_info_flags();
                scratch_setup_info_flags.set_used_for_long_term_reference(0);
                scratch_setup_info_flags.set_unused_for_reference(1);
                let scratch_setup_info_value = StdVideoDecodeH265ReferenceInfo {
                    flags: scratch_setup_info_flags,
                    PicOrderCntVal: current_pic_order_cnt_val,
                };
                let mut scratch_setup_dpb_info = vk::VideoDecodeH265DpbSlotInfoKHR::default()
                    .std_reference_info(&scratch_setup_info_value);
                let (scratch_image_view, scratch_base_array_layer) = if use_per_slot_picture_views {
                    (decode_image_views[slot], 0)
                } else {
                    (decode_image_view, slot as u32)
                };
                let scratch_picture_resource = vk::VideoPictureResourceInfoKHR::default()
                    .coded_offset(vk::Offset2D { x: 0, y: 0 })
                    .coded_extent(coded_extent)
                    .base_array_layer(scratch_base_array_layer)
                    .image_view_binding(scratch_image_view);
                let scratch_slot_index = i32::try_from(slot)
                    .map_err(|_| "decode scratch slot index exceeds i32 range".to_string())?;
                let scratch_setup_slot = vk::VideoReferenceSlotInfoKHR::default()
                    .slot_index(scratch_slot_index)
                    .picture_resource(&scratch_picture_resource)
                    .push_next(&mut scratch_setup_dpb_info);
                let decode_info = decode_info_builder
                    .setup_reference_slot(&scratch_setup_slot)
                    .push_next(&mut h265_picture_info);
                // SAFETY: command buffer is recording and decode info references live local data.
                unsafe {
                    (video_decode_device.fp().cmd_decode_video_khr)(command_buffer, &decode_info);
                }
            }
            let sample_offset = single_readback_buffer_size
                .checked_mul(
                    u64::try_from(submitted_index)
                        .map_err(|_| "submitted access-unit index exceeds u64 range".to_string())?,
                )
                .ok_or_else(|| "decode readback sample offset overflow".to_string())?;
            let mut readback_regions = readback_regions_template.clone();
            for region in &mut readback_regions {
                region.buffer_offset = region
                    .buffer_offset
                    .checked_add(sample_offset)
                    .ok_or_else(|| "decode readback region offset overflow".to_string())?;
                // With a single array-layer DPB image, each slot occupies a separate layer;
                // set the copy source to the correct layer for this slot.
                region.image_subresource.base_array_layer = slot as u32;
            }

            // Reference frames in experimental mode: defer copy to just before slot reuse or
            // end-of-stream, eliminating DST_KHR → TRANSFER_SRC → DST_KHR layout cycles while
            // the slot is actively referenced by subsequent decode commands.  Those cycles appear
            // to invalidate NVIDIA RTX 3080's internal DPB cache, causing P-frames that reference
            // the cycled slot to produce all-zero output.
            // Non-reference frames (slot 0 shared scratch) must be copied immediately since the
            // slot is reused on the very next non-reference frame.
            if is_reference {
                // Defer: store the readback regions for post-scope or eviction-triggered copy.
                // The slot stays in VIDEO_DECODE_DST_KHR; no layout transition here.
                per_slot_deferred[slot] = Some(readback_regions);
            } else {
                let decode_to_copy_image_barrier = vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
                    .src_access_mask(vk::AccessFlags2::VIDEO_DECODE_WRITE_KHR)
                    .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                    .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                    .old_layout(decode_working_layout)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .image(decode_images[0])
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: slot as u32,
                        layer_count: 1,
                    });
                let decode_to_copy_dependency = vk::DependencyInfo::default()
                    .image_memory_barriers(std::slice::from_ref(&decode_to_copy_image_barrier));
                // SAFETY: decode output image and readback buffer are valid resources.
                unsafe {
                    device.cmd_pipeline_barrier2(command_buffer, &decode_to_copy_dependency);
                    device.cmd_copy_image_to_buffer(
                        command_buffer,
                        decode_images[0],
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        readback_buffer,
                        &readback_regions,
                    );
                }
                // In COINCIDE mode the scratch slot stays in VIDEO_DECODE_DST_KHR (matching
                // the working layout used throughout); non-COINCIDE mode also uses DST_KHR.
                let post_copy_layout = decode_working_layout;
                let copy_to_decode_image_barrier = vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                    .src_access_mask(vk::AccessFlags2::TRANSFER_READ)
                    .dst_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
                    .dst_access_mask(vk::AccessFlags2::VIDEO_DECODE_WRITE_KHR)
                    .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .new_layout(post_copy_layout)
                    .image(decode_images[0])
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: slot as u32,
                        layer_count: 1,
                    });
                let copy_to_decode_dependency = vk::DependencyInfo::default()
                    .image_memory_barriers(std::slice::from_ref(&copy_to_decode_image_barrier));
                // SAFETY: image barrier transitions the subresource to its next usage layout.
                unsafe {
                    device.cmd_pipeline_barrier2(command_buffer, &copy_to_decode_dependency);
                }
            }
            // In experimental (COINCIDE) mode all images stay in DPB_KHR; in non-experimental
            // mode reference frames go to DPB_KHR and non-reference frames stay in DST_KHR.
            slot_in_dpb_layout[slot] = experimental_dpb_enabled || is_reference;
            if experimental_dpb_enabled && is_reference {
                active_reference_slots.push(HevcActiveReferenceSlot {
                    slot,
                    pic_order_cnt_val: current_pic_order_cnt_val,
                });
                let evicted = active_reference_slots
                    .len()
                    .saturating_sub(max_active_reference_slots);
                if evicted > 0 {
                    active_reference_slots.drain(0..evicted);
                }
            }
            if use_scope_per_access_unit && submitted_index + 1 < payload.access_units.len() {
                // SAFETY: command buffer is recording inside a video coding scope; ending and
                // restarting the same live session between access units follows Vulkan ordering.
                unsafe {
                    (video_queue_device.fp().cmd_end_video_coding_khr)(
                        command_buffer,
                        &vk::VideoEndCodingInfoKHR::default(),
                    );
                    (video_queue_device.fp().cmd_begin_video_coding_khr)(
                        command_buffer,
                        &begin_coding_info,
                    );
                }
            }
        }
        // End the single video coding scope after all frames have been decoded and copied.
        // SAFETY: command buffer is in a video coding scope started before the loop.
        unsafe {
            (video_queue_device.fp().cmd_end_video_coding_khr)(
                command_buffer,
                &vk::VideoEndCodingInfoKHR::default(),
            );
        }
        // Post-scope batch copy: flush all reference frames whose readbacks are still pending.
        // The video coding scope is now closed, so layout transitions from VIDEO_DECODE_DST_KHR
        // to TRANSFER_SRC_OPTIMAL no longer interact with NVIDIA's DPB cache.
        let deferred_slot_indices: Vec<usize> = per_slot_deferred
            .iter()
            .enumerate()
            .filter_map(|(idx, maybe)| maybe.as_ref().map(|_| idx))
            .collect();
        if !deferred_slot_indices.is_empty() {
            let post_scope_barriers: Vec<vk::ImageMemoryBarrier2> = deferred_slot_indices
                .iter()
                .map(|&slot_idx| {
                    vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
                        // Include both WRITE and READ: the last use of each reference slot is a
                        // VIDEO_DECODE_READ by a later P/B-frame; both must be ordered before the
                        // layout transition.
                        .src_access_mask(
                            vk::AccessFlags2::VIDEO_DECODE_WRITE_KHR
                                | vk::AccessFlags2::VIDEO_DECODE_READ_KHR,
                        )
                        .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                        .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                        .old_layout(decode_working_layout)
                        .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                        .image(decode_images[0])
                        .subresource_range(vk::ImageSubresourceRange {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            base_mip_level: 0,
                            level_count: 1,
                            base_array_layer: slot_idx as u32,
                            layer_count: 1,
                        })
                })
                .collect();
            // SAFETY: barriers transition deferred slots; copies read from each slot's layer.
            unsafe {
                device.cmd_pipeline_barrier2(
                    command_buffer,
                    &vk::DependencyInfo::default().image_memory_barriers(&post_scope_barriers),
                );
                for regions in per_slot_deferred.iter().flatten() {
                    device.cmd_copy_image_to_buffer(
                        command_buffer,
                        decode_images[0],
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        readback_buffer,
                        regions,
                    );
                }
            }
        }
        submitted_access_units = u32::try_from(payload.access_units.len()).unwrap_or(u32::MAX);
        // SAFETY: command buffer is in recording state.
        unsafe { device.end_command_buffer(command_buffer) }
            .map_err(|err| format!("vkEndCommandBuffer failed: {err}"))?;

        // SAFETY: fence create info is valid and the device is alive.
        fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }
            .map_err(|err| format!("vkCreateFence failed: {err}"))?;
        let submit_info =
            vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&command_buffer));
        // SAFETY: queue and command buffer are valid; fence belongs to the same device.
        unsafe { device.queue_submit(queue, std::slice::from_ref(&submit_info), fence) }
            .map_err(|err| format!("vkQueueSubmit failed: {err}"))?;
        // SAFETY: waiting for a fence created by the same device is valid.
        unsafe { device.wait_for_fences(std::slice::from_ref(&fence), true, 1_000_000_000) }
            .map_err(|err| format!("vkWaitForFences failed: {err}"))?;

        // SAFETY: the full readback buffer range is within the allocated host-visible memory.
        let readback_ptr = unsafe {
            device.map_memory(
                readback_buffer_memory,
                0,
                readback_buffer_size,
                vk::MemoryMapFlags::empty(),
            )
        }
        .map_err(|err| format!("vkMapMemory for decode readback buffer failed: {err}"))?;
        let readback_len = usize::try_from(readback_buffer_size)
            .map_err(|_| "decode readback buffer size exceeds usize range".to_string())?;
        readback_bytes = readback_sample_stride;
        // SAFETY: pointer returned by `vkMapMemory` is valid for `readback_len` bytes.
        let readback_slice =
            unsafe { std::slice::from_raw_parts(readback_ptr.cast::<u8>(), readback_len) };
        readback_sample = readback_slice.to_vec();
        // SAFETY: pointer returned by `vkMapMemory` is valid for `readback_len` bytes.
        readback_non_zero = readback_slice.iter().any(|&byte| byte != 0);
        if std::env::var("VIDEO_HW_VULKAN_HEVC_DEBUG_READBACK_SUMS").as_deref() == Ok("1") {
            let y_bytes = usize::try_from(coded_extent.width)
                .ok()
                .and_then(|w| {
                    usize::try_from(coded_extent.height)
                        .ok()
                        .and_then(|h| w.checked_mul(h))
                })
                .unwrap_or(0);
            let sample_count = usize::try_from(readback_sample_count).unwrap_or(0).min(10);
            for sample_index in 0..sample_count {
                let start = sample_index.saturating_mul(readback_sample_stride);
                let end = start.saturating_add(y_bytes).min(readback_slice.len());
                let sum = readback_slice[start..end]
                    .iter()
                    .fold(0_u64, |acc, &byte| acc.saturating_add(u64::from(byte)));
                eprintln!("[RAWBUF] sample={} y_sum={sum}", sample_index + 1);
            }
        }
        // SAFETY: readback buffer memory was mapped in this scope and must be unmapped once.
        unsafe {
            device.unmap_memory(readback_buffer_memory);
        }
        Ok(())
    })();

    // SAFETY: every handle below was created by `device` and is no longer used afterward.
    unsafe {
        if fence != vk::Fence::null() {
            device.destroy_fence(fence, None);
        }
        if command_pool != vk::CommandPool::null() {
            device.destroy_command_pool(command_pool, None);
        }
        for view in decode_image_views {
            if view != vk::ImageView::null() {
                device.destroy_image_view(view, None);
            }
        }
        if decode_image_view != vk::ImageView::null() {
            device.destroy_image_view(decode_image_view, None);
        }
        for slot_image in decode_images {
            if slot_image != vk::Image::null() {
                device.destroy_image(slot_image, None);
            }
        }
        for slot_memory in decode_image_memories {
            if slot_memory != vk::DeviceMemory::null() {
                device.free_memory(slot_memory, None);
            }
        }
        for per_au_buf in src_buffer_per_au {
            if per_au_buf != vk::Buffer::null() {
                device.destroy_buffer(per_au_buf, None);
            }
        }
        if src_buffer_memory != vk::DeviceMemory::null() {
            device.free_memory(src_buffer_memory, None);
        }
        for memory in src_buffer_memories {
            if memory != vk::DeviceMemory::null() {
                device.free_memory(memory, None);
            }
        }
        if readback_buffer != vk::Buffer::null() {
            device.destroy_buffer(readback_buffer, None);
        }
        if readback_buffer_memory != vk::DeviceMemory::null() {
            device.free_memory(readback_buffer_memory, None);
        }
        if video_session_parameters != vk::VideoSessionParametersKHR::null() {
            (video_queue_device.fp().destroy_video_session_parameters_khr)(
                device.handle(),
                video_session_parameters,
                std::ptr::null(),
            );
        }
        if video_session != vk::VideoSessionKHR::null() {
            (video_queue_device.fp().destroy_video_session_khr)(
                device.handle(),
                video_session,
                std::ptr::null(),
            );
        }
        for memory in session_memories {
            device.free_memory(memory, None);
        }
    }

    if let Some(marker_path) = experimental_dpb_marker_path.take() {
        let _ = std::fs::remove_file(marker_path);
    }

    if let Err(err) = submit_result {
        return HevcDecodeSubmitExecutionProbe::Failed(format!(
            "{err}; experimental_dpb_mode={}, experimental_dpb_status={experimental_dpb_status:?}",
            experimental_dpb_mode.as_str()
        ));
    }

    HevcDecodeSubmitExecutionProbe::Ready {
        queue_family_index: queue_family_index.0,
        output_format,
        coded_width: parameter_sets.coded_width,
        coded_height: parameter_sets.coded_height,
        readback_non_zero,
        readback_bytes,
        readback_planes,
        readback_sample_stride,
        readback_sample_count,
        readback_sample,
        submitted_access_units,
        experimental_dpb_enabled,
        experimental_dpb_mode: experimental_dpb_mode.as_str(),
        experimental_dpb_status,
    }
}

pub(super) fn build_hevc_submit_probe_bitstream_payload(
    parameter_sets: &HevcParameterSets,
    bitstream: &[u8],
    access_unit_limit: usize,
    offset_alignment: u64,
    size_alignment: u64,
) -> Result<HevcSubmitProbeBitstreamPayload, String> {
    let parsed_pps = parse_hevc_pps(&parameter_sets.pps)?;
    let max_poc_lsb =
        i32::try_from(parameter_sets.parsed_sps.max_pic_order_cnt_lsb()).unwrap_or(i32::MAX / 2);
    let mut unwrapper =
        HevcPocUnwrapper::new(parameter_sets.parsed_sps.log2_max_pic_order_cnt_lsb_minus4);
    let mut bytes: Vec<u8> = Vec::new();
    let mut access_units = Vec::new();
    for nalu in split_annexb_nalus(bitstream) {
        let Some(nal_unit_type) = hevc_nal_type_raw(nalu) else {
            continue;
        };
        if nal_unit_type > 31 {
            continue;
        }
        let slice_header = parse_hevc_slice_header(nalu, &parsed_pps, &parameter_sets.parsed_sps)
            .map_err(|err| {
            format!("failed to parse HEVC slice header while building submit payload: {err}")
        })?;
        if !slice_header.is_first_slice_segment {
            continue;
        }
        let temporal_id = hevc_nalu_temporal_id(nalu);
        let poc_full =
            unwrapper.advance(slice_header.pic_order_cnt_lsb, nal_unit_type, temporal_id);
        // Adjust inline RPS absolute POCs from LSB-space to full POC space so they match the
        // full POCs stored in active_reference_slots.pic_order_cnt_val.
        let mut inline_pocs = slice_header.inline_short_term_ref_pic_set_pocs;
        if let Some(ref mut pocs) = inline_pocs {
            adjust_inline_rps_pocs_to_full_poc(pocs, poc_full, max_poc_lsb);
        }
        // Pad the buffer so this AU starts on an offset-alignment boundary.
        let current_len =
            u64::try_from(bytes.len()).map_err(|_| "bitstream buffer length exceeds u64")?;
        let buffer_offset = align_up(current_len, offset_alignment);
        let padding = usize::try_from(buffer_offset - current_len)
            .map_err(|_| "bitstream alignment padding exceeds usize")?;
        bytes.resize(bytes.len() + padding, 0u8);

        // Write the VCL NALU in Annex B format. VPS/SPS/PPS are provided via video session
        // parameters; only the slice NALU is needed here.
        if std::env::var("VIDEO_HW_VULKAN_HEVC_PREFIX_PARAM_SETS").as_deref() == Ok("1") {
            for parameter_set_nalu in [
                &parameter_sets.vps,
                &parameter_sets.sps,
                &parameter_sets.pps,
            ] {
                bytes.extend_from_slice(&[0, 0, 0, 1]);
                bytes.extend_from_slice(parameter_set_nalu);
            }
        }
        let vcl_start = u64::try_from(bytes.len())
            .map_err(|_| "bitstream buffer length exceeds u64")?
            .saturating_sub(buffer_offset);
        let start_code_len = 3_u64;
        bytes.extend_from_slice(&[0, 0, 1]);
        bytes.extend_from_slice(nalu);
        // `slice_segment_offset` is the byte offset from `buffer_offset` to the RBSP payload
        // (past the 3-byte Annex-B start code and the 2-byte NAL unit header). Keep this as the
        // normalized RBSP offset; the decode loop derives the driver-facing offset from it.
        let slice_segment_offset = u32::try_from(
            vcl_start
                .saturating_add(start_code_len)
                .saturating_add(u64::from(HEVC_NAL_HEADER_SIZE)),
        )
        .map_err(|_| "slice segment offset exceeds u32")?;
        let vcl_end =
            u64::try_from(bytes.len()).map_err(|_| "bitstream buffer length exceeds u64")?;
        let vcl_size = vcl_end - buffer_offset;
        let buffer_range = align_up(vcl_size, size_alignment);

        access_units.push(HevcSubmitProbeAccessUnit {
            header: HevcAccessUnitHeader {
                nal_unit_type: slice_header.nal_unit_type,
                pps_id: slice_header.pps_id,
                pic_order_cnt_lsb: slice_header.pic_order_cnt_lsb,
                temporal_id,
                poc_full,
            },
            buffer_offset,
            buffer_range,
            vcl_size,
            slice_segment_offset,
            slice_type: slice_header.slice_type,
            short_term_ref_pic_set_idx: slice_header.short_term_ref_pic_set_idx,
            inline_short_term_ref_pic_set_pocs: inline_pocs,
            num_bits_for_st_ref_pic_set_in_slice: slice_header.num_bits_for_st_ref_pic_set_in_slice,
            num_delta_pocs_of_ref_rps_idx: slice_header.num_delta_pocs_of_ref_rps_idx,
        });
        if access_units.len() >= access_unit_limit {
            break;
        }
    }
    if access_units.is_empty() {
        return Err(
            "submit execution probe requires at least one access-unit-leading VCL NALU".to_string(),
        );
    }

    // Ensure the bytes buffer is large enough to cover the last AU's full aligned range.
    let last_au = &access_units[access_units.len() - 1];
    let required_len = usize::try_from(last_au.buffer_offset + last_au.buffer_range)
        .map_err(|_| "total bitstream buffer size exceeds usize")?;
    if bytes.len() < required_len {
        bytes.resize(required_len, 0u8);
    }

    Ok(HevcSubmitProbeBitstreamPayload {
        bytes,
        access_units,
        parsed_pps,
    })
}

fn select_memory_type_index(
    memory_properties: &vk::PhysicalDeviceMemoryProperties,
    memory_type_bits: u32,
    required: vk::MemoryPropertyFlags,
) -> Option<u32> {
    let memory_type_count = usize::try_from(memory_properties.memory_type_count).ok()?;
    memory_properties
        .memory_types
        .iter()
        .take(memory_type_count)
        .enumerate()
        .find_map(|(index, memory_type)| {
            let index_u32 = u32::try_from(index).ok()?;
            let mask = 1_u32.checked_shl(index_u32)?;
            ((memory_type_bits & mask) != 0 && memory_type.property_flags.contains(required))
                .then_some(index_u32)
        })
}

pub(super) fn align_up(value: u64, alignment: u64) -> u64 {
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
