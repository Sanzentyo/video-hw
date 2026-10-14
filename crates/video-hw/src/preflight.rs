use crate::{
    Backend, BackendKind, Codec, DecodeOutputMode, DecoderConfig, EncodeInputFormat, EncodedLayout,
    EncoderConfig, backend_supports_output_mode, preflight_decoder_capability,
    preflight_encoder_capability,
};

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
            let usable =
                capability.decode_is_usable(request.require_hardware) && output_mode_supported;
            DecodePreflightReport {
                requested_backend: request.backend,
                resolved_backend: Some(kind),
                output_mode: request.output_mode,
                supported_by_contract,
                usable_in_current_runtime: usable,
                decode_supported: Some(capability.contract.decode.supported),
                hardware_acceleration: Some(capability.decode_runtime.hardware_acceleration),
                output_mode_supported: Some(output_mode_supported),
                reason: if usable {
                    None
                } else {
                    Some(format!(
                        "decode_supported={}, hardware_acceleration={}, output_mode_supported={}",
                        capability.contract.decode.supported,
                        capability.decode_runtime.hardware_acceleration,
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
            let runtime_usable = capability.encode_is_usable(request.require_hardware);
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
                        capability.encode_runtime.hardware_acceleration,
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
