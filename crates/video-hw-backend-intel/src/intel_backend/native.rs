//! oneVPL codec/configuration conversion and native status adaptation.

use std::panic::{AssertUnwindSafe, catch_unwind};

use onevpl::constants::{ApiVersion, ImplementationType};

use super::*;

#[derive(Debug, Clone, Copy)]
pub(super) enum OneVplOperation {
    Decode,
    Encode,
}

pub(super) fn configured_onevpl_loader(
    codec: Codec,
    operation: OneVplOperation,
    use_hardware: bool,
) -> Result<onevpl::Loader, BackendError> {
    let mut loader = catch_onevpl_loader_initialization(onevpl::Loader::new)?;
    let implementation = if use_hardware {
        ImplementationType::HARDWARE
    } else {
        ImplementationType::SOFTWARE
    };
    loader
        .set_filter_property("mfxImplDescription.Impl", implementation, None)
        .map_err(|status| map_onevpl_status(status, "Loader::set implementation filter"))?;
    let (property, codec) = match operation {
        OneVplOperation::Decode => (
            "mfxImplDescription.mfxDecoderDescription.decoder.CodecID",
            to_onevpl_codec(codec),
        ),
        OneVplOperation::Encode => (
            "mfxImplDescription.mfxEncoderDescription.encoder.CodecID",
            to_onevpl_codec(codec),
        ),
    };
    loader
        .set_filter_property(property, codec, None)
        .map_err(|status| map_onevpl_status(status, "Loader::set codec filter"))?;
    loader
        .set_filter_property(
            "mfxImplDescription.ApiVersion.Version",
            ApiVersion::new(2, 2),
            None,
        )
        .map_err(|status| map_onevpl_status(status, "Loader::set API version filter"))?;
    Ok(loader)
}

pub(super) fn catch_onevpl_loader_initialization<T>(
    initialize: impl FnOnce() -> Result<T, MfxStatus>,
) -> Result<T, BackendError> {
    match catch_unwind(AssertUnwindSafe(initialize)) {
        Ok(result) => result.map_err(|status| map_onevpl_status(status, "Loader::new")),
        Err(payload) => {
            let reason = payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic payload".to_string());
            Err(BackendError::UnsupportedConfig(format!(
                "oneVPL loader initialization panicked: {reason}"
            )))
        }
    }
}

pub(super) fn to_onevpl_codec(codec: Codec) -> OneVplCodec {
    match codec {
        Codec::H264 => OneVplCodec::AVC,
        Codec::Hevc => OneVplCodec::HEVC,
        Codec::Av1 => OneVplCodec::AV1,
    }
}

pub(super) fn default_target_kbps(width: usize, height: usize, fps: u32) -> u16 {
    let pixels_per_second = width
        .saturating_mul(height)
        .saturating_mul(usize::try_from(fps).unwrap_or(30));
    let target = (pixels_per_second / 3_000).clamp(800, usize::from(u16::MAX));
    u16::try_from(target).unwrap_or(u16::MAX)
}

pub(super) fn parse_rate_control_method(raw: &str) -> Option<RateControlMethod> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "cbr" => Some(RateControlMethod::CBR),
        "vbr" => Some(RateControlMethod::VBR),
        "cqp" => Some(RateControlMethod::CQP),
        "avbr" => Some(RateControlMethod::AVBR),
        "icq" => Some(RateControlMethod::ICQ),
        "qvbr" => Some(RateControlMethod::QVBR),
        _ => None,
    }
}

pub(super) fn default_rate_control_method(codec: Codec) -> RateControlMethod {
    match codec {
        Codec::H264 => RateControlMethod::CBR,
        Codec::Hevc | Codec::Av1 => RateControlMethod::CQP,
    }
}

pub(super) fn env_bool(name: &str) -> Option<bool> {
    std::env::var(name).ok().map(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    })
}

pub(super) fn map_onevpl_status(status: MfxStatus, context: &str) -> BackendError {
    match status {
        MfxStatus::MoreData
        | MfxStatus::MoreBitstream
        | MfxStatus::DeviceBusy
        | MfxStatus::InExecution
        | MfxStatus::TaskBusy
        | MfxStatus::TaskWorking
        | MfxStatus::AllocTimeoutExpired => {
            BackendError::TemporaryBackpressure(format!("{context}: {status:?}"))
        }
        MfxStatus::DeviceLost | MfxStatus::GpuHang => {
            BackendError::DeviceLost(format!("{context}: {status:?}"))
        }
        MfxStatus::Unsupported
        | MfxStatus::NotInitialized
        | MfxStatus::NotFound
        | MfxStatus::NotImplemented
        | MfxStatus::IncompatibleVideoParam
        | MfxStatus::PartialAcceleration => {
            BackendError::UnsupportedConfig(format!("{context}: {status:?}"))
        }
        MfxStatus::InvalidVideoParam if context.starts_with("Session::encoder") => {
            BackendError::UnsupportedConfig(format!("{context}: {status:?}"))
        }
        MfxStatus::InvalidVideoParam
        | MfxStatus::InvalidHandle
        | MfxStatus::NotEnoughBuffer
        | MfxStatus::OutOfRange
        | MfxStatus::NullPtr => BackendError::InvalidInput(format!("{context}: {status:?}")),
        _ => BackendError::Backend(format!("{context}: {status:?}")),
    }
}
