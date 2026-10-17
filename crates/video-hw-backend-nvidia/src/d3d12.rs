//! External D3D12 encoding on the producer's device, without CUDA initialization.
//!
//! The SDK owns registration, mapping, codec completion, and packet storage.
//! The application owns the producer queue and obeys the external image lease's
//! synchronization contract. CPU image conversion/readback is not performed.

pub use nvidia_video_codec_sdk::d3d12::{
    Codec, D3d12Encoder as ExternalD3d12Encoder, EncodeError, EncodedAccessUnit,
    EncoderCapabilities, EncoderConfig, FenceValue, H264Config, HevcLosslessConfig,
    TimestampMicros,
};
