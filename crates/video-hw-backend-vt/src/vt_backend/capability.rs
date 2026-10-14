//! VideoToolbox capability, codec, option, timing, and metrics configuration.

use super::*;

pub(super) fn vt_capability_report(codec: Codec) -> CapabilityReport {
    let encode_supported = codec != Codec::Av1;
    let encoded_layout = vt_encoded_layout(codec);
    CapabilityReport {
        codec,
        contract: CapabilityContract {
            decode: DecodeCapability {
                supported: true,
                output_modes: vec![
                    DecodeOutputCapability {
                        mode: DecodeOutputMode::Metadata,
                        origin: DecodeOutputOrigin::MetadataOnly,
                    },
                    DecodeOutputCapability {
                        mode: DecodeOutputMode::Nv12,
                        origin: DecodeOutputOrigin::ConvertedFromBgra,
                    },
                    DecodeOutputCapability {
                        mode: DecodeOutputMode::Rgb24,
                        origin: DecodeOutputOrigin::ConvertedFromBgra,
                    },
                ],
                streaming_mode: StreamingMode::PushReap,
                fallback_policy: FallbackPolicy::OsManaged,
                requires_side_data: codec == Codec::Av1,
                dimension_constraints: DimensionConstraints::default(),
            },
            encode: EncodeCapability {
                supported: encode_supported,
                input_formats: if encode_supported {
                    vec![EncodeInputFormat::Argb8888]
                } else {
                    Vec::new()
                },
                encoded_layouts: if encode_supported {
                    vec![encoded_layout]
                } else {
                    Vec::new()
                },
                streaming_mode: StreamingMode::FlushOnly,
                fallback_policy: FallbackPolicy::OsManaged,
                dimension_constraints: DimensionConstraints::default(),
            },
        },
        decode_runtime: RuntimeCapability {
            status: RuntimeStatus::NotProbed,
            hardware_acceleration: if codec == Codec::Av1 {
                VTDecompressionSession::is_hardware_decode_supported(to_cm_codec_type(codec))
            } else {
                true
            },
            reason: None,
        },
        encode_runtime: RuntimeCapability {
            status: if encode_supported {
                RuntimeStatus::NotProbed
            } else {
                RuntimeStatus::Unavailable
            },
            hardware_acceleration: encode_supported,
            reason: (!encode_supported)
                .then(|| "VideoToolbox AV1 encode is unsupported".to_string()),
        },
    }
}

pub(super) fn vt_encoded_layout(codec: Codec) -> EncodedLayout {
    match codec {
        Codec::H264 => EncodedLayout::Avcc,
        Codec::Hevc => EncodedLayout::Hvcc,
        Codec::Av1 => EncodedLayout::Av1,
    }
}

pub(super) fn to_cm_codec_type(codec: Codec) -> CMVideoCodecType {
    match codec {
        Codec::H264 => kCMVideoCodecType_H264,
        Codec::Hevc => kCMVideoCodecType_HEVC,
        Codec::Av1 => kCMVideoCodecType_AV1,
    }
}

pub(super) fn codec_label(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "h264",
        Codec::Hevc => "hevc",
        Codec::Av1 => "av1",
    }
}

pub(super) fn ensure_vt_codec_implemented(
    codec: Codec,
    operation: &str,
) -> Result<(), BackendError> {
    if codec == Codec::Av1 && operation == "encode" {
        return Err(BackendError::UnsupportedConfig(format!(
            "VideoToolbox AV1 {operation} is not implemented in video-hw yet"
        )));
    }
    Ok(())
}

pub(super) fn frame_encode_properties(force_keyframe: bool) -> CFDictionary<CFString, CFType> {
    if !force_keyframe {
        return empty_dictionary();
    }
    let mut props = CFMutableDictionary::<CFString, CFType>::new();
    let key = CFString::from(EncodeFrameOptionKey::ForceKeyFrame);
    props.add(&key, &CFBoolean::true_value().as_CFType());
    props.to_immutable()
}

pub(super) fn cm_time_from_90k(pts_90k: i64) -> CMTime {
    CMTime::make(pts_90k.max(0), 90_000)
}

pub(super) fn update_peak(peak: &AtomicUsize, value: usize) {
    let mut current = peak.load(Ordering::Relaxed);
    while value > current {
        match peak.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(latest) => current = latest,
        }
    }
}

pub(super) fn update_jitter_samples(
    jitter_samples: &mut SampleStats,
    last_pts_90k: &mut Option<i64>,
    current_pts_90k: Option<i64>,
    expected_frame_ms: f64,
) {
    let Some(current) = current_pts_90k else {
        return;
    };
    if let Some(previous) = *last_pts_90k {
        let delta_ms = (current.saturating_sub(previous) as f64) / 90.0;
        jitter_samples.push_value((delta_ms - expected_frame_ms).abs());
    }
    *last_pts_90k = Some(current);
}

pub(super) fn vt_decoder_options(config: &DecoderConfig) -> VtDecoderOptions {
    match &config.backend_options {
        BackendDecoderOptions::VideoToolbox(options) => options.clone(),
        BackendDecoderOptions::Default
        | BackendDecoderOptions::Nvidia(_)
        | BackendDecoderOptions::Intel(_)
        | BackendDecoderOptions::Vulkan(_)
        | BackendDecoderOptions::Android(_) => VtDecoderOptions::default(),
    }
}

pub(super) fn vt_encoder_options(backend_options: BackendEncoderOptions) -> VtEncoderOptions {
    match backend_options {
        BackendEncoderOptions::VideoToolbox(options) => options,
        BackendEncoderOptions::Default
        | BackendEncoderOptions::Nvidia(_)
        | BackendEncoderOptions::Intel(_)
        | BackendEncoderOptions::Vulkan(_)
        | BackendEncoderOptions::Android(_) => VtEncoderOptions::default(),
    }
}

pub(super) fn option_or_env_bool(option: Option<bool>, env_name: &str) -> bool {
    option.unwrap_or_else(|| {
        std::env::var(env_name)
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

pub(super) fn packed_access_units_bytes(access_units: &[AccessUnit]) -> usize {
    access_units
        .iter()
        .map(|au| {
            au.nalus
                .iter()
                .map(|nal| nal.len().saturating_add(4))
                .sum::<usize>()
        })
        .sum()
}
