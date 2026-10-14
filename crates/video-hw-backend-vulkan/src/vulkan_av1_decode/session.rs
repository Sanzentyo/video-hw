//! Vulkan AV1 decode session, resource, and command-buffer ownership.

use super::*;

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Av1DecodeExtensionFlags {
    pub(super) has_video_queue: bool,
    pub(super) has_video_decode_queue: bool,
    pub(super) has_video_decode_av1: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Av1AdapterDecodeSupport {
    pub(super) extensions: Av1DecodeExtensionFlags,
    pub(super) decode_queue_family_index: Option<u32>,
    pub(super) decode_transfer_queue_family_index: Option<u32>,
}

#[derive(Debug, Clone)]
pub(super) struct Av1DecodeCapabilitySnapshot {
    pub(super) min_coded_width: u32,
    pub(super) min_coded_height: u32,
    pub(super) max_coded_width: u32,
    pub(super) max_coded_height: u32,
    pub(super) min_bitstream_buffer_offset_alignment: u64,
    pub(super) min_bitstream_buffer_size_alignment: u64,
    pub(super) max_dpb_slots: u32,
    pub(super) max_active_reference_pictures: u32,
    pub(super) max_level: ash::vk::native::StdVideoAV1Level,
    pub(super) std_header_version: vk::ExtensionProperties,
    pub(super) decode_output_formats: Vec<vk::Format>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Av1DecodeSessionParameterProbeConfig<'a> {
    pub(super) physical_device: vk::PhysicalDevice,
    pub(super) queue_family_index: u32,
    pub(super) capability_snapshot: &'a Av1DecodeCapabilitySnapshot,
    pub(super) picture_format: vk::Format,
    pub(super) std_sequence_header: &'a StdVideoAV1SequenceHeader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Av1DecodeSessionBootstrapSummary {
    pub(super) memory_requirement_count: usize,
    pub(super) memory_total_size: u64,
    pub(super) memory_max_alignment: u64,
    pub(super) memory_bound_count: usize,
}

#[derive(Debug, Clone)]
pub(super) struct Av1DecodeSessionMemoryPlan {
    pub(super) requirements: Vec<vk::VideoSessionMemoryRequirementsKHR<'static>>,
    pub(super) summary: Av1DecodeSessionBootstrapSummary,
}

pub(super) struct Av1DecodeSourceBufferResource {
    pub(super) buffer: vk::Buffer,
    pub(super) memory: vk::DeviceMemory,
}

pub(super) struct Av1DecodeImageResource {
    pub(super) image: vk::Image,
    pub(super) memory: vk::DeviceMemory,
    pub(super) dst_view: vk::ImageView,
    pub(super) reference_view: vk::ImageView,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Av1DecodePictureViews {
    pub(crate) dst: vk::ImageView,
    pub(crate) reference: vk::ImageView,
}

pub(super) struct Av1DecodeReadbackBufferResource {
    pub(super) buffer: vk::Buffer,
    pub(super) memory: vk::DeviceMemory,
    pub(super) size: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Av1DecodeReadbackSample {
    pub(super) mapped_bytes: usize,
    pub(super) non_zero: bool,
    pub(super) data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Av1DecodeBitstreamSessionProbeOptions {
    pub(super) record_command_buffer: bool,
    pub(super) submit_command_buffer: bool,
    pub(super) readback: bool,
}

pub(super) struct Av1DecodeSessionResource {
    pub(super) session: vk::VideoSessionKHR,
    pub(super) parameters: vk::VideoSessionParametersKHR,
    pub(super) memories: Vec<vk::DeviceMemory>,
    pub(super) summary: Av1DecodeSessionBootstrapSummary,
}

pub(super) struct Av1DecodeCommandRecordConfig<'a> {
    pub(super) instance: &'a ash::Instance,
    pub(super) device: &'a ash::Device,
    pub(super) queue_family_index: u32,
    pub(super) submit_command_buffer: bool,
    pub(super) upload_plan: &'a Av1DecodeBitstreamUploadPlan,
    pub(super) command: &'a Av1DecodeCommandSkeleton,
    pub(super) video_session: vk::VideoSessionKHR,
    pub(super) video_session_parameters: vk::VideoSessionParametersKHR,
    pub(super) source_buffer: vk::Buffer,
    pub(super) decode_image: vk::Image,
    pub(super) dst_image_view: vk::ImageView,
    pub(super) reference_image_view: vk::ImageView,
    pub(super) readback: Option<Av1DecodeCommandReadbackConfig<'a>>,
}

pub(super) struct Av1DecodeStatusQueryResource {
    pub(super) pool: vk::QueryPool,
}

pub(super) struct Av1DecodeCommandReadbackConfig<'a> {
    pub(super) buffer: vk::Buffer,
    pub(super) plan: &'a Av1DecodeReadbackPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Av1DecodeCommandBufferRecordMode {
    BarrierOnly,
    BeginEnd,
    ResetEnd,
    FirstDecode,
    Full,
}

pub(super) fn probe_av1_decode_session_parameters(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
    capability_snapshot: &Av1DecodeCapabilitySnapshot,
) -> Result<(), String> {
    let device = create_av1_decode_device(instance, physical_device, queue_family_index)?;
    let result = create_and_destroy_av1_decode_session_parameters(
        instance,
        &device,
        physical_device,
        queue_family_index,
        capability_snapshot,
    );
    // SAFETY: The device is no longer used after this point.
    unsafe {
        device.destroy_device(None);
    }
    result.map(|_| ())
}

pub(super) fn probe_av1_decode_session_parameters_for_bitstream_with_instance(
    entry: &ash::Entry,
    instance: &ash::Instance,
    bitstream: &[u8],
    std_sequence_header: &StdVideoAV1SequenceHeader,
    coded_width: u32,
    coded_height: u32,
    options: Av1DecodeBitstreamSessionProbeOptions,
) -> Result<Av1DecodeBitstreamSessionProbe, String> {
    // SAFETY: `instance` is valid here; we only enumerate physical device handles.
    let physical_devices = unsafe { instance.enumerate_physical_devices() }
        .map_err(|err| format!("failed to enumerate physical devices: {err}"))?;
    if physical_devices.is_empty() {
        return Err(
            "no Vulkan physical devices available for AV1 bitstream session probe".to_string(),
        );
    }

    let mut probe_errors = Vec::new();
    for physical_device in physical_devices {
        let support = query_av1_adapter_decode_support(instance, physical_device)
            .map_err(|err| format!("failed to enumerate device extensions: {err}"))?;
        if !support.extensions.supports_av1_decode() {
            continue;
        }
        let queue_family_index = if options.readback {
            support.decode_transfer_queue_family_index
        } else {
            support.decode_queue_family_index
        };
        let Some(queue_family_index) = queue_family_index else {
            if options.readback {
                probe_errors.push(
                        "AV1 decode readback requires a queue family with VIDEO_DECODE and TRANSFER support".to_string(),
                    );
            }
            continue;
        };
        let snapshot = match query_av1_decode_capability_snapshot(entry, instance, physical_device)
        {
            Ok(snapshot) => snapshot,
            Err(err) => {
                probe_errors.push(err);
                continue;
            }
        };
        if let Err(err) = validate_av1_decode_capability_snapshot(&snapshot) {
            probe_errors.push(err);
            continue;
        }
        if coded_width < snapshot.min_coded_width
            || coded_width > snapshot.max_coded_width
            || coded_height < snapshot.min_coded_height
            || coded_height > snapshot.max_coded_height
        {
            probe_errors.push(format!(
                "AV1 bitstream coded extent {coded_width}x{coded_height} is outside adapter range {}x{}..{}x{}",
                snapshot.min_coded_width,
                snapshot.min_coded_height,
                snapshot.max_coded_width,
                snapshot.max_coded_height
            ));
            continue;
        }
        let picture_format = *snapshot
            .decode_output_formats
            .first()
            .ok_or_else(|| "AV1 bitstream session probe has no output format".to_string())?;
        let device = match create_av1_decode_device(instance, physical_device, queue_family_index) {
            Ok(device) => device,
            Err(err) => {
                probe_errors.push(err);
                continue;
            }
        };
        let upload_plan = match build_av1_aligned_decode_bitstream_upload_plan(
            bitstream,
            snapshot.min_bitstream_buffer_offset_alignment,
            snapshot.min_bitstream_buffer_size_alignment,
            snapshot.max_dpb_slots,
        ) {
            Ok(plan) => plan,
            Err(err) => {
                probe_errors.push(err);
                // SAFETY: The device is no longer used after this point.
                unsafe {
                    device.destroy_device(None);
                }
                continue;
            }
        };
        let upload_result =
            create_av1_decode_source_buffer(instance, &device, physical_device, &upload_plan.bytes);
        let command = match build_av1_key_frame_decode_command_skeleton_from_decodes(
            &upload_plan.decodes,
            snapshot.max_dpb_slots,
        ) {
            Ok(command) => command,
            Err(err) => {
                probe_errors.push(err);
                // SAFETY: The device is no longer used after this point.
                unsafe {
                    device.destroy_device(None);
                }
                continue;
            }
        };
        let image_plan = match command.decode_image_plan(picture_format) {
            Ok(plan) => plan,
            Err(err) => {
                probe_errors.push(err);
                // SAFETY: The device is no longer used after this point.
                unsafe {
                    device.destroy_device(None);
                }
                continue;
            }
        };
        let readback_plan = match command.decode_readback_plan(picture_format) {
            Ok(plan) => plan,
            Err(err) => {
                probe_errors.push(err);
                // SAFETY: The device is no longer used after this point.
                unsafe {
                    device.destroy_device(None);
                }
                continue;
            }
        };
        let result = upload_result.and_then(|source_buffer| {
            let image =
                match create_av1_decode_image(instance, &device, physical_device, &image_plan) {
                    Ok(image) => image,
                    Err(err) => {
                        destroy_av1_decode_source_buffer(&device, source_buffer);
                        return Err(err);
                    }
                };
            let source_barrier = Av1DecodeCommandSkeleton::vk_decode_source_memory_barrier();
            if source_barrier.dst_access_mask != vk::AccessFlags2::VIDEO_DECODE_READ_KHR {
                destroy_av1_decode_image(&device, image);
                destroy_av1_decode_source_buffer(&device, source_buffer);
                return Err(
                    "AV1 decode source barrier does not target VIDEO_DECODE_READ_KHR".to_string(),
                );
            }
            let image_barrier = match command.vk_decode_image_init_barrier(image.image) {
                Ok(barrier) => barrier,
                Err(err) => {
                    destroy_av1_decode_image(&device, image);
                    destroy_av1_decode_source_buffer(&device, source_buffer);
                    return Err(err);
                }
            };
            let session = match create_av1_decode_session_with_parameters(
                instance,
                &device,
                Av1DecodeSessionParameterProbeConfig {
                    physical_device,
                    queue_family_index,
                    capability_snapshot: &snapshot,
                    picture_format,
                    std_sequence_header,
                },
            ) {
                Ok(session) => session,
                Err(err) => {
                    destroy_av1_decode_image(&device, image);
                    destroy_av1_decode_source_buffer(&device, source_buffer);
                    return Err(err);
                }
            };
            let command_buffer_record_requested = options.record_command_buffer;
            let command_buffer_submit_requested =
                command_buffer_record_requested && options.submit_command_buffer;
            let readback_requested = options.readback;
            if readback_requested
                && (!command_buffer_record_requested || !command_buffer_submit_requested)
            {
                destroy_av1_decode_session_resource(instance, &device, session);
                destroy_av1_decode_image(&device, image);
                destroy_av1_decode_source_buffer(&device, source_buffer);
                return Err(
                    "AV1 decode readback probe requires command-buffer record and submit"
                        .to_string(),
                );
            }
            let readback_buffer = if readback_requested {
                match create_av1_decode_readback_buffer(
                    instance,
                    &device,
                    physical_device,
                    readback_plan.buffer_size,
                ) {
                    Ok(buffer) => {
                        if let Err(err) = initialize_av1_decode_readback_buffer(&device, &buffer) {
                            destroy_av1_decode_readback_buffer(&device, buffer);
                            destroy_av1_decode_session_resource(instance, &device, session);
                            destroy_av1_decode_image(&device, image);
                            destroy_av1_decode_source_buffer(&device, source_buffer);
                            return Err(err);
                        }
                        Some(buffer)
                    }
                    Err(err) => {
                        destroy_av1_decode_session_resource(instance, &device, session);
                        destroy_av1_decode_image(&device, image);
                        destroy_av1_decode_source_buffer(&device, source_buffer);
                        return Err(err);
                    }
                }
            } else {
                None
            };
            let record_result = if command_buffer_record_requested {
                record_and_destroy_av1_decode_command_buffer(Av1DecodeCommandRecordConfig {
                    instance,
                    device: &device,
                    queue_family_index,
                    submit_command_buffer: command_buffer_submit_requested,
                    upload_plan: &upload_plan,
                    command: &command,
                    video_session: session.session,
                    video_session_parameters: session.parameters,
                    source_buffer: source_buffer.buffer,
                    decode_image: image.image,
                    dst_image_view: image.dst_view,
                    reference_image_view: image.reference_view,
                    readback: readback_buffer.as_ref().map(|buffer| {
                        Av1DecodeCommandReadbackConfig {
                            buffer: buffer.buffer,
                            plan: &readback_plan,
                        }
                    }),
                })
            } else {
                upload_plan.record_decode_command_sequence(
                    &command,
                    session.session,
                    session.parameters,
                    source_buffer.buffer,
                    Av1DecodePictureViews {
                        dst: image.dst_view,
                        reference: image.reference_view,
                    },
                    |_| Ok(()),
                )
            };
            let summary = session.summary;
            let decode_image_barrier_layers = image_barrier.subresource_range.layer_count;
            let record_summary = match record_result {
                Ok(summary) => summary,
                Err(err) => {
                    if let Some(readback_buffer) = readback_buffer {
                        destroy_av1_decode_readback_buffer(&device, readback_buffer);
                    }
                    destroy_av1_decode_session_resource(instance, &device, session);
                    destroy_av1_decode_image(&device, image);
                    destroy_av1_decode_source_buffer(&device, source_buffer);
                    return Err(err);
                }
            };
            let readback_sample = if let Some(readback_resource) = &readback_buffer {
                match map_av1_decode_readback_buffer(&device, readback_resource) {
                    Ok(sample) => sample,
                    Err(err) => {
                        if let Some(readback_resource) = readback_buffer {
                            destroy_av1_decode_readback_buffer(&device, readback_resource);
                        }
                        destroy_av1_decode_session_resource(instance, &device, session);
                        destroy_av1_decode_image(&device, image);
                        destroy_av1_decode_source_buffer(&device, source_buffer);
                        return Err(err);
                    }
                }
            } else {
                Av1DecodeReadbackSample::default()
            };
            if let Some(readback_buffer) = readback_buffer {
                destroy_av1_decode_readback_buffer(&device, readback_buffer);
            }
            destroy_av1_decode_session_resource(instance, &device, session);
            destroy_av1_decode_image(&device, image);
            destroy_av1_decode_source_buffer(&device, source_buffer);
            Ok((
                summary,
                record_summary,
                decode_image_barrier_layers,
                readback_sample,
                command_buffer_record_requested,
                command_buffer_submit_requested,
            ))
        });
        // SAFETY: The device is no longer used after this point.
        unsafe {
            device.destroy_device(None);
        }
        match result {
            Ok((
                summary,
                record_summary,
                decode_image_barrier_layers,
                readback_sample,
                command_buffer_recorded,
                command_buffer_submitted,
            )) => {
                return Ok(Av1DecodeBitstreamSessionProbe {
                    coded_width,
                    coded_height,
                    picture_format,
                    min_bitstream_buffer_offset_alignment: snapshot
                        .min_bitstream_buffer_offset_alignment,
                    min_bitstream_buffer_size_alignment: snapshot
                        .min_bitstream_buffer_size_alignment,
                    session_memory_requirement_count: summary.memory_requirement_count,
                    session_memory_total_size: summary.memory_total_size,
                    session_memory_max_alignment: summary.memory_max_alignment,
                    session_memory_bound_count: summary.memory_bound_count,
                    bitstream_upload_bytes: upload_plan.bytes.len(),
                    decode_image_layers: image_plan.array_layers,
                    decode_image_barrier_layers,
                    readback_bytes: readback_plan.buffer_size,
                    readback_region_count: readback_plan.regions.len(),
                    readback_sample_stride: readback_plan.sample_stride,
                    readback_sample_count: readback_plan.sample_count,
                    readback_mapped_bytes: readback_sample.mapped_bytes,
                    readback_non_zero: readback_sample.non_zero,
                    readback_sample: readback_sample.data,
                    command_record_decode_count: record_summary.decode_count,
                    command_buffer_recorded,
                    command_buffer_submitted,
                    decode_query_status_raw: record_summary.decode_query_status_raw,
                });
            }
            Err(err) => probe_errors.push(err),
        }
    }

    if probe_errors.is_empty() {
        Err("no compatible Vulkan AV1 decode adapter found for bitstream session probe".to_string())
    } else {
        Err(format!(
            "AV1 bitstream session probe failed on all candidate adapters: {}",
            probe_errors.join("; ")
        ))
    }
}

pub(super) fn create_and_destroy_av1_decode_session_parameters(
    instance: &ash::Instance,
    device: &ash::Device,
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
    capability_snapshot: &Av1DecodeCapabilitySnapshot,
) -> Result<Av1DecodeSessionBootstrapSummary, String> {
    let picture_format = *capability_snapshot
        .decode_output_formats
        .first()
        .ok_or_else(|| "AV1 decode session parameter probe has no output format".to_string())?;
    let coded_width = preferred_av1_probe_extent(
        capability_snapshot.min_coded_width,
        capability_snapshot.max_coded_width,
    );
    let coded_height = preferred_av1_probe_extent(
        capability_snapshot.min_coded_height,
        capability_snapshot.max_coded_height,
    );
    let std_sequence_header = build_probe_av1_std_sequence_header(coded_width, coded_height)?;
    create_and_destroy_av1_decode_session_parameters_with_header(
        instance,
        device,
        Av1DecodeSessionParameterProbeConfig {
            physical_device,
            queue_family_index,
            capability_snapshot,
            picture_format,
            std_sequence_header: &std_sequence_header,
        },
    )
}

pub(super) fn create_and_destroy_av1_decode_session_parameters_with_header(
    instance: &ash::Instance,
    device: &ash::Device,
    config: Av1DecodeSessionParameterProbeConfig<'_>,
) -> Result<Av1DecodeSessionBootstrapSummary, String> {
    let resource = create_av1_decode_session_with_parameters(instance, device, config)?;
    let summary = resource.summary;
    destroy_av1_decode_session_resource(instance, device, resource);
    Ok(summary)
}

pub(super) fn create_av1_decode_session_with_parameters(
    instance: &ash::Instance,
    device: &ash::Device,
    config: Av1DecodeSessionParameterProbeConfig<'_>,
) -> Result<Av1DecodeSessionResource, String> {
    let mut decode_av1_profile = vk::VideoDecodeAV1ProfileInfoKHR::default()
        .std_profile(StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_MAIN)
        .film_grain_support(false);
    let mut decode_usage = vk::VideoDecodeUsageInfoKHR::default()
        .video_usage_hints(vk::VideoDecodeUsageFlagsKHR::DEFAULT);
    let profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_AV1)
        .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
        .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .push_next(&mut decode_av1_profile)
        .push_next(&mut decode_usage);

    let create_info = vk::VideoSessionCreateInfoKHR::default()
        .queue_family_index(config.queue_family_index)
        .video_profile(&profile)
        .picture_format(config.picture_format)
        .max_coded_extent(vk::Extent2D {
            width: config.capability_snapshot.max_coded_width,
            height: config.capability_snapshot.max_coded_height,
        })
        .reference_picture_format(config.picture_format)
        .max_dpb_slots(config.capability_snapshot.max_dpb_slots)
        .max_active_reference_pictures(config.capability_snapshot.max_active_reference_pictures)
        .std_header_version(&config.capability_snapshot.std_header_version);
    let video_queue_device = ash::khr::video_queue::Device::new(instance, device);
    let mut video_session = vk::VideoSessionKHR::null();

    // SAFETY: All pointers in `create_info` live until the call returns.
    let session_result = unsafe {
        (video_queue_device.fp().create_video_session_khr)(
            device.handle(),
            &create_info,
            std::ptr::null(),
            &mut video_session,
        )
    };
    if session_result != vk::Result::SUCCESS {
        return Err(format!(
            "vkCreateVideoSessionKHR for AV1 decode failed: {session_result:?}"
        ));
    }

    let resource_result =
        query_av1_decode_session_memory_requirements(device, &video_queue_device, video_session)
            .and_then(|memory_plan| {
                let session_memories = bind_av1_decode_session_memory(
                    instance,
                    device,
                    &video_queue_device,
                    config.physical_device,
                    video_session,
                    &memory_plan.requirements,
                )?;
                let parameters = match create_av1_decode_session_parameters(
                    device,
                    &video_queue_device,
                    video_session,
                    config.std_sequence_header,
                ) {
                    Ok(parameters) => parameters,
                    Err(err) => {
                        for memory in session_memories {
                            // SAFETY: These allocations were created by this device in this scope.
                            unsafe { device.free_memory(memory, None) };
                        }
                        return Err(err);
                    }
                };
                let mut summary = memory_plan.summary;
                summary.memory_bound_count = session_memories.len();
                Ok(Av1DecodeSessionResource {
                    session: video_session,
                    parameters,
                    memories: session_memories,
                    summary,
                })
            });

    if resource_result.is_err() {
        // SAFETY: `video_session` was created by this device and is no longer used.
        unsafe {
            (video_queue_device.fp().destroy_video_session_khr)(
                device.handle(),
                video_session,
                std::ptr::null(),
            );
        }
    }
    resource_result
}

pub(super) fn query_av1_decode_session_memory_requirements(
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    video_session: vk::VideoSessionKHR,
) -> Result<Av1DecodeSessionMemoryPlan, String> {
    let mut requirement_count = 0_u32;
    // SAFETY: Count query writes only `requirement_count` for a valid video session.
    let count_result = unsafe {
        (video_queue_device
            .fp()
            .get_video_session_memory_requirements_khr)(
            device.handle(),
            video_session,
            &mut requirement_count,
            std::ptr::null_mut(),
        )
    };
    if count_result != vk::Result::SUCCESS {
        return Err(format!(
            "vkGetVideoSessionMemoryRequirementsKHR for AV1 decode count query failed: {count_result:?}"
        ));
    }
    if requirement_count == 0 {
        return Err(
            "vkGetVideoSessionMemoryRequirementsKHR for AV1 decode returned no memory requirements"
                .to_string(),
        );
    }

    let mut requirements =
        vec![vk::VideoSessionMemoryRequirementsKHR::default(); requirement_count as usize];
    // SAFETY: `requirements` has storage for the count returned above.
    let query_result = unsafe {
        (video_queue_device
            .fp()
            .get_video_session_memory_requirements_khr)(
            device.handle(),
            video_session,
            &mut requirement_count,
            requirements.as_mut_ptr(),
        )
    };
    if query_result != vk::Result::SUCCESS {
        return Err(format!(
            "vkGetVideoSessionMemoryRequirementsKHR for AV1 decode query failed: {query_result:?}"
        ));
    }
    requirements.truncate(requirement_count as usize);

    let memory_total_size = requirements
        .iter()
        .map(|requirement| requirement.memory_requirements.size)
        .sum();
    let memory_max_alignment = requirements
        .iter()
        .map(|requirement| requirement.memory_requirements.alignment)
        .max()
        .unwrap_or(0);

    Ok(Av1DecodeSessionMemoryPlan {
        summary: Av1DecodeSessionBootstrapSummary {
            memory_requirement_count: requirements.len(),
            memory_total_size,
            memory_max_alignment,
            memory_bound_count: 0,
        },
        requirements,
    })
}

pub(super) fn bind_av1_decode_session_memory(
    instance: &ash::Instance,
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    physical_device: vk::PhysicalDevice,
    video_session: vk::VideoSessionKHR,
    requirements: &[vk::VideoSessionMemoryRequirementsKHR<'_>],
) -> Result<Vec<vk::DeviceMemory>, String> {
    if requirements.is_empty() {
        return Err("AV1 decode session memory bind requires at least one requirement".to_string());
    }

    // SAFETY: `physical_device` belongs to `instance`; this only reads memory metadata.
    let memory_properties =
        unsafe { instance.get_physical_device_memory_properties(physical_device) };
    let mut memories = Vec::with_capacity(requirements.len());
    let mut bindings = Vec::with_capacity(requirements.len());

    for requirement in requirements {
        let requirement_info = requirement.memory_requirements;
        let memory_type_index = select_av1_decode_memory_type_index(
            &memory_properties,
            requirement_info.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .or_else(|| {
            select_av1_decode_memory_type_index(
                &memory_properties,
                requirement_info.memory_type_bits,
                vk::MemoryPropertyFlags::empty(),
            )
        })
        .ok_or_else(|| {
            format!(
                "no compatible memory type for AV1 video session bind index {} (bits=0x{:X})",
                requirement.memory_bind_index, requirement_info.memory_type_bits
            )
        })?;
        let allocation_size = requirement_info.size.max(1);
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(allocation_size)
            .memory_type_index(memory_type_index);
        // SAFETY: Allocation info is derived from Vulkan memory requirements.
        let memory = unsafe { device.allocate_memory(&allocate_info, None) }
            .map_err(|err| format!("vkAllocateMemory for AV1 video session bind failed: {err}"))?;
        memories.push(memory);
        bindings.push(
            vk::BindVideoSessionMemoryInfoKHR::default()
                .memory_bind_index(requirement.memory_bind_index)
                .memory(memory)
                .memory_offset(0)
                .memory_size(allocation_size),
        );
    }

    // SAFETY: Bind infos reference memory allocations above and live through the call.
    let bind_result = unsafe {
        (video_queue_device.fp().bind_video_session_memory_khr)(
            device.handle(),
            video_session,
            u32::try_from(bindings.len())
                .map_err(|_| "AV1 session binding count exceeds u32 range".to_string())?,
            bindings.as_ptr(),
        )
    };
    if bind_result != vk::Result::SUCCESS {
        for memory in memories.drain(..) {
            // SAFETY: These allocations were created by this device in this scope.
            unsafe { device.free_memory(memory, None) };
        }
        return Err(format!(
            "vkBindVideoSessionMemoryKHR for AV1 decode failed: {bind_result:?}"
        ));
    }

    Ok(memories)
}

pub(super) fn select_av1_decode_memory_type_index(
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

pub(super) fn create_av1_decode_source_buffer(
    instance: &ash::Instance,
    device: &ash::Device,
    physical_device: vk::PhysicalDevice,
    bytes: &[u8],
) -> Result<Av1DecodeSourceBufferResource, String> {
    if bytes.is_empty() {
        return Err("AV1 decode source buffer upload requires non-empty bytes".to_string());
    }

    let buffer_size =
        u64::try_from(bytes.len()).map_err(|_| "AV1 source upload size exceeds u64")?;
    let create_info = vk::BufferCreateInfo::default()
        .size(buffer_size)
        .usage(vk::BufferUsageFlags::VIDEO_DECODE_SRC_KHR)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: Buffer create info contains only POD values.
    let buffer = unsafe { device.create_buffer(&create_info, None) }
        .map_err(|err| format!("vkCreateBuffer for AV1 decode source failed: {err}"))?;

    let upload_result = (|| -> Result<vk::DeviceMemory, String> {
        // SAFETY: `buffer` was created by this device.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        // SAFETY: `physical_device` belongs to `instance`; this only reads memory metadata.
        let memory_properties =
            unsafe { instance.get_physical_device_memory_properties(physical_device) };
        let memory_type_index = select_av1_decode_memory_type_index(
            &memory_properties,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or_else(|| {
            format!(
                "no HOST_VISIBLE|HOST_COHERENT memory type for AV1 decode source buffer (bits=0x{:X})",
                requirements.memory_type_bits
            )
        })?;
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size.max(1))
            .memory_type_index(memory_type_index);
        // SAFETY: Allocation info is derived from Vulkan memory requirements.
        let memory = unsafe { device.allocate_memory(&allocate_info, None) }
            .map_err(|err| format!("vkAllocateMemory for AV1 decode source failed: {err}"))?;
        // SAFETY: Buffer and memory were created by this device and offset 0 satisfies alignment
        // by construction for a fresh allocation.
        if let Err(err) = unsafe { device.bind_buffer_memory(buffer, memory, 0) } {
            // SAFETY: `memory` was allocated above and has not been bound successfully.
            unsafe { device.free_memory(memory, None) };
            return Err(format!(
                "vkBindBufferMemory for AV1 decode source failed: {err}"
            ));
        }
        // SAFETY: Memory is HOST_VISIBLE|HOST_COHERENT and the mapped range fits the allocation.
        let mapped =
            unsafe { device.map_memory(memory, 0, buffer_size, vk::MemoryMapFlags::empty()) };
        let mapped = match mapped {
            Ok(mapped) => mapped,
            Err(err) => {
                // SAFETY: `memory` was allocated above and is no longer used after map failure.
                unsafe { device.free_memory(memory, None) };
                return Err(format!("vkMapMemory for AV1 decode source failed: {err}"));
            }
        };
        // SAFETY: `mapped` points to at least `bytes.len()` writable bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.cast::<u8>(), bytes.len());
            device.unmap_memory(memory);
        }
        Ok(memory)
    })();

    match upload_result {
        Ok(memory) => Ok(Av1DecodeSourceBufferResource { buffer, memory }),
        Err(err) => {
            // SAFETY: The buffer was created above and is no longer used after upload failure.
            unsafe {
                device.destroy_buffer(buffer, None);
            }
            Err(err)
        }
    }
}

pub(super) fn destroy_av1_decode_source_buffer(
    device: &ash::Device,
    resource: Av1DecodeSourceBufferResource,
) {
    // SAFETY: The resource was created by this device and is no longer used.
    unsafe {
        device.destroy_buffer(resource.buffer, None);
        device.free_memory(resource.memory, None);
    }
}

pub(super) fn create_av1_decode_readback_buffer(
    instance: &ash::Instance,
    device: &ash::Device,
    physical_device: vk::PhysicalDevice,
    size: u64,
) -> Result<Av1DecodeReadbackBufferResource, String> {
    if size == 0 {
        return Err("AV1 decode readback buffer requires non-zero size".to_string());
    }

    let create_info = vk::BufferCreateInfo::default()
        .size(size)
        .usage(vk::BufferUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: Buffer create info contains only POD values.
    let buffer = unsafe { device.create_buffer(&create_info, None) }
        .map_err(|err| format!("vkCreateBuffer for AV1 decode readback failed: {err}"))?;

    let result = (|| -> Result<vk::DeviceMemory, String> {
        // SAFETY: `buffer` was created by this device.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        // SAFETY: `physical_device` belongs to `instance`; this only reads memory metadata.
        let memory_properties =
            unsafe { instance.get_physical_device_memory_properties(physical_device) };
        let memory_type_index = select_av1_decode_memory_type_index(
            &memory_properties,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or_else(|| {
            format!(
                "no HOST_VISIBLE|HOST_COHERENT memory type for AV1 decode readback buffer (bits=0x{:X})",
                requirements.memory_type_bits
            )
        })?;
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size.max(size))
            .memory_type_index(memory_type_index);
        // SAFETY: Allocation info is derived from Vulkan memory requirements.
        let memory = unsafe { device.allocate_memory(&allocate_info, None) }
            .map_err(|err| format!("vkAllocateMemory for AV1 decode readback failed: {err}"))?;
        // SAFETY: Buffer and memory were created by this device.
        if let Err(err) = unsafe { device.bind_buffer_memory(buffer, memory, 0) } {
            // SAFETY: `memory` was allocated above and has not been bound successfully.
            unsafe { device.free_memory(memory, None) };
            return Err(format!(
                "vkBindBufferMemory for AV1 decode readback failed: {err}"
            ));
        }
        Ok(memory)
    })();

    match result {
        Ok(memory) => Ok(Av1DecodeReadbackBufferResource {
            buffer,
            memory,
            size,
        }),
        Err(err) => {
            // SAFETY: The buffer was created above and is no longer used after failure.
            unsafe {
                device.destroy_buffer(buffer, None);
            }
            Err(err)
        }
    }
}

pub(super) fn initialize_av1_decode_readback_buffer(
    device: &ash::Device,
    resource: &Av1DecodeReadbackBufferResource,
) -> Result<(), String> {
    let mapped_len = usize::try_from(resource.size)
        .map_err(|_| "AV1 decode readback buffer size exceeds usize range".to_string())?;
    // SAFETY: The readback memory is HOST_VISIBLE|HOST_COHERENT and the requested range fits the
    // allocation used for the buffer.
    let mapped = unsafe {
        device.map_memory(
            resource.memory,
            0,
            resource.size,
            vk::MemoryMapFlags::empty(),
        )
    }
    .map_err(|err| format!("vkMapMemory for AV1 decode readback initialization failed: {err}"))?;
    // SAFETY: The mapped pointer is valid for `mapped_len` bytes until unmap.
    let mapped_slice = unsafe { std::slice::from_raw_parts_mut(mapped.cast::<u8>(), mapped_len) };
    mapped_slice.fill(0xcd);
    // SAFETY: HOST_COHERENT memory does not require an explicit flush, and the memory is no longer
    // accessed after unmap until the queued transfer writes it.
    unsafe {
        device.unmap_memory(resource.memory);
    }
    Ok(())
}

pub(super) fn map_av1_decode_readback_buffer(
    device: &ash::Device,
    resource: &Av1DecodeReadbackBufferResource,
) -> Result<Av1DecodeReadbackSample, String> {
    let mapped_len = usize::try_from(resource.size)
        .map_err(|_| "AV1 decode readback buffer size exceeds usize range".to_string())?;
    // SAFETY: The readback memory is HOST_VISIBLE|HOST_COHERENT and the requested range fits the
    // allocation used for the buffer.
    let mapped = unsafe {
        device.map_memory(
            resource.memory,
            0,
            resource.size,
            vk::MemoryMapFlags::empty(),
        )
    }
    .map_err(|err| format!("vkMapMemory for AV1 decode readback failed: {err}"))?;
    // SAFETY: The mapped pointer is valid for `mapped_len` bytes until unmap.
    let mapped_slice = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), mapped_len) };
    let data = mapped_slice.to_vec();
    let non_zero = mapped_slice.iter().any(|&byte| byte != 0);
    // SAFETY: The memory was mapped above and is no longer accessed after this point.
    unsafe {
        device.unmap_memory(resource.memory);
    }
    Ok(Av1DecodeReadbackSample {
        mapped_bytes: mapped_len,
        non_zero,
        data,
    })
}

pub(super) fn destroy_av1_decode_readback_buffer(
    device: &ash::Device,
    resource: Av1DecodeReadbackBufferResource,
) {
    // SAFETY: The resource was created by this device and is no longer used.
    unsafe {
        device.destroy_buffer(resource.buffer, None);
        device.free_memory(resource.memory, None);
    }
}

pub(super) fn create_av1_decode_image(
    instance: &ash::Instance,
    device: &ash::Device,
    physical_device: vk::PhysicalDevice,
    plan: &Av1DecodeImagePlan,
) -> Result<Av1DecodeImageResource, String> {
    let mut decode_av1_profile = vk::VideoDecodeAV1ProfileInfoKHR::default()
        .std_profile(StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_MAIN)
        .film_grain_support(false);
    let mut decode_usage = vk::VideoDecodeUsageInfoKHR::default()
        .video_usage_hints(vk::VideoDecodeUsageFlagsKHR::DEFAULT);
    let image_profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_AV1)
        .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
        .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .push_next(&mut decode_av1_profile)
        .push_next(&mut decode_usage);
    let image_profiles = [image_profile];
    let mut profile_list = vk::VideoProfileListInfoKHR::default().profiles(&image_profiles);
    let image_create_info = vk::ImageCreateInfo {
        p_next: (&mut profile_list as *mut vk::VideoProfileListInfoKHR<'_>).cast(),
        ..vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(plan.format)
            .extent(vk::Extent3D {
                width: plan.extent.width,
                height: plan.extent.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(plan.array_layers)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(plan.usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
    };
    // SAFETY: Image create info references a live profile list and device is valid.
    let image = unsafe { device.create_image(&image_create_info, None) }
        .map_err(|err| format!("vkCreateImage for AV1 decode image failed: {err}"))?;

    let result = (|| -> Result<(vk::DeviceMemory, vk::ImageView, vk::ImageView), String> {
        // SAFETY: `image` was created by this device.
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        // SAFETY: `physical_device` belongs to `instance`; this only reads memory metadata.
        let memory_properties =
            unsafe { instance.get_physical_device_memory_properties(physical_device) };
        let memory_type_index = select_av1_decode_memory_type_index(
            &memory_properties,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .or_else(|| {
            select_av1_decode_memory_type_index(
                &memory_properties,
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::empty(),
            )
        })
        .ok_or_else(|| {
            format!(
                "no compatible memory type for AV1 decode image (bits=0x{:X})",
                requirements.memory_type_bits
            )
        })?;
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size.max(1))
            .memory_type_index(memory_type_index);
        // SAFETY: Allocation info is derived from Vulkan image memory requirements.
        let memory = unsafe { device.allocate_memory(&allocate_info, None) }
            .map_err(|err| format!("vkAllocateMemory for AV1 decode image failed: {err}"))?;
        // SAFETY: image and memory belong to this device, and the allocation satisfies the
        // queried size, memory-type, and offset-alignment requirements.
        if let Err(err) = unsafe { device.bind_image_memory(image, memory, 0) } {
            // SAFETY: `memory` was allocated above and has not been bound successfully.
            unsafe { device.free_memory(memory, None) };
            return Err(format!(
                "vkBindImageMemory for AV1 decode image failed: {err}"
            ));
        }
        let view_type = if plan.array_layers == 1 {
            vk::ImageViewType::TYPE_2D
        } else {
            vk::ImageViewType::TYPE_2D_ARRAY
        };
        let mut dst_view_usage_create_info = vk::ImageViewUsageCreateInfo::default().usage(
            vk::ImageUsageFlags::VIDEO_DECODE_DST_KHR | vk::ImageUsageFlags::VIDEO_DECODE_DPB_KHR,
        );
        let dst_view_create_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(view_type)
            .format(plan.format)
            .push_next(&mut dst_view_usage_create_info)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: plan.array_layers,
            });
        // SAFETY: Image view create info references a valid image and subresource range.
        let dst_view =
            unsafe { device.create_image_view(&dst_view_create_info, None) }.map_err(|err| {
                // SAFETY: `memory` was allocated and bound above and is no longer used after failure.
                unsafe { device.free_memory(memory, None) };
                format!("vkCreateImageView for AV1 decode image failed: {err}")
            })?;
        let mut reference_view_usage_create_info = vk::ImageViewUsageCreateInfo::default()
            .usage(vk::ImageUsageFlags::VIDEO_DECODE_DPB_KHR);
        let reference_view_create_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(view_type)
            .format(plan.format)
            .push_next(&mut reference_view_usage_create_info)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: plan.array_layers,
            });
        // SAFETY: Image view create info references a valid image and subresource range.
        let reference_view = unsafe { device.create_image_view(&reference_view_create_info, None) }
            .map_err(|err| {
                // SAFETY: `dst_view` and `memory` were created above and are no longer used after failure.
                unsafe {
                    device.destroy_image_view(dst_view, None);
                    device.free_memory(memory, None);
                }
                format!("vkCreateImageView for AV1 decode reference image failed: {err}")
            })?;
        Ok((memory, dst_view, reference_view))
    })();

    match result {
        Ok((memory, dst_view, reference_view)) => Ok(Av1DecodeImageResource {
            image,
            memory,
            dst_view,
            reference_view,
        }),
        Err(err) => {
            // SAFETY: The image was created above and is no longer used.
            unsafe {
                device.destroy_image(image, None);
            }
            Err(err)
        }
    }
}

pub(super) fn destroy_av1_decode_image(device: &ash::Device, resource: Av1DecodeImageResource) {
    // SAFETY: The resource was created by this device and is no longer used.
    unsafe {
        device.destroy_image_view(resource.reference_view, None);
        device.destroy_image_view(resource.dst_view, None);
        device.destroy_image(resource.image, None);
        device.free_memory(resource.memory, None);
    }
}

pub(super) fn record_and_destroy_av1_decode_command_buffer(
    config: Av1DecodeCommandRecordConfig<'_>,
) -> Result<Av1DecodeCommandRecordSummary, String> {
    let device = config.device;
    let command_pool_info =
        vk::CommandPoolCreateInfo::default().queue_family_index(config.queue_family_index);
    // SAFETY: Command pool create info references the selected decode queue family.
    let command_pool = unsafe { device.create_command_pool(&command_pool_info, None) }
        .map_err(|err| format!("vkCreateCommandPool for AV1 decode record failed: {err}"))?;

    let result = (|| -> Result<Av1DecodeCommandRecordSummary, String> {
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: Allocate info references a live command pool.
        let command_buffers = unsafe { device.allocate_command_buffers(&allocate_info) }
            .map_err(|err| format!("vkAllocateCommandBuffers for AV1 decode failed: {err}"))?;
        let command_buffer = command_buffers.first().copied().ok_or_else(|| {
            "vkAllocateCommandBuffers returned no command buffer for AV1 decode".to_string()
        })?;
        let record_mode = av1_decode_command_buffer_record_mode_from_env();
        if config.submit_command_buffer && record_mode == Av1DecodeCommandBufferRecordMode::Full {
            record_submit_av1_decode_reset_command_buffer(
                device,
                config.instance,
                command_pool,
                config.queue_family_index,
                config.video_session,
                config.video_session_parameters,
            )?;
        }
        let status_query = if config.submit_command_buffer
            && record_mode == Av1DecodeCommandBufferRecordMode::Full
            && std::env::var("VIDEO_HW_VULKAN_AV1_QUERY_STATUS").as_deref() == Ok("1")
        {
            Some(create_av1_decode_status_query_resource(device)?)
        } else {
            None
        };

        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: Command buffer is valid and not already recording.
        unsafe { device.begin_command_buffer(command_buffer, &begin_info) }
            .map_err(|err| format!("vkBeginCommandBuffer for AV1 decode failed: {err}"))?;

        let source_barrier = Av1DecodeCommandSkeleton::vk_decode_source_memory_barrier();
        let image_barrier = config
            .command
            .vk_decode_image_init_barrier(config.decode_image)?;
        let dependency_info = vk::DependencyInfo::default()
            .memory_barriers(std::slice::from_ref(&source_barrier))
            .image_memory_barriers(std::slice::from_ref(&image_barrier));
        // SAFETY: Barriers reference live probe resources and command buffer is recording.
        unsafe {
            device.cmd_pipeline_barrier2(command_buffer, &dependency_info);
        }
        if record_mode == Av1DecodeCommandBufferRecordMode::BarrierOnly {
            // SAFETY: Command buffer is recording and only contains pipeline barriers.
            unsafe { device.end_command_buffer(command_buffer) }
                .map_err(|err| format!("vkEndCommandBuffer for AV1 decode failed: {err}"))?;
            return Ok(Av1DecodeCommandRecordSummary::default());
        }

        let video_queue_device = ash::khr::video_queue::Device::new(config.instance, device);
        let video_decode_device =
            ash::khr::video_decode_queue::Device::new(config.instance, device);
        let mut emitted_first_decode = false;
        let mut query_started = false;
        let mut record_summary = Av1DecodeCommandRecordSummary::default();
        if let Some(status_query) = &status_query {
            // SAFETY: Query pool belongs to this device and the command buffer is recording.
            unsafe {
                device.cmd_reset_query_pool(command_buffer, status_query.pool, 0, 1);
            }
        }
        config.upload_plan.visit_decode_command_sequence(
            config.command,
            config.video_session,
            config.video_session_parameters,
            config.source_buffer,
            Av1DecodePictureViews {
                dst: config.dst_image_view,
                reference: config.reference_image_view,
            },
            |visit| {
                match visit {
                    Av1DecodeCommandVisit::BeginCoding(info) => {
                        // SAFETY: Command buffer is recording; `info` references live resources.
                        unsafe {
                            (video_queue_device.fp().cmd_begin_video_coding_khr)(
                                command_buffer,
                                info,
                            );
                        }
                        record_summary.begin_count += 1;
                    }
                    Av1DecodeCommandVisit::ResetCoding(info) => {
                        if config.submit_command_buffer
                            && record_mode == Av1DecodeCommandBufferRecordMode::Full
                        {
                            record_summary.reset_count += 1;
                            return;
                        }
                        if record_mode == Av1DecodeCommandBufferRecordMode::BeginEnd {
                            return;
                        }
                        // SAFETY: Command buffer is inside a video coding scope.
                        unsafe {
                            (video_queue_device.fp().cmd_control_video_coding_khr)(
                                command_buffer,
                                info,
                            );
                        }
                        record_summary.reset_count += 1;
                    }
                    Av1DecodeCommandVisit::DecodeFrame { decode_info, .. } => {
                        if matches!(
                            record_mode,
                            Av1DecodeCommandBufferRecordMode::BeginEnd
                                | Av1DecodeCommandBufferRecordMode::ResetEnd
                        ) || (record_mode == Av1DecodeCommandBufferRecordMode::FirstDecode
                            && emitted_first_decode)
                        {
                            return;
                        }
                        // SAFETY: Command buffer is inside a video coding scope and decode info
                        // chains are scoped to this call.
                        unsafe {
                            if let Some(status_query) = &status_query
                                && !query_started
                            {
                                device.cmd_begin_query(
                                    command_buffer,
                                    status_query.pool,
                                    0,
                                    vk::QueryControlFlags::empty(),
                                );
                                query_started = true;
                            }
                            (video_decode_device.fp().cmd_decode_video_khr)(
                                command_buffer,
                                decode_info,
                            );
                        }
                        emitted_first_decode = true;
                        record_summary.decode_count += 1;
                    }
                    Av1DecodeCommandVisit::EndCoding(info) => {
                        // SAFETY: Command buffer is inside a video coding scope.
                        unsafe {
                            if let Some(status_query) = &status_query
                                && query_started
                            {
                                device.cmd_end_query(command_buffer, status_query.pool, 0);
                            }
                            (video_queue_device.fp().cmd_end_video_coding_khr)(
                                command_buffer,
                                info,
                            );
                        }
                        record_summary.end_count += 1;
                    }
                }
            },
        )?;

        if let Some(readback) = config.readback {
            if record_mode != Av1DecodeCommandBufferRecordMode::Full {
                return Err(
                    "AV1 decode readback requires full command-buffer record mode".to_string(),
                );
            }
            let decode_to_copy_image_barrier = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::VIDEO_DECODE_KHR)
                .src_access_mask(vk::AccessFlags2::VIDEO_DECODE_WRITE_KHR)
                .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                .old_layout(vk::ImageLayout::VIDEO_DECODE_DPB_KHR)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .image(config.decode_image)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: readback.plan.sample_count,
                });
            let decode_to_copy_dependency = vk::DependencyInfo::default()
                .image_memory_barriers(std::slice::from_ref(&decode_to_copy_image_barrier));
            // SAFETY: Decode image and readback buffer are live and command buffer is recording.
            unsafe {
                device.cmd_pipeline_barrier2(command_buffer, &decode_to_copy_dependency);
                device.cmd_copy_image_to_buffer(
                    command_buffer,
                    config.decode_image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    readback.buffer,
                    &readback.plan.regions,
                );
            }
            let copy_to_host_barrier = vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                .dst_access_mask(vk::AccessFlags2::HOST_READ);
            let copy_to_host_dependency = vk::DependencyInfo::default()
                .memory_barriers(std::slice::from_ref(&copy_to_host_barrier));
            // SAFETY: The barrier only orders transfer writes before host readback after submit.
            unsafe {
                device.cmd_pipeline_barrier2(command_buffer, &copy_to_host_dependency);
            }
        }

        // SAFETY: Command buffer is recording and all command data has been emitted.
        unsafe { device.end_command_buffer(command_buffer) }
            .map_err(|err| format!("vkEndCommandBuffer for AV1 decode failed: {err}"))?;
        if config.submit_command_buffer {
            submit_av1_decode_command_buffer(device, config.queue_family_index, command_buffer)?;
        }
        if let Some(status_query) = status_query {
            record_summary.decode_query_status_raw =
                Some(read_av1_decode_query_status(device, status_query.pool)?.as_raw());
            destroy_av1_decode_status_query_resource(device, status_query);
        }
        Ok(record_summary)
    })();

    // SAFETY: Command pool and allocated command buffers are no longer used.
    unsafe {
        device.destroy_command_pool(command_pool, None);
    }
    result
}

pub(super) fn create_av1_decode_status_query_resource(
    device: &ash::Device,
) -> Result<Av1DecodeStatusQueryResource, String> {
    let mut decode_av1_profile = vk::VideoDecodeAV1ProfileInfoKHR::default()
        .std_profile(StdVideoAV1Profile_STD_VIDEO_AV1_PROFILE_MAIN)
        .film_grain_support(false);
    let mut decode_usage = vk::VideoDecodeUsageInfoKHR::default()
        .video_usage_hints(vk::VideoDecodeUsageFlagsKHR::DEFAULT);
    let mut profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(vk::VideoCodecOperationFlagsKHR::DECODE_AV1)
        .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
        .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .push_next(&mut decode_av1_profile)
        .push_next(&mut decode_usage);
    let create_info = vk::QueryPoolCreateInfo::default()
        .query_type(vk::QueryType::RESULT_STATUS_ONLY_KHR)
        .query_count(1)
        .push_next(&mut profile);
    // SAFETY: Query pool create info references stack profile structs alive for the call.
    let pool = unsafe { device.create_query_pool(&create_info, None) }
        .map_err(|err| format!("vkCreateQueryPool for AV1 decode status failed: {err}"))?;
    Ok(Av1DecodeStatusQueryResource { pool })
}

pub(super) fn read_av1_decode_query_status(
    device: &ash::Device,
    query_pool: vk::QueryPool,
) -> Result<vk::QueryResultStatusKHR, String> {
    let mut status = vk::QueryResultStatusKHR::NOT_READY;
    // SAFETY: Query pool belongs to this device and query 0 was ended before submit completion.
    unsafe {
        device.get_query_pool_results(
            query_pool,
            0,
            std::slice::from_mut(&mut status),
            vk::QueryResultFlags::WAIT | vk::QueryResultFlags::WITH_STATUS_KHR,
        )
    }
    .map_err(|err| format!("vkGetQueryPoolResults for AV1 decode status failed: {err}"))?;
    Ok(status)
}

pub(super) fn destroy_av1_decode_status_query_resource(
    device: &ash::Device,
    resource: Av1DecodeStatusQueryResource,
) {
    // SAFETY: Query pool was created by this device and is no longer in use after submit wait.
    unsafe {
        device.destroy_query_pool(resource.pool, None);
    }
}

pub(super) fn record_submit_av1_decode_reset_command_buffer(
    device: &ash::Device,
    instance: &ash::Instance,
    command_pool: vk::CommandPool,
    queue_family_index: u32,
    video_session: vk::VideoSessionKHR,
    video_session_parameters: vk::VideoSessionParametersKHR,
) -> Result<(), String> {
    let allocate_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    // SAFETY: Allocate info references a live command pool.
    let reset_command_buffer = unsafe { device.allocate_command_buffers(&allocate_info) }
        .map_err(|err| format!("vkAllocateCommandBuffers for AV1 decode reset failed: {err}"))?
        .first()
        .copied()
        .ok_or_else(|| {
            "vkAllocateCommandBuffers returned no command buffer for AV1 decode reset".to_string()
        })?;
    let begin_info =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: Command buffer is valid and not already recording.
    unsafe { device.begin_command_buffer(reset_command_buffer, &begin_info) }
        .map_err(|err| format!("vkBeginCommandBuffer for AV1 decode reset failed: {err}"))?;

    let video_queue_device = ash::khr::video_queue::Device::new(instance, device);
    let begin_coding_info = vk::VideoBeginCodingInfoKHR::default()
        .video_session(video_session)
        .video_session_parameters(video_session_parameters);
    let reset_control =
        vk::VideoCodingControlInfoKHR::default().flags(vk::VideoCodingControlFlagsKHR::RESET);
    let end_coding_info = vk::VideoEndCodingInfoKHR::default();
    // SAFETY: Command buffer is recording; the reset scope references a live video session.
    unsafe {
        (video_queue_device.fp().cmd_begin_video_coding_khr)(
            reset_command_buffer,
            &begin_coding_info,
        );
        (video_queue_device.fp().cmd_control_video_coding_khr)(
            reset_command_buffer,
            &reset_control,
        );
        (video_queue_device.fp().cmd_end_video_coding_khr)(reset_command_buffer, &end_coding_info);
    }
    // SAFETY: Command buffer is recording and all reset command data has been emitted.
    unsafe { device.end_command_buffer(reset_command_buffer) }
        .map_err(|err| format!("vkEndCommandBuffer for AV1 decode reset failed: {err}"))?;
    submit_av1_decode_command_buffer(device, queue_family_index, reset_command_buffer)
}

pub(super) fn submit_av1_decode_command_buffer(
    device: &ash::Device,
    queue_family_index: u32,
    command_buffer: vk::CommandBuffer,
) -> Result<(), String> {
    let fence_info = vk::FenceCreateInfo::default();
    // SAFETY: Fence create info contains only POD values.
    let fence = unsafe { device.create_fence(&fence_info, None) }
        .map_err(|err| format!("vkCreateFence for AV1 decode submit failed: {err}"))?;
    let result = (|| -> Result<(), String> {
        // SAFETY: Queue family index is the decode queue family used to create the device.
        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };
        let command_buffers = [command_buffer];
        let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers);
        // SAFETY: Submit info references a recorded command buffer that stays alive until wait.
        unsafe { device.queue_submit(queue, std::slice::from_ref(&submit_info), fence) }
            .map_err(|err| format!("vkQueueSubmit for AV1 decode failed: {err}"))?;
        // SAFETY: Fence belongs to this device and was used for the submit above.
        unsafe { device.wait_for_fences(std::slice::from_ref(&fence), true, 5_000_000_000) }
            .map_err(|err| format!("vkWaitForFences for AV1 decode failed: {err}"))?;
        Ok(())
    })();
    // SAFETY: Fence is no longer used after the wait/error path.
    unsafe {
        device.destroy_fence(fence, None);
    }
    result
}

pub(super) fn av1_decode_command_buffer_record_mode_from_env() -> Av1DecodeCommandBufferRecordMode {
    match std::env::var("VIDEO_HW_VULKAN_AV1_RECORD_MODE")
        .ok()
        .as_deref()
    {
        Some("barrier_only") | Some("barrier") => Av1DecodeCommandBufferRecordMode::BarrierOnly,
        Some("begin_end") | Some("begin") => Av1DecodeCommandBufferRecordMode::BeginEnd,
        Some("reset_end") | Some("reset") => Av1DecodeCommandBufferRecordMode::ResetEnd,
        Some("first_decode") | Some("decode") => Av1DecodeCommandBufferRecordMode::FirstDecode,
        _ => Av1DecodeCommandBufferRecordMode::Full,
    }
}

pub(super) fn build_av1_decode_readback_plan(
    format: vk::Format,
    coded_width: u32,
    coded_height: u32,
    sample_count: u32,
) -> Result<Av1DecodeReadbackPlan, String> {
    if coded_width == 0 || coded_height == 0 {
        return Err(format!(
            "invalid coded extent for AV1 decode readback: {coded_width}x{coded_height}"
        ));
    }
    if sample_count == 0 {
        return Err("AV1 decode readback requires at least one sample".to_string());
    }

    let mut frame_regions = Vec::new();
    let mut next_offset = 0_u64;
    let mut push_region = |aspect_mask: vk::ImageAspectFlags,
                           plane_width: u32,
                           plane_height: u32,
                           bytes_per_texel: u64|
     -> Result<(), String> {
        let row_bytes = u64::from(plane_width)
            .checked_mul(bytes_per_texel)
            .ok_or_else(|| "AV1 decode readback row size overflowed u64".to_string())?;
        let plane_bytes = row_bytes
            .checked_mul(u64::from(plane_height))
            .ok_or_else(|| "AV1 decode readback plane size overflowed u64".to_string())?;
        let offset = align_av1_decode_readback_offset(next_offset, 4);
        frame_regions.push(
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
            .ok_or_else(|| "AV1 decode readback buffer size overflowed u64".to_string())?;
        Ok(())
    };

    match format {
        vk::Format::G8_B8R8_2PLANE_420_UNORM => {
            push_region(vk::ImageAspectFlags::PLANE_0, coded_width, coded_height, 1)?;
            push_region(
                vk::ImageAspectFlags::PLANE_1,
                coded_width.div_ceil(2),
                coded_height.div_ceil(2),
                2,
            )?;
        }
        other => {
            return Err(format!(
                "AV1 decode readback is not implemented for output format {other:?}"
            ));
        }
    }

    let sample_stride = align_av1_decode_readback_offset(next_offset, 4);
    let mut regions = Vec::with_capacity(
        frame_regions
            .len()
            .saturating_mul(usize::try_from(sample_count).unwrap_or(usize::MAX)),
    );
    for sample_index in 0..sample_count {
        let sample_offset = sample_stride
            .checked_mul(u64::from(sample_index))
            .ok_or_else(|| "AV1 decode readback sample offset overflowed u64".to_string())?;
        for region in &frame_regions {
            let mut region = *region;
            region.buffer_offset = sample_offset
                .checked_add(region.buffer_offset)
                .ok_or_else(|| "AV1 decode readback region offset overflowed u64".to_string())?;
            region.image_subresource.base_array_layer = sample_index;
            regions.push(region);
        }
    }
    let buffer_size = sample_stride
        .checked_mul(u64::from(sample_count))
        .ok_or_else(|| "AV1 decode readback buffer size overflowed u64".to_string())?;

    Ok(Av1DecodeReadbackPlan {
        buffer_size,
        sample_stride,
        sample_count,
        regions,
    })
}

pub(super) fn align_av1_decode_readback_offset(value: u64, alignment: u64) -> u64 {
    if alignment <= 1 {
        return value;
    }
    value.div_ceil(alignment) * alignment
}

pub(super) fn create_av1_decode_session_parameters(
    device: &ash::Device,
    video_queue_device: &ash::khr::video_queue::Device,
    video_session: vk::VideoSessionKHR,
    std_sequence_header: &StdVideoAV1SequenceHeader,
) -> Result<vk::VideoSessionParametersKHR, String> {
    let color_config = default_av1_main_420_8bit_color_config();
    let timing_info = default_av1_timing_info();
    let mut std_sequence_header = *std_sequence_header;
    std_sequence_header.pColorConfig = &color_config;
    std_sequence_header.pTimingInfo = &timing_info;
    let mut decode_av1_session_parameters =
        vk::VideoDecodeAV1SessionParametersCreateInfoKHR::default()
            .std_sequence_header(&std_sequence_header);
    let create_info = vk::VideoSessionParametersCreateInfoKHR::default()
        .video_session(video_session)
        .video_session_parameters_template(vk::VideoSessionParametersKHR::null())
        .push_next(&mut decode_av1_session_parameters);
    let mut video_session_parameters = vk::VideoSessionParametersKHR::null();

    // SAFETY: `create_info` references stack data alive for this call.
    let result = unsafe {
        (video_queue_device.fp().create_video_session_parameters_khr)(
            device.handle(),
            &create_info,
            std::ptr::null(),
            &mut video_session_parameters,
        )
    };
    if result != vk::Result::SUCCESS {
        return Err(format!(
            "vkCreateVideoSessionParametersKHR for AV1 decode failed: {result:?}"
        ));
    }

    Ok(video_session_parameters)
}

pub(super) fn default_av1_timing_info() -> StdVideoAV1TimingInfo {
    StdVideoAV1TimingInfo {
        flags: StdVideoAV1TimingInfoFlags {
            _bitfield_align_1: [],
            _bitfield_1: StdVideoAV1TimingInfoFlags::new_bitfield_1(0, 0),
        },
        num_units_in_display_tick: 0,
        time_scale: 0,
        num_ticks_per_picture_minus_1: 0,
    }
}

pub(super) fn default_av1_main_420_8bit_color_config() -> StdVideoAV1ColorConfig {
    StdVideoAV1ColorConfig {
        flags: StdVideoAV1ColorConfigFlags {
            _bitfield_align_1: [],
            _bitfield_1: StdVideoAV1ColorConfigFlags::new_bitfield_1(0, 0, 0, 0, 0),
        },
        BitDepth: 8,
        subsampling_x: 1,
        subsampling_y: 1,
        reserved1: 0,
        color_primaries: StdVideoAV1ColorPrimaries_STD_VIDEO_AV1_COLOR_PRIMARIES_BT_UNSPECIFIED,
        transfer_characteristics:
            StdVideoAV1TransferCharacteristics_STD_VIDEO_AV1_TRANSFER_CHARACTERISTICS_UNSPECIFIED,
        matrix_coefficients:
            StdVideoAV1MatrixCoefficients_STD_VIDEO_AV1_MATRIX_COEFFICIENTS_UNSPECIFIED,
        chroma_sample_position:
            StdVideoAV1ChromaSamplePosition_STD_VIDEO_AV1_CHROMA_SAMPLE_POSITION_UNKNOWN,
    }
}

pub(super) fn destroy_av1_decode_session_resource(
    instance: &ash::Instance,
    device: &ash::Device,
    mut resource: Av1DecodeSessionResource,
) {
    let video_queue_device = ash::khr::video_queue::Device::new(instance, device);
    // SAFETY: The handles were created by this device and are no longer used.
    unsafe {
        (video_queue_device.fp().destroy_video_session_parameters_khr)(
            device.handle(),
            resource.parameters,
            std::ptr::null(),
        );
        for memory in resource.memories.drain(..) {
            device.free_memory(memory, None);
        }
        (video_queue_device.fp().destroy_video_session_khr)(
            device.handle(),
            resource.session,
            std::ptr::null(),
        );
    }
}

pub(super) fn build_probe_av1_std_sequence_header(
    coded_width: u32,
    coded_height: u32,
) -> Result<StdVideoAV1SequenceHeader, String> {
    if coded_width == 0 || coded_height == 0 {
        return Err(format!(
            "AV1 probe sequence header dimensions must be positive, got {coded_width}x{coded_height}"
        ));
    }
    let width_minus_1 = coded_width - 1;
    let height_minus_1 = coded_height - 1;
    let frame_width_bits_minus_1 = u8::try_from(31 - width_minus_1.leading_zeros())
        .map_err(|_| "AV1 probe width bit count does not fit in u8".to_string())?;
    let frame_height_bits_minus_1 = u8::try_from(31 - height_minus_1.leading_zeros())
        .map_err(|_| "AV1 probe height bit count does not fit in u8".to_string())?;
    build_av1_std_sequence_header(&ParsedAv1SequenceHeader {
        seq_profile: 0,
        still_picture: true,
        reduced_still_picture_header: true,
        frame_width_bits_minus_1,
        frame_height_bits_minus_1,
        max_frame_width_minus_1: width_minus_1,
        max_frame_height_minus_1: height_minus_1,
        use_128x128_superblock: false,
        enable_filter_intra: false,
        enable_intra_edge_filter: false,
        enable_interintra_compound: false,
        enable_masked_compound: false,
        enable_warped_motion: false,
        enable_dual_filter: false,
        enable_order_hint: false,
        enable_jnt_comp: false,
        enable_ref_frame_mvs: false,
        frame_id_numbers_present_flag: false,
        enable_superres: false,
        enable_cdef: false,
        enable_restoration: false,
        film_grain_params_present: false,
        separate_uv_delta_q: false,
        timing_info_present_flag: false,
        initial_display_delay_present_flag: false,
        order_hint_bits_minus_1: 0,
        seq_force_screen_content_tools: AV1_SELECT_SCREEN_CONTENT_TOOLS,
        seq_force_integer_mv: AV1_SELECT_INTEGER_MV,
    })
}

pub(super) fn preferred_av1_probe_extent(min: u32, max: u32) -> u32 {
    16.clamp(min, max)
}

pub(super) fn create_av1_decode_device(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
) -> Result<ash::Device, String> {
    let priorities = [1.0_f32];
    let queue_create_info = vk::DeviceQueueCreateInfo::default()
        .queue_family_index(queue_family_index)
        .queue_priorities(&priorities);
    let extension_names = [
        vk::KHR_VIDEO_QUEUE_NAME.as_ptr(),
        vk::KHR_VIDEO_DECODE_QUEUE_NAME.as_ptr(),
        vk::KHR_VIDEO_DECODE_AV1_NAME.as_ptr(),
        vk::KHR_SYNCHRONIZATION2_NAME.as_ptr(),
    ];
    let mut synchronization2_features =
        vk::PhysicalDeviceSynchronization2Features::default().synchronization2(true);
    let create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(std::slice::from_ref(&queue_create_info))
        .enabled_extension_names(&extension_names)
        .push_next(&mut synchronization2_features);

    // SAFETY: All pointers referenced by `create_info` live until the call returns.
    unsafe { instance.create_device(physical_device, &create_info, None) }
        .map_err(|err| format!("logical device initialization failed: {err}"))
}
