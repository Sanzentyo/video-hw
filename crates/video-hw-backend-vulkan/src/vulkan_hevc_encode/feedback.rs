//! Encode-feedback query setup and bitstream readback interpretation.

use super::*;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct HevcEncodeFeedbackQueryResult {
    pub(super) offset: u32,
    pub(super) bytes_written: u32,
    pub(super) status: vk::QueryResultStatusKHR,
}

pub(super) struct HevcEncodeFeedbackOutput {
    pub(super) query_result: HevcEncodeFeedbackQueryResult,
    pub(super) bitstream_memory_offset: u64,
    pub(super) head16: String,
    pub(super) bitstream: Vec<u8>,
}

impl Default for HevcEncodeFeedbackQueryResult {
    fn default() -> Self {
        Self {
            offset: 0,
            bytes_written: 0,
            status: vk::QueryResultStatusKHR::NOT_READY,
        }
    }
}

pub(super) fn create_hevc_encode_feedback_query_pool(
    device: &ash::Device,
    supported_flags: vk::VideoEncodeFeedbackFlagsKHR,
) -> Result<vk::QueryPool, String> {
    let flags = vk::VideoEncodeFeedbackFlagsKHR::BITSTREAM_BYTES_WRITTEN
        | if supported_flags.contains(vk::VideoEncodeFeedbackFlagsKHR::BITSTREAM_BUFFER_OFFSET) {
            vk::VideoEncodeFeedbackFlagsKHR::BITSTREAM_BUFFER_OFFSET
        } else {
            vk::VideoEncodeFeedbackFlagsKHR::empty()
        };
    let mut feedback_create_info =
        vk::QueryPoolVideoEncodeFeedbackCreateInfoKHR::default().encode_feedback_flags(flags);
    let mut h265_profile = vk::VideoEncodeH265ProfileInfoKHR::default()
        .std_profile_idc(StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
    let mut encode_usage = vk::VideoEncodeUsageInfoKHR::default()
        .video_usage_hints(vk::VideoEncodeUsageFlagsKHR::DEFAULT);
    let mut profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(vk::VideoCodecOperationFlagsKHR::ENCODE_H265)
        .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
        .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .push_next(&mut encode_usage)
        .push_next(&mut h265_profile);
    let create_info = vk::QueryPoolCreateInfo::default()
        .query_type(vk::QueryType::VIDEO_ENCODE_FEEDBACK_KHR)
        .query_count(1)
        .push_next(&mut profile)
        .push_next(&mut feedback_create_info);
    // SAFETY: query pool create info references stack data that lives through the call.
    unsafe { device.create_query_pool(&create_info, None) }
        .map_err(|err| format!("vkCreateQueryPool for HEVC encode feedback failed: {err}"))
}

pub(super) fn read_hevc_encode_feedback_query(
    device: &ash::Device,
    query_pool: vk::QueryPool,
) -> Result<HevcEncodeFeedbackQueryResult, String> {
    let mut result = [HevcEncodeFeedbackQueryResult::default()];
    // SAFETY: query pool is valid, result buffer matches the requested query count.
    unsafe {
        device.get_query_pool_results(
            query_pool,
            0,
            &mut result,
            vk::QueryResultFlags::WAIT | vk::QueryResultFlags::WITH_STATUS_KHR,
        )
    }
    .map_err(|err| format!("vkGetQueryPoolResults for HEVC encode feedback failed: {err}"))?;
    Ok(result[0])
}

pub(super) fn read_hevc_encode_feedback_output(
    device: &ash::Device,
    query_pool: vk::QueryPool,
    dst_buffer_memory: vk::DeviceMemory,
    dst_buffer_offset: u64,
) -> Result<HevcEncodeFeedbackOutput, String> {
    let query_result = read_hevc_encode_feedback_query(device, query_pool)?;
    if query_result.status != vk::QueryResultStatusKHR::COMPLETE {
        return Err(format!(
            "encode feedback status was not COMPLETE: {:?}",
            query_result.status
        ));
    }
    let bitstream_memory_offset =
        hevc_encode_probe_bitstream_memory_offset(dst_buffer_offset, query_result)?;
    let bitstream = read_hevc_encode_dst_bytes(
        device,
        dst_buffer_memory,
        bitstream_memory_offset,
        query_result.bytes_written,
    )?;
    if let Some(output_path) = hevc_encode_probe_output_path() {
        std::fs::write(&output_path, &bitstream).map_err(|err| {
            format!(
                "failed to write HEVC encode probe output to '{}': {err}",
                output_path.display()
            )
        })?;
    }
    let head16 = hevc_encode_head16(&bitstream);
    Ok(HevcEncodeFeedbackOutput {
        query_result,
        bitstream_memory_offset,
        head16,
        bitstream,
    })
}

pub(super) fn hevc_encode_probe_bitstream_memory_offset(
    dst_buffer_offset: u64,
    query_result: HevcEncodeFeedbackQueryResult,
) -> Result<u64, String> {
    dst_buffer_offset
        .checked_add(u64::from(query_result.offset))
        .ok_or_else(|| "encode feedback bitstream memory offset overflow".to_string())
}

pub(super) fn read_hevc_encode_dst_bytes(
    device: &ash::Device,
    dst_buffer_memory: vk::DeviceMemory,
    offset: u64,
    bytes_to_read: u32,
) -> Result<Vec<u8>, String> {
    if bytes_to_read == 0 {
        return Ok(Vec::new());
    }
    let size = u64::from(bytes_to_read);
    // SAFETY: memory is owned by this device and the requested range is limited to the
    // implementation-reported encoded byte count.
    let ptr =
        unsafe { device.map_memory(dst_buffer_memory, offset, size, vk::MemoryMapFlags::empty()) }
            .map_err(|err| format!("vkMapMemory for HEVC encode dst output failed: {err}"))?;
    // SAFETY: mapped range is valid for `bytes_to_read` bytes until unmap.
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), bytes_to_read as usize) };
    let output = bytes.to_vec();
    // SAFETY: pointer was returned by `vkMapMemory` above.
    unsafe {
        device.unmap_memory(dst_buffer_memory);
    }
    Ok(output)
}

pub(super) fn hevc_encode_head16(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}
