use backend_contract::{
    Codec, DecoderConfig, EncodedPacket, Frame, VideoDecoder, VideoEncoder,
};
use std::fmt;

pub use backend_contract::{BackendError, CapabilityReport, DecodeSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    VideoToolbox,
    Nvidia,
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VideoToolbox => f.write_str("videotoolbox"),
            Self::Nvidia => f.write_str("nvidia"),
        }
    }
}

enum DecoderInner {
    VideoToolbox(vt_backend::VtDecoderAdapter),
    Nvidia(nvidia_backend::NvidiaDecoderAdapter),
}

impl VideoDecoder for DecoderInner {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        match self {
            Self::VideoToolbox(inner) => inner.query_capability(codec),
            Self::Nvidia(inner) => inner.query_capability(codec),
        }
    }

    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        match self {
            Self::VideoToolbox(inner) => inner.push_bitstream_chunk(chunk, pts_90k),
            Self::Nvidia(inner) => inner.push_bitstream_chunk(chunk, pts_90k),
        }
    }

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        match self {
            Self::VideoToolbox(inner) => inner.flush(),
            Self::Nvidia(inner) => inner.flush(),
        }
    }

    fn decode_summary(&self) -> DecodeSummary {
        match self {
            Self::VideoToolbox(inner) => inner.decode_summary(),
            Self::Nvidia(inner) => inner.decode_summary(),
        }
    }
}

enum EncoderInner {
    VideoToolbox(vt_backend::VtEncoderAdapter),
    Nvidia(nvidia_backend::NvidiaEncoderAdapter),
}

impl VideoEncoder for EncoderInner {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        match self {
            Self::VideoToolbox(inner) => inner.query_capability(codec),
            Self::Nvidia(inner) => inner.query_capability(codec),
        }
    }

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        match self {
            Self::VideoToolbox(inner) => inner.push_frame(frame),
            Self::Nvidia(inner) => inner.push_frame(frame),
        }
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        match self {
            Self::VideoToolbox(inner) => inner.flush(),
            Self::Nvidia(inner) => inner.flush(),
        }
    }
}

pub struct Decoder {
    inner: DecoderInner,
}

impl Decoder {
    pub fn new(kind: BackendKind, config: DecoderConfig) -> Self {
        let inner = match kind {
            BackendKind::VideoToolbox => {
                DecoderInner::VideoToolbox(vt_backend::VtDecoderAdapter::new(config))
            }
            BackendKind::Nvidia => DecoderInner::Nvidia(nvidia_backend::NvidiaDecoderAdapter::new()),
        };
        Self { inner }
    }

    pub fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        self.inner.push_bitstream_chunk(chunk, pts_90k)
    }

    pub fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        self.inner.flush()
    }

    pub fn decode_summary(&self) -> DecodeSummary {
        self.inner.decode_summary()
    }

    pub fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        self.inner.query_capability(codec)
    }
}

pub struct Encoder {
    inner: EncoderInner,
}

impl Encoder {
    pub fn new(kind: BackendKind, codec: Codec, fps: i32, require_hardware: bool) -> Self {
        let inner = match kind {
            BackendKind::VideoToolbox => EncoderInner::VideoToolbox(vt_backend::VtEncoderAdapter::with_config(
                codec,
                fps,
                require_hardware,
            )),
            BackendKind::Nvidia => EncoderInner::Nvidia(nvidia_backend::NvidiaEncoderAdapter::new()),
        };
        Self { inner }
    }

    pub fn push_frame(
        &mut self,
        frame: Frame,
    ) -> Result<Vec<EncodedPacket>, BackendError> {
        self.inner.push_frame(frame)
    }

    pub fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        self.inner.flush()
    }

    pub fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        self.inner.query_capability(codec)
    }
}
