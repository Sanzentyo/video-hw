use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, VtBackendError>;

#[derive(Debug, Error)]
pub enum VtBackendError {
    #[error("unsupported codec: {0}")]
    UnsupportedCodec(String),
    #[error("missing parameter set: {0}")]
    MissingParameterSet(&'static str),
    #[error("input stream has no decodable access unit")]
    EmptyAccessUnit,
    #[error("rtc parser error: {0}")]
    RtcParser(String),
    #[error("video toolbox error({context}): {status}")]
    VideoToolbox { context: &'static str, status: i32 },
    #[error("core media error({context}): {status}")]
    CoreMedia { context: &'static str, status: i32 },
    #[error("core video error({context}): {status}")]
    CoreVideo { context: &'static str, status: i32 },
    #[error("failed to read file: {path}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write file: {path}")]
    WriteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
