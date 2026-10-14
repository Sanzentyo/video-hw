//! Decode readback, pixel-layout adaptation, and encoder-input validation.

use super::*;

pub(super) fn build_backend_frames(
    decoded: Vec<VkFrame<RawFrameData>>,
    output_mode: DecodeOutputMode,
    start_pts_90k: i64,
    fps: i32,
) -> Result<Vec<Frame>, BackendError> {
    let pts_step = decode_pts_step(fps);
    decoded
        .into_iter()
        .enumerate()
        .map(|(index, decoded_frame)| {
            let width = usize::try_from(decoded_frame.data.width).map_err(|_| {
                BackendError::InvalidInput("decoded width does not fit in usize".to_string())
            })?;
            let height = usize::try_from(decoded_frame.data.height).map_err(|_| {
                BackendError::InvalidInput("decoded height does not fit in usize".to_string())
            })?;
            let pts_90k =
                Some(start_pts_90k.saturating_add(usize_to_i64(index).saturating_mul(pts_step)));
            let mut frame = metadata_only_frame(width, height, pts_90k);
            match output_mode {
                DecodeOutputMode::Metadata => {}
                DecodeOutputMode::Nv12 => {
                    frame.pixel_format = Some(u32::from_le_bytes(*b"NV12"));
                    frame.nv12 = Some(raw_nv12_payload(&decoded_frame.data)?);
                }
                DecodeOutputMode::Rgb24 => {
                    frame.rgb24 = Some(raw_nv12_to_rgb24(&decoded_frame.data)?);
                }
            }
            Ok(frame)
        })
        .collect::<Result<Vec<_>, BackendError>>()
}

pub(super) fn metadata_only_frame(width: usize, height: usize, pts_90k: Option<i64>) -> Frame {
    Frame {
        width,
        height,
        pixel_format: None,
        pts_90k,
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        argb: None,
        rgb24: None,
        nv12: None,
        force_keyframe: false,
    }
}

pub(super) fn build_hevc_metadata_decode_info_flags(
    slot: usize,
    probe_covered: bool,
    readback_non_zero: bool,
) -> u32 {
    let slot_bits = u32::try_from(slot).unwrap_or(u32::MAX) << HEVC_DECODE_INFO_SLOT_SHIFT;
    let readback_bits = if probe_covered && readback_non_zero {
        HEVC_DECODE_INFO_READBACK_NON_ZERO_FLAG
    } else {
        0
    };
    let probe_bits = if probe_covered {
        HEVC_DECODE_INFO_PROBE_COVERED_FLAG
    } else {
        0
    };
    slot_bits | readback_bits | probe_bits
}

pub(super) fn hevc_probe_readback_sample_at(
    readback_sample: &[u8],
    sample_stride: usize,
    sample_index: usize,
) -> Result<&[u8], BackendError> {
    if sample_stride == 0 {
        return Err(BackendError::UnsupportedConfig(
            "Vulkan HEVC readback sample stride must be greater than zero".to_string(),
        ));
    }
    let start = sample_index.checked_mul(sample_stride).ok_or_else(|| {
        BackendError::UnsupportedConfig("Vulkan HEVC readback sample offset overflow".to_string())
    })?;
    let end = start.checked_add(sample_stride).ok_or_else(|| {
        BackendError::UnsupportedConfig(
            "Vulkan HEVC readback sample end offset overflow".to_string(),
        )
    })?;
    let sample = readback_sample.get(start..end).ok_or_else(|| {
        BackendError::UnsupportedConfig(format!(
            "Vulkan HEVC readback sample range out of bounds: index={sample_index}, stride={sample_stride}, total={}",
            readback_sample.len()
        ))
    })?;
    Ok(sample)
}

pub(super) fn hevc_probe_readback_to_nv12_payload(
    output_format: vk::Format,
    readback_sample: &[u8],
    width: usize,
    height: usize,
) -> Result<Nv12FramePayload, BackendError> {
    if output_format != vk::Format::G8_B8R8_2PLANE_420_UNORM {
        return Err(BackendError::UnsupportedConfig(format!(
            "Vulkan HEVC non-metadata output currently requires G8_B8R8_2PLANE_420_UNORM readback (got {output_format:?})"
        )));
    }
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::UnsupportedConfig(format!(
            "Vulkan HEVC non-metadata output currently requires even coded extent, got {}x{}",
            width, height
        )));
    }
    let expected_len = width
        .checked_mul(height)
        .and_then(|y| y.checked_add(y / 2))
        .ok_or_else(|| {
            BackendError::UnsupportedConfig(
                "Vulkan HEVC readback size overflow while preparing NV12 payload".to_string(),
            )
        })?;
    if readback_sample.len() < expected_len {
        return Err(BackendError::UnsupportedConfig(format!(
            "Vulkan HEVC readback sample too short for NV12 payload: got {}, need at least {expected_len}",
            readback_sample.len(),
        )));
    }
    Ok(Nv12FramePayload {
        pitch: width,
        data: readback_sample[..expected_len].to_vec(),
    })
}

pub(super) fn av1_readback_to_nv12_payload(
    readback_sample: &[u8],
    width: usize,
    height: usize,
) -> Result<Nv12FramePayload, BackendError> {
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::UnsupportedConfig(format!(
            "Vulkan AV1 non-metadata output currently requires even coded extent, got {}x{}",
            width, height
        )));
    }
    let expected_len = width
        .checked_mul(height)
        .and_then(|y| y.checked_add(y / 2))
        .ok_or_else(|| {
            BackendError::UnsupportedConfig(
                "Vulkan AV1 readback size overflow while preparing NV12 payload".to_string(),
            )
        })?;
    if readback_sample.len() < expected_len {
        return Err(BackendError::UnsupportedConfig(format!(
            "Vulkan AV1 readback sample too short for NV12 payload: got {}, need at least {expected_len}",
            readback_sample.len(),
        )));
    }
    Ok(Nv12FramePayload {
        pitch: width,
        data: readback_sample[..expected_len].to_vec(),
    })
}

pub(super) fn ensure_hevc_non_metadata_probe_coverage(
    metadata_only: bool,
    total_access_units: usize,
    probe_covered_access_units: usize,
    readback_bytes: usize,
    readback_planes: u32,
    submitted_access_units: u32,
) -> Result<(), BackendError> {
    if metadata_only {
        return Ok(());
    }
    if probe_covered_access_units == 0 || total_access_units == 0 {
        return Err(BackendError::UnsupportedConfig(format!(
            "Vulkan HEVC non-metadata output requires at least one probe-covered access unit; readback_handoff=bytes:{readback_bytes}, planes:{readback_planes}, submitted_access_units:{submitted_access_units}"
        )));
    }
    if total_access_units > probe_covered_access_units {
        return Err(BackendError::UnsupportedConfig(format!(
            "Vulkan HEVC non-metadata output currently supports up to {probe_covered_access_units} probe-covered access units, but stream has {total_access_units}; readback_handoff=bytes:{readback_bytes}, planes:{readback_planes}, submitted_access_units:{submitted_access_units}"
        )));
    }
    Ok(())
}

pub(super) fn raw_nv12_to_rgb24(raw: &RawFrameData) -> Result<Vec<u8>, BackendError> {
    let width = usize::try_from(raw.width).map_err(|_| {
        BackendError::InvalidInput("decoded width does not fit in usize".to_string())
    })?;
    let height = usize::try_from(raw.height).map_err(|_| {
        BackendError::InvalidInput("decoded height does not fit in usize".to_string())
    })?;
    nv12_bytes_to_pixels(&raw.frame, width, height, width, PixelOutputLayout::Rgb24)
}

pub(super) fn raw_nv12_payload(raw: &RawFrameData) -> Result<Nv12FramePayload, BackendError> {
    let width = usize::try_from(raw.width).map_err(|_| {
        BackendError::InvalidInput("decoded width does not fit in usize".to_string())
    })?;
    let height = usize::try_from(raw.height).map_err(|_| {
        BackendError::InvalidInput("decoded height does not fit in usize".to_string())
    })?;
    let expected = checked_tight_nv12_size(width, height)?;
    if raw.frame.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "decoded NV12 payload size mismatch: expected {expected}, got {}",
            raw.frame.len()
        )));
    }
    Ok(Nv12FramePayload {
        pitch: width,
        data: raw.frame.clone(),
    })
}

pub(super) fn nv12_bytes_to_pixels(
    data: &[u8],
    width: usize,
    height: usize,
    pitch: usize,
    layout: PixelOutputLayout,
) -> Result<Vec<u8>, BackendError> {
    let width = u32::try_from(width)
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| {
            BackendError::InvalidInput("NV12 width must fit u32 and be non-zero".to_string())
        })?;
    let height = u32::try_from(height)
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| {
            BackendError::InvalidInput("NV12 height must fit u32 and be non-zero".to_string())
        })?;
    DecodedFrame::Nv12 {
        dims: Dimensions { width, height },
        pitch,
        pts_90k: None,
        data: data.to_vec(),
    }
    .to_pixel_buffer(
        layout,
        ColorConvertOptions {
            matrix: ColorMatrix::Bt601,
            range: ColorRange::Limited,
        },
    )
    .map(|pixels| pixels.data)
}

pub(super) fn frame_to_nv12_payload(
    frame: &Frame,
    width: usize,
    height: usize,
) -> Result<RawFrameData, BackendError> {
    let tight_size = checked_tight_nv12_size(width, height)?;
    let dimensions = checked_dimensions(width, height, "frame")?;
    if let Some(nv12) = frame.nv12.as_ref() {
        nv12.validate(dimensions)?;
        let source_luma_size = nv12.pitch.checked_mul(height).ok_or_else(|| {
            BackendError::InvalidInput("NV12 source luma size overflow".to_string())
        })?;
        let source_chroma_size = nv12.pitch.checked_mul(height / 2).ok_or_else(|| {
            BackendError::InvalidInput("NV12 source chroma size overflow".to_string())
        })?;
        let source_size = source_luma_size
            .checked_add(source_chroma_size)
            .ok_or_else(|| BackendError::InvalidInput("NV12 source size overflow".to_string()))?;
        if nv12.data.len() != source_size {
            return Err(BackendError::InvalidInput(format!(
                "NV12 payload size mismatch for pitch {}: expected {source_size}, got {}",
                nv12.pitch,
                nv12.data.len()
            )));
        }
        let packed = if nv12.pitch == width {
            nv12.data.clone()
        } else {
            let mut packed = Vec::with_capacity(tight_size);
            for row in 0..height {
                let start = row * nv12.pitch;
                packed.extend_from_slice(&nv12.data[start..start + width]);
            }
            for row in 0..height / 2 {
                let start = source_luma_size + row * nv12.pitch;
                packed.extend_from_slice(&nv12.data[start..start + width]);
            }
            packed
        };
        let width_u32 = u32::try_from(width).map_err(|_| {
            BackendError::InvalidInput("frame width does not fit in u32".to_string())
        })?;
        let height_u32 = u32::try_from(height).map_err(|_| {
            BackendError::InvalidInput("frame height does not fit in u32".to_string())
        })?;
        return Ok(RawFrameData {
            frame: packed,
            width: width_u32,
            height: height_u32,
        });
    }

    let argb = frame.argb.as_ref().ok_or_else(|| {
        BackendError::InvalidInput("Vulkan encode requires ARGB input payload".to_string())
    })?;
    let nv12 = Nv12FramePayload::from_argb8888(dimensions, argb)?;
    if nv12.pitch != width {
        return Err(BackendError::InvalidInput(format!(
            "NV12 pitch mismatch: expected {}, got {}",
            width, nv12.pitch
        )));
    }
    let width_u32 = u32::try_from(width)
        .map_err(|_| BackendError::InvalidInput("frame width does not fit in u32".to_string()))?;
    let height_u32 = u32::try_from(height)
        .map_err(|_| BackendError::InvalidInput("frame height does not fit in u32".to_string()))?;
    Ok(RawFrameData {
        frame: nv12.data,
        width: width_u32,
        height: height_u32,
    })
}

pub(super) fn checked_dimensions(
    width: usize,
    height: usize,
    role: &str,
) -> Result<Dimensions, BackendError> {
    let width = u32::try_from(width)
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| {
            BackendError::InvalidInput(format!(
                "{role} width must fit u32 and be greater than zero"
            ))
        })?;
    let height = u32::try_from(height)
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| {
            BackendError::InvalidInput(format!(
                "{role} height must fit u32 and be greater than zero"
            ))
        })?;
    Ok(Dimensions { width, height })
}

pub(super) fn checked_tight_nv12_size(width: usize, height: usize) -> Result<usize, BackendError> {
    if width == 0 || height == 0 {
        return Err(BackendError::InvalidInput(
            "NV12 dimensions must be positive".to_string(),
        ));
    }
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(BackendError::InvalidInput(format!(
            "NV12 dimensions must be even, got {width}x{height}"
        )));
    }
    let luma_size = width
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("NV12 luma size overflow".to_string()))?;
    luma_size
        .checked_add(luma_size / 2)
        .ok_or_else(|| BackendError::InvalidInput("NV12 payload size overflow".to_string()))
}

pub(super) fn validate_vulkan_encode_frame(
    frame: &Frame,
    expected_dimensions: Option<(usize, usize)>,
    fps: i32,
) -> Result<(), BackendError> {
    checked_tight_nv12_size(frame.width, frame.height)?;
    if fps <= 0 {
        return Err(BackendError::InvalidInput(format!(
            "Vulkan encoder fps must be positive, got {fps}"
        )));
    }
    if let Some((width, height)) = expected_dimensions
        && (frame.width != width || frame.height != height)
    {
        return Err(BackendError::InvalidInput(format!(
            "mixed frame dimensions are unsupported: expected {width}x{height}, got {}x{}",
            frame.width, frame.height
        )));
    }
    if frame.nv12.is_none() && frame.argb.is_none() {
        return Err(BackendError::InvalidInput(
            "Vulkan encode requires NV12 or ARGB input payload".to_string(),
        ));
    }
    Ok(())
}
