mod adapter;
mod backend;
mod preflight;
mod session;

use adapter::{
    backend_frame_to_decoded_frame, backend_packet_to_encoded_chunk,
    encode_frame_into_backend_frame, normalize_bitstream_input,
};
use backend::{
    backend_supports_output_mode, preflight_decoder_capability, preflight_encoder_capability,
    resolve_explicit_backend,
};

pub use backend::{
    Backend, BackendKind, BackendParseError, DecoderBackend, EncoderBackend,
    SessionSwitchingEncoderBackend, select_decoder_backend, select_encoder_backend,
};
pub use preflight::{
    DecodePreflightReport, DecodePreflightRequest, EncodePreflightReport, EncodePreflightRequest,
    preflight_decode, preflight_encode,
};
pub use session::{AnyDecodeSession, AnyEncodeSession, DecodeSession, EncodeSession};

#[cfg(all(target_os = "android", feature = "backend-android"))]
pub use video_hw_backend_android::{
    AndroidDecoderAdapter, AndroidEncoderAdapter, AndroidSurfaceDecoder,
    AndroidSurfaceDecoderConfig, AndroidSurfaceEncoder, AndroidSurfaceEncoderConfig,
};
#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
pub use video_hw_backend_intel::{IntelDecoderAdapter, IntelEncoderAdapter};
#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
pub use video_hw_backend_nvidia::{NvDecoderAdapter, NvEncoderAdapter};
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
pub use video_hw_backend_vt::{VtDecoderAdapter, VtEncoderAdapter};
#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
pub use video_hw_backend_vulkan::{VulkanAdapterReport, vulkan_adapter_reports};
#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
pub use video_hw_backend_vulkan::{VulkanDecoderAdapter, VulkanEncoderAdapter};
pub use video_hw_core::Nv12FramePayload;
pub use video_hw_core::bitstream;
pub use video_hw_core::{
    AndroidDecoderOptions, AndroidEncoderOptions, BackendDecoderOptions, BackendEncoderOptions,
    BackendError, BackendErrorKind, BitstreamInput, CapabilityContract, CapabilityReport, Codec,
    ColorConvertOptions, ColorMatrix, ColorMetadata, ColorRange, DecodeCapability,
    DecodeOutputCapability, DecodeOutputMode, DecodeOutputOrigin, DecodeSummary, DecodedFrame,
    DecoderConfig, DimensionConstraints, Dimensions, EncodeCapability, EncodeFrame,
    EncodeInputFormat, EncodedChunk, EncodedContentKind, EncodedLayout, EncoderConfig,
    FallbackPolicy, IntelDecoderOptions, IntelEncoderOptions, InvalidDimensions, Nv12FrameRef,
    NvidiaDecoderOptions, NvidiaEncoderOptions, NvidiaSessionConfig, PitchBytes, PixelBufferOwned,
    PixelOutputLayout, RawFrameBuffer, Rgb24FrameRef, RuntimeCapability, RuntimeStatus,
    SessionSwitchMode, SessionSwitchRequest, StreamingMode, Timestamp90k, VtDecoderOptions,
    VtEncoderOptions, VtSessionConfig, VulkanDecoderOptions, VulkanEncoderOptions,
};
pub(crate) use video_hw_core::{EncodedPacket, Frame, VideoDecoder, VideoEncoder};

#[cfg(test)]
mod tests;
#[cfg(all(windows, feature = "backend-nvidia-d3d12"))]
pub use video_hw_backend_nvidia::d3d12;

#[cfg(feature = "backend-android-imagereader")]
pub use video_hw_backend_android::image_reader;
