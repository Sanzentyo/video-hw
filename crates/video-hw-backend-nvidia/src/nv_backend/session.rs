//! NVENC session resources, packet locking, codec configuration, and input upload adaptation.

use super::*;

#[derive(Clone, Copy)]
pub(super) struct SafeFlushOptions {
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) fps: i32,
    pub(super) codec: Codec,
    pub(super) max_in_flight: usize,
    pub(super) report_metrics: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum NvInputLayout {
    Argb,
    Nv12,
}

impl NvInputLayout {
    pub(super) fn input_format(self) -> EncodeInputFormat {
        match self {
            Self::Argb => EncodeInputFormat::Argb8888,
            Self::Nv12 => EncodeInputFormat::Nv12,
        }
    }
}

pub(super) struct NvEncodeSession {
    pub(super) session: nvidia_video_codec_sdk::Session,
    pub(super) input_layout: NvInputLayout,
}

impl NvEncodeSession {
    pub(super) fn new(
        session: nvidia_video_codec_sdk::Session,
        input_layout: NvInputLayout,
    ) -> Self {
        Self {
            session,
            input_layout,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct PendingSessionSwitch {
    pub(super) config: NvidiaSessionConfig,
    pub(super) mode: SessionSwitchMode,
    pub(super) target_generation: u64,
}

pub(super) struct SafeBufferPair<'a> {
    pub(super) input: nvidia_video_codec_sdk::Buffer<'a>,
    pub(super) output: nvidia_video_codec_sdk::Bitstream<'a>,
}

pub(super) struct SafePendingOutput<'a> {
    pub(super) pair: SafeBufferPair<'a>,
    pub(super) pts_90k: Option<i64>,
    pub(super) is_keyframe_hint: bool,
}

pub(super) fn lock_safe_output_packet(
    codec: Codec,
    mut pending: SafePendingOutput<'_>,
) -> Result<(EncodedPacket, SafeBufferPair<'_>), BackendError> {
    let (data, picture_type) = {
        let lock = pending.pair.output.lock().map_err(map_encode_error)?;
        (lock.data().to_vec(), lock.picture_type())
    };
    let is_keyframe = matches!(
        picture_type,
        nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_PIC_TYPE::NV_ENC_PIC_TYPE_I
            | nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_PIC_TYPE::NV_ENC_PIC_TYPE_IDR
    ) || (picture_type
        == nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_PIC_TYPE::NV_ENC_PIC_TYPE_UNKNOWN
        && pending.is_keyframe_hint);
    Ok((
        EncodedPacket {
            codec,
            layout: nvidia_encoded_layout(codec),
            kind: EncodedContentKind::Frame,
            data,
            pts_90k: pending.pts_90k,
            is_keyframe,
        },
        pending.pair,
    ))
}

pub(super) fn nvidia_encoded_layout(codec: Codec) -> EncodedLayout {
    match codec {
        Codec::H264 | Codec::Hevc => EncodedLayout::AnnexB,
        Codec::Av1 => EncodedLayout::Av1,
    }
}

pub(super) fn to_decode_codec(codec: Codec) -> DecodeCodec {
    match codec {
        Codec::H264 => DecodeCodec::H264,
        Codec::Hevc => DecodeCodec::H265,
        Codec::Av1 => DecodeCodec::Av1,
    }
}

pub(super) fn to_encode_guid(codec: Codec) -> nvidia_video_codec_sdk::sys::nvEncodeAPI::GUID {
    match codec {
        Codec::H264 => nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_CODEC_H264_GUID,
        Codec::Hevc => nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_CODEC_HEVC_GUID,
        Codec::Av1 => nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_CODEC_AV1_GUID,
    }
}

pub(super) fn configure_codec_preset(
    codec: Codec,
    config: &mut nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_CONFIG,
) {
    match codec {
        Codec::H264 => {
            config.profileGUID =
                nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_H264_PROFILE_MAIN_GUID;
            // SAFETY: `encodeCodecConfig` is a bindgen union and this branch
            // initializes the member selected by the H.264 encode GUID.
            let h264_config = unsafe { &mut config.encodeCodecConfig.h264Config };
            h264_config.set_disableSPSPPS(0);
            h264_config.set_repeatSPSPPS(1);
            h264_config.set_outputAUD(1);
            h264_config.chromaFormatIDC = 1;
            h264_config.idrPeriod = config.gopLength;
        }
        Codec::Hevc => {}
        Codec::Av1 => {
            // SAFETY: `encodeCodecConfig` is a bindgen union and this branch
            // initializes the member selected by the AV1 encode GUID.
            let av1_config = unsafe { &mut config.encodeCodecConfig.av1Config };
            av1_config.set_disableSeqHdr(0);
            av1_config.set_repeatSeqHdr(1);
        }
    }
}

pub(super) fn configure_rate_control(
    config: &mut nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_CONFIG,
    target_bitrate: Option<u32>,
) {
    let Some(target_bitrate) = target_bitrate else {
        return;
    };
    let target_bitrate = target_bitrate.max(1);
    config.rcParams.rateControlMode =
        nvidia_video_codec_sdk::sys::nvEncodeAPI::NV_ENC_PARAMS_RC_MODE::NV_ENC_PARAMS_RC_CBR;
    config.rcParams.averageBitRate = target_bitrate;
    config.rcParams.maxBitRate = target_bitrate;
}

pub(super) fn map_encode_error(error: nvidia_video_codec_sdk::EncodeError) -> BackendError {
    match error.kind() {
        ErrorKind::NeedMoreInput | ErrorKind::EncoderBusy | ErrorKind::LockBusy => {
            BackendError::TemporaryBackpressure(error.to_string())
        }
        ErrorKind::DeviceNotExist => BackendError::DeviceLost(error.to_string()),
        ErrorKind::UnsupportedDevice
        | ErrorKind::UnsupportedParam
        | ErrorKind::NoEncodeDevice
        | ErrorKind::InvalidEncoderDevice => BackendError::UnsupportedConfig(error.to_string()),
        ErrorKind::InvalidParam | ErrorKind::InvalidCall => {
            BackendError::InvalidInput(error.to_string())
        }
        _ => BackendError::Backend(error.to_string()),
    }
}

pub(super) fn update_jitter_samples(
    jitter_samples: &mut SampleStats,
    last_pts_90k: &mut Option<i64>,
    current_pts_90k: Option<i64>,
    expected_frame_ms: f64,
) {
    let Some(current) = current_pts_90k else {
        return;
    };
    if let Some(previous) = *last_pts_90k {
        let delta_ms = (current.saturating_sub(previous) as f64) / 90.0;
        jitter_samples.push_value((delta_ms - expected_frame_ms).abs());
    }
    *last_pts_90k = Some(current);
}

pub(super) enum NvInputFrame<'a> {
    Argb(&'a [u8]),
    Nv12(&'a Nv12FramePayload),
}

pub(super) fn validate_nv_encode_frame(
    frame: &Frame,
    input_format: EncodeInputFormat,
) -> Result<(), BackendError> {
    let dims = Dimensions::try_from((frame.width, frame.height))
        .map_err(|error| BackendError::InvalidInput(error.to_string()))?;
    match frame_buffer_for_layout(frame, input_format)? {
        NvInputFrame::Argb(argb) => {
            let expected = frame
                .width
                .checked_mul(frame.height)
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or_else(|| {
                    BackendError::InvalidInput("ARGB frame size overflows usize".to_string())
                })?;
            if argb.len() != expected {
                return Err(BackendError::InvalidInput(format!(
                    "argb payload size mismatch: expected {expected}, got {}",
                    argb.len()
                )));
            }
        }
        NvInputFrame::Nv12(nv12) => nv12.validate(dims)?,
    }
    Ok(())
}

pub(super) fn missing_input_payload_error(input_format: EncodeInputFormat) -> BackendError {
    let payload = match input_format {
        EncodeInputFormat::Argb8888 => "ARGB",
        EncodeInputFormat::Nv12 => "NV12",
    };
    BackendError::InvalidInput(format!("NVIDIA encode requires {payload} input payload"))
}

pub(super) fn frame_buffer_for_layout<'a>(
    frame: &'a Frame,
    input_format: EncodeInputFormat,
) -> Result<NvInputFrame<'a>, BackendError> {
    match input_format {
        EncodeInputFormat::Argb8888 => frame
            .argb
            .as_deref()
            .map(NvInputFrame::Argb)
            .ok_or_else(|| missing_input_payload_error(input_format)),
        EncodeInputFormat::Nv12 => frame
            .nv12
            .as_ref()
            .map(NvInputFrame::Nv12)
            .ok_or_else(|| missing_input_payload_error(input_format)),
    }
}

pub(super) fn upload_frame_to_input(
    input: &mut nvidia_video_codec_sdk::Buffer<'_>,
    frame: &Frame,
    input_layout: NvInputLayout,
    width: usize,
    height: usize,
) -> Result<usize, BackendError> {
    match frame_buffer_for_layout(frame, input_layout.input_format())? {
        NvInputFrame::Argb(argb) => {
            let expected_len = width
                .checked_mul(height)
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or_else(|| {
                    BackendError::InvalidInput("argb payload size overflows usize".to_string())
                })?;
            if argb.len() != expected_len {
                return Err(BackendError::InvalidInput(format!(
                    "argb payload size mismatch: expected {}, got {}",
                    expected_len,
                    argb.len()
                )));
            }
            let nvenc_buf = argb_to_nvenc_format(argb);
            let mut lock = input.lock().map_err(map_encode_error)?;
            let row_bytes = width.checked_mul(4).ok_or_else(|| {
                BackendError::InvalidInput("argb row size overflows usize".to_string())
            })?;
            // SAFETY: `nvenc_buf` was checked to contain exactly `height` rows of
            // `row_bytes`, and NVENC created this input for the same width/height.
            unsafe {
                lock.write_pitched(&nvenc_buf, row_bytes, height);
            }
            Ok(argb.len())
        }
        NvInputFrame::Nv12(nv12) => {
            let dims = Dimensions::try_from((width, height))
                .map_err(|error| BackendError::InvalidInput(error.to_string()))?;
            nv12.validate(dims)?;
            let pitch = nv12.pitch;
            let data = nv12.data.as_slice();
            let rows = height.checked_add(height / 2).ok_or_else(|| {
                BackendError::InvalidInput("nv12 row count overflows usize".to_string())
            })?;
            let expected_len = pitch.checked_mul(rows).ok_or_else(|| {
                BackendError::InvalidInput("nv12 payload size overflows usize".to_string())
            })?;
            if data.len() < expected_len {
                return Err(BackendError::InvalidInput(format!(
                    "nv12 payload size mismatch: expected at least {}, got {}",
                    expected_len,
                    data.len()
                )));
            }
            let packed_len = width.checked_mul(rows).ok_or_else(|| {
                BackendError::InvalidInput("packed nv12 size overflows usize".to_string())
            })?;
            let mut packed = Vec::with_capacity(packed_len);
            for row in 0..rows {
                let src_offset = row.checked_mul(pitch).ok_or_else(|| {
                    BackendError::InvalidInput("nv12 row offset overflows usize".to_string())
                })?;
                packed.extend_from_slice(&data[src_offset..src_offset + width]);
            }
            let mut lock = input.lock().map_err(map_encode_error)?;
            // SAFETY: `packed` contains exactly `rows` tightly packed rows of `width`
            // bytes, and NVENC created this input for the validated frame dimensions.
            unsafe {
                lock.write_pitched(&packed, width, rows);
            }
            Ok(packed.len())
        }
    }
}

/// Convert internal TRUE-ARGB bytes `[A, R, G, B]` (as stored in [`RawFrameBuffer::Argb8888`])
/// to the byte layout required by `NV_ENC_BUFFER_FORMAT_ABGR`.
///
/// NVENC's `ABGR` format is word-ordered in little-endian, so a 32-bit `0xAABBGGRR` pixel is
/// stored as bytes `[R, G, B, A]`.
pub(super) fn argb_to_nvenc_format(argb: &[u8]) -> Vec<u8> {
    argb.chunks_exact(4)
        .flat_map(|px| [px[1], px[2], px[3], px[0]])
        .collect()
}
