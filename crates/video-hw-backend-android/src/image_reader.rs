//! Exact-timestamp Android ImageReader decoding and validated CPU plane views.

#[cfg(all(target_os = "android", not(target_arch = "aarch64")))]
compile_error!("ImageReader NDK FFI is verified only for aarch64 Android API 32+");

mod planes;
pub use planes::{CompactYuv, CropRect, PlaneView, YuvImageView};

#[cfg(target_os = "android")]
mod decoder;
#[cfg(target_os = "android")]
mod ffi;
#[cfg(target_os = "android")]
pub use decoder::{CloseOutcome, DecodedImageLease, DecoderConfig, ImageReaderDecoder};

/// Plane, timestamp, or platform decoder operation failed.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("ImageReader contract: {0}")]
    Contract(&'static str),
    #[error("NDK {operation} failed with status {status}")]
    Media {
        operation: &'static str,
        status: i32,
    },
    #[error("decoder has an outstanding image lease")]
    Borrowed,
    #[error("decoder observation deadline; recreate required")]
    Deadline,
}

/// Exact positive microseconds, representable as signed Android nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimestampMicros(std::num::NonZeroU64);
impl TimestampMicros {
    pub fn new(value: u64) -> Result<Self, DecodeError> {
        if value > i64::MAX.unsigned_abs() / 1000 {
            return Err(DecodeError::Contract("PTS exceeds signed nanoseconds"));
        }
        std::num::NonZeroU64::new(value)
            .map(Self)
            .ok_or(DecodeError::Contract("zero PTS"))
    }
    pub const fn get(self) -> u64 {
        self.0.get()
    }
    pub fn from_image_nanoseconds(value: i64) -> Result<Self, DecodeError> {
        if value <= 0 || value % 1000 != 0 {
            return Err(DecodeError::Contract(
                "image PTS is not positive exact microseconds",
            ));
        }
        Self::new(value.unsigned_abs() / 1000)
    }
}
