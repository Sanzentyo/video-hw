use video_hw_core::{
    CapabilityContract, CapabilityReport, Codec, DecodeCapability, DecodeOutputCapability,
    DecodeOutputMode, DecodeOutputOrigin, DimensionConstraints, EncodeCapability,
    EncodeInputFormat, EncodedLayout, FallbackPolicy, RuntimeCapability, RuntimeStatus,
    StreamingMode,
};

#[must_use]
pub(crate) fn android_capability_report(codec: Codec, encode: bool) -> CapabilityReport {
    let runtime = runtime_capability(codec, encode);
    let codec_supported = matches!(codec, Codec::H264 | Codec::Hevc | Codec::Av1);
    let runtime_available = !matches!(runtime.status, RuntimeStatus::Unavailable);
    let decode_supported = codec_supported && !encode && runtime_available;
    let encode_supported = codec_supported && encode && runtime_available;
    let encoded_layouts = match codec {
        Codec::H264 | Codec::Hevc => vec![EncodedLayout::AnnexB],
        Codec::Av1 => vec![EncodedLayout::Av1],
    };

    CapabilityReport {
        codec,
        contract: CapabilityContract {
            decode: DecodeCapability {
                supported: decode_supported,
                output_modes: vec![DecodeOutputCapability {
                    mode: DecodeOutputMode::Metadata,
                    origin: DecodeOutputOrigin::MetadataOnly,
                }],
                streaming_mode: StreamingMode::PushReap,
                fallback_policy: FallbackPolicy::OsManaged,
                requires_side_data: true,
                dimension_constraints: DimensionConstraints {
                    min_width: 16,
                    min_height: 16,
                    width_alignment: 2,
                    height_alignment: 2,
                },
            },
            encode: EncodeCapability {
                supported: encode_supported,
                input_formats: vec![EncodeInputFormat::Nv12, EncodeInputFormat::Argb8888],
                encoded_layouts,
                streaming_mode: StreamingMode::PushReap,
                fallback_policy: FallbackPolicy::OsManaged,
                dimension_constraints: DimensionConstraints {
                    min_width: 16,
                    min_height: 16,
                    width_alignment: 2,
                    height_alignment: 2,
                },
            },
        },
        runtime,
    }
}

#[cfg(target_os = "android")]
fn runtime_capability(codec: Codec, encode: bool) -> RuntimeCapability {
    let mime = crate::codec::mime_for_codec(codec);
    let available = if encode {
        crate::ffi::codec::can_create_encoder(mime)
    } else {
        crate::ffi::codec::can_create_decoder(mime)
    };
    RuntimeCapability {
        status: if available {
            RuntimeStatus::Available
        } else {
            RuntimeStatus::Unavailable
        },
        hardware_acceleration: false,
        reason: (!available).then(|| format!("AMediaCodec could not create codec for {mime}")),
    }
}

#[cfg(not(target_os = "android"))]
fn runtime_capability(_codec: Codec, _encode: bool) -> RuntimeCapability {
    RuntimeCapability {
        status: RuntimeStatus::NotProbed,
        hardware_acceleration: false,
        reason: Some("Android MediaCodec runtime is only available on Android".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_decode_contract_advertises_metadata_only_with_side_data() {
        let report = android_capability_report(Codec::H264, false);

        assert!(report.supports_decode_output_mode(DecodeOutputMode::Metadata));
        assert!(!report.supports_decode_output_mode(DecodeOutputMode::Nv12));
        assert!(!report.supports_decode_output_mode(DecodeOutputMode::Rgb24));
        assert!(report.contract.decode.requires_side_data);
    }

    #[test]
    fn android_runtime_does_not_claim_guaranteed_hardware_acceleration() {
        let report = android_capability_report(Codec::H264, true);

        assert!(!report.runtime.hardware_acceleration);
    }
}
