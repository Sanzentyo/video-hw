use std::fmt::{self, Display};

use crate::{Codec, DecodeOutputMode, EncodeInputFormat};

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
        }
    }
}
