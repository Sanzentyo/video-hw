//! Static Intel codec capabilities and encoded-layout contracts.

use std::sync::OnceLock;

use super::*;

pub(super) fn intel_capability_report(codec: Codec) -> CapabilityReport {
    let supported = matches!(codec, Codec::H264 | Codec::Hevc | Codec::Av1);
    let encoded_layout = intel_encoded_layout(codec);
    let (decode_runtime, encode_runtime) = intel_runtime_capabilities(codec);
    CapabilityReport {
        codec,
        contract: CapabilityContract {
            decode: DecodeCapability {
                supported,
                output_modes: if supported {
                    vec![
                        DecodeOutputCapability {
                            mode: DecodeOutputMode::Metadata,
                            origin: DecodeOutputOrigin::MetadataOnly,
                        },
                        DecodeOutputCapability {
                            mode: DecodeOutputMode::Nv12,
                            origin: DecodeOutputOrigin::ConvertedFromArgb,
                        },
                        DecodeOutputCapability {
                            mode: DecodeOutputMode::Rgb24,
                            origin: DecodeOutputOrigin::ConvertedFromArgb,
                        },
                    ]
                } else {
                    Vec::new()
                },
                streaming_mode: StreamingMode::FlushOnly,
                fallback_policy: FallbackPolicy::HardwareThenSoftware,
                requires_side_data: false,
                dimension_constraints: DimensionConstraints::default(),
            },
            encode: EncodeCapability {
                supported,
                input_formats: if supported {
                    vec![EncodeInputFormat::Argb8888, EncodeInputFormat::Nv12]
                } else {
                    Vec::new()
                },
                encoded_layouts: if supported {
                    vec![encoded_layout]
                } else {
                    Vec::new()
                },
                streaming_mode: StreamingMode::FlushOnly,
                fallback_policy: FallbackPolicy::HardwareThenSoftware,
                dimension_constraints: DimensionConstraints::default(),
            },
        },
        decode_runtime,
        encode_runtime,
    }
}

fn intel_runtime_capabilities(codec: Codec) -> (RuntimeCapability, RuntimeCapability) {
    static H264: OnceLock<(RuntimeCapability, RuntimeCapability)> = OnceLock::new();
    static HEVC: OnceLock<(RuntimeCapability, RuntimeCapability)> = OnceLock::new();
    static AV1: OnceLock<(RuntimeCapability, RuntimeCapability)> = OnceLock::new();
    let cache = match codec {
        Codec::H264 => &H264,
        Codec::Hevc => &HEVC,
        Codec::Av1 => &AV1,
    };
    cache
        .get_or_init(|| {
            (
                probe_onevpl_runtime(codec, false),
                probe_onevpl_runtime(codec, true),
            )
        })
        .clone()
}

fn probe_onevpl_runtime(codec: Codec, encode: bool) -> RuntimeCapability {
    let hardware = try_onevpl_session(codec, encode, true);
    if hardware.is_ok() {
        return RuntimeCapability {
            status: RuntimeStatus::Available,
            hardware_acceleration: true,
            reason: None,
        };
    }
    let software = try_onevpl_session(codec, encode, false);
    if software.is_ok() {
        return RuntimeCapability {
            status: RuntimeStatus::Available,
            hardware_acceleration: false,
            reason: hardware.err(),
        };
    }
    RuntimeCapability {
        status: RuntimeStatus::Unavailable,
        hardware_acceleration: false,
        reason: Some(format!(
            "oneVPL hardware probe failed: {}; software probe failed: {}",
            hardware.expect_err("hardware probe was checked above"),
            software.expect_err("software probe was checked above")
        )),
    }
}

fn try_onevpl_session(codec: Codec, encode: bool, hardware: bool) -> Result<(), String> {
    let operation = if encode {
        OneVplOperation::Encode
    } else {
        OneVplOperation::Decode
    };
    let mut loader =
        configured_onevpl_loader(codec, operation, hardware).map_err(|error| error.to_string())?;
    loader
        .new_session(0)
        .map(|_| ())
        .map_err(|status| format!("{status:?}"))
}

pub(super) fn intel_encoded_layout(codec: Codec) -> EncodedLayout {
    match codec {
        Codec::H264 | Codec::Hevc => EncodedLayout::AnnexB,
        Codec::Av1 => EncodedLayout::Av1,
    }
}
