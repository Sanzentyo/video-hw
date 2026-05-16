use std::collections::VecDeque;
use std::fmt;
use std::str::FromStr;
use std::time::{Duration, Instant};

mod contract;
mod pipeline;
mod transform;

pub use contract::Nv12FramePayload;
pub use contract::{
    BackendDecoderOptions, BackendEncoderOptions, BackendError, BackendErrorKind, BitstreamInput,
    CapabilityContract, CapabilityReport, Codec, ColorConvertOptions, ColorMatrix, ColorMetadata,
    ColorRange, DecodeCapability, DecodeOutputCapability, DecodeOutputMode, DecodeOutputOrigin,
    DecodeSummary, DecodedFrame, DecoderConfig, DimensionConstraints, Dimensions, EncodeCapability,
    EncodeFrame, EncodeInputFormat, EncodedChunk, EncodedLayout, EncoderConfig, FallbackPolicy,
    IntelDecoderOptions, IntelEncoderOptions, Nv12FrameRef, NvidiaDecoderOptions,
    NvidiaEncoderOptions, NvidiaSessionConfig, PitchBytes, PixelBufferOwned, PixelHeight,
    PixelOutputLayout, PixelWidth, RawFrameBuffer, Rgb24FrameRef, RuntimeCapability, RuntimeStatus,
    SessionSwitchMode, SessionSwitchRequest, StreamingMode, Timestamp90k, VtDecoderOptions,
    VtEncoderOptions, VtSessionConfig, VulkanDecoderOptions, VulkanEncoderOptions,
};
pub(crate) use contract::{EncodedPacket, Frame, VideoDecoder, VideoEncoder};
pub use pipeline::{
    BoundedQueueRx, BoundedQueueTx, InFlightCredits, QueueRecvError, QueueSendError, QueueStats,
    bounded_queue,
};
pub use transform::{
    ColorRequest, Nv12Frame, RgbFrame, TransformDispatcher, TransformJob, TransformResult,
    make_argb_to_nv12_dummy, nv12_to_rgb24, should_enqueue_transform,
};
#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
pub use video_hw_backend_intel::{IntelDecoderAdapter, IntelEncoderAdapter};
#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
pub use video_hw_backend_nvidia::{NvDecoderAdapter, NvEncoderAdapter};
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use video_hw_backend_vt as vt_backend;
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
pub use video_hw_backend_vt::{VtDecoderAdapter, VtEncoderAdapter};
#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
pub use video_hw_backend_vulkan::{VulkanAdapterReport, vulkan_adapter_reports};
#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
pub use video_hw_backend_vulkan::{VulkanDecoderAdapter, VulkanEncoderAdapter};
pub use video_hw_core::bitstream;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    VideoToolbox,
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    Nvidia,
    #[cfg(all(
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ))]
    Intel,
    #[cfg(all(
        feature = "backend-vulkan",
        any(target_os = "linux", target_os = "windows")
    ))]
    Vulkan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    #[default]
    Auto,
    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    VideoToolbox,
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    Nvidia,
    #[cfg(all(
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ))]
    Intel,
    #[cfg(all(
        feature = "backend-vulkan",
        any(target_os = "linux", target_os = "windows")
    ))]
    Vulkan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendParseError {
    input: String,
}

impl BackendParseError {
    fn new(input: &str) -> Self {
        Self {
            input: input.to_string(),
        }
    }
}

impl fmt::Display for BackendParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unsupported backend: {}", self.input)
    }
}

impl std::error::Error for BackendParseError {}

#[cfg(any(
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
impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox => f.write_str("videotoolbox"),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia => f.write_str("nvidia"),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Intel => f.write_str("intel"),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Vulkan => f.write_str("vulkan"),
        }
    }
}

#[cfg(not(any(
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
impl fmt::Display for BackendKind {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {}
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Backend::Auto => f.write_str("auto"),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Backend::VideoToolbox => f.write_str("videotoolbox"),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Backend::Nvidia => f.write_str("nvidia"),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Backend::Intel => f.write_str("intel"),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Backend::Vulkan => f.write_str("vulkan"),
        }
    }
}

impl Backend {
    #[must_use]
    pub fn supported() -> Vec<Self> {
        [
            Some(Self::Auto),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Some(Self::VideoToolbox),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Some(Self::Nvidia),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Some(Self::Intel),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Some(Self::Vulkan),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

impl FromStr for Backend {
    type Err = BackendParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            "vt" | "videotoolbox" => Ok(Self::VideoToolbox),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            "nvidia" | "nv" => Ok(Self::Nvidia),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            "intel" | "qsv" => Ok(Self::Intel),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            "vulkan" | "vk" => Ok(Self::Vulkan),
            _ => Err(BackendParseError::new(raw)),
        }
    }
}

#[cfg(any(
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
impl BackendKind {
    #[must_use]
    pub fn os_default() -> Self {
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        {
            BackendKind::VideoToolbox
        }
        #[cfg(all(
            not(all(target_os = "macos", feature = "backend-vt")),
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        {
            BackendKind::Nvidia
        }
        #[cfg(all(
            not(all(target_os = "macos", feature = "backend-vt")),
            not(feature = "backend-nvidia"),
            feature = "backend-intel",
            any(target_os = "linux", target_os = "windows")
        ))]
        {
            BackendKind::Intel
        }
        #[cfg(all(
            not(all(target_os = "macos", feature = "backend-vt")),
            not(any(feature = "backend-nvidia", feature = "backend-intel")),
            feature = "backend-vulkan",
            any(target_os = "linux", target_os = "windows")
        ))]
        {
            BackendKind::Vulkan
        }
    }
}

#[cfg(any(
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
impl Default for BackendKind {
    fn default() -> Self {
        Self::os_default()
    }
}

impl Backend {
    pub fn resolve_decoder(self, config: &DecoderConfig) -> Result<BackendKind, BackendError> {
        match self {
            Backend::Auto => select_decoder_backend(config),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Backend::VideoToolbox => Ok(BackendKind::VideoToolbox),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Backend::Nvidia => Ok(BackendKind::Nvidia),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Backend::Intel => Ok(BackendKind::Intel),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Backend::Vulkan => Ok(BackendKind::Vulkan),
        }
    }

    pub fn resolve_encoder(self, config: &EncoderConfig) -> Result<BackendKind, BackendError> {
        match self {
            Backend::Auto => select_encoder_backend(config),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Backend::VideoToolbox => Ok(BackendKind::VideoToolbox),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Backend::Nvidia => Ok(BackendKind::Nvidia),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Backend::Intel => Ok(BackendKind::Intel),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Backend::Vulkan => Ok(BackendKind::Vulkan),
        }
    }
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
fn preferred_backend_order() -> Vec<BackendKind> {
    vec![BackendKind::VideoToolbox]
}

#[cfg(all(
    any(target_os = "linux", target_os = "windows"),
    any(
        feature = "backend-nvidia",
        feature = "backend-intel",
        feature = "backend-vulkan"
    )
))]
fn preferred_backend_order() -> Vec<BackendKind> {
    vec![
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        BackendKind::Nvidia,
        #[cfg(all(
            feature = "backend-intel",
            any(target_os = "linux", target_os = "windows")
        ))]
        BackendKind::Intel,
        #[cfg(all(
            feature = "backend-vulkan",
            any(target_os = "linux", target_os = "windows")
        ))]
        BackendKind::Vulkan,
    ]
}

#[cfg(any(
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
fn probe_decoder_capability(
    kind: BackendKind,
    config: &DecoderConfig,
) -> Result<CapabilityReport, BackendError> {
    match kind {
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        BackendKind::VideoToolbox => {
            let probe = <vt_backend::VtDecoderAdapter as DecoderBackend>::from_decoder_config(
                config.clone(),
            );
            probe.query_capability(config.codec)
        }
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        BackendKind::Nvidia => {
            let probe = <NvDecoderAdapter as DecoderBackend>::from_decoder_config(config.clone());
            probe.query_capability(config.codec)
        }
        #[cfg(all(
            feature = "backend-intel",
            any(target_os = "linux", target_os = "windows")
        ))]
        BackendKind::Intel => {
            let probe =
                <IntelDecoderAdapter as DecoderBackend>::from_decoder_config(config.clone());
            probe.query_capability(config.codec)
        }
        #[cfg(all(
            feature = "backend-vulkan",
            any(target_os = "linux", target_os = "windows")
        ))]
        BackendKind::Vulkan => {
            let probe =
                <VulkanDecoderAdapter as DecoderBackend>::from_decoder_config(config.clone());
            probe.query_capability(config.codec)
        }
    }
}

#[cfg(any(
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
fn probe_encoder_capability(
    kind: BackendKind,
    config: &EncoderConfig,
) -> Result<CapabilityReport, BackendError> {
    match kind {
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        BackendKind::VideoToolbox => {
            let probe = <vt_backend::VtEncoderAdapter as EncoderBackend>::from_encoder_config(
                config.clone(),
            );
            probe.query_capability(config.codec)
        }
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        BackendKind::Nvidia => {
            let probe = <NvEncoderAdapter as EncoderBackend>::from_encoder_config(config.clone());
            probe.query_capability(config.codec)
        }
        #[cfg(all(
            feature = "backend-intel",
            any(target_os = "linux", target_os = "windows")
        ))]
        BackendKind::Intel => {
            let probe =
                <IntelEncoderAdapter as EncoderBackend>::from_encoder_config(config.clone());
            probe.query_capability(config.codec)
        }
        #[cfg(all(
            feature = "backend-vulkan",
            any(target_os = "linux", target_os = "windows")
        ))]
        BackendKind::Vulkan => {
            let probe =
                <VulkanEncoderAdapter as EncoderBackend>::from_encoder_config(config.clone());
            probe.query_capability(config.codec)
        }
    }
}

#[cfg(any(
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
pub fn select_decoder_backend(config: &DecoderConfig) -> Result<BackendKind, BackendError> {
    let mut diagnostics = Vec::new();
    for candidate in preferred_backend_order() {
        match probe_decoder_capability(candidate, config) {
            Ok(capability) => {
                if capability.contract.decode.supported
                    && (!config.require_hardware || capability.runtime.hardware_acceleration)
                    && capability.supports_decode_output_mode(config.output_mode)
                {
                    return Ok(candidate);
                }
                diagnostics.push(format!(
                    "{candidate:?}: decode_supported={}, hw_accel={}, output_mode_supported={}",
                    capability.contract.decode.supported,
                    capability.runtime.hardware_acceleration,
                    capability.supports_decode_output_mode(config.output_mode)
                ));
            }
            Err(err) => diagnostics.push(format!("{candidate:?}: {err}")),
        }
    }
    let detail = if diagnostics.is_empty() {
        "no eligible backend candidate".to_string()
    } else {
        diagnostics.join("; ")
    };
    Err(BackendError::UnsupportedConfig(format!(
        "auto backend selection failed for decode ({:?}): {}",
        config.codec, detail
    )))
}

#[cfg(not(any(
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
pub fn select_decoder_backend(_config: &DecoderConfig) -> Result<BackendKind, BackendError> {
    Err(BackendError::UnsupportedConfig(
        "no backend feature enabled".to_string(),
    ))
}

#[cfg(any(
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
pub fn select_encoder_backend(config: &EncoderConfig) -> Result<BackendKind, BackendError> {
    let mut diagnostics = Vec::new();
    for candidate in preferred_backend_order() {
        match probe_encoder_capability(candidate, config) {
            Ok(capability) => {
                if capability.contract.encode.supported
                    && (!config.require_hardware || capability.runtime.hardware_acceleration)
                    && capability.supports_encode_input_format(config.input_format)
                {
                    return Ok(candidate);
                }
                diagnostics.push(format!(
                    "{candidate:?}: encode_supported={}, hw_accel={}, input_format_supported={}",
                    capability.contract.encode.supported,
                    capability.runtime.hardware_acceleration,
                    capability.supports_encode_input_format(config.input_format)
                ));
            }
            Err(err) => diagnostics.push(format!("{candidate:?}: {err}")),
        }
    }
    let detail = if diagnostics.is_empty() {
        "no eligible backend candidate".to_string()
    } else {
        diagnostics.join("; ")
    };
    Err(BackendError::UnsupportedConfig(format!(
        "auto backend selection failed for encode ({:?}): {}",
        config.codec, detail
    )))
}

#[cfg(not(any(
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
pub fn select_encoder_backend(_config: &EncoderConfig) -> Result<BackendKind, BackendError> {
    Err(BackendError::UnsupportedConfig(
        "no backend feature enabled".to_string(),
    ))
}

pub trait DecoderBackend: VideoDecoder {
    const BACKEND_KIND: BackendKind;

    fn from_decoder_config(config: DecoderConfig) -> Self;

    fn supports_output_mode(mode: DecodeOutputMode) -> bool {
        let _ = mode;
        true
    }
}

fn backend_supports_output_mode(kind: BackendKind, mode: DecodeOutputMode) -> bool {
    let _ = mode;
    match kind {
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        BackendKind::VideoToolbox => {
            <vt_backend::VtDecoderAdapter as DecoderBackend>::supports_output_mode(mode)
        }
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        BackendKind::Nvidia => <NvDecoderAdapter as DecoderBackend>::supports_output_mode(mode),
        #[cfg(all(
            feature = "backend-intel",
            any(target_os = "linux", target_os = "windows")
        ))]
        BackendKind::Intel => <IntelDecoderAdapter as DecoderBackend>::supports_output_mode(mode),
        #[cfg(all(
            feature = "backend-vulkan",
            any(target_os = "linux", target_os = "windows")
        ))]
        BackendKind::Vulkan => <VulkanDecoderAdapter as DecoderBackend>::supports_output_mode(mode),
    }
}

#[cfg(any(
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
fn preflight_decoder_capability(
    kind: BackendKind,
    config: &DecoderConfig,
) -> Result<CapabilityReport, BackendError> {
    probe_decoder_capability(kind, config)
}

#[cfg(not(any(
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
fn preflight_decoder_capability(
    _kind: BackendKind,
    _config: &DecoderConfig,
) -> Result<CapabilityReport, BackendError> {
    Err(BackendError::UnsupportedConfig(
        "no backend feature enabled".to_string(),
    ))
}

#[cfg(any(
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
fn preflight_encoder_capability(
    kind: BackendKind,
    config: &EncoderConfig,
) -> Result<CapabilityReport, BackendError> {
    probe_encoder_capability(kind, config)
}

#[cfg(not(any(
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
fn preflight_encoder_capability(
    _kind: BackendKind,
    _config: &EncoderConfig,
) -> Result<CapabilityReport, BackendError> {
    Err(BackendError::UnsupportedConfig(
        "no backend feature enabled".to_string(),
    ))
}

pub trait EncoderBackend: VideoEncoder {
    const BACKEND_KIND: BackendKind;

    fn from_encoder_config(config: EncoderConfig) -> Self;
}

pub trait SessionSwitchingEncoderBackend: EncoderBackend {}

trait DynDecodeSession {
    fn backend_kind(&self) -> BackendKind;
    fn submit(&mut self, input: BitstreamInput) -> Result<(), BackendError>;
    fn try_reap(&mut self) -> Result<Option<DecodedFrame>, BackendError>;
    fn flush(&mut self) -> Result<Vec<DecodedFrame>, BackendError>;
    fn summary(&self) -> DecodeSummary;
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError>;
}

trait DynEncodeSession {
    fn backend_kind(&self) -> BackendKind;
    fn submit(&mut self, frame: EncodeFrame) -> Result<(), BackendError>;
    fn try_reap(&mut self) -> Result<Option<EncodedChunk>, BackendError>;
    fn flush(&mut self) -> Result<Vec<EncodedChunk>, BackendError>;
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError>;
    fn request_session_switch(&mut self, request: SessionSwitchRequest)
    -> Result<(), BackendError>;
    fn requires_periodic_fragment_flush(&self) -> bool;
}

pub struct AnyDecodeSession {
    inner: Box<dyn DynDecodeSession>,
}

#[derive(Debug, Clone)]
pub struct DecodePreflightRequest {
    pub backend: Backend,
    pub codec: Codec,
    pub output_mode: DecodeOutputMode,
    pub require_hardware: bool,
}

#[derive(Debug, Clone)]
pub struct DecodePreflightReport {
    pub requested_backend: Backend,
    pub resolved_backend: Option<BackendKind>,
    pub output_mode: DecodeOutputMode,
    pub supported_by_contract: bool,
    pub usable_in_current_runtime: bool,
    pub decode_supported: Option<bool>,
    pub hardware_acceleration: Option<bool>,
    pub output_mode_supported: Option<bool>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EncodePreflightRequest {
    pub backend: Backend,
    pub codec: Codec,
    pub input_format: EncodeInputFormat,
    pub require_hardware: bool,
    pub expected_layout: Option<EncodedLayout>,
}

#[derive(Debug, Clone)]
pub struct EncodePreflightReport {
    pub requested_backend: Backend,
    pub resolved_backend: Option<BackendKind>,
    pub input_format: EncodeInputFormat,
    pub expected_layout: Option<EncodedLayout>,
    pub supported_by_contract: bool,
    pub usable_in_current_runtime: bool,
    pub accepted_input_formats: Vec<EncodeInputFormat>,
    pub encoded_layouts: Vec<EncodedLayout>,
    pub reason: Option<String>,
}

pub fn preflight_decode(request: DecodePreflightRequest) -> DecodePreflightReport {
    let mut config = DecoderConfig::new(request.codec, 30, request.require_hardware);
    config.output_mode = request.output_mode;
    let resolved = request.backend.resolve_decoder(&config);
    let Ok(kind) = resolved else {
        return DecodePreflightReport {
            requested_backend: request.backend,
            resolved_backend: None,
            output_mode: request.output_mode,
            supported_by_contract: false,
            usable_in_current_runtime: false,
            decode_supported: None,
            hardware_acceleration: None,
            output_mode_supported: None,
            reason: Some(resolved.unwrap_err().to_string()),
        };
    };
    let supported_by_contract = backend_supports_output_mode(kind, request.output_mode);
    if !supported_by_contract {
        return DecodePreflightReport {
            requested_backend: request.backend,
            resolved_backend: Some(kind),
            output_mode: request.output_mode,
            supported_by_contract,
            usable_in_current_runtime: false,
            decode_supported: None,
            hardware_acceleration: None,
            output_mode_supported: Some(false),
            reason: Some(format!(
                "{kind:?} decoder does not support DecodeOutputMode::{} by contract",
                request.output_mode
            )),
        };
    }
    let runtime = preflight_decoder_capability(kind, &config);
    match runtime {
        Ok(capability) => {
            let output_mode_supported = capability.supports_decode_output_mode(request.output_mode);
            let usable = capability.contract.decode.supported
                && (!request.require_hardware || capability.runtime.hardware_acceleration)
                && output_mode_supported;
            DecodePreflightReport {
                requested_backend: request.backend,
                resolved_backend: Some(kind),
                output_mode: request.output_mode,
                supported_by_contract,
                usable_in_current_runtime: usable,
                decode_supported: Some(capability.contract.decode.supported),
                hardware_acceleration: Some(capability.runtime.hardware_acceleration),
                output_mode_supported: Some(output_mode_supported),
                reason: if usable {
                    None
                } else {
                    Some(format!(
                        "decode_supported={}, hardware_acceleration={}, output_mode_supported={}",
                        capability.contract.decode.supported,
                        capability.runtime.hardware_acceleration,
                        capability.supports_decode_output_mode(request.output_mode)
                    ))
                },
            }
        }
        Err(err) => DecodePreflightReport {
            requested_backend: request.backend,
            resolved_backend: Some(kind),
            output_mode: request.output_mode,
            supported_by_contract,
            usable_in_current_runtime: false,
            decode_supported: None,
            hardware_acceleration: None,
            output_mode_supported: None,
            reason: Some(err.to_string()),
        },
    }
}

pub fn preflight_encode(request: EncodePreflightRequest) -> EncodePreflightReport {
    let config = EncoderConfig::new(
        request.codec,
        30,
        request.require_hardware,
        request.input_format,
    );
    let resolved = request.backend.resolve_encoder(&config);
    let Ok(kind) = resolved else {
        return EncodePreflightReport {
            requested_backend: request.backend,
            resolved_backend: None,
            input_format: request.input_format,
            expected_layout: request.expected_layout,
            supported_by_contract: false,
            usable_in_current_runtime: false,
            accepted_input_formats: Vec::new(),
            encoded_layouts: Vec::new(),
            reason: Some(resolved.unwrap_err().to_string()),
        };
    };
    match preflight_encoder_capability(kind, &config) {
        Ok(capability) => {
            let input_supported = capability.supports_encode_input_format(request.input_format);
            let layout_supported = request
                .expected_layout
                .is_none_or(|layout| capability.supports_encoded_layout(layout));
            let supported_by_contract =
                capability.contract.encode.supported && input_supported && layout_supported;
            let runtime_usable =
                !request.require_hardware || capability.runtime.hardware_acceleration;
            let usable = supported_by_contract && runtime_usable;
            EncodePreflightReport {
                requested_backend: request.backend,
                resolved_backend: Some(kind),
                input_format: request.input_format,
                expected_layout: request.expected_layout,
                supported_by_contract,
                usable_in_current_runtime: usable,
                accepted_input_formats: capability.contract.encode.input_formats,
                encoded_layouts: capability.contract.encode.encoded_layouts,
                reason: if usable {
                    None
                } else {
                    Some(format!(
                        "encode_supported={}, hardware_acceleration={}, input_format_supported={}, expected_layout_supported={}",
                        capability.contract.encode.supported,
                        capability.runtime.hardware_acceleration,
                        input_supported,
                        layout_supported
                    ))
                },
            }
        }
        Err(err) => EncodePreflightReport {
            requested_backend: request.backend,
            resolved_backend: Some(kind),
            input_format: request.input_format,
            expected_layout: request.expected_layout,
            supported_by_contract: false,
            usable_in_current_runtime: false,
            accepted_input_formats: Vec::new(),
            encoded_layouts: Vec::new(),
            reason: Some(err.to_string()),
        },
    }
}

impl AnyDecodeSession {
    pub fn new(backend: Backend, config: DecoderConfig) -> Result<Self, BackendError> {
        let kind = backend.resolve_decoder(&config)?;
        Self::with_backend_kind(kind, config)
    }

    pub fn with_backend_kind(
        kind: BackendKind,
        config: DecoderConfig,
    ) -> Result<Self, BackendError> {
        if !backend_supports_output_mode(kind, config.output_mode) {
            return Err(BackendError::UnsupportedConfig(format!(
                "{kind:?} decoder does not support DecodeOutputMode::{}",
                config.output_mode
            )));
        }
        match kind {
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            BackendKind::VideoToolbox => Ok(Self {
                inner: Box::new(DecodeSession::<VtDecoderAdapter>::new(config)),
            }),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Nvidia => Ok(Self {
                inner: Box::new(DecodeSession::<NvDecoderAdapter>::new(config)),
            }),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Intel => Ok(Self {
                inner: Box::new(DecodeSession::<IntelDecoderAdapter>::new(config)),
            }),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Vulkan => Ok(Self {
                inner: Box::new(DecodeSession::<VulkanDecoderAdapter>::new(config)),
            }),
        }
    }

    #[must_use]
    pub fn backend_kind(&self) -> BackendKind {
        self.inner.backend_kind()
    }

    pub fn submit(&mut self, input: BitstreamInput) -> Result<(), BackendError> {
        self.inner.submit(input)
    }

    pub fn try_reap(&mut self) -> Result<Option<DecodedFrame>, BackendError> {
        self.inner.try_reap()
    }

    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>, BackendError> {
        self.inner.flush()
    }

    pub fn summary(&self) -> DecodeSummary {
        self.inner.summary()
    }

    pub fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        self.inner.query_capability(codec)
    }
}

pub struct AnyEncodeSession {
    inner: Box<dyn DynEncodeSession>,
}

impl AnyEncodeSession {
    pub fn new(backend: Backend, config: EncoderConfig) -> Result<Self, BackendError> {
        let kind = backend.resolve_encoder(&config)?;
        Self::with_backend_kind(kind, config)
    }

    pub fn with_backend_kind(
        kind: BackendKind,
        config: EncoderConfig,
    ) -> Result<Self, BackendError> {
        if let Ok(capability) = preflight_encoder_capability(kind, &config)
            && !capability.supports_encode_input_format(config.input_format)
        {
            return Err(BackendError::UnsupportedConfig(format!(
                "{kind:?} encoder does not support {} input",
                config.input_format
            )));
        }
        match kind {
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            BackendKind::VideoToolbox => Ok(Self {
                inner: Box::new(EncodeSession::<VtEncoderAdapter>::new(config)),
            }),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Nvidia => Ok(Self {
                inner: Box::new(EncodeSession::<NvEncoderAdapter>::new(config)),
            }),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Intel => Ok(Self {
                inner: Box::new(EncodeSession::<IntelEncoderAdapter>::new(config)),
            }),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Vulkan => Ok(Self {
                inner: Box::new(EncodeSession::<VulkanEncoderAdapter>::new(config)),
            }),
        }
    }

    #[must_use]
    pub fn backend_kind(&self) -> BackendKind {
        self.inner.backend_kind()
    }

    pub fn submit(&mut self, frame: EncodeFrame) -> Result<(), BackendError> {
        self.inner.submit(frame)
    }

    pub fn try_reap(&mut self) -> Result<Option<EncodedChunk>, BackendError> {
        self.inner.try_reap()
    }

    pub fn flush(&mut self) -> Result<Vec<EncodedChunk>, BackendError> {
        self.inner.flush()
    }

    pub fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        self.inner.query_capability(codec)
    }

    pub fn request_session_switch(
        &mut self,
        request: SessionSwitchRequest,
    ) -> Result<(), BackendError> {
        self.inner.request_session_switch(request)
    }

    #[must_use]
    pub fn requires_periodic_fragment_flush(&self) -> bool {
        self.inner.requires_periodic_fragment_flush()
    }
}

pub struct DecodeSession<D: VideoDecoder> {
    decoder_inner: D,
    output_mode: DecodeOutputMode,
    ready: VecDeque<DecodedFrame>,
}

impl<D: VideoDecoder> DecodeSession<D> {
    #[must_use]
    pub fn from_decoder(output_mode: DecodeOutputMode, decoder: D) -> Self {
        Self {
            decoder_inner: decoder,
            output_mode,
            ready: VecDeque::new(),
        }
    }

    pub fn submit(&mut self, input: BitstreamInput) -> Result<(), BackendError> {
        let (annexb, pts_90k) = normalize_bitstream_input(input)?;
        let outputs = self
            .decoder_inner
            .push_bitstream_chunk(&annexb, pts_90k)?
            .into_iter()
            .map(|frame| backend_frame_to_decoded_frame(frame, self.output_mode))
            .collect::<Result<Vec<_>, _>>()?;
        self.ready.extend(outputs);
        Ok(())
    }

    pub fn try_reap(&mut self) -> Result<Option<DecodedFrame>, BackendError> {
        if let Some(frame) = self.ready.pop_front() {
            return Ok(Some(frame));
        }
        let polled = self
            .decoder_inner
            .try_reap()?
            .into_iter()
            .map(|frame| backend_frame_to_decoded_frame(frame, self.output_mode))
            .collect::<Result<Vec<_>, _>>()?;
        self.ready.extend(polled);
        Ok(self.ready.pop_front())
    }

    pub fn reap_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<DecodedFrame>, BackendError> {
        if let Some(frame) = self.try_reap()? {
            return Ok(Some(frame));
        }
        if timeout.is_zero() {
            return Ok(None);
        }
        let Some(deadline) = Instant::now().checked_add(timeout) else {
            return Ok(None);
        };
        const POLL_INTERVAL: Duration = Duration::from_millis(1);
        loop {
            if let Some(frame) = self.try_reap()? {
                return Ok(Some(frame));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let remaining = deadline.saturating_duration_since(now);
            std::thread::sleep(remaining.min(POLL_INTERVAL));
        }
    }

    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>, BackendError> {
        let mut out = std::mem::take(&mut self.ready)
            .into_iter()
            .collect::<Vec<_>>();
        out.extend(
            self.decoder_inner
                .flush()?
                .into_iter()
                .map(|frame| backend_frame_to_decoded_frame(frame, self.output_mode))
                .collect::<Result<Vec<_>, _>>()?,
        );
        Ok(out)
    }

    pub fn summary(&self) -> DecodeSummary {
        self.decoder_inner.decode_summary()
    }

    pub fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        self.decoder_inner.query_capability(codec)
    }
}

impl<D> DecodeSession<D>
where
    D: DecoderBackend,
{
    pub fn new(config: DecoderConfig) -> Self {
        let output_mode = config.output_mode;
        let decoder = D::from_decoder_config(config);
        Self::from_decoder(output_mode, decoder)
    }

    #[must_use]
    pub fn backend_kind_static() -> BackendKind {
        D::BACKEND_KIND
    }
}

impl<D> DynDecodeSession for DecodeSession<D>
where
    D: DecoderBackend + 'static,
{
    fn backend_kind(&self) -> BackendKind {
        D::BACKEND_KIND
    }

    fn submit(&mut self, input: BitstreamInput) -> Result<(), BackendError> {
        DecodeSession::submit(self, input)
    }

    fn try_reap(&mut self) -> Result<Option<DecodedFrame>, BackendError> {
        DecodeSession::try_reap(self)
    }

    fn flush(&mut self) -> Result<Vec<DecodedFrame>, BackendError> {
        DecodeSession::flush(self)
    }

    fn summary(&self) -> DecodeSummary {
        DecodeSession::summary(self)
    }

    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        DecodeSession::query_capability(self, codec)
    }
}

pub struct EncodeSession<E: VideoEncoder> {
    backend_kind: BackendKind,
    input_format: EncodeInputFormat,
    encoder_inner: E,
    ready: VecDeque<EncodedChunk>,
}

impl<E: VideoEncoder> EncodeSession<E> {
    #[must_use]
    pub fn from_encoder(
        backend_kind: BackendKind,
        input_format: EncodeInputFormat,
        encoder: E,
    ) -> Self {
        Self {
            backend_kind,
            input_format,
            encoder_inner: encoder,
            ready: VecDeque::new(),
        }
    }

    #[must_use]
    pub fn backend_kind(&self) -> BackendKind {
        self.backend_kind
    }

    pub fn submit(&mut self, frame: EncodeFrame) -> Result<(), BackendError> {
        let actual_format = frame.buffer.input_format();
        if actual_format != self.input_format {
            return Err(BackendError::InvalidInput(format!(
                "encoder configured for {} input, got {}",
                self.input_format, actual_format
            )));
        }
        let backend_frame = encode_frame_into_backend_frame(frame)?;
        let outputs = self
            .encoder_inner
            .push_frame(backend_frame)?
            .into_iter()
            .map(|packet| backend_packet_to_encoded_chunk(self.backend_kind, packet))
            .collect::<Vec<_>>();
        self.ready.extend(outputs);
        Ok(())
    }

    pub fn try_reap(&mut self) -> Result<Option<EncodedChunk>, BackendError> {
        if let Some(chunk) = self.ready.pop_front() {
            return Ok(Some(chunk));
        }
        let polled = self
            .encoder_inner
            .try_reap()?
            .into_iter()
            .map(|packet| backend_packet_to_encoded_chunk(self.backend_kind, packet))
            .collect::<Vec<_>>();
        self.ready.extend(polled);
        Ok(self.ready.pop_front())
    }

    pub fn reap_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<EncodedChunk>, BackendError> {
        if let Some(chunk) = self.try_reap()? {
            return Ok(Some(chunk));
        }
        if timeout.is_zero() {
            return Ok(None);
        }
        let Some(deadline) = Instant::now().checked_add(timeout) else {
            return Ok(None);
        };
        const POLL_INTERVAL: Duration = Duration::from_millis(1);
        loop {
            if let Some(chunk) = self.try_reap()? {
                return Ok(Some(chunk));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let remaining = deadline.saturating_duration_since(now);
            std::thread::sleep(remaining.min(POLL_INTERVAL));
        }
    }

    pub fn flush(&mut self) -> Result<Vec<EncodedChunk>, BackendError> {
        let mut out = std::mem::take(&mut self.ready)
            .into_iter()
            .collect::<Vec<_>>();
        out.extend(
            self.encoder_inner
                .flush()?
                .into_iter()
                .map(|packet| backend_packet_to_encoded_chunk(self.backend_kind, packet))
                .collect::<Vec<_>>(),
        );
        Ok(out)
    }

    pub fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        self.encoder_inner.query_capability(codec)
    }

    pub fn request_session_switch(
        &mut self,
        request: SessionSwitchRequest,
    ) -> Result<(), BackendError> {
        self.encoder_inner.request_session_switch(request)
    }
}

impl<E> EncodeSession<E>
where
    E: EncoderBackend,
{
    pub fn new(config: EncoderConfig) -> Self {
        let input_format = config.input_format;
        let encoder = E::from_encoder_config(config);
        Self::from_encoder(E::BACKEND_KIND, input_format, encoder)
    }

    #[must_use]
    pub fn backend_kind_static() -> BackendKind {
        E::BACKEND_KIND
    }
}

impl<E> EncodeSession<E>
where
    E: SessionSwitchingEncoderBackend,
{
    pub fn request_session_switch_strict(
        &mut self,
        request: SessionSwitchRequest,
    ) -> Result<(), BackendError> {
        self.encoder_inner.request_session_switch(request)
    }
}

impl<E> DynEncodeSession for EncodeSession<E>
where
    E: EncoderBackend + 'static,
{
    fn backend_kind(&self) -> BackendKind {
        EncodeSession::backend_kind(self)
    }

    fn submit(&mut self, frame: EncodeFrame) -> Result<(), BackendError> {
        EncodeSession::submit(self, frame)
    }

    fn try_reap(&mut self) -> Result<Option<EncodedChunk>, BackendError> {
        EncodeSession::try_reap(self)
    }

    fn flush(&mut self) -> Result<Vec<EncodedChunk>, BackendError> {
        EncodeSession::flush(self)
    }

    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        EncodeSession::query_capability(self, codec)
    }

    fn request_session_switch(
        &mut self,
        request: SessionSwitchRequest,
    ) -> Result<(), BackendError> {
        EncodeSession::request_session_switch(self, request)
    }

    fn requires_periodic_fragment_flush(&self) -> bool {
        true
    }
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
impl DecoderBackend for vt_backend::VtDecoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::VideoToolbox;

    fn from_decoder_config(config: DecoderConfig) -> Self {
        Self::new(config)
    }
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
impl EncoderBackend for vt_backend::VtEncoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::VideoToolbox;

    fn from_encoder_config(config: EncoderConfig) -> Self {
        Self::with_config_and_options(
            config.codec,
            config.fps,
            config.require_hardware,
            config.backend_options,
        )
    }
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
impl SessionSwitchingEncoderBackend for vt_backend::VtEncoderAdapter {}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
impl DecoderBackend for NvDecoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Nvidia;

    fn from_decoder_config(config: DecoderConfig) -> Self {
        Self::new(config)
    }

    fn supports_output_mode(mode: DecodeOutputMode) -> bool {
        matches!(
            mode,
            DecodeOutputMode::Metadata | DecodeOutputMode::Nv12 | DecodeOutputMode::Rgb24
        )
    }
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
impl EncoderBackend for NvEncoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Nvidia;

    fn from_encoder_config(config: EncoderConfig) -> Self {
        Self::with_config(
            config.codec,
            config.fps,
            config.require_hardware,
            config.backend_options,
        )
    }
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
impl SessionSwitchingEncoderBackend for NvEncoderAdapter {}

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
impl DecoderBackend for IntelDecoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Intel;

    fn from_decoder_config(config: DecoderConfig) -> Self {
        Self::new(config)
    }

    fn supports_output_mode(mode: DecodeOutputMode) -> bool {
        matches!(
            mode,
            DecodeOutputMode::Metadata | DecodeOutputMode::Nv12 | DecodeOutputMode::Rgb24
        )
    }
}

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
impl EncoderBackend for IntelEncoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Intel;

    fn from_encoder_config(config: EncoderConfig) -> Self {
        Self::with_config(
            config.codec,
            config.fps,
            config.require_hardware,
            config.backend_options,
        )
    }
}

#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
impl DecoderBackend for VulkanDecoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Vulkan;

    fn from_decoder_config(config: DecoderConfig) -> Self {
        Self::new(config)
    }

    fn supports_output_mode(mode: DecodeOutputMode) -> bool {
        matches!(
            mode,
            DecodeOutputMode::Metadata | DecodeOutputMode::Nv12 | DecodeOutputMode::Rgb24
        )
    }
}

#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
impl EncoderBackend for VulkanEncoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Vulkan;

    fn from_encoder_config(config: EncoderConfig) -> Self {
        Self::with_config(
            config.codec,
            config.fps,
            config.require_hardware,
            config.backend_options,
        )
    }
}

fn normalize_bitstream_input(
    input: BitstreamInput,
) -> Result<(Vec<u8>, Option<i64>), BackendError> {
    match input {
        BitstreamInput::AnnexBChunk { chunk, pts_90k } => Ok((chunk, pts_90k.map(|v| v.0))),
        BitstreamInput::AccessUnitRawNal {
            codec: _,
            nalus,
            pts_90k,
        } => {
            let mut out = Vec::new();
            for nalu in nalus {
                bitstream::append_annexb_nalu(&mut out, &nalu);
            }
            Ok((out, pts_90k.map(|v| v.0)))
        }
        BitstreamInput::LengthPrefixedSample {
            codec: Codec::Av1,
            sample,
            pts_90k,
        } => Ok((sample, pts_90k.map(|v| v.0))),
        BitstreamInput::LengthPrefixedSample {
            codec: _,
            sample,
            pts_90k,
        } => Ok((
            bitstream::length_prefixed_to_annexb(
                bitstream::LengthPrefixedSampleRef::new(&sample),
                bitstream::NalLengthSize::Four,
            )?
            .into_inner(),
            pts_90k.map(|v| v.0),
        )),
    }
}

fn backend_frame_to_decoded_frame(
    frame: Frame,
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
                validate_nv12_payload(&nv12, frame.width, frame.height)?;
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
            let (pitch, data) = argb_to_nv12(argb, frame.width, frame.height)?;
            return Ok(DecodedFrame::Nv12 {
                dims,
                pitch,
                pts_90k: frame.pts_90k.map(Timestamp90k),
                data,
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
            let data = if let Some(argb) = frame_argb_payload(&frame) {
                argb_to_rgb24(argb, frame.width, frame.height)?
            } else if let Some(nv12) = frame.nv12 {
                validate_nv12_payload(&nv12, frame.width, frame.height)?;
                nv12_to_rgb24(&Nv12Frame {
                    width: frame.width,
                    height: frame.height,
                    pitch: nv12.pitch,
                    pts_90k: frame.pts_90k,
                    data: nv12.data,
                })?
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

#[cfg(any(
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
fn frame_argb_payload(frame: &Frame) -> Option<&[u8]> {
    frame.argb.as_deref()
}

#[cfg(not(any(
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
fn frame_argb_payload(_frame: &Frame) -> Option<&[u8]> {
    None
}

fn validate_nv12_payload(
    nv12: &Nv12FramePayload,
    width: usize,
    height: usize,
) -> Result<(), BackendError> {
    if width == 0 || height == 0 {
        return Err(BackendError::InvalidInput(
            "nv12 frame dimensions must be positive".to_string(),
        ));
    }
    if nv12.pitch < width {
        return Err(BackendError::InvalidInput(format!(
            "nv12 pitch is smaller than width: pitch={}, width={width}",
            nv12.pitch
        )));
    }
    let luma_size = nv12
        .pitch
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("nv12 luma size overflow".to_string()))?;
    let expected = luma_size
        .checked_add(luma_size / 2)
        .ok_or_else(|| BackendError::InvalidInput("nv12 total size overflow".to_string()))?;
    if nv12.data.len() < expected {
        return Err(BackendError::InvalidInput(format!(
            "nv12 payload size mismatch: expected at least {expected}, got {}",
            nv12.data.len()
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
    let mut out = Vec::with_capacity(width * height * 3);
    for px in argb.chunks_exact(4) {
        out.extend_from_slice(&[px[1], px[2], px[3]]);
    }
    Ok(out)
}

fn argb_to_nv12(
    argb: &[u8],
    width: usize,
    height: usize,
) -> Result<(usize, Vec<u8>), BackendError> {
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
    if width == 0 || height == 0 {
        return Err(BackendError::InvalidInput(
            "argb frame dimensions must be positive".to_string(),
        ));
    }
    let pitch = width;
    let y_size = pitch
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("nv12 luma size overflow".to_string()))?;
    let uv_size = y_size / 2;
    let mut out = vec![0_u8; y_size + uv_size];
    let uv_base = y_size;
    for y in (0..height).step_by(2) {
        let uv_row = uv_base + (y / 2) * pitch;
        for x in (0..width).step_by(2) {
            let mut u_acc = 0_i32;
            let mut v_acc = 0_i32;
            let mut sample_count = 0_i32;

            for dy in 0..2 {
                let py = y + dy;
                if py >= height {
                    continue;
                }
                let y_row = py * pitch;
                for dx in 0..2 {
                    let px = x + dx;
                    if px >= width {
                        continue;
                    }
                    let src = (py * width + px) * 4;
                    let r = i32::from(argb[src + 1]);
                    let g = i32::from(argb[src + 2]);
                    let b = i32::from(argb[src + 3]);

                    let yy = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
                    let uu = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
                    let vv = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;

                    out[y_row + px] = yy.clamp(0, 255) as u8;
                    u_acc += uu;
                    v_acc += vv;
                    sample_count += 1;
                }
            }

            let dst = uv_row + x;
            let denom = sample_count.max(1);
            out[dst] = (u_acc / denom).clamp(0, 255) as u8;
            if x + 1 < pitch {
                out[dst + 1] = (v_acc / denom).clamp(0, 255) as u8;
            }
        }
    }

    Ok((pitch, out))
}

fn encode_frame_into_backend_frame(frame: EncodeFrame) -> Result<Frame, BackendError> {
    let EncodeFrame {
        dims,
        pts_90k,
        buffer,
        force_keyframe,
    } = frame;
    let width = dims.width.get() as usize;
    let height = dims.height.get() as usize;
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    let (argb, nv12) = match buffer {
        RawFrameBuffer::Argb8888(data) => (Some(data), None),
        RawFrameBuffer::Argb8888Shared(data) => (Some(data.to_vec()), None),
        RawFrameBuffer::Nv12 { pitch, data } => {
            let payload = Nv12FramePayload { pitch, data };
            validate_nv12_payload(&payload, width, height)?;
            (None, Some(payload))
        }
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    match buffer {
        RawFrameBuffer::Argb8888(_) | RawFrameBuffer::Argb8888Shared(_) => {}
        RawFrameBuffer::Nv12 { .. } => {
            return Err(BackendError::InvalidInput(
                "RawFrameBuffer::Nv12 is not supported on this target".to_string(),
            ));
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let _ = force_keyframe;
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let nv12 = None;
    Ok(Frame {
        width,
        height,
        pixel_format: None,
        pts_90k: pts_90k.map(|v| v.0),
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        argb,
        nv12,
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        force_keyframe,
    })
}

#[cfg(any(
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
fn backend_packet_to_encoded_chunk(kind: BackendKind, packet: EncodedPacket) -> EncodedChunk {
    let layout = match (kind, packet.codec) {
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        (BackendKind::VideoToolbox, Codec::H264) => EncodedLayout::Avcc,
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        (BackendKind::VideoToolbox, Codec::Hevc) => EncodedLayout::Hvcc,
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        (BackendKind::VideoToolbox, Codec::Av1) => EncodedLayout::Av1,
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        (BackendKind::Nvidia, Codec::Av1) => EncodedLayout::Av1,
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        (BackendKind::Nvidia, _) => EncodedLayout::AnnexB,
        #[cfg(all(
            feature = "backend-intel",
            any(target_os = "linux", target_os = "windows")
        ))]
        (BackendKind::Intel, Codec::Av1) => EncodedLayout::Av1,
        #[cfg(all(
            feature = "backend-intel",
            any(target_os = "linux", target_os = "windows")
        ))]
        (BackendKind::Intel, _) => EncodedLayout::AnnexB,
        #[cfg(all(
            feature = "backend-vulkan",
            any(target_os = "linux", target_os = "windows")
        ))]
        (BackendKind::Vulkan, _) => EncodedLayout::AnnexB,
    };
    EncodedChunk {
        codec: packet.codec,
        layout,
        data: packet.data,
        pts_90k: packet.pts_90k.map(Timestamp90k),
        is_keyframe: packet.is_keyframe,
    }
}

#[cfg(not(any(
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
fn backend_packet_to_encoded_chunk(kind: BackendKind, _packet: EncodedPacket) -> EncodedChunk {
    match kind {}
}

fn dimensions_from_backend_frame(width: usize, height: usize) -> Option<Dimensions> {
    let width = u32::try_from(width)
        .ok()
        .and_then(std::num::NonZeroU32::new)?;
    let height = u32::try_from(height)
        .ok()
        .and_then(std::num::NonZeroU32::new)?;
    Some(Dimensions { width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(
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
    #[test]
    fn backend_default_resolves_to_os_default() {
        assert_eq!(BackendKind::default(), BackendKind::os_default());
    }

    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    #[test]
    fn backend_explicit_nvidia_resolution_returns_concrete_kind() {
        let decode = Backend::Nvidia
            .resolve_decoder(&DecoderConfig::new(Codec::H264, 30, false))
            .expect("nvidia explicit backend resolution should succeed");
        assert_eq!(decode, BackendKind::Nvidia);

        let encode = Backend::Nvidia
            .resolve_encoder(&EncoderConfig::new(
                Codec::H264,
                30,
                false,
                EncodeInputFormat::Argb8888,
            ))
            .expect("nvidia explicit backend resolution should succeed");
        assert_eq!(encode, BackendKind::Nvidia);
    }

    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    #[test]
    fn static_nvidia_backend_sessions_report_backend_kind() {
        assert_eq!(
            DecodeSession::<NvDecoderAdapter>::backend_kind_static(),
            BackendKind::Nvidia
        );
        assert_eq!(
            EncodeSession::<NvEncoderAdapter>::backend_kind_static(),
            BackendKind::Nvidia
        );

        let decode_session =
            DecodeSession::<NvDecoderAdapter>::new(DecoderConfig::new(Codec::H264, 30, false));
        assert!(
            decode_session
                .query_capability(Codec::H264)
                .map(|cap| cap.contract.decode.supported)
                .unwrap_or(false)
        );

        let encode_session = EncodeSession::<NvEncoderAdapter>::new(EncoderConfig::new(
            Codec::H264,
            30,
            false,
            EncodeInputFormat::Argb8888,
        ));
        assert_eq!(encode_session.backend_kind(), BackendKind::Nvidia);
    }

    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    #[test]
    fn strict_session_switch_api_is_available_for_nvidia_static_session() {
        let mut session = EncodeSession::<NvEncoderAdapter>::new(EncoderConfig::new(
            Codec::H264,
            30,
            false,
            EncodeInputFormat::Argb8888,
        ));
        let _ = session.request_session_switch_strict(SessionSwitchRequest::Nvidia {
            config: NvidiaSessionConfig {
                gop_length: Some(60),
                frame_interval_p: Some(1),
                force_idr_on_activate: true,
            },
            mode: SessionSwitchMode::Immediate,
        });
    }

    #[cfg(all(
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ))]
    #[test]
    fn static_intel_backend_sessions_report_backend_kind() {
        assert_eq!(
            DecodeSession::<IntelDecoderAdapter>::backend_kind_static(),
            BackendKind::Intel
        );
        assert_eq!(
            EncodeSession::<IntelEncoderAdapter>::backend_kind_static(),
            BackendKind::Intel
        );

        let decode_session =
            DecodeSession::<IntelDecoderAdapter>::new(DecoderConfig::new(Codec::Hevc, 30, false));
        assert!(
            decode_session
                .query_capability(Codec::Hevc)
                .map(|cap| cap.contract.decode.supported)
                .unwrap_or(false)
        );

        let encode_session = EncodeSession::<IntelEncoderAdapter>::new(EncoderConfig::new(
            Codec::Hevc,
            30,
            false,
            EncodeInputFormat::Argb8888,
        ));
        assert_eq!(encode_session.backend_kind(), BackendKind::Intel);
    }

    #[cfg(all(
        feature = "backend-vulkan",
        any(target_os = "linux", target_os = "windows")
    ))]
    #[test]
    fn static_vulkan_backend_sessions_report_backend_kind() {
        assert_eq!(
            DecodeSession::<VulkanDecoderAdapter>::backend_kind_static(),
            BackendKind::Vulkan
        );
        assert_eq!(
            EncodeSession::<VulkanEncoderAdapter>::backend_kind_static(),
            BackendKind::Vulkan
        );

        let decode_session =
            DecodeSession::<VulkanDecoderAdapter>::new(DecoderConfig::new(Codec::H264, 30, false));
        assert!(
            decode_session
                .query_capability(Codec::H264)
                .map(|cap| cap.contract.decode.supported)
                .unwrap_or(false)
        );

        let encode_session = EncodeSession::<VulkanEncoderAdapter>::new(EncoderConfig::new(
            Codec::H264,
            30,
            false,
            EncodeInputFormat::Argb8888,
        ));
        assert_eq!(encode_session.backend_kind(), BackendKind::Vulkan);
    }

    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    #[test]
    fn static_vt_backend_sessions_report_backend_kind() {
        assert_eq!(
            DecodeSession::<VtDecoderAdapter>::backend_kind_static(),
            BackendKind::VideoToolbox
        );
        assert_eq!(
            EncodeSession::<VtEncoderAdapter>::backend_kind_static(),
            BackendKind::VideoToolbox
        );
    }

    #[test]
    fn unpack_length_prefixed_sample_to_annexb_converts_nals() {
        let sample = [
            0, 0, 0, 2, 0x67, 0x64, //
            0, 0, 0, 3, 0x68, 0xEE, 0x3C,
        ];
        let annexb = bitstream::length_prefixed_to_annexb(
            bitstream::LengthPrefixedSampleRef::new(&sample),
            bitstream::NalLengthSize::Four,
        )
        .unwrap()
        .into_inner();
        assert_eq!(
            annexb,
            vec![
                0, 0, 0, 1, 0x67, 0x64, //
                0, 0, 0, 1, 0x68, 0xEE, 0x3C
            ]
        );
    }

    #[test]
    fn av1_length_prefixed_sample_is_forwarded_as_obu_payload() {
        let sample = vec![0x12, 0x00, 0x0a, 0x0b, 0x14, 0x00, 0x24];
        let (chunk, pts) = normalize_bitstream_input(BitstreamInput::LengthPrefixedSample {
            codec: Codec::Av1,
            sample: sample.clone(),
            pts_90k: Some(Timestamp90k(1234)),
        })
        .expect("AV1 MP4 sample should be forwarded as OBU payload");

        assert_eq!(chunk, sample);
        assert_eq!(pts, Some(1234));
    }

    #[test]
    fn decoder_config_default_output_mode_is_metadata() {
        let config = DecoderConfig::new(Codec::H264, 30, false);
        assert_eq!(config.output_mode, DecodeOutputMode::Metadata);
    }

    #[test]
    fn decode_nv12_mode_rejects_missing_pixel_payload() {
        let frame = Frame {
            width: 640,
            height: 360,
            pixel_format: None,
            pts_90k: Some(0),
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            argb: None,
            nv12: None,
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            force_keyframe: false,
        };

        let err = backend_frame_to_decoded_frame(frame, DecodeOutputMode::Nv12).unwrap_err();
        assert!(matches!(err, BackendError::UnsupportedConfig(_)));
    }

    #[test]
    fn decode_rgb24_mode_rejects_missing_pixel_payload() {
        let frame = Frame {
            width: 640,
            height: 360,
            pixel_format: None,
            pts_90k: Some(0),
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            argb: None,
            nv12: None,
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            force_keyframe: false,
        };

        let err = backend_frame_to_decoded_frame(frame, DecodeOutputMode::Rgb24).unwrap_err();
        assert!(matches!(err, BackendError::UnsupportedConfig(_)));
    }

    #[test]
    fn decode_nv12_mode_uses_native_nv12_payload() {
        let data = vec![16, 32, 48, 64, 128, 128];
        let frame = Frame {
            width: 2,
            height: 2,
            pixel_format: Some(u32::from_le_bytes(*b"NV12")),
            pts_90k: Some(9000),
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            argb: None,
            nv12: Some(Nv12FramePayload {
                pitch: 2,
                data: data.clone(),
            }),
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            force_keyframe: false,
        };

        let out = backend_frame_to_decoded_frame(frame, DecodeOutputMode::Nv12).unwrap();
        match out {
            DecodedFrame::Nv12 {
                pitch,
                data: out_data,
                ..
            } => {
                assert_eq!(pitch, 2);
                assert_eq!(out_data, data);
            }
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    #[test]
    fn decode_rgb24_mode_converts_native_nv12_payload() {
        let frame = Frame {
            width: 2,
            height: 2,
            pixel_format: Some(u32::from_le_bytes(*b"NV12")),
            pts_90k: Some(9000),
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            argb: None,
            nv12: Some(Nv12FramePayload {
                pitch: 2,
                data: vec![128; 6],
            }),
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            force_keyframe: false,
        };

        let out = backend_frame_to_decoded_frame(frame, DecodeOutputMode::Rgb24).unwrap();
        match out {
            DecodedFrame::Rgb24 { data, .. } => assert_eq!(data.len(), 2 * 2 * 3),
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    #[cfg(any(
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
    #[test]
    fn decode_rgb24_mode_converts_argb_payload() {
        let frame = Frame {
            width: 2,
            height: 1,
            pixel_format: None,
            pts_90k: Some(9000),
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            argb: Some(vec![255, 10, 20, 30, 255, 40, 50, 60]),
            nv12: None,
            force_keyframe: false,
        };

        let out = backend_frame_to_decoded_frame(frame, DecodeOutputMode::Rgb24).unwrap();
        match out {
            DecodedFrame::Rgb24 { data, .. } => assert_eq!(data, vec![10, 20, 30, 40, 50, 60]),
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    #[cfg(any(
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
    #[test]
    fn decode_nv12_mode_converts_argb_payload() {
        let frame = Frame {
            width: 2,
            height: 2,
            pixel_format: None,
            pts_90k: Some(9000),
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            argb: Some(vec![
                255, 10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120,
            ]),
            nv12: None,
            force_keyframe: false,
        };

        let out = backend_frame_to_decoded_frame(frame, DecodeOutputMode::Nv12).unwrap();
        match out {
            DecodedFrame::Nv12 { pitch, data, .. } => {
                assert_eq!(pitch, 2);
                assert_eq!(data.len(), 2 * 2 + (2 * 2 / 2));
            }
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    #[test]
    fn encoded_layout_is_inferred_from_backend_and_codec() {
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        {
            let vt_h264 = backend_packet_to_encoded_chunk(
                BackendKind::VideoToolbox,
                EncodedPacket {
                    codec: Codec::H264,
                    data: vec![1, 2, 3],
                    pts_90k: Some(9000),
                    is_keyframe: true,
                },
            );
            assert_eq!(vt_h264.layout, EncodedLayout::Avcc);

            let vt_hevc = backend_packet_to_encoded_chunk(
                BackendKind::VideoToolbox,
                EncodedPacket {
                    codec: Codec::Hevc,
                    data: vec![1, 2, 3],
                    pts_90k: None,
                    is_keyframe: false,
                },
            );
            assert_eq!(vt_hevc.layout, EncodedLayout::Hvcc);

            let vt_av1 = backend_packet_to_encoded_chunk(
                BackendKind::VideoToolbox,
                EncodedPacket {
                    codec: Codec::Av1,
                    data: vec![1, 2, 3],
                    pts_90k: None,
                    is_keyframe: false,
                },
            );
            assert_eq!(vt_av1.layout, EncodedLayout::Av1);
        }

        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        {
            let nv = backend_packet_to_encoded_chunk(
                BackendKind::Nvidia,
                EncodedPacket {
                    codec: Codec::H264,
                    data: vec![1],
                    pts_90k: None,
                    is_keyframe: false,
                },
            );
            assert_eq!(nv.layout, EncodedLayout::AnnexB);

            let nv_av1 = backend_packet_to_encoded_chunk(
                BackendKind::Nvidia,
                EncodedPacket {
                    codec: Codec::Av1,
                    data: vec![1],
                    pts_90k: None,
                    is_keyframe: false,
                },
            );
            assert_eq!(nv_av1.layout, EncodedLayout::Av1);
        }
        #[cfg(all(
            feature = "backend-intel",
            any(target_os = "linux", target_os = "windows")
        ))]
        {
            let intel = backend_packet_to_encoded_chunk(
                BackendKind::Intel,
                EncodedPacket {
                    codec: Codec::H264,
                    data: vec![1],
                    pts_90k: None,
                    is_keyframe: false,
                },
            );
            assert_eq!(intel.layout, EncodedLayout::AnnexB);

            let intel_av1 = backend_packet_to_encoded_chunk(
                BackendKind::Intel,
                EncodedPacket {
                    codec: Codec::Av1,
                    data: vec![1],
                    pts_90k: None,
                    is_keyframe: false,
                },
            );
            assert_eq!(intel_av1.layout, EncodedLayout::Av1);
        }
        #[cfg(all(
            feature = "backend-vulkan",
            any(target_os = "linux", target_os = "windows")
        ))]
        {
            let vulkan = backend_packet_to_encoded_chunk(
                BackendKind::Vulkan,
                EncodedPacket {
                    codec: Codec::H264,
                    data: vec![1],
                    pts_90k: None,
                    is_keyframe: false,
                },
            );
            assert_eq!(vulkan.layout, EncodedLayout::AnnexB);
        }
    }

    #[test]
    fn encode_frame_into_backend_frame_rejects_invalid_nv12_payload() {
        let dims = Dimensions {
            width: std::num::NonZeroU32::new(640).unwrap(),
            height: std::num::NonZeroU32::new(360).unwrap(),
        };
        let result = encode_frame_into_backend_frame(EncodeFrame {
            dims,
            pts_90k: Some(Timestamp90k(0)),
            buffer: RawFrameBuffer::Nv12 {
                pitch: 320,
                data: vec![0; 16],
            },
            force_keyframe: false,
        });
        assert!(matches!(result, Err(BackendError::InvalidInput(_))));
    }

    #[test]
    fn raw_frame_buffer_reports_input_format() {
        assert_eq!(
            RawFrameBuffer::Argb8888(vec![0; 4]).input_format(),
            EncodeInputFormat::Argb8888
        );
        assert_eq!(
            RawFrameBuffer::Nv12 {
                pitch: 2,
                data: vec![0; 6]
            }
            .input_format(),
            EncodeInputFormat::Nv12
        );
    }

    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    #[test]
    fn preflight_encode_rejects_nvidia_nv12_by_contract() {
        let report = preflight_encode(EncodePreflightRequest {
            backend: Backend::Nvidia,
            codec: Codec::H264,
            input_format: EncodeInputFormat::Nv12,
            require_hardware: true,
            expected_layout: Some(EncodedLayout::AnnexB),
        });
        assert_eq!(report.resolved_backend, Some(BackendKind::Nvidia));
        assert!(!report.supported_by_contract);
        assert!(!report.usable_in_current_runtime);
        assert_eq!(
            report.accepted_input_formats,
            vec![EncodeInputFormat::Argb8888]
        );
    }

    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    #[test]
    fn preflight_encode_rejects_vt_nv12_by_contract() {
        let report = preflight_encode(EncodePreflightRequest {
            backend: Backend::VideoToolbox,
            codec: Codec::H264,
            input_format: EncodeInputFormat::Nv12,
            require_hardware: false,
            expected_layout: Some(EncodedLayout::Avcc),
        });
        assert_eq!(report.resolved_backend, Some(BackendKind::VideoToolbox));
        assert!(!report.supported_by_contract);
        assert!(!report.usable_in_current_runtime);
        assert_eq!(
            report.accepted_input_formats,
            vec![EncodeInputFormat::Argb8888]
        );
        assert_eq!(report.encoded_layouts, vec![EncodedLayout::Avcc]);
    }

    #[cfg(all(
        feature = "backend-nvidia",
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ))]
    #[test]
    fn auto_encode_skips_nvidia_for_nv12_input() {
        let config = EncoderConfig::new(Codec::H264, 30, true, EncodeInputFormat::Nv12);
        assert_eq!(
            Backend::Auto.resolve_encoder(&config).unwrap(),
            BackendKind::Intel
        );
    }

    #[cfg(any(
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
    #[test]
    fn decode_reap_timeout_waits_until_deadline_when_empty() {
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        let mut session =
            DecodeSession::<VtDecoderAdapter>::new(DecoderConfig::new(Codec::H264, 30, false));
        #[cfg(all(
            not(all(target_os = "macos", feature = "backend-vt")),
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        let mut session =
            DecodeSession::<NvDecoderAdapter>::new(DecoderConfig::new(Codec::H264, 30, false));
        #[cfg(all(
            not(all(target_os = "macos", feature = "backend-vt")),
            not(feature = "backend-nvidia"),
            feature = "backend-intel",
            any(target_os = "linux", target_os = "windows")
        ))]
        let mut session =
            DecodeSession::<IntelDecoderAdapter>::new(DecoderConfig::new(Codec::H264, 30, false));
        #[cfg(all(
            not(all(target_os = "macos", feature = "backend-vt")),
            not(feature = "backend-nvidia"),
            not(feature = "backend-intel"),
            feature = "backend-vulkan",
            any(target_os = "linux", target_os = "windows")
        ))]
        let mut session =
            DecodeSession::<VulkanDecoderAdapter>::new(DecoderConfig::new(Codec::H264, 30, false));
        let timeout = Duration::from_millis(8);
        let start = std::time::Instant::now();
        let out = session.reap_timeout(timeout).unwrap();
        let elapsed = start.elapsed();

        assert!(out.is_none());
        assert!(elapsed >= timeout);
    }

    #[cfg(any(
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
    #[test]
    fn encode_reap_timeout_waits_until_deadline_when_empty() {
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        let mut session = EncodeSession::<VtEncoderAdapter>::new(EncoderConfig::new(
            Codec::H264,
            30,
            false,
            EncodeInputFormat::Argb8888,
        ));
        #[cfg(all(
            not(all(target_os = "macos", feature = "backend-vt")),
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        let mut session = EncodeSession::<NvEncoderAdapter>::new(EncoderConfig::new(
            Codec::H264,
            30,
            false,
            EncodeInputFormat::Argb8888,
        ));
        #[cfg(all(
            not(all(target_os = "macos", feature = "backend-vt")),
            not(feature = "backend-nvidia"),
            feature = "backend-intel",
            any(target_os = "linux", target_os = "windows")
        ))]
        let mut session = EncodeSession::<IntelEncoderAdapter>::new(EncoderConfig::new(
            Codec::H264,
            30,
            false,
            EncodeInputFormat::Argb8888,
        ));
        #[cfg(all(
            not(all(target_os = "macos", feature = "backend-vt")),
            not(feature = "backend-nvidia"),
            not(feature = "backend-intel"),
            feature = "backend-vulkan",
            any(target_os = "linux", target_os = "windows")
        ))]
        let mut session = EncodeSession::<VulkanEncoderAdapter>::new(EncoderConfig::new(
            Codec::H264,
            30,
            false,
            EncodeInputFormat::Argb8888,
        ));
        let timeout = Duration::from_millis(8);
        let start = std::time::Instant::now();
        let out = session.reap_timeout(timeout).unwrap();
        let elapsed = start.elapsed();

        assert!(out.is_none());
        assert!(elapsed >= timeout);
    }
}
