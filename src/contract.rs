use std::num::NonZeroU32;
use std::sync::Arc;
use std::{fmt, fmt::Display};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
}

impl Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::H264 => f.write_str("h264"),
            Self::Hevc => f.write_str("hevc"),
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
    Rgb24(Vec<u8>),
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
    Opaque,
}

impl Display for EncodedLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AnnexB => f.write_str("annexb"),
            Self::Avcc => f.write_str("avcc"),
            Self::Hvcc => f.write_str("hvcc"),
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
pub struct ColorMetadata {
    pub color_primaries: Option<i32>,
    pub transfer_function: Option<i32>,
    pub ycbcr_matrix: Option<i32>,
}

#[derive(Debug, Clone)]
pub(crate) struct Frame {
    pub width: usize,
    pub height: usize,
    pub pixel_format: Option<u32>,
    pub pts_90k: Option<i64>,
    pub decode_info_flags: Option<u32>,
    pub color_primaries: Option<i32>,
    pub transfer_function: Option<i32>,
    pub ycbcr_matrix: Option<i32>,
    #[cfg(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        )
    ))]
    pub argb: Option<Vec<u8>>,
    #[cfg(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        )
    ))]
    pub force_keyframe: bool,
}

#[derive(Debug, Clone)]
pub struct DecoderConfig {
    pub codec: Codec,
    pub fps: i32,
    pub require_hardware: bool,
    pub backend_options: BackendDecoderOptions,
}

impl DecoderConfig {
    #[must_use]
    pub fn new(codec: Codec, fps: i32, require_hardware: bool) -> Self {
        Self {
            codec,
            fps,
            require_hardware,
            backend_options: BackendDecoderOptions::default(),
        }
    }
}

impl Display for DecoderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DecoderConfig(codec={}, fps={}, require_hardware={})",
            self.codec, self.fps, self.require_hardware
        )
    }
}

#[derive(Debug, Clone)]
pub struct EncoderConfig {
    pub codec: Codec,
    pub fps: i32,
    pub require_hardware: bool,
    pub backend_options: BackendEncoderOptions,
}

impl EncoderConfig {
    #[must_use]
    pub fn new(codec: Codec, fps: i32, require_hardware: bool) -> Self {
        Self {
            codec,
            fps,
            require_hardware,
            backend_options: BackendEncoderOptions::default(),
        }
    }
}

impl Display for EncoderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "EncoderConfig(codec={}, fps={}, require_hardware={})",
            self.codec, self.fps, self.require_hardware
        )
    }
}

#[derive(Debug, Clone, Default)]
pub enum BackendDecoderOptions {
    #[default]
    Default,
    Nvidia(NvidiaDecoderOptions),
}

#[derive(Debug, Clone, Default)]
pub enum BackendEncoderOptions {
    #[default]
    Default,
    Nvidia(NvidiaEncoderOptions),
}

#[derive(Debug, Clone, Default)]
pub struct NvidiaDecoderOptions {
    pub report_metrics: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct NvidiaEncoderOptions {
    pub max_in_flight_outputs: usize,
    pub gop_length: Option<u32>,
    pub frame_interval_p: Option<i32>,
    pub report_metrics: Option<bool>,
    pub safe_lifetime_mode: Option<bool>,
    pub enable_pipeline_scheduler: Option<bool>,
    pub pipeline_queue_capacity: Option<usize>,
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
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
pub(crate) struct EncodedPacket {
    pub codec: Codec,
    pub data: Vec<u8>,
    pub pts_90k: Option<i64>,
    pub is_keyframe: bool,
}

#[derive(Debug, Clone)]
#[cfg(not(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
)))]
pub(crate) struct EncodedPacket;

#[derive(Debug, Clone)]
pub struct CapabilityReport {
    pub codec: Codec,
    pub decode_supported: bool,
    pub encode_supported: bool,
    pub hardware_acceleration: bool,
}

impl Display for CapabilityReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CapabilityReport(codec={}, decode_supported={}, encode_supported={}, hardware_acceleration={})",
            self.codec, self.decode_supported, self.encode_supported, self.hardware_acceleration
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

pub(crate) trait VideoDecoder {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError>;

    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError>;

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError>;

    fn decode_summary(&self) -> DecodeSummary;
}

pub(crate) trait VideoEncoder {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError>;

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError>;

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError>;

    fn request_session_switch(
        &mut self,
        _request: SessionSwitchRequest,
    ) -> Result<(), BackendError> {
        Err(BackendError::UnsupportedConfig(
            "session switching is not supported by this backend".to_string(),
        ))
    }
    #[cfg(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        )
    ))]
    fn pipeline_generation_hint(&self) -> Option<u64> {
        None
    }
}
