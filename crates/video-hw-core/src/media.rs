use std::num::NonZeroU32;
use std::sync::Arc;
use std::{fmt, fmt::Display};

use crate::{BackendError, bitstream, pixel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

impl Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::H264 => f.write_str("h264"),
            Self::Hevc => f.write_str("hevc"),
            Self::Av1 => f.write_str("av1"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dimensions {
    pub width: NonZeroU32,
    pub height: NonZeroU32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidDimensions {
    #[error("width must be greater than zero")]
    ZeroWidth,
    #[error("height must be greater than zero")]
    ZeroHeight,
    #[error("width {0} exceeds u32")]
    WidthOutOfRange(usize),
    #[error("height {0} exceeds u32")]
    HeightOutOfRange(usize),
}

impl Dimensions {
    pub fn new(width: u32, height: u32) -> Result<Self, InvalidDimensions> {
        let width = NonZeroU32::new(width).ok_or(InvalidDimensions::ZeroWidth)?;
        let height = NonZeroU32::new(height).ok_or(InvalidDimensions::ZeroHeight)?;
        Ok(Self { width, height })
    }

    #[must_use]
    pub const fn width(self) -> u32 {
        self.width.get()
    }

    #[must_use]
    pub const fn height(self) -> u32 {
        self.height.get()
    }
}

impl TryFrom<(usize, usize)> for Dimensions {
    type Error = InvalidDimensions;

    fn try_from((width, height): (usize, usize)) -> Result<Self, Self::Error> {
        let width = u32::try_from(width).map_err(|_| InvalidDimensions::WidthOutOfRange(width))?;
        let height =
            u32::try_from(height).map_err(|_| InvalidDimensions::HeightOutOfRange(height))?;
        Self::new(width, height)
    }
}

impl Display for Dimensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamp90k(pub i64);

impl Display for Timestamp90k {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@90k", self.0)
    }
}

#[derive(Debug, Clone)]
/// Decoder input with framing made explicit at the API boundary.
pub enum BitstreamInput {
    /// A non-empty streaming Annex-B chunk. It may end between access units.
    AnnexBChunk {
        chunk: Vec<u8>,
        pts_90k: Option<Timestamp90k>,
    },
    /// One complete H.264 or HEVC access unit represented as raw NAL units.
    AccessUnitRawNal {
        codec: Codec,
        nalus: Vec<Vec<u8>>,
        pts_90k: Option<Timestamp90k>,
    },
    /// One complete H.264 or HEVC MP4 sample with its declared NAL length size.
    LengthPrefixedSample {
        codec: Codec,
        sample: Vec<u8>,
        nal_length_size: bitstream::NalLengthSize,
        pts_90k: Option<Timestamp90k>,
    },
    /// One complete AV1 low-overhead OBU sample.
    Av1ObuSample {
        sample: Vec<u8>,
        pts_90k: Option<Timestamp90k>,
    },
}

#[derive(Debug, Clone)]
pub enum RawFrameBuffer {
    Argb8888(Vec<u8>),
    Argb8888Shared(Arc<[u8]>),
    Nv12 { pitch: usize, data: Vec<u8> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeInputFormat {
    Argb8888,
    Nv12,
}

impl Display for EncodeInputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Argb8888 => f.write_str("argb8888"),
            Self::Nv12 => f.write_str("nv12"),
        }
    }
}

impl RawFrameBuffer {
    #[must_use]
    pub fn input_format(&self) -> EncodeInputFormat {
        match self {
            Self::Argb8888(_) | Self::Argb8888Shared(_) => EncodeInputFormat::Argb8888,
            Self::Nv12 { .. } => EncodeInputFormat::Nv12,
        }
    }

    pub fn validate(&self, dims: Dimensions) -> Result<(), BackendError> {
        match self {
            Self::Argb8888(data) => validate_argb8888(dims, data),
            Self::Argb8888Shared(data) => validate_argb8888(dims, data),
            Self::Nv12 { pitch, data } => pixel::validate_nv12_ref(dims, *pitch, data),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EncodeFrame {
    pub dims: Dimensions,
    pub pts_90k: Option<Timestamp90k>,
    pub buffer: RawFrameBuffer,
    pub force_keyframe: bool,
}

impl EncodeFrame {
    pub fn validate(&self) -> Result<(), BackendError> {
        self.buffer.validate(self.dims)
    }
}

fn validate_argb8888(dims: Dimensions, data: &[u8]) -> Result<(), BackendError> {
    let expected = (dims.width() as usize)
        .checked_mul(dims.height() as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| BackendError::InvalidInput("argb size overflow".to_string()))?;
    if data.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "argb payload size mismatch: expected {expected}, got {}",
            data.len()
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodedLayout {
    AnnexB,
    Avcc,
    Hvcc,
    Av1,
    Opaque,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodedContentKind {
    Frame,
    CodecConfig,
}

impl Display for EncodedLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AnnexB => f.write_str("annexb"),
            Self::Avcc => f.write_str("avcc"),
            Self::Hvcc => f.write_str("hvcc"),
            Self::Av1 => f.write_str("av1"),
            Self::Opaque => f.write_str("opaque"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EncodedChunk {
    pub codec: Codec,
    pub layout: EncodedLayout,
    pub kind: EncodedContentKind,
    pub data: Vec<u8>,
    pub pts_90k: Option<Timestamp90k>,
    pub is_keyframe: bool,
}

impl EncodedChunk {
    pub fn payload_ref(&self) -> bitstream::EncodedPayloadRef<'_> {
        bitstream::EncodedPayloadRef {
            codec: self.codec,
            layout: self.layout,
            data: &self.data,
        }
    }

    pub fn to_annexb(&self) -> Result<bitstream::AnnexBAccessUnit, bitstream::BitstreamError> {
        bitstream::payload_to_annexb(self.payload_ref(), bitstream::NalLengthSize::Four)
    }

    pub fn to_nalus(&self) -> Result<Vec<bitstream::NalUnit>, bitstream::BitstreamError> {
        bitstream::payload_to_nalus(self.payload_ref(), bitstream::NalLengthSize::Four)
    }

    pub fn to_decode_payload(&self) -> Result<bitstream::DecodePayload, bitstream::BitstreamError> {
        bitstream::payload_to_decode_payload(self.payload_ref(), bitstream::NalLengthSize::Four)
    }

    pub fn to_length_prefixed_sample(
        &self,
        nal_length_size: bitstream::NalLengthSize,
    ) -> Result<bitstream::LengthPrefixedSample, bitstream::BitstreamError> {
        match (self.codec, self.layout) {
            (Codec::H264 | Codec::Hevc, EncodedLayout::AnnexB) => {
                bitstream::annexb_to_length_prefixed(
                    bitstream::AnnexBAccessUnitRef::new(&self.data),
                    nal_length_size,
                )
            }
            (Codec::H264, EncodedLayout::Avcc) | (Codec::Hevc, EncodedLayout::Hvcc) => {
                let annexb = bitstream::length_prefixed_to_annexb(
                    bitstream::LengthPrefixedSampleRef::new(&self.data),
                    bitstream::NalLengthSize::Four,
                )?;
                bitstream::annexb_to_length_prefixed(annexb.as_ref(), nal_length_size)
            }
            (_, EncodedLayout::Opaque) => Err(bitstream::BitstreamError::OpaquePayload),
            (codec, layout) => Err(bitstream::BitstreamError::UnsupportedLayout { codec, layout }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_chunk_reframes_existing_avcc_to_requested_length_size() {
        let chunk = EncodedChunk {
            codec: Codec::H264,
            layout: EncodedLayout::Avcc,
            kind: EncodedContentKind::Frame,
            data: vec![0, 0, 0, 2, 0x65, 0x80],
            pts_90k: None,
            is_keyframe: true,
        };

        let reframed = chunk
            .to_length_prefixed_sample(bitstream::NalLengthSize::One)
            .expect("two-byte NAL fits a one-byte length field");

        assert_eq!(reframed.as_bytes(), &[2, 0x65, 0x80]);
    }

    #[test]
    fn encoded_chunk_rejects_malformed_existing_avcc() {
        let chunk = EncodedChunk {
            codec: Codec::H264,
            layout: EncodedLayout::Avcc,
            kind: EncodedContentKind::Frame,
            data: vec![0, 0, 0, 3, 0x65, 0x80],
            pts_90k: None,
            is_keyframe: true,
        };

        assert!(
            chunk
                .to_length_prefixed_sample(bitstream::NalLengthSize::Four)
                .is_err()
        );
    }
}

#[derive(Debug, Clone)]
pub enum DecodedFrame {
    Metadata {
        dims: Option<Dimensions>,
        pts_90k: Option<Timestamp90k>,
        pixel_format: Option<u32>,
        decode_info_flags: Option<u32>,
        color: Option<ColorMetadata>,
    },
    Nv12 {
        dims: Dimensions,
        pitch: usize,
        pts_90k: Option<Timestamp90k>,
        data: Vec<u8>,
    },
    Rgb24 {
        dims: Dimensions,
        pts_90k: Option<Timestamp90k>,
        data: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelOutputLayout {
    Rgb24,
    Rgba8888,
    Argb8888,
    Bgra8888,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorMatrix {
    Bt601,
    #[default]
    Bt709,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorRange {
    #[default]
    Limited,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ColorConvertOptions {
    pub matrix: ColorMatrix,
    pub range: ColorRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelBufferOwned {
    pub layout: PixelOutputLayout,
    pub dims: Dimensions,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PitchBytes(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nv12FrameRef<'a> {
    pub dims: Dimensions,
    pub pitch: PitchBytes,
    pub pts_90k: Option<Timestamp90k>,
    pub data: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb24FrameRef<'a> {
    pub dims: Dimensions,
    pub pts_90k: Option<Timestamp90k>,
    pub data: &'a [u8],
}

impl DecodedFrame {
    pub fn as_nv12(&self) -> Option<Nv12FrameRef<'_>> {
        match self {
            Self::Nv12 {
                dims,
                pitch,
                pts_90k,
                data,
            } => Some(Nv12FrameRef {
                dims: *dims,
                pitch: PitchBytes(*pitch),
                pts_90k: *pts_90k,
                data,
            }),
            _ => None,
        }
    }

    pub fn as_rgb24(&self) -> Option<Rgb24FrameRef<'_>> {
        match self {
            Self::Rgb24 {
                dims,
                pts_90k,
                data,
            } => Some(Rgb24FrameRef {
                dims: *dims,
                pts_90k: *pts_90k,
                data,
            }),
            _ => None,
        }
    }

    pub fn to_pixel_buffer(
        &self,
        layout: PixelOutputLayout,
        options: ColorConvertOptions,
    ) -> Result<PixelBufferOwned, BackendError> {
        match self {
            Self::Rgb24 { dims, data, .. } => pixel::rgb24_to_pixel_buffer(*dims, data, layout),
            Self::Nv12 {
                dims, pitch, data, ..
            } => pixel::nv12_to_pixel_buffer(*dims, *pitch, data, layout, options),
            Self::Metadata { .. } => Err(BackendError::UnsupportedConfig(
                "metadata-only decoded frame has no pixel payload".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DecodeOutputMode {
    #[default]
    Metadata,
    Nv12,
    Rgb24,
}

impl Display for DecodeOutputMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Metadata => f.write_str("metadata"),
            Self::Nv12 => f.write_str("nv12"),
            Self::Rgb24 => f.write_str("rgb24"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorMetadata {
    pub color_primaries: Option<i32>,
    pub transfer_function: Option<i32>,
    pub ycbcr_matrix: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct Nv12FramePayload {
    pub pitch: usize,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub pixel_format: Option<u32>,
    pub pts_90k: Option<i64>,
    pub decode_info_flags: Option<u32>,
    pub color_primaries: Option<i32>,
    pub transfer_function: Option<i32>,
    pub ycbcr_matrix: Option<i32>,
    pub argb: Option<Vec<u8>>,
    pub rgb24: Option<Vec<u8>>,
    pub nv12: Option<Nv12FramePayload>,
    pub force_keyframe: bool,
}
