use std::num::NonZeroU32;
use std::sync::Arc;
use std::{fmt, fmt::Display};

pub mod bitstream;

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
pub enum BitstreamInput {
    AnnexBChunk {
        chunk: Vec<u8>,
        pts_90k: Option<Timestamp90k>,
    },
    AccessUnitRawNal {
        codec: Codec,
        nalus: Vec<Vec<u8>>,
        pts_90k: Option<Timestamp90k>,
    },
    LengthPrefixedSample {
        codec: Codec,
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
}

#[derive(Debug, Clone)]
pub struct EncodeFrame {
    pub dims: Dimensions,
    pub pts_90k: Option<Timestamp90k>,
    pub buffer: RawFrameBuffer,
    pub force_keyframe: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodedLayout {
    AnnexB,
    Avcc,
    Hvcc,
    Av1,
    Opaque,
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
            is_keyframe: self.is_keyframe,
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
        match self.layout {
            EncodedLayout::AnnexB => bitstream::annexb_to_length_prefixed(
                bitstream::AnnexBAccessUnitRef::new(&self.data),
                nal_length_size,
            ),
            EncodedLayout::Avcc | EncodedLayout::Hvcc => {
                Ok(bitstream::LengthPrefixedSample::new(self.data.clone()))
            }
            EncodedLayout::Av1 => Ok(bitstream::LengthPrefixedSample::new(self.data.clone())),
            EncodedLayout::Opaque => Err(bitstream::BitstreamError::OpaquePayload),
        }
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
pub struct PixelWidth(pub NonZeroU32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelHeight(pub NonZeroU32);

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
    pub fn try_as_nv12(&self) -> Option<Nv12FrameRef<'_>> {
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

    pub fn try_as_rgb24(&self) -> Option<Rgb24FrameRef<'_>> {
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
            Self::Rgb24 { dims, data, .. } => rgb24_to_pixel_buffer(*dims, data, layout),
            Self::Nv12 {
                dims, pitch, data, ..
            } => nv12_to_pixel_buffer(*dims, *pitch, data, layout, options),
            Self::Metadata { .. } => Err(BackendError::UnsupportedConfig(
                "metadata-only decoded frame has no pixel payload".to_string(),
            )),
        }
    }
}

fn rgb24_to_pixel_buffer(
    dims: Dimensions,
    data: &[u8],
    layout: PixelOutputLayout,
) -> Result<PixelBufferOwned, BackendError> {
    let pixel_count = dims.width.get() as usize * dims.height.get() as usize;
    if data.len() != pixel_count * 3 {
        return Err(BackendError::InvalidInput(format!(
            "rgb24 payload size mismatch: expected {}, got {}",
            pixel_count * 3,
            data.len()
        )));
    }
    let out = match layout {
        PixelOutputLayout::Rgb24 => data.to_vec(),
        PixelOutputLayout::Rgba8888 => {
            let mut out = Vec::with_capacity(pixel_count * 4);
            for rgb in data.chunks_exact(3) {
                out.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
            out
        }
        PixelOutputLayout::Argb8888 => {
            let mut out = Vec::with_capacity(pixel_count * 4);
            for rgb in data.chunks_exact(3) {
                out.extend_from_slice(&[255, rgb[0], rgb[1], rgb[2]]);
            }
            out
        }
        PixelOutputLayout::Bgra8888 => {
            let mut out = Vec::with_capacity(pixel_count * 4);
            for rgb in data.chunks_exact(3) {
                out.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
            }
            out
        }
    };
    Ok(PixelBufferOwned {
        layout,
        dims,
        data: out,
    })
}

fn nv12_to_pixel_buffer(
    dims: Dimensions,
    pitch: usize,
    data: &[u8],
    layout: PixelOutputLayout,
    options: ColorConvertOptions,
) -> Result<PixelBufferOwned, BackendError> {
    let width = dims.width.get() as usize;
    let height = dims.height.get() as usize;
    if pitch < width {
        return Err(BackendError::InvalidInput(format!(
            "nv12 pitch is smaller than width: pitch={pitch}, width={width}"
        )));
    }
    let y_size = pitch
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("nv12 luma size overflow".to_string()))?;
    if data.len() < y_size + y_size / 2 {
        return Err(BackendError::InvalidInput(format!(
            "nv12 payload size mismatch: expected at least {}, got {}",
            y_size + y_size / 2,
            data.len()
        )));
    }

    let mut rgb = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        for x in 0..width {
            let yv = data[y * pitch + x];
            let uv_index = y_size + (y / 2) * pitch + (x / 2) * 2;
            let u = data[uv_index];
            let v = data[uv_index + 1];
            let (r, g, b) = yuv_to_rgb(yv, u, v, options);
            rgb.extend_from_slice(&[r, g, b]);
        }
    }
    rgb24_to_pixel_buffer(dims, &rgb, layout)
}

fn yuv_to_rgb(y: u8, u: u8, v: u8, options: ColorConvertOptions) -> (u8, u8, u8) {
    let y_i = i32::from(y);
    let c = match options.range {
        ColorRange::Limited => (y_i - 16).max(0),
        ColorRange::Full => y_i,
    };
    let d = i32::from(u) - 128;
    let e = i32::from(v) - 128;
    let (r, g, b) = match (options.matrix, options.range) {
        (ColorMatrix::Bt601, ColorRange::Limited) => (
            (298 * c + 409 * e + 128) >> 8,
            (298 * c - 100 * d - 208 * e + 128) >> 8,
            (298 * c + 516 * d + 128) >> 8,
        ),
        (ColorMatrix::Bt709, ColorRange::Limited) => (
            (298 * c + 459 * e + 128) >> 8,
            (298 * c - 55 * d - 136 * e + 128) >> 8,
            (298 * c + 541 * d + 128) >> 8,
        ),
        (ColorMatrix::Bt601, ColorRange::Full) => (
            y_i + ((1436 * e) >> 10),
            y_i - ((352 * d + 731 * e) >> 10),
            y_i + ((1815 * d) >> 10),
        ),
        (ColorMatrix::Bt709, ColorRange::Full) => (
            y_i + ((1613 * e) >> 10),
            y_i - ((192 * d + 479 * e) >> 10),
            y_i + ((1900 * d) >> 10),
        ),
    };
    (clamp_u8(r), clamp_u8(g), clamp_u8(b))
}

fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
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
    #[cfg(any(
        target_os = "android",
        target_os = "macos",
        target_os = "linux",
        target_os = "windows"
    ))]
    pub argb: Option<Vec<u8>>,
    pub nv12: Option<Nv12FramePayload>,
    #[cfg(any(
        target_os = "android",
        target_os = "macos",
        target_os = "linux",
        target_os = "windows"
    ))]
    pub force_keyframe: bool,
}

#[derive(Debug, Clone)]
pub struct DecoderConfig {
    pub codec: Codec,
    pub fps: i32,
    pub require_hardware: bool,
    pub output_mode: DecodeOutputMode,
    pub backend_options: BackendDecoderOptions,
}

impl DecoderConfig {
    #[must_use]
    pub fn new(codec: Codec, fps: i32, require_hardware: bool) -> Self {
        Self {
            codec,
            fps,
            require_hardware,
            output_mode: DecodeOutputMode::default(),
            backend_options: BackendDecoderOptions::default(),
        }
    }
}

impl Display for DecoderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DecoderConfig(codec={}, fps={}, require_hardware={}, output_mode={})",
            self.codec, self.fps, self.require_hardware, self.output_mode
        )
    }
}

#[derive(Debug, Clone)]
pub struct EncoderConfig {
    pub codec: Codec,
    pub fps: i32,
    pub require_hardware: bool,
    pub input_format: EncodeInputFormat,
    pub backend_options: BackendEncoderOptions,
}

impl EncoderConfig {
    #[must_use]
    pub fn new(
        codec: Codec,
        fps: i32,
        require_hardware: bool,
        input_format: EncodeInputFormat,
    ) -> Self {
        Self {
            codec,
            fps,
            require_hardware,
            input_format,
            backend_options: BackendEncoderOptions::default(),
        }
    }
}

impl Display for EncoderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "EncoderConfig(codec={}, fps={}, require_hardware={}, input_format={})",
            self.codec, self.fps, self.require_hardware, self.input_format
        )
    }
}

#[derive(Debug, Clone, Default)]
pub enum BackendDecoderOptions {
    #[default]
    Default,
    VideoToolbox(VtDecoderOptions),
    Nvidia(NvidiaDecoderOptions),
    Intel(IntelDecoderOptions),
    Vulkan(VulkanDecoderOptions),
    Android(AndroidDecoderOptions),
}

#[derive(Debug, Clone, Default)]
pub enum BackendEncoderOptions {
    #[default]
    Default,
    VideoToolbox(VtEncoderOptions),
    Nvidia(NvidiaEncoderOptions),
    Intel(IntelEncoderOptions),
    Vulkan(VulkanEncoderOptions),
    Android(AndroidEncoderOptions),
}

#[derive(Debug, Clone, Default)]
pub struct VtDecoderOptions {
    pub report_metrics: Option<bool>,
    pub enable_pipeline_scheduler: Option<bool>,
    pub pipeline_queue_capacity: Option<usize>,
    pub video_width: Option<u16>,
    pub video_height: Option<u16>,
    pub av1c_record: Option<Vec<u8>>,
    pub av1_config_obus: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default)]
pub struct NvidiaDecoderOptions {
    pub report_metrics: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct VtEncoderOptions {
    pub report_metrics: Option<bool>,
    pub enable_pipeline_scheduler: Option<bool>,
    pub pipeline_queue_capacity: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct IntelDecoderOptions {
    pub force_software: bool,
}

#[derive(Debug, Clone, Default)]
pub struct VulkanDecoderOptions {
    pub allow_software_fallback: Option<bool>,
    pub adapter_index: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct AndroidDecoderOptions {
    pub codec_name: Option<String>,
    pub video_width: Option<u16>,
    pub video_height: Option<u16>,
    pub timeout_us: i64,
}

impl Default for AndroidDecoderOptions {
    fn default() -> Self {
        Self {
            codec_name: None,
            video_width: None,
            video_height: None,
            timeout_us: 10_000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NvidiaEncoderOptions {
    pub max_in_flight_outputs: usize,
    pub gop_length: Option<u32>,
    pub frame_interval_p: Option<i32>,
    pub target_bitrate: Option<u32>,
    pub report_metrics: Option<bool>,
    pub safe_lifetime_mode: Option<bool>,
    pub enable_pipeline_scheduler: Option<bool>,
    pub pipeline_queue_capacity: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct IntelEncoderOptions {
    pub target_kbps: Option<u16>,
    pub gop_length: Option<u16>,
    pub force_software: bool,
    pub hevc_use_vpp: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct VulkanEncoderOptions {
    pub allow_software_fallback: Option<bool>,
    pub adapter_index: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct AndroidEncoderOptions {
    pub codec_name: Option<String>,
    pub bitrate: Option<u32>,
    pub i_frame_interval_sec: Option<i32>,
    pub timeout_us: i64,
}

impl Default for AndroidEncoderOptions {
    fn default() -> Self {
        Self {
            codec_name: None,
            bitrate: None,
            i_frame_interval_sec: Some(1),
            timeout_us: 10_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSwitchMode {
    Immediate,
    OnNextKeyframe,
    DrainThenSwap,
}

impl Display for SessionSwitchMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Immediate => f.write_str("immediate"),
            Self::OnNextKeyframe => f.write_str("on_next_keyframe"),
            Self::DrainThenSwap => f.write_str("drain_then_swap"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NvidiaSessionConfig {
    pub gop_length: Option<u32>,
    pub frame_interval_p: Option<i32>,
    pub force_idr_on_activate: bool,
}

impl Display for NvidiaSessionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "NvidiaSessionConfig(gop_length={:?}, frame_interval_p={:?}, force_idr_on_activate={})",
            self.gop_length, self.frame_interval_p, self.force_idr_on_activate
        )
    }
}

#[derive(Debug, Clone)]
pub struct VtSessionConfig {
    pub force_keyframe_on_activate: bool,
}

impl Display for VtSessionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "VtSessionConfig(force_keyframe_on_activate={})",
            self.force_keyframe_on_activate
        )
    }
}

#[derive(Debug, Clone)]
pub enum SessionSwitchRequest {
    Nvidia {
        config: NvidiaSessionConfig,
        mode: SessionSwitchMode,
    },
    VideoToolbox {
        config: VtSessionConfig,
        mode: SessionSwitchMode,
    },
}

impl Display for SessionSwitchRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nvidia { config, mode } => {
                write!(f, "SessionSwitchRequest::Nvidia({}, mode={})", config, mode)
            }
            Self::VideoToolbox { config, mode } => {
                write!(
                    f,
                    "SessionSwitchRequest::VideoToolbox({}, mode={})",
                    config, mode
                )
            }
        }
    }
}

impl Default for NvidiaEncoderOptions {
    fn default() -> Self {
        Self {
            max_in_flight_outputs: 6,
            gop_length: None,
            frame_interval_p: None,
            target_bitrate: None,
            report_metrics: None,
            safe_lifetime_mode: None,
            enable_pipeline_scheduler: None,
            pipeline_queue_capacity: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DecodeSummary {
    pub decoded_frames: usize,
    pub width: Option<usize>,
    pub height: Option<usize>,
    pub pixel_format: Option<u32>,
}

impl Display for DecodeSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DecodeSummary(decoded_frames={}, width={:?}, height={:?}, pixel_format={:?})",
            self.decoded_frames, self.width, self.height, self.pixel_format
        )
    }
}

#[derive(Debug, Clone)]
#[cfg(any(
    all(target_os = "android", feature = "backend-android"),
    all(target_os = "macos", feature = "backend-vt"),
    all(
        any(
            feature = "backend-nvidia",
            feature = "backend-intel",
            feature = "backend-vulkan"
        ),
        any(target_os = "linux", target_os = "windows")
    )
))]
pub struct EncodedPacket {
    pub codec: Codec,
    pub data: Vec<u8>,
    pub pts_90k: Option<i64>,
    pub is_keyframe: bool,
}

#[derive(Debug, Clone)]
#[cfg(not(any(
    all(target_os = "android", feature = "backend-android"),
    all(target_os = "macos", feature = "backend-vt"),
    all(
        any(
            feature = "backend-nvidia",
            feature = "backend-intel",
            feature = "backend-vulkan"
        ),
        any(target_os = "linux", target_os = "windows")
    )
)))]
pub struct EncodedPacket;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeOutputOrigin {
    MetadataOnly,
    Native,
    ConvertedFromArgb,
    ConvertedFromNv12,
    ConvertedFromBgra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeOutputCapability {
    pub mode: DecodeOutputMode,
    pub origin: DecodeOutputOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamingMode {
    PushReap,
    FlushOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackPolicy {
    NoFallback,
    HardwareThenSoftware,
    OsManaged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DimensionConstraints {
    pub min_width: u32,
    pub min_height: u32,
    pub width_alignment: u32,
    pub height_alignment: u32,
}

impl Default for DimensionConstraints {
    fn default() -> Self {
        Self {
            min_width: 1,
            min_height: 1,
            width_alignment: 1,
            height_alignment: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeCapability {
    pub supported: bool,
    pub output_modes: Vec<DecodeOutputCapability>,
    pub streaming_mode: StreamingMode,
    pub fallback_policy: FallbackPolicy,
    pub requires_side_data: bool,
    pub dimension_constraints: DimensionConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodeCapability {
    pub supported: bool,
    pub input_formats: Vec<EncodeInputFormat>,
    pub encoded_layouts: Vec<EncodedLayout>,
    pub streaming_mode: StreamingMode,
    pub fallback_policy: FallbackPolicy,
    pub dimension_constraints: DimensionConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityContract {
    pub decode: DecodeCapability,
    pub encode: EncodeCapability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeStatus {
    Available,
    Unavailable,
    NotProbed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCapability {
    pub status: RuntimeStatus,
    pub hardware_acceleration: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityReport {
    pub codec: Codec,
    pub contract: CapabilityContract,
    pub runtime: RuntimeCapability,
}

impl CapabilityReport {
    #[must_use]
    pub fn supports_decode_output_mode(&self, mode: DecodeOutputMode) -> bool {
        self.contract
            .decode
            .output_modes
            .iter()
            .any(|capability| capability.mode == mode)
    }

    #[must_use]
    pub fn supports_encode_input_format(&self, format: EncodeInputFormat) -> bool {
        self.contract.encode.input_formats.contains(&format)
    }

    #[must_use]
    pub fn supports_encoded_layout(&self, layout: EncodedLayout) -> bool {
        self.contract.encode.encoded_layouts.contains(&layout)
    }
}

impl Display for CapabilityReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CapabilityReport(codec={}, decode_supported={}, encode_supported={}, hw_accel={}, runtime={:?}, decode_outputs={:?}, encode_inputs={:?}, encoded_layouts={:?})",
            self.codec,
            self.contract.decode.supported,
            self.contract.encode.supported,
            self.runtime.hardware_acceleration,
            self.runtime.status,
            self.contract.decode.output_modes,
            self.contract.encode.input_formats,
            self.contract.encode.encoded_layouts
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("unsupported codec: {0:?}")]
    UnsupportedCodec(Codec),
    #[error("unsupported config: {0}")]
    UnsupportedConfig(String),
    #[error("invalid bitstream: {0}")]
    InvalidBitstream(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("temporary backpressure: {0}")]
    TemporaryBackpressure(String),
    #[error("device lost: {0}")]
    DeviceLost(String),
    #[error("backend error: {0}")]
    Backend(String),
}

impl From<bitstream::BitstreamError> for BackendError {
    fn from(value: bitstream::BitstreamError) -> Self {
        match value {
            bitstream::BitstreamError::UnsupportedLayout { codec, layout } => {
                Self::InvalidBitstream(format!(
                    "unsupported bitstream layout: codec={codec}, layout={layout}"
                ))
            }
            bitstream::BitstreamError::OpaquePayload => {
                Self::InvalidBitstream("opaque encoded payload cannot be converted".to_string())
            }
            other => Self::InvalidBitstream(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendErrorKind {
    UnsupportedCodec,
    UnsupportedConfig,
    InvalidBitstream,
    InvalidInput,
    TemporaryBackpressure,
    DeviceLost,
    Backend,
}

impl BackendError {
    #[must_use]
    pub fn kind(&self) -> BackendErrorKind {
        match self {
            Self::UnsupportedCodec(_) => BackendErrorKind::UnsupportedCodec,
            Self::UnsupportedConfig(_) => BackendErrorKind::UnsupportedConfig,
            Self::InvalidBitstream(_) => BackendErrorKind::InvalidBitstream,
            Self::InvalidInput(_) => BackendErrorKind::InvalidInput,
            Self::TemporaryBackpressure(_) => BackendErrorKind::TemporaryBackpressure,
            Self::DeviceLost(_) => BackendErrorKind::DeviceLost,
            Self::Backend(_) => BackendErrorKind::Backend,
        }
    }

    #[must_use]
    pub fn is_runtime_unavailable(&self) -> bool {
        matches!(
            self.kind(),
            BackendErrorKind::UnsupportedConfig | BackendErrorKind::DeviceLost
        )
    }
}

pub trait VideoDecoder {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError>;

    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError>;

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError>;

    fn try_reap(&mut self) -> Result<Vec<Frame>, BackendError> {
        Ok(Vec::new())
    }

    fn decode_summary(&self) -> DecodeSummary;
}

pub trait VideoEncoder {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError>;

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError>;

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError>;

    fn try_reap(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        Ok(Vec::new())
    }

    fn request_session_switch(
        &mut self,
        _request: SessionSwitchRequest,
    ) -> Result<(), BackendError> {
        Err(BackendError::UnsupportedConfig(
            "session switching is not supported by this backend".to_string(),
        ))
    }
    #[cfg(any(
        all(target_os = "android", feature = "backend-android"),
        all(target_os = "macos", feature = "backend-vt"),
        all(
            any(
                feature = "backend-nvidia",
                feature = "backend-intel",
                feature = "backend-vulkan"
            ),
            any(target_os = "linux", target_os = "windows")
        )
    ))]
    fn pipeline_generation_hint(&self) -> Option<u64> {
        None
    }
}
