use crate::{
    BackendError, BitstreamInput, Codec, ColorConvertOptions, ColorMetadata, DecodeOutputMode,
    DecodedFrame, Dimensions, EncodeFrame, EncodedChunk, EncodedPacket, Frame, Nv12FramePayload,
    PixelOutputLayout, RawFrameBuffer, Timestamp90k, bitstream,
};

pub(super) fn normalize_bitstream_input(
    input: BitstreamInput,
    expected_codec: Codec,
) -> Result<(Vec<u8>, Option<i64>), BackendError> {
    match input {
        BitstreamInput::AnnexBChunk { chunk, pts_90k } => {
            if chunk.is_empty() {
                return Err(BackendError::InvalidInput(
                    "Annex B chunk is empty; use flush() to signal end of stream".to_string(),
                ));
            }
            Ok((chunk, pts_90k.map(|v| v.0)))
        }
        BitstreamInput::AccessUnitRawNal {
            codec,
            nalus,
            pts_90k,
        } => {
            validate_input_codec(expected_codec, codec)?;
            if codec == Codec::Av1 {
                return Err(BackendError::InvalidInput(
                    "AV1 bitstreams use OBU payloads, not NAL access units".to_string(),
                ));
            }
            if nalus.is_empty() {
                return Err(BackendError::InvalidInput(
                    "NAL access unit does not contain any NAL units".to_string(),
                ));
            }
            let mut out = Vec::new();
            for nalu in nalus {
                if bitstream::NalUnitRef::new(&nalu).as_bytes().is_empty() {
                    return Err(BackendError::InvalidInput(
                        "NAL access unit contains an empty NAL unit".to_string(),
                    ));
                }
                bitstream::append_annexb_nalu(&mut out, &nalu);
            }
            Ok((out, pts_90k.map(|v| v.0)))
        }
        BitstreamInput::LengthPrefixedSample {
            codec,
            sample,
            nal_length_size,
            pts_90k,
        } => {
            validate_input_codec(expected_codec, codec)?;
            if sample.is_empty() {
                return Err(BackendError::InvalidInput(
                    "length-prefixed sample is empty".to_string(),
                ));
            }
            if codec == Codec::Av1 {
                return Err(BackendError::InvalidInput(
                    "AV1 bitstreams use BitstreamInput::Av1ObuSample, not NAL-length-prefixed samples"
                        .to_string(),
                ));
            }
            Ok((
                bitstream::length_prefixed_to_annexb(
                    bitstream::LengthPrefixedSampleRef::new(&sample),
                    nal_length_size,
                )?
                .into_inner(),
                pts_90k.map(|v| v.0),
            ))
        }
        BitstreamInput::Av1ObuSample { sample, pts_90k } => {
            validate_input_codec(expected_codec, Codec::Av1)?;
            if sample.is_empty() {
                return Err(BackendError::InvalidInput(
                    "AV1 OBU sample is empty".to_string(),
                ));
            }
            Ok((sample, pts_90k.map(|v| v.0)))
        }
    }
}

fn validate_input_codec(expected: Codec, actual: Codec) -> Result<(), BackendError> {
    if actual != expected {
        return Err(BackendError::InvalidInput(format!(
            "bitstream codec mismatch: decoder expects {expected}, input declares {actual}"
        )));
    }
    Ok(())
}

pub(super) fn backend_frame_to_decoded_frame(
    mut frame: Frame,
    mode: DecodeOutputMode,
) -> Result<DecodedFrame, BackendError> {
    match mode {
        DecodeOutputMode::Metadata => {}
        DecodeOutputMode::Nv12 => {
            let dims =
                dimensions_from_backend_frame(frame.width, frame.height).ok_or_else(|| {
                    BackendError::InvalidInput(
                        "decoded frame dimensions are invalid for DecodeOutputMode::Nv12"
                            .to_string(),
                    )
                })?;
            if let Some(nv12) = frame.nv12 {
                nv12.validate(dims)?;
                return Ok(DecodedFrame::Nv12 {
                    dims,
                    pitch: nv12.pitch,
                    pts_90k: frame.pts_90k.map(Timestamp90k),
                    data: nv12.data,
                });
            }
            let argb = frame_argb_payload(&frame).ok_or_else(|| {
                BackendError::UnsupportedConfig(
                    "DecodeOutputMode::Nv12 requires backend NV12 or ARGB payload".to_string(),
                )
            })?;
            let nv12 = Nv12FramePayload::from_argb8888(dims, argb)?;
            return Ok(DecodedFrame::Nv12 {
                dims,
                pitch: nv12.pitch,
                pts_90k: frame.pts_90k.map(Timestamp90k),
                data: nv12.data,
            });
        }
        DecodeOutputMode::Rgb24 => {
            let dims =
                dimensions_from_backend_frame(frame.width, frame.height).ok_or_else(|| {
                    BackendError::InvalidInput(
                        "decoded frame dimensions are invalid for DecodeOutputMode::Rgb24"
                            .to_string(),
                    )
                })?;
            let data = if let Some(rgb24) = frame.rgb24.take() {
                validate_rgb24_payload(&rgb24, frame.width, frame.height)?;
                rgb24
            } else if let Some(argb) = frame_argb_payload(&frame) {
                argb_to_rgb24(argb, frame.width, frame.height)?
            } else if let Some(nv12) = frame.nv12.take() {
                nv12.validate(dims)?;
                DecodedFrame::Nv12 {
                    dims,
                    pitch: nv12.pitch,
                    pts_90k: frame.pts_90k.map(Timestamp90k),
                    data: nv12.data,
                }
                .to_pixel_buffer(PixelOutputLayout::Rgb24, ColorConvertOptions::default())?
                .data
            } else {
                return Err(BackendError::UnsupportedConfig(
                    "DecodeOutputMode::Rgb24 requires backend NV12 or ARGB payload".to_string(),
                ));
            };
            return Ok(DecodedFrame::Rgb24 {
                dims,
                pts_90k: frame.pts_90k.map(Timestamp90k),
                data,
            });
        }
    }
    let dims = dimensions_from_backend_frame(frame.width, frame.height);
    let color = if frame.color_primaries.is_some()
        || frame.transfer_function.is_some()
        || frame.ycbcr_matrix.is_some()
    {
        Some(ColorMetadata {
            color_primaries: frame.color_primaries,
            transfer_function: frame.transfer_function,
            ycbcr_matrix: frame.ycbcr_matrix,
        })
    } else {
        None
    };
    Ok(DecodedFrame::Metadata {
        dims,
        pts_90k: frame.pts_90k.map(Timestamp90k),
        pixel_format: frame.pixel_format,
        decode_info_flags: frame.decode_info_flags,
        color,
    })
}

fn frame_argb_payload(frame: &Frame) -> Option<&[u8]> {
    frame.argb.as_deref()
}

fn validate_rgb24_payload(rgb24: &[u8], width: usize, height: usize) -> Result<(), BackendError> {
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| BackendError::InvalidInput("rgb24 size overflow".to_string()))?;
    if rgb24.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "rgb24 payload size mismatch: expected {expected}, got {}",
            rgb24.len()
        )));
    }
    Ok(())
}

fn argb_to_rgb24(argb: &[u8], width: usize, height: usize) -> Result<Vec<u8>, BackendError> {
    let expected = width
        .checked_mul(height)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| BackendError::InvalidInput("argb size overflow".to_string()))?;
    if argb.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "argb payload size mismatch: expected {}, got {}",
            expected,
            argb.len()
        )));
    }
    let output_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| BackendError::InvalidInput("rgb24 size overflow".to_string()))?;
    let mut out = Vec::new();
    out.try_reserve_exact(output_len).map_err(|error| {
        BackendError::InvalidInput(format!(
            "cannot allocate {output_len} bytes for RGB24 frame: {error}"
        ))
    })?;
    for px in argb.as_chunks::<4>().0 {
        out.extend_from_slice(&[px[1], px[2], px[3]]);
    }
    Ok(out)
}

pub(super) fn encode_frame_into_backend_frame(frame: EncodeFrame) -> Result<Frame, BackendError> {
    let EncodeFrame {
        dims,
        pts_90k,
        buffer,
        force_keyframe,
    } = frame;
    let width = dims.width.get() as usize;
    let height = dims.height.get() as usize;
    let (argb, nv12) = match buffer {
        RawFrameBuffer::Argb8888(data) => (Some(data), None),
        RawFrameBuffer::Argb8888Shared(data) => (Some(data.to_vec()), None),
        RawFrameBuffer::Nv12 { pitch, data } => {
            let payload = Nv12FramePayload { pitch, data };
            payload.validate(dims)?;
            (None, Some(payload))
        }
    };
    Ok(Frame {
        width,
        height,
        pixel_format: None,
        pts_90k: pts_90k.map(|v| v.0),
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        argb,
        rgb24: None,
        nv12,
        force_keyframe,
    })
}

pub(super) fn backend_packet_to_encoded_chunk(packet: EncodedPacket) -> EncodedChunk {
    EncodedChunk {
        codec: packet.codec,
        layout: packet.layout,
        kind: packet.kind,
        data: packet.data,
        pts_90k: packet.pts_90k.map(Timestamp90k),
        is_keyframe: packet.is_keyframe,
    }
}

fn dimensions_from_backend_frame(width: usize, height: usize) -> Option<Dimensions> {
    Dimensions::try_from((width, height)).ok()
}
