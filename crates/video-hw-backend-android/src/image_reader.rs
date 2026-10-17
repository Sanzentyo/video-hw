//! Exact-timestamp Android ImageReader decoding, borrowed hardware buffers and CPU views.

#[cfg(all(target_os = "android", not(target_arch = "aarch64")))]
compile_error!("ImageReader NDK FFI is verified only for aarch64 Android API 32+");

mod planes;
pub use planes::{CompactYuv, CropRect, PlaneView, YuvImageView};

#[cfg(any(target_os = "android", test))]
mod hardware_buffer;
#[cfg(target_os = "android")]
pub use hardware_buffer::HardwareBufferDescriptor;

#[cfg(target_os = "android")]
mod decoder;
#[cfg(target_os = "android")]
mod ffi;
#[cfg(target_os = "android")]
pub use decoder::{
    CloseOutcome, DecodedImageLease, DecoderConfig, HardwareBufferBorrow, ImageReaderDecoder,
};

/// Explicit coding syntax for a named ImageReader decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageReaderCodec {
    /// H.264/AVC.
    H264,
    /// H.265/HEVC.
    Hevc,
}
impl ImageReaderCodec {
    /// Fixed NDK MIME corresponding to the selected coding syntax.
    pub const fn mime(self) -> &'static std::ffi::CStr {
        match self {
            Self::H264 => c"video/avc",
            Self::Hevc => c"video/hevc",
        }
    }
}

#[cfg(test)]
mod codec_tests {
    use super::ImageReaderCodec;
    #[test]
    fn explicit_codec_has_one_exact_ndk_mime() {
        assert_eq!(
            ImageReaderCodec::H264.mime().to_bytes_with_nul(),
            b"video/avc\0"
        );
        assert_eq!(
            ImageReaderCodec::Hevc.mime().to_bytes_with_nul(),
            b"video/hevc\0"
        );
        assert_ne!(ImageReaderCodec::H264, ImageReaderCodec::Hevc);
    }
}

/// Plane, timestamp, or platform decoder operation failed.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("ImageReader contract: {0}")]
    Contract(&'static str),
    #[error(
        "hardware buffer descriptor: extent={extent:?}, image={image_extent:?}, layers={layers}, format={format:#x}, usage={usage:#x}, stride={stride}"
    )]
    HardwareBufferDescriptor {
        extent: [u32; 2],
        image_extent: [u32; 2],
        layers: u32,
        format: u32,
        usage: u64,
        stride: u32,
    },
    #[error("NDK {operation} failed with status {status}")]
    Media {
        operation: &'static str,
        status: i32,
    },
    #[error("{operation} failed: {source}")]
    System {
        operation: &'static str,
        #[source]
        source: std::io::Error,
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
