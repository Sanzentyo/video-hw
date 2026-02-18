use backend_contract::{
    Codec, DecoderConfig, EncodedPacket, Frame, VideoDecoder, VideoEncoder,
};

pub use backend_contract::{BackendError, CapabilityReport, DecodeSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    VideoToolbox,
    Nvidia,
}

pub struct Decoder {
    inner: Box<dyn VideoDecoder>,
}

impl Decoder {
    pub fn new(kind: BackendKind, config: DecoderConfig) -> Self {
        let inner: Box<dyn VideoDecoder> = match kind {
            BackendKind::VideoToolbox => Box::new(vt_backend::VtDecoderAdapter::new(config)),
            BackendKind::Nvidia => Box::new(nvidia_backend::NvidiaDecoderAdapter::new()),
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
    inner: Box<dyn VideoEncoder>,
}

impl Encoder {
    pub fn new(kind: BackendKind, codec: Codec, fps: i32, require_hardware: bool) -> Self {
        let inner: Box<dyn VideoEncoder> = match kind {
            BackendKind::VideoToolbox => Box::new(vt_backend::VtEncoderAdapter::with_config(
                codec,
                fps,
                require_hardware,
            )),
            BackendKind::Nvidia => Box::new(nvidia_backend::NvidiaEncoderAdapter::new()),
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
