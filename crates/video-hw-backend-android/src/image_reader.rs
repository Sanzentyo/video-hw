//! Exact-timestamp Android ImageReader decoding, owned/borrowed GPU leases and CPU views.
//!
//! Decoder configuration requires an explicit `video_hw_core::ColorRange`. Every
//! output-format change must report BT.709 and that range before an image can be
//! published. Missing or contradictory metadata fails decoding. Acquired images
//! and hardware-buffer borrows expose the validated immutable `DecodedColorimetry`,
//! independently of any hardware-buffer sampler-conversion suggestions.
//!
//! `DecoderConfig::new` also requires `ImageLeaseCapacity::One` or `Two`.
//! A capacity unit is reserved before queueing an access unit and stays occupied
//! until its acquired image is returned. Two supports keeping one image while
//! decoding the next on the same thread. It does not permit concurrent decoder
//! calls, increase the process's two-decoder limit, or relax exact increasing PTS.
//!
//! Use `DecodedImageLease::hardware_buffer` for a scoped GPU borrow, or consume
//! the image with `into_hardware_buffer` for a movable `HardwareBufferLease`.
//! Both share metadata and CPU-access validation. The owned capability exposes
//! no CPU mapping; all external GPU use must finish before it is dropped.
//! `try_close` reports `Borrowed` while either kind of image owner remains live.

#[cfg(all(target_os = "android", not(target_arch = "aarch64")))]
compile_error!("ImageReader NDK FFI is verified only for aarch64 Android API 32+");

mod planes;
pub use planes::{CompactYuv, CropRect, PlaneView, YuvImageView};
mod color;
pub use color::DecodedColorimetry;

/// Software admission bound per image axis. A codec's actual capabilities and
/// the available bytes of every decoded plane remain independent constraints.
pub const MAX_IMAGE_DIMENSION: u32 = 8192;

#[cfg(any(target_os = "android", test))]
mod hardware_buffer;
#[cfg(any(target_os = "android", test))]
pub use hardware_buffer::HardwareBufferDescriptor;

#[cfg(any(target_os = "android", test))]
mod decoder;
#[cfg(any(target_os = "android", test))]
mod ffi;
#[cfg(any(target_os = "android", test))]
pub use decoder::{
    CloseOutcome, DecodedImageLease, DecoderConfig, HardwareBufferBorrow, HardwareBufferLease,
    ImageReaderDecoder,
};

/// Maximum acquired or acquiring images held by one decoder's consumer.
///
/// Two allows decoding the next image while the preceding image is still held.
/// It does not make the decoder concurrent or change the reader's internal pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageLeaseCapacity {
    One,
    Two,
}
#[cfg(any(target_os = "android", test))]
impl ImageLeaseCapacity {
    const fn count(self) -> u8 {
        match self {
            Self::One => 1,
            Self::Two => 2,
        }
    }
}

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

// MediaFormat accepts integer FPS. Reject zero and overflow before codec allocation.
#[cfg(any(target_os = "android", test))]
fn decoder_operating_rate(fps: u32) -> Result<i32, DecodeError> {
    if fps == 0 {
        return Err(DecodeError::Contract(
            "decoder operating rate must be positive",
        ));
    }
    i32::try_from(fps)
        .map_err(|_| DecodeError::Contract("decoder operating rate exceeds MediaFormat integer"))
}

#[cfg(test)]
mod codec_tests {
    use super::{ImageReaderCodec, decoder_operating_rate};

    #[test]
    fn requested_operating_rate_preserves_integer_fps() {
        assert_eq!(decoder_operating_rate(15).unwrap(), 15);
        assert_eq!(decoder_operating_rate(30).unwrap(), 30);
        assert_eq!(decoder_operating_rate(i32::MAX as u32).unwrap(), i32::MAX);
    }

    #[test]
    fn invalid_operating_rate_is_rejected_before_codec_allocation() {
        assert!(decoder_operating_rate(0).is_err());
        assert!(decoder_operating_rate(i32::MAX as u32 + 1).is_err());
        assert!(decoder_operating_rate(u32::MAX).is_err());
    }
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
    #[error(
        "output colorimetry differs: standard={standard:?}, range={range:?}, expected BT709 {expected_range:?}"
    )]
    OutputColorimetry {
        standard: Option<i32>,
        range: Option<i32>,
        expected_range: video_hw_core::ColorRange,
    },
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
    #[error("decoder image lease capacity is occupied")]
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
