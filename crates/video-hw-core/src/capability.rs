use std::fmt::{self, Display};

use crate::{Codec, DecodeOutputMode, EncodeInputFormat, EncodedContentKind, EncodedLayout};

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
pub struct EncodedPacket {
    pub codec: Codec,
    pub layout: EncodedLayout,
    pub kind: EncodedContentKind,
    pub data: Vec<u8>,
    pub pts_90k: Option<i64>,
    pub is_keyframe: bool,
}

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
    pub decode_runtime: RuntimeCapability,
    pub encode_runtime: RuntimeCapability,
}

impl CapabilityReport {
    #[must_use]
    pub fn supports_decode_output_mode(&self, mode: DecodeOutputMode) -> bool {
        self.contract.decode.supported
            && self
                .contract
                .decode
                .output_modes
                .iter()
                .any(|capability| capability.mode == mode)
    }

    #[must_use]
    pub fn supports_encode_input_format(&self, format: EncodeInputFormat) -> bool {
        self.contract.encode.supported && self.contract.encode.input_formats.contains(&format)
    }

    #[must_use]
    pub fn supports_encoded_layout(&self, layout: EncodedLayout) -> bool {
        self.contract.encode.supported && self.contract.encode.encoded_layouts.contains(&layout)
    }

    #[must_use]
    pub fn decode_is_usable(&self, require_hardware: bool) -> bool {
        self.contract.decode.supported
            && !matches!(self.decode_runtime.status, RuntimeStatus::Unavailable)
            && (!require_hardware || self.decode_runtime.hardware_acceleration)
    }

    #[must_use]
    pub fn encode_is_usable(&self, require_hardware: bool) -> bool {
        self.contract.encode.supported
            && !matches!(self.encode_runtime.status, RuntimeStatus::Unavailable)
            && (!require_hardware || self.encode_runtime.hardware_acceleration)
    }
}

impl Display for CapabilityReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CapabilityReport(codec={}, decode_supported={}, encode_supported={}, decode_hw_accel={}, decode_runtime={:?}, encode_hw_accel={}, encode_runtime={:?}, decode_outputs={:?}, encode_inputs={:?}, encoded_layouts={:?})",
            self.codec,
            self.contract.decode.supported,
            self.contract.encode.supported,
            self.decode_runtime.hardware_acceleration,
            self.decode_runtime.status,
            self.encode_runtime.hardware_acceleration,
            self.encode_runtime.status,
            self.contract.decode.output_modes,
            self.contract.encode.input_formats,
            self.contract.encode.encoded_layouts
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_and_encode_runtime_availability_are_independent() {
        let report = CapabilityReport {
            codec: Codec::H264,
            contract: CapabilityContract {
                decode: DecodeCapability {
                    supported: true,
                    output_modes: vec![DecodeOutputCapability {
                        mode: DecodeOutputMode::Metadata,
                        origin: DecodeOutputOrigin::MetadataOnly,
                    }],
                    streaming_mode: StreamingMode::PushReap,
                    fallback_policy: FallbackPolicy::NoFallback,
                    requires_side_data: false,
                    dimension_constraints: DimensionConstraints::default(),
                },
                encode: EncodeCapability {
                    supported: true,
                    input_formats: vec![EncodeInputFormat::Argb8888],
                    encoded_layouts: vec![EncodedLayout::AnnexB],
                    streaming_mode: StreamingMode::PushReap,
                    fallback_policy: FallbackPolicy::NoFallback,
                    dimension_constraints: DimensionConstraints::default(),
                },
            },
            decode_runtime: RuntimeCapability {
                status: RuntimeStatus::Available,
                hardware_acceleration: true,
                reason: None,
            },
            encode_runtime: RuntimeCapability {
                status: RuntimeStatus::Unavailable,
                hardware_acceleration: false,
                reason: Some("encode queue unavailable".to_string()),
            },
        };

        assert!(report.decode_is_usable(true));
        assert!(!report.encode_is_usable(false));
    }

    #[test]
    fn not_probed_runtime_can_be_attempted_but_hardware_requirement_is_enforced() {
        let mut report = CapabilityReport {
            codec: Codec::Av1,
            contract: CapabilityContract {
                decode: DecodeCapability {
                    supported: true,
                    output_modes: Vec::new(),
                    streaming_mode: StreamingMode::FlushOnly,
                    fallback_policy: FallbackPolicy::OsManaged,
                    requires_side_data: false,
                    dimension_constraints: DimensionConstraints::default(),
                },
                encode: EncodeCapability {
                    supported: false,
                    input_formats: Vec::new(),
                    encoded_layouts: Vec::new(),
                    streaming_mode: StreamingMode::FlushOnly,
                    fallback_policy: FallbackPolicy::NoFallback,
                    dimension_constraints: DimensionConstraints::default(),
                },
            },
            decode_runtime: RuntimeCapability {
                status: RuntimeStatus::NotProbed,
                hardware_acceleration: false,
                reason: None,
            },
            encode_runtime: RuntimeCapability {
                status: RuntimeStatus::Unavailable,
                hardware_acceleration: false,
                reason: None,
            },
        };

        assert!(report.decode_is_usable(false));
        assert!(!report.decode_is_usable(true));
        report.decode_runtime.hardware_acceleration = true;
        assert!(report.decode_is_usable(true));
    }
}
