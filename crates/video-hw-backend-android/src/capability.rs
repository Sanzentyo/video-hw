use video_hw_core::{
    CapabilityContract, CapabilityReport, Codec, DecodeCapability, DecodeOutputCapability,
    DecodeOutputMode, DecodeOutputOrigin, DimensionConstraints, EncodeCapability,
    EncodeInputFormat, EncodedLayout, FallbackPolicy, RuntimeCapability, RuntimeStatus,
    StreamingMode,
};

#[must_use]
pub(crate) fn android_capability_report(codec: Codec) -> CapabilityReport {
    let decode_runtime = runtime_capability(codec, false);
    let encode_runtime = runtime_capability(codec, true);
    let codec_supported = matches!(codec, Codec::H264 | Codec::Hevc | Codec::Av1);
    let encoded_layouts = match codec {
        Codec::H264 | Codec::Hevc => vec![EncodedLayout::AnnexB],
        Codec::Av1 => vec![EncodedLayout::Av1],
    };

    CapabilityReport {
        codec,
        contract: CapabilityContract {
            decode: DecodeCapability {
                supported: codec_supported,
                output_modes: if codec_supported {
                    vec![DecodeOutputCapability {
                        mode: DecodeOutputMode::Metadata,
                        origin: DecodeOutputOrigin::MetadataOnly,
                    }]
                } else {
                    Vec::new()
                },
                streaming_mode: StreamingMode::PushReap,
                fallback_policy: FallbackPolicy::OsManaged,
                requires_side_data: codec_supported,
                dimension_constraints: DimensionConstraints {
                    min_width: 16,
                    min_height: 16,
                    width_alignment: 2,
                    height_alignment: 2,
                },
            },
            encode: EncodeCapability {
                supported: codec_supported,
                input_formats: if codec_supported {
                    vec![EncodeInputFormat::Nv12, EncodeInputFormat::Argb8888]
                } else {
                    Vec::new()
                },
                encoded_layouts: if codec_supported {
                    encoded_layouts
                } else {
                    Vec::new()
                },
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
        decode_runtime,
        encode_runtime,
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
        let report = android_capability_report(Codec::H264);

        assert!(report.supports_decode_output_mode(DecodeOutputMode::Metadata));
        assert!(!report.supports_decode_output_mode(DecodeOutputMode::Nv12));
        assert!(!report.supports_decode_output_mode(DecodeOutputMode::Rgb24));
        assert!(report.contract.decode.requires_side_data);
    }

    #[test]
    fn android_runtime_does_not_claim_guaranteed_hardware_acceleration() {
        let report = android_capability_report(Codec::H264);

        assert!(!report.decode_runtime.hardware_acceleration);
        assert!(!report.encode_runtime.hardware_acceleration);
    }

    #[test]
    fn capability_report_describes_both_directions() {
        let report = android_capability_report(Codec::H264);

        assert!(!report.contract.encode.input_formats.is_empty());
        assert!(!report.contract.encode.encoded_layouts.is_empty());
        assert!(!report.contract.decode.output_modes.is_empty());
        assert!(report.contract.decode.requires_side_data);
    }
}
