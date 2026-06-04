//! Capability skeleton for Android MediaCodec.

use video_hw_core::{
    BackendError, CapabilityContract, CapabilityReport, Codec, DecodeCapability,
    DecodeOutputCapability, DecodeOutputMode, DecodeOutputOrigin, DimensionConstraints,
    EncodeCapability, EncodeInputFormat, EncodedLayout, FallbackPolicy, RuntimeCapability,
    RuntimeStatus, StreamingMode,
};

#[derive(Debug, Clone)]
pub struct AndroidCodecReport {
    pub name: String,
    pub mime: String,
    pub is_encoder: bool,
    pub is_hardware_accelerated: Option<bool>,
    pub is_software_only: Option<bool>,
    pub is_vendor: Option<bool>,
}

pub fn android_codec_reports() -> Result<Vec<AndroidCodecReport>, BackendError> {
    // TODO:
    // - NDK-only MVP: probe create/configure per mime.
    // - jni-capabilities: enumerate MediaCodecList and MediaCodecInfo.
    Ok(Vec::new())
}

pub fn android_capability_report(codec: Codec, encoder: bool) -> Result<CapabilityReport, BackendError> {
    let mime_supported_by_contract = matches!(codec, Codec::H264 | Codec::Hevc | Codec::Av1);
    let runtime_available = probe_codec_available(codec, encoder);

    let encoded_layouts = match codec {
        Codec::H264 | Codec::Hevc => vec![EncodedLayout::AnnexB],
        Codec::Av1 => vec![EncodedLayout::Av1],
    };

    Ok(CapabilityReport {
        codec,
        contract: CapabilityContract {
            decode: DecodeCapability {
                supported: mime_supported_by_contract,
                output_modes: if mime_supported_by_contract {
                    vec![
                        DecodeOutputCapability {
                            mode: DecodeOutputMode::Metadata,
                            origin: DecodeOutputOrigin::MetadataOnly,
                        },
                        DecodeOutputCapability {
                            mode: DecodeOutputMode::Nv12,
                            origin: DecodeOutputOrigin::Native,
                        },
                        DecodeOutputCapability {
                            mode: DecodeOutputMode::Rgb24,
                            origin: DecodeOutputOrigin::ConvertedFromNv12,
                        },
                    ]
                } else {
                    Vec::new()
                },
                streaming_mode: StreamingMode::Streaming,
                fallback_policy: FallbackPolicy::BackendControlled,
                requires_side_data: true,
                dimension_constraints: DimensionConstraints::default(),
            },
            encode: EncodeCapability {
                supported: mime_supported_by_contract,
                input_formats: if mime_supported_by_contract {
                    vec![EncodeInputFormat::Nv12, EncodeInputFormat::Argb8888]
                } else {
                    Vec::new()
                },
                encoded_layouts: if mime_supported_by_contract { encoded_layouts } else { Vec::new() },
                streaming_mode: StreamingMode::Streaming,
                fallback_policy: FallbackPolicy::BackendControlled,
                dimension_constraints: DimensionConstraints::default(),
            },
        },
        runtime: RuntimeCapability {
            status: if runtime_available { RuntimeStatus::Available } else { RuntimeStatus::Unavailable },
            hardware_acceleration: runtime_available,
            reason: if runtime_available { None } else { Some("no Android MediaCodec implementation passed probe".to_string()) },
        },
    })
}

fn probe_codec_available(_codec: Codec, _encoder: bool) -> bool {
    // TODO:
    // - Create by MIME.
    // - Configure with minimal valid format.
    // - If require_hardware is set, use JNI MediaCodecInfo on API 29+.
    false
}
