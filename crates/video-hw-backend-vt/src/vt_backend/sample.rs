//! VideoToolbox access-unit packing, format descriptions, and encoded sample metadata.

use super::*;

pub(super) struct PackedSample {
    pub(super) data: Vec<u8>,
}

pub(super) trait SamplePacker {
    fn pack(&mut self, access_unit: &AccessUnit) -> Result<PackedSample, BackendError>;
}

#[derive(Debug, Default)]
struct AvccHvccPacker;

impl SamplePacker for AvccHvccPacker {
    fn pack(&mut self, access_unit: &AccessUnit) -> Result<PackedSample, BackendError> {
        let total_size = access_unit.nalus.iter().try_fold(0usize, |total, nal| {
            let framed_len = nal.len().checked_add(4).ok_or_else(|| {
                BackendError::InvalidBitstream("length-prefixed NAL size overflow".to_string())
            })?;
            total.checked_add(framed_len).ok_or_else(|| {
                BackendError::InvalidBitstream(
                    "length-prefixed access unit size overflow".to_string(),
                )
            })
        })?;
        let mut data = Vec::with_capacity(total_size);

        for nal in &access_unit.nalus {
            let len = u32::try_from(nal.len()).map_err(|_| {
                BackendError::InvalidBitstream(format!(
                    "NAL unit exceeds 32-bit AVCC/HVCC length: {} bytes",
                    nal.len()
                ))
            })?;
            let len = len.to_be_bytes();
            data.extend_from_slice(&len);
            data.extend_from_slice(nal);
        }

        Ok(PackedSample { data })
    }
}

#[derive(Debug)]
pub(super) struct VtSamplePacker<'a> {
    av1_config_obus: Option<&'a [u8]>,
}

impl<'a> VtSamplePacker<'a> {
    pub(super) fn new(av1_config_obus: Option<&'a [u8]>) -> Self {
        Self { av1_config_obus }
    }
}

impl SamplePacker for VtSamplePacker<'_> {
    fn pack(&mut self, access_unit: &AccessUnit) -> Result<PackedSample, BackendError> {
        if let Some(config_obus) = self.av1_config_obus {
            let mut data = access_unit.nalus.concat();
            if data.starts_with(config_obus) {
                data.drain(..config_obus.len());
            }
            return Ok(PackedSample { data });
        }
        AvccHvccPacker.pack(access_unit)
    }
}

pub(super) fn create_format_description(
    codec: Codec,
    parameter_sets: &[Vec<u8>],
    options: &VtDecoderOptions,
) -> Result<CMVideoFormatDescription, BackendError> {
    let refs = parameter_sets
        .iter()
        .map(|v| v.as_slice())
        .collect::<Vec<_>>();
    match codec {
        Codec::H264 => {
            CMVideoFormatDescription::from_h264_parameter_sets(&refs, 4).map_err(|status| {
                cm_error("CMVideoFormatDescription::from_h264_parameter_sets", status)
            })
        }
        Codec::Hevc => {
            CMVideoFormatDescription::from_hevc_parameter_sets(&refs, 4, Some(&empty_dictionary()))
                .map_err(|status| {
                    cm_error("CMVideoFormatDescription::from_hevc_parameter_sets", status)
                })
        }
        Codec::Av1 => create_av1_format_description(options),
    }
}

fn create_av1_format_description(
    options: &VtDecoderOptions,
) -> Result<CMVideoFormatDescription, BackendError> {
    let width = options.video_width.ok_or_else(|| {
        BackendError::UnsupportedConfig(
            "VideoToolbox AV1 decode requires fMP4 track width in VtDecoderOptions".to_string(),
        )
    })?;
    let height = options.video_height.ok_or_else(|| {
        BackendError::UnsupportedConfig(
            "VideoToolbox AV1 decode requires fMP4 track height in VtDecoderOptions".to_string(),
        )
    })?;
    let av1c_record = options.av1c_record.as_ref().ok_or_else(|| {
        BackendError::UnsupportedConfig(
            "VideoToolbox AV1 decode requires av1C record in VtDecoderOptions".to_string(),
        )
    })?;
    if options
        .av1_config_obus
        .as_deref()
        .unwrap_or_default()
        .is_empty()
    {
        return Err(BackendError::UnsupportedConfig(
            "VideoToolbox AV1 decode requires av1C config OBUs in VtDecoderOptions".to_string(),
        ));
    }

    let av1c_data = CFData::from_buffer(av1c_record);
    let av1c_type = av1c_data.as_CFType();
    let mut atoms = CFMutableDictionary::<CFString, CFType>::new();
    atoms.add(&CFString::new("av1C"), &av1c_type);

    let mut extensions = CFMutableDictionary::<CFString, CFType>::new();
    // SAFETY: this wraps CoreMedia's process-lifetime constant string without transferring it.
    let sample_description_atoms_key = unsafe {
        CFString::wrap_under_get_rule(kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms)
    };
    let atoms_dict = atoms.to_immutable();
    let atoms_type = atoms_dict.as_CFType();
    extensions.add(&sample_description_atoms_key, &atoms_type);

    CMVideoFormatDescription::new(
        kCMVideoCodecType_AV1,
        i32::from(width),
        i32::from(height),
        Some(&extensions.to_immutable()),
    )
    .map_err(|status| vt_error("CMVideoFormatDescriptionCreate(AV1)", status))
}

pub(super) fn empty_dictionary() -> CFDictionary<CFString, CFType> {
    CFMutableDictionary::<CFString, CFType>::new().to_immutable()
}

pub(super) fn decode_destination_image_buffer_attributes(
    output_mode: DecodeOutputMode,
) -> Option<CFDictionary<CFString, CFType>> {
    if matches!(output_mode, DecodeOutputMode::Metadata) {
        return None;
    }

    let mut attributes = CFMutableDictionary::<CFString, CFType>::new();
    attributes.add(
        &CFString::from(CVPixelBufferKeys::PixelFormatType),
        &CFNumber::from(kCVPixelFormatType_32BGRA as i32).as_CFType(),
    );
    Some(attributes.to_immutable())
}

pub(super) fn detect_keyframe_from_avcc_hvcc_payload(codec: Codec, payload: &[u8]) -> Option<bool> {
    let mut offset = 0usize;
    let mut saw_slice = false;
    let mut saw_irap = false;

    while offset.saturating_add(4) <= payload.len() {
        let len = u32::from_be_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]) as usize;
        offset = offset.saturating_add(4);

        if len == 0 || offset.saturating_add(len) > payload.len() {
            break;
        }
        let nalu = &payload[offset..offset + len];
        offset = offset.saturating_add(len);
        if nalu.is_empty() {
            continue;
        }

        match codec {
            Codec::H264 => {
                let nalu_type = nalu[0] & 0x1f;
                if nalu_type == 5 {
                    saw_irap = true;
                    saw_slice = true;
                } else if (1..=5).contains(&nalu_type) {
                    saw_slice = true;
                }
            }
            Codec::Hevc => {
                // HEVC nal_unit_type in bits[6:1] of first byte.
                let nalu_type = (nalu[0] >> 1) & 0x3f;
                if (16..=21).contains(&nalu_type) {
                    saw_irap = true;
                    saw_slice = true;
                } else if nalu_type <= 31 {
                    saw_slice = true;
                }
            }
            Codec::Av1 => return None,
        }
    }

    if saw_slice { Some(saw_irap) } else { None }
}

pub(super) fn parameter_sets_from_sample_buffer_ref(
    codec: Codec,
    sample_buffer_ref: CMSampleBufferRef,
) -> Option<Vec<Vec<u8>>> {
    // SAFETY: the encode callback already established that `sample_buffer_ref` is non-null and
    // keeps the sample alive while its borrowed format-description reference is inspected.
    let format_description_ref = unsafe { CMSampleBufferGetFormatDescription(sample_buffer_ref) };
    if format_description_ref.is_null() {
        return None;
    }

    match codec {
        Codec::H264 => h264_parameter_sets(format_description_ref),
        Codec::Hevc => hevc_parameter_sets(format_description_ref),
        Codec::Av1 => None,
    }
}

fn h264_parameter_sets(format_description_ref: CMFormatDescriptionRef) -> Option<Vec<Vec<u8>>> {
    let mut parameter_set_pointer = std::ptr::null();
    let mut parameter_set_size = 0usize;
    let mut parameter_set_count = 0usize;
    let mut nal_unit_header_length = 0i32;
    // SAFETY: `format_description_ref` is valid for this call; all output pointers target local
    // storage, and index zero requests the first H.264 parameter set.
    let status = unsafe {
        CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            format_description_ref,
            0,
            &mut parameter_set_pointer,
            &mut parameter_set_size,
            &mut parameter_set_count,
            &mut nal_unit_header_length,
        )
    };
    if status != 0 || parameter_set_pointer.is_null() {
        return None;
    }
    let mut sets = Vec::with_capacity(parameter_set_count);
    // SAFETY: CoreMedia returned a non-null pointer and byte length for the retained format
    // description; the bytes are copied before that description can be released.
    sets.push(
        unsafe { std::slice::from_raw_parts(parameter_set_pointer, parameter_set_size) }.to_vec(),
    );
    for index in 1..parameter_set_count {
        let mut parameter_set_pointer = std::ptr::null();
        let mut parameter_set_size = 0usize;
        // SAFETY: the format description remains retained, the index is below the count returned
        // by CoreMedia, and both output pointers target local storage.
        let status = unsafe {
            CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                format_description_ref,
                index,
                &mut parameter_set_pointer,
                &mut parameter_set_size,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if status != 0 || parameter_set_pointer.is_null() {
            return None;
        }
        // SAFETY: CoreMedia returned a non-null pointer and length for this indexed set; copy it
        // while the owning format description is retained.
        sets.push(
            unsafe { std::slice::from_raw_parts(parameter_set_pointer, parameter_set_size) }
                .to_vec(),
        );
    }
    Some(sets)
}

fn hevc_parameter_sets(format_description_ref: CMFormatDescriptionRef) -> Option<Vec<Vec<u8>>> {
    let mut parameter_set_pointer = std::ptr::null();
    let mut parameter_set_size = 0usize;
    let mut parameter_set_count = 0usize;
    let mut nal_unit_header_length = 0i32;
    // SAFETY: `format_description_ref` is valid for this call; all output pointers target local
    // storage, and index zero requests the first HEVC parameter set.
    let status = unsafe {
        CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
            format_description_ref,
            0,
            &mut parameter_set_pointer,
            &mut parameter_set_size,
            &mut parameter_set_count,
            &mut nal_unit_header_length,
        )
    };
    if status != 0 || parameter_set_pointer.is_null() {
        return None;
    }
    let mut sets = Vec::with_capacity(parameter_set_count);
    // SAFETY: CoreMedia returned a non-null pointer and byte length for the retained format
    // description; the bytes are copied before that description can be released.
    sets.push(
        unsafe { std::slice::from_raw_parts(parameter_set_pointer, parameter_set_size) }.to_vec(),
    );
    for index in 1..parameter_set_count {
        let mut parameter_set_pointer = std::ptr::null();
        let mut parameter_set_size = 0usize;
        // SAFETY: the format description remains retained, the index is below the count returned
        // by CoreMedia, and both output pointers target local storage.
        let status = unsafe {
            CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
                format_description_ref,
                index,
                &mut parameter_set_pointer,
                &mut parameter_set_size,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if status != 0 || parameter_set_pointer.is_null() {
            return None;
        }
        // SAFETY: CoreMedia returned a non-null pointer and length for this indexed set; copy it
        // while the owning format description is retained.
        sets.push(
            unsafe { std::slice::from_raw_parts(parameter_set_pointer, parameter_set_size) }
                .to_vec(),
        );
    }
    Some(sets)
}

pub(super) fn prepend_length_prefixed_parameter_sets(
    sample: &mut Vec<u8>,
    parameter_sets: &[Vec<u8>],
) -> Result<(), BackendError> {
    if parameter_sets.is_empty() {
        return Ok(());
    }

    let parameter_bytes = parameter_sets.iter().try_fold(0usize, |total, set| {
        let framed_len = set.len().checked_add(4).ok_or_else(|| {
            BackendError::Backend("VideoToolbox parameter-set size overflow".to_string())
        })?;
        total.checked_add(framed_len).ok_or_else(|| {
            BackendError::Backend("VideoToolbox parameter-set total size overflow".to_string())
        })
    })?;
    let capacity = parameter_bytes.checked_add(sample.len()).ok_or_else(|| {
        BackendError::Backend("VideoToolbox encoded sample size overflow".to_string())
    })?;
    let mut prefixed = Vec::with_capacity(capacity);
    for parameter_set in parameter_sets {
        let len = u32::try_from(parameter_set.len()).map_err(|_| {
            BackendError::Backend(format!(
                "VideoToolbox parameter set exceeds 32-bit length: {} bytes",
                parameter_set.len()
            ))
        })?;
        let len = len.to_be_bytes();
        prefixed.extend_from_slice(&len);
        prefixed.extend_from_slice(parameter_set);
    }
    prefixed.extend_from_slice(sample);
    *sample = prefixed;
    Ok(())
}
