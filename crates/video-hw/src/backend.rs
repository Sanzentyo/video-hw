use std::fmt;
use std::str::FromStr;

#[cfg(all(target_os = "android", feature = "backend-android"))]
use crate::{AndroidDecoderAdapter, AndroidEncoderAdapter};
use crate::{
    BackendError, CapabilityReport, DecodeOutputMode, DecoderConfig, EncoderConfig, VideoDecoder,
    VideoEncoder,
};
#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
use crate::{IntelDecoderAdapter, IntelEncoderAdapter};
#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
use crate::{NvDecoderAdapter, NvEncoderAdapter};
#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
use crate::{VulkanDecoderAdapter, VulkanEncoderAdapter};
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use video_hw_backend_vt as vt_backend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Android,
    VideoToolbox,
    Nvidia,
    Intel,
    Vulkan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    #[default]
    Auto,
    Android,
    VideoToolbox,
    Nvidia,
    Intel,
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

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Android => f.write_str("android"),
            Self::VideoToolbox => f.write_str("videotoolbox"),
            Self::Nvidia => f.write_str("nvidia"),
            Self::Intel => f.write_str("intel"),
            Self::Vulkan => f.write_str("vulkan"),
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Backend::Auto => f.write_str("auto"),
            Backend::Android => f.write_str("android"),
            Backend::VideoToolbox => f.write_str("videotoolbox"),
            Backend::Nvidia => f.write_str("nvidia"),
            Backend::Intel => f.write_str("intel"),
            Backend::Vulkan => f.write_str("vulkan"),
        }
    }
}

impl Backend {
    #[must_use]
    pub fn supported() -> Vec<Self> {
        [
            Some(Self::Auto),
            #[cfg(all(target_os = "android", feature = "backend-android"))]
            Some(Self::Android),
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
            "android" | "mediacodec" | "mc" => Ok(Self::Android),
            "vt" | "videotoolbox" => Ok(Self::VideoToolbox),
            "nvidia" | "nv" => Ok(Self::Nvidia),
            "intel" | "qsv" => Ok(Self::Intel),
            "vulkan" | "vk" => Ok(Self::Vulkan),
            _ => Err(BackendParseError::new(raw)),
        }
    }
}

impl BackendKind {
    #[must_use]
    pub const fn is_compiled(self) -> bool {
        match self {
            Self::Android => cfg!(all(target_os = "android", feature = "backend-android")),
            Self::VideoToolbox => cfg!(all(target_os = "macos", feature = "backend-vt")),
            Self::Nvidia => cfg!(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            )),
            Self::Intel => cfg!(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            )),
            Self::Vulkan => cfg!(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            )),
        }
    }

    #[must_use]
    pub fn os_default() -> Option<Self> {
        [
            Self::Android,
            Self::VideoToolbox,
            Self::Nvidia,
            Self::Intel,
            Self::Vulkan,
        ]
        .into_iter()
        .find(|kind| kind.is_compiled())
    }
}

impl Backend {
    pub fn resolve_decoder(self, config: &DecoderConfig) -> Result<BackendKind, BackendError> {
        match self {
            Backend::Auto => select_decoder_backend(config),
            Backend::Android => resolve_explicit_backend(BackendKind::Android),
            Backend::VideoToolbox => resolve_explicit_backend(BackendKind::VideoToolbox),
            Backend::Nvidia => resolve_explicit_backend(BackendKind::Nvidia),
            Backend::Intel => resolve_explicit_backend(BackendKind::Intel),
            Backend::Vulkan => resolve_explicit_backend(BackendKind::Vulkan),
        }
    }

    pub fn resolve_encoder(self, config: &EncoderConfig) -> Result<BackendKind, BackendError> {
        match self {
            Backend::Auto => select_encoder_backend(config),
            Backend::Android => resolve_explicit_backend(BackendKind::Android),
            Backend::VideoToolbox => resolve_explicit_backend(BackendKind::VideoToolbox),
            Backend::Nvidia => resolve_explicit_backend(BackendKind::Nvidia),
            Backend::Intel => resolve_explicit_backend(BackendKind::Intel),
            Backend::Vulkan => resolve_explicit_backend(BackendKind::Vulkan),
        }
    }
}

pub(super) fn resolve_explicit_backend(kind: BackendKind) -> Result<BackendKind, BackendError> {
    kind.is_compiled().then_some(kind).ok_or_else(|| {
        BackendError::UnsupportedConfig(format!("{kind} backend is not compiled for this target"))
    })
}

#[cfg(all(target_os = "android", feature = "backend-android"))]
fn preferred_backend_order() -> Vec<BackendKind> {
    vec![BackendKind::Android]
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
fn probe_decoder_capability(
    kind: BackendKind,
    config: &DecoderConfig,
) -> Result<CapabilityReport, BackendError> {
    match kind {
        #[cfg(all(target_os = "android", feature = "backend-android"))]
        BackendKind::Android => {
            let probe =
                <AndroidDecoderAdapter as DecoderBackend>::from_decoder_config(config.clone());
            probe.query_capability(config.codec)
        }
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
        kind => Err(BackendError::UnsupportedConfig(format!(
            "{kind} decoder backend is not compiled for this target"
        ))),
    }
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
fn probe_encoder_capability(
    kind: BackendKind,
    config: &EncoderConfig,
) -> Result<CapabilityReport, BackendError> {
    match kind {
        #[cfg(all(target_os = "android", feature = "backend-android"))]
        BackendKind::Android => {
            let probe =
                <AndroidEncoderAdapter as EncoderBackend>::from_encoder_config(config.clone());
            probe.query_capability(config.codec)
        }
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
        kind => Err(BackendError::UnsupportedConfig(format!(
            "{kind} encoder backend is not compiled for this target"
        ))),
    }
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
pub fn select_decoder_backend(config: &DecoderConfig) -> Result<BackendKind, BackendError> {
    let mut diagnostics = Vec::new();
    for candidate in preferred_backend_order() {
        match probe_decoder_capability(candidate, config) {
            Ok(capability) => {
                if capability.decode_is_usable(config.require_hardware)
                    && capability.supports_decode_output_mode(config.output_mode)
                {
                    return Ok(candidate);
                }
                diagnostics.push(format!(
                    "{candidate:?}: decode_supported={}, runtime={:?}, hw_accel={}, output_mode_supported={}",
                    capability.contract.decode.supported,
                    capability.decode_runtime.status,
                    capability.decode_runtime.hardware_acceleration,
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
pub fn select_decoder_backend(_config: &DecoderConfig) -> Result<BackendKind, BackendError> {
    Err(BackendError::UnsupportedConfig(
        "no backend feature enabled".to_string(),
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
pub fn select_encoder_backend(config: &EncoderConfig) -> Result<BackendKind, BackendError> {
    let mut diagnostics = Vec::new();
    for candidate in preferred_backend_order() {
        match probe_encoder_capability(candidate, config) {
            Ok(capability) => {
                if capability.encode_is_usable(config.require_hardware)
                    && capability.supports_encode_input_format(config.input_format)
                {
                    return Ok(candidate);
                }
                diagnostics.push(format!(
                    "{candidate:?}: encode_supported={}, runtime={:?}, hw_accel={}, input_format_supported={}",
                    capability.contract.encode.supported,
                    capability.encode_runtime.status,
                    capability.encode_runtime.hardware_acceleration,
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

pub(super) fn backend_supports_output_mode(kind: BackendKind, mode: DecodeOutputMode) -> bool {
    let _ = mode;
    match kind {
        #[cfg(all(target_os = "android", feature = "backend-android"))]
        BackendKind::Android => {
            <AndroidDecoderAdapter as DecoderBackend>::supports_output_mode(mode)
        }
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
        _ => false,
    }
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
pub(super) fn preflight_decoder_capability(
    kind: BackendKind,
    config: &DecoderConfig,
) -> Result<CapabilityReport, BackendError> {
    probe_decoder_capability(kind, config)
}

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
pub(super) fn preflight_decoder_capability(
    _kind: BackendKind,
    _config: &DecoderConfig,
) -> Result<CapabilityReport, BackendError> {
    Err(BackendError::UnsupportedConfig(
        "no backend feature enabled".to_string(),
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
pub(super) fn preflight_encoder_capability(
    kind: BackendKind,
    config: &EncoderConfig,
) -> Result<CapabilityReport, BackendError> {
    probe_encoder_capability(kind, config)
}

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
pub(super) fn preflight_encoder_capability(
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
