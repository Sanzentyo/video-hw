use crate::{Codec, bitstream};

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("unsupported codec: {0:?}")]
    UnsupportedCodec(Codec),
    #[error("unsupported config: {0}")]
    UnsupportedConfig(String),
    #[error("invalid bitstream: {0}")]
    InvalidBitstream(String),
    #[error(transparent)]
    Bitstream(#[from] bitstream::BitstreamError),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("temporary backpressure: {0}")]
    TemporaryBackpressure(String),
    #[error("device lost: {0}")]
    DeviceLost(String),
    #[error("backend error: {0}")]
    Backend(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendErrorKind {
    UnsupportedCodec,
    UnsupportedConfig,
    InvalidBitstream,
    InvalidInput,
    TemporaryBackpressure,
    DeviceLost,
    Backend,
}

impl BackendError {
    #[must_use]
    pub fn kind(&self) -> BackendErrorKind {
        match self {
            Self::UnsupportedCodec(_) => BackendErrorKind::UnsupportedCodec,
            Self::UnsupportedConfig(_) => BackendErrorKind::UnsupportedConfig,
            Self::InvalidBitstream(_) | Self::Bitstream(_) => BackendErrorKind::InvalidBitstream,
            Self::InvalidInput(_) => BackendErrorKind::InvalidInput,
            Self::TemporaryBackpressure(_) => BackendErrorKind::TemporaryBackpressure,
            Self::DeviceLost(_) => BackendErrorKind::DeviceLost,
            Self::Backend(_) => BackendErrorKind::Backend,
        }
    }

    #[must_use]
    pub fn is_runtime_unavailable(&self) -> bool {
        matches!(
            self.kind(),
            BackendErrorKind::UnsupportedConfig | BackendErrorKind::DeviceLost
        )
    }
}
