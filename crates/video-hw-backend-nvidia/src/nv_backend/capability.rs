//! NVIDIA codec contracts with direction-specific runtime capability.

use std::sync::OnceLock;

use super::*;

pub(super) fn nvidia_capability_report(codec: Codec) -> CapabilityReport {
    let supported = matches!(codec, Codec::H264 | Codec::Hevc | Codec::Av1);
    let encoded_layout = nvidia_encoded_layout(codec);
    let (decode_runtime, encode_runtime) = nvidia_runtime_capabilities(codec);
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
                streaming_mode: StreamingMode::PushReap,
                fallback_policy: FallbackPolicy::NoFallback,
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
                fallback_policy: FallbackPolicy::NoFallback,
                dimension_constraints: DimensionConstraints::default(),
            },
        },
        decode_runtime,
        encode_runtime,
    }
}

fn nvidia_runtime_capabilities(codec: Codec) -> (RuntimeCapability, RuntimeCapability) {
    static H264: OnceLock<(RuntimeCapability, RuntimeCapability)> = OnceLock::new();
    static HEVC: OnceLock<(RuntimeCapability, RuntimeCapability)> = OnceLock::new();
    static AV1: OnceLock<(RuntimeCapability, RuntimeCapability)> = OnceLock::new();
    let cache = match codec {
        Codec::H264 => &H264,
        Codec::Hevc => &HEVC,
        Codec::Av1 => &AV1,
    };
    cache
        .get_or_init(|| probe_nvidia_runtime_capabilities(codec))
        .clone()
}

fn probe_nvidia_runtime_capabilities(codec: Codec) -> (RuntimeCapability, RuntimeCapability) {
    let context = match CudaContext::new(0) {
        Ok(context) => context,
        Err(error) => {
            let reason = format!("failed to initialize CUDA context: {error}");
            return (
                unavailable_runtime(reason.clone()),
                unavailable_runtime(reason),
            );
        }
    };
    let decode_runtime = match NvMetaDecoder::new(
        Arc::clone(&context),
        to_decode_codec(codec),
        DecodeOutputMode::Metadata,
    ) {
        Ok(_) => available_runtime(),
        Err(error) => unavailable_runtime(error.to_string()),
    };
    let encode_runtime = match Encoder::initialize_with_cuda(context) {
        Ok(encoder) => match encoder.get_encode_guids() {
            Ok(guids) if guids.contains(&to_encode_guid(codec)) => available_runtime(),
            Ok(_) => unavailable_runtime(format!("NVENC does not advertise {codec} encode")),
            Err(error) => unavailable_runtime(format!("failed to query NVENC codecs: {error}")),
        },
        Err(error) => unavailable_runtime(format!("failed to initialize NVENC: {error}")),
    };
    (decode_runtime, encode_runtime)
}

fn available_runtime() -> RuntimeCapability {
    RuntimeCapability {
        status: RuntimeStatus::Available,
        hardware_acceleration: true,
        reason: None,
    }
}

fn unavailable_runtime(reason: String) -> RuntimeCapability {
    RuntimeCapability {
        status: RuntimeStatus::Unavailable,
        hardware_acceleration: false,
        reason: Some(reason),
    }
}
