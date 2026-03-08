use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, Instant};

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
mod backend_transform_adapter;
#[cfg(any(
    test,
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
mod bitstream;
mod contract;
#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
mod nv_backend;
#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
mod nv_meta_decoder;
mod pipeline;
#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
mod pipeline_scheduler;
mod transform;

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
mod vt_backend;

pub use contract::{
    BackendDecoderOptions, BackendEncoderOptions, BackendError, BackendErrorKind, BitstreamInput,
    CapabilityReport, Codec, ColorMetadata, DecodeOutputMode, DecodeSummary, DecodedFrame, DecoderConfig,
    Dimensions, EncodeFrame, EncodedChunk, EncodedLayout, EncoderConfig, NvidiaDecoderOptions,
    NvidiaEncoderOptions, NvidiaSessionConfig, RawFrameBuffer, SessionSwitchMode,
    SessionSwitchRequest, Timestamp90k, VtSessionConfig,
};
pub(crate) use contract::{EncodedPacket, Frame, VideoDecoder, VideoEncoder};
pub use pipeline::{
    BoundedQueueRx, BoundedQueueTx, InFlightCredits, QueueRecvError, QueueSendError, QueueStats,
    bounded_queue,
};
pub use transform::{
    ColorRequest, Nv12Frame, RgbFrame, TransformDispatcher, TransformJob, TransformResult,
    make_argb_to_nv12_dummy, nv12_to_rgb24, should_enqueue_transform,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        )
    ),
    derive(Default)
)]
pub enum BackendKind {
    #[cfg(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        )
    ))]
    #[cfg_attr(
        any(
            all(target_os = "macos", feature = "backend-vt"),
            all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            )
        ),
        default
    )]
    Auto,
    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    VideoToolbox,
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    Nvidia,
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(any(
                all(target_os = "macos", feature = "backend-vt"),
                all(
                    feature = "backend-nvidia",
                    any(target_os = "linux", target_os = "windows")
                )
            ))]
            Self::Auto => f.write_str("auto"),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox => f.write_str("videotoolbox"),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia => f.write_str("nvidia"),
        }
    }
}

#[cfg(not(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
)))]
impl fmt::Display for BackendKind {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {}
    }
}

pub type Backend = BackendKind;

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
enum DecoderInner {
    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    VideoToolbox(vt_backend::VtDecoderAdapter),
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    Nvidia(Box<nv_backend::NvDecoderAdapter>),
    Unsupported(UnsupportedDecoderAdapter),
}

#[cfg(not(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
)))]
enum DecoderInner {
    NoBackend,
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
impl VideoDecoder for DecoderInner {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        match self {
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(inner) => inner.query_capability(codec),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(inner) => inner.query_capability(codec),
            Self::Unsupported(inner) => inner.query_capability(codec),
        }
    }

    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        match self {
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(inner) => inner.push_bitstream_chunk(chunk, pts_90k),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(inner) => inner.push_bitstream_chunk(chunk, pts_90k),
            Self::Unsupported(inner) => inner.push_bitstream_chunk(chunk, pts_90k),
        }
    }

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        match self {
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(inner) => inner.flush(),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(inner) => inner.flush(),
            Self::Unsupported(inner) => inner.flush(),
        }
    }

    fn try_reap(&mut self) -> Result<Vec<Frame>, BackendError> {
        match self {
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(inner) => inner.try_reap(),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(inner) => inner.try_reap(),
            Self::Unsupported(inner) => inner.try_reap(),
        }
    }

    fn decode_summary(&self) -> DecodeSummary {
        match self {
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(inner) => inner.decode_summary(),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(inner) => inner.decode_summary(),
            Self::Unsupported(inner) => inner.decode_summary(),
        }
    }
}

#[cfg(not(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
)))]
impl VideoDecoder for DecoderInner {
    fn query_capability(&self, _codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(CapabilityReport {
            codec: _codec,
            decode_supported: false,
            encode_supported: false,
            hardware_acceleration: false,
        })
    }

    fn push_bitstream_chunk(
        &mut self,
        _chunk: &[u8],
        _pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        Err(BackendError::UnsupportedConfig(
            "no backend feature enabled".to_string(),
        ))
    }

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        Err(BackendError::UnsupportedConfig(
            "no backend feature enabled".to_string(),
        ))
    }

    fn try_reap(&mut self) -> Result<Vec<Frame>, BackendError> {
        Ok(Vec::new())
    }

    fn decode_summary(&self) -> DecodeSummary {
        DecodeSummary {
            decoded_frames: 0,
            width: None,
            height: None,
            pixel_format: None,
        }
    }
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
enum EncoderInner {
    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    VideoToolbox(vt_backend::VtEncoderAdapter),
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    Nvidia(Box<nv_backend::NvEncoderAdapter>),
    Unsupported(UnsupportedEncoderAdapter),
}

#[cfg(not(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
)))]
enum EncoderInner {
    NoBackend,
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
impl VideoEncoder for EncoderInner {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        match self {
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(inner) => inner.query_capability(codec),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(inner) => inner.query_capability(codec),
            Self::Unsupported(inner) => inner.query_capability(codec),
        }
    }

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        match self {
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(inner) => inner.push_frame(frame),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(inner) => inner.push_frame(frame),
            Self::Unsupported(inner) => inner.push_frame(frame),
        }
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        match self {
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(inner) => inner.flush(),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(inner) => inner.flush(),
            Self::Unsupported(inner) => inner.flush(),
        }
    }

    fn try_reap(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        match self {
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(inner) => inner.try_reap(),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(inner) => inner.try_reap(),
            Self::Unsupported(inner) => inner.try_reap(),
        }
    }

    fn request_session_switch(
        &mut self,
        request: SessionSwitchRequest,
    ) -> Result<(), BackendError> {
        match self {
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(inner) => inner.request_session_switch(request),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(inner) => inner.request_session_switch(request),
            Self::Unsupported(inner) => inner.request_session_switch(request),
        }
    }
}

#[cfg(not(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
)))]
impl VideoEncoder for EncoderInner {
    fn query_capability(&self, _codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(CapabilityReport {
            codec: _codec,
            decode_supported: false,
            encode_supported: false,
            hardware_acceleration: false,
        })
    }

    fn push_frame(&mut self, _frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        Err(BackendError::UnsupportedConfig(
            "no backend feature enabled".to_string(),
        ))
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        Err(BackendError::UnsupportedConfig(
            "no backend feature enabled".to_string(),
        ))
    }

    fn try_reap(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        Ok(Vec::new())
    }

    fn request_session_switch(
        &mut self,
        _request: SessionSwitchRequest,
    ) -> Result<(), BackendError> {
        Err(BackendError::UnsupportedConfig(
            "no backend feature enabled".to_string(),
        ))
    }
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
impl BackendKind {
    #[must_use]
    pub fn os_default() -> Self {
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        {
            BackendKind::VideoToolbox
        }
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        {
            BackendKind::Nvidia
        }
    }
}

pub struct DecodeSession {
    decoder_inner: DecoderInner,
    output_mode: DecodeOutputMode,
    ready: VecDeque<DecodedFrame>,
}

impl DecodeSession {
    pub fn new(backend: Backend, config: DecoderConfig) -> Self {
        let output_mode = config.output_mode;
        #[cfg(any(
            all(target_os = "macos", feature = "backend-vt"),
            all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            )
        ))]
        let decoder_inner: DecoderInner = match resolve_decoder_backend(backend, &config) {
            Ok(selected) => build_decoder_inner(selected, config),
            Err(err) => DecoderInner::Unsupported(UnsupportedDecoderAdapter::new(err.to_string())),
        };
        #[cfg(not(any(
            all(target_os = "macos", feature = "backend-vt"),
            all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            )
        )))]
        let decoder_inner = build_decoder_inner(backend, config);
        Self {
            decoder_inner,
            output_mode,
            ready: VecDeque::new(),
        }
    }

    pub fn submit(&mut self, input: BitstreamInput) -> Result<(), BackendError> {
        let (annexb, pts_90k) = match input {
            BitstreamInput::AnnexBChunk { chunk, pts_90k } => (chunk, pts_90k.map(|v| v.0)),
            BitstreamInput::AccessUnitRawNal {
                codec: _,
                nalus,
                pts_90k,
            } => (
                pack_access_unit_nalus_to_annexb(&nalus),
                pts_90k.map(|v| v.0),
            ),
            BitstreamInput::LengthPrefixedSample {
                codec: _,
                sample,
                pts_90k,
            } => (
                unpack_length_prefixed_sample_to_annexb(&sample)?,
                pts_90k.map(|v| v.0),
            ),
        };
        let outputs = self
            .decoder_inner
            .push_bitstream_chunk(&annexb, pts_90k)?
            .into_iter()
            .map(|frame| backend_frame_to_decoded_frame(frame, self.output_mode))
            .collect::<Result<Vec<_>, _>>()?;
        self.ready.extend(outputs);
        Ok(())
    }

    pub fn try_reap(&mut self) -> Result<Option<DecodedFrame>, BackendError> {
        if let Some(frame) = self.ready.pop_front() {
            return Ok(Some(frame));
        }
        let polled = self
            .decoder_inner
            .try_reap()?
            .into_iter()
            .map(|frame| backend_frame_to_decoded_frame(frame, self.output_mode))
            .collect::<Result<Vec<_>, _>>()?;
        self.ready.extend(polled);
        Ok(self.ready.pop_front())
    }

    pub fn reap_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<DecodedFrame>, BackendError> {
        if let Some(frame) = self.try_reap()? {
            return Ok(Some(frame));
        }
        if timeout.is_zero() {
            return Ok(None);
        }
        let Some(deadline) = Instant::now().checked_add(timeout) else {
            return Ok(None);
        };
        const POLL_INTERVAL: Duration = Duration::from_millis(1);
        loop {
            if let Some(frame) = self.try_reap()? {
                return Ok(Some(frame));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let remaining = deadline.saturating_duration_since(now);
            std::thread::sleep(remaining.min(POLL_INTERVAL));
        }
    }

    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>, BackendError> {
        let mut out = std::mem::take(&mut self.ready)
            .into_iter()
            .collect::<Vec<_>>();
        out.extend(
            self.decoder_inner
                .flush()?
                .into_iter()
                .map(|frame| backend_frame_to_decoded_frame(frame, self.output_mode))
                .collect::<Result<Vec<_>, _>>()?,
        );
        Ok(out)
    }

    pub fn summary(&self) -> DecodeSummary {
        self.decoder_inner.decode_summary()
    }

    pub fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        self.decoder_inner.query_capability(codec)
    }
}

pub struct EncodeSession {
    backend_kind: BackendKind,
    encoder_inner: EncoderInner,
    ready: VecDeque<EncodedChunk>,
}

impl EncodeSession {
    pub fn new(backend: Backend, config: EncoderConfig) -> Self {
        #[cfg(any(
            all(target_os = "macos", feature = "backend-vt"),
            all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            )
        ))]
        let (backend_kind, encoder_inner): (BackendKind, EncoderInner) =
            match resolve_encoder_backend(backend, &config) {
                Ok(selected) => (selected, build_encoder_inner(selected, config)),
                Err(err) => (
                    fallback_backend_kind(backend),
                    EncoderInner::Unsupported(UnsupportedEncoderAdapter::new(err.to_string())),
                ),
            };
        #[cfg(not(any(
            all(target_os = "macos", feature = "backend-vt"),
            all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            )
        )))]
        let (backend_kind, encoder_inner) = (backend, build_encoder_inner(backend, config));
        Self {
            backend_kind,
            encoder_inner,
            ready: VecDeque::new(),
        }
    }

    pub fn submit(&mut self, frame: EncodeFrame) -> Result<(), BackendError> {
        let backend_frame = encode_frame_into_backend_frame(frame)?;
        let outputs = self
            .encoder_inner
            .push_frame(backend_frame)?
            .into_iter()
            .map(|packet| backend_packet_to_encoded_chunk(self.backend_kind, packet))
            .collect::<Vec<_>>();
        self.ready.extend(outputs);
        Ok(())
    }

    pub fn try_reap(&mut self) -> Result<Option<EncodedChunk>, BackendError> {
        if let Some(chunk) = self.ready.pop_front() {
            return Ok(Some(chunk));
        }
        let polled = self
            .encoder_inner
            .try_reap()?
            .into_iter()
            .map(|packet| backend_packet_to_encoded_chunk(self.backend_kind, packet))
            .collect::<Vec<_>>();
        self.ready.extend(polled);
        Ok(self.ready.pop_front())
    }

    pub fn reap_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<EncodedChunk>, BackendError> {
        if let Some(chunk) = self.try_reap()? {
            return Ok(Some(chunk));
        }
        if timeout.is_zero() {
            return Ok(None);
        }
        let Some(deadline) = Instant::now().checked_add(timeout) else {
            return Ok(None);
        };
        const POLL_INTERVAL: Duration = Duration::from_millis(1);
        loop {
            if let Some(chunk) = self.try_reap()? {
                return Ok(Some(chunk));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let remaining = deadline.saturating_duration_since(now);
            std::thread::sleep(remaining.min(POLL_INTERVAL));
        }
    }

    pub fn flush(&mut self) -> Result<Vec<EncodedChunk>, BackendError> {
        let mut out = std::mem::take(&mut self.ready)
            .into_iter()
            .collect::<Vec<_>>();
        out.extend(
            self.encoder_inner
                .flush()?
                .into_iter()
                .map(|packet| backend_packet_to_encoded_chunk(self.backend_kind, packet))
                .collect::<Vec<_>>(),
        );
        Ok(out)
    }

    pub fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        self.encoder_inner.query_capability(codec)
    }

    pub fn request_session_switch(
        &mut self,
        request: SessionSwitchRequest,
    ) -> Result<(), BackendError> {
        self.encoder_inner.request_session_switch(request)
    }
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
struct UnsupportedDecoderAdapter {
    message: String,
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
impl UnsupportedDecoderAdapter {
    fn new(message: String) -> Self {
        Self { message }
    }
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
impl VideoDecoder for UnsupportedDecoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(CapabilityReport {
            codec,
            decode_supported: false,
            encode_supported: false,
            hardware_acceleration: false,
        })
    }

    fn push_bitstream_chunk(
        &mut self,
        _chunk: &[u8],
        _pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        Err(BackendError::UnsupportedConfig(self.message.clone()))
    }

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        Err(BackendError::UnsupportedConfig(self.message.clone()))
    }

    fn decode_summary(&self) -> DecodeSummary {
        DecodeSummary {
            decoded_frames: 0,
            width: None,
            height: None,
            pixel_format: None,
        }
    }
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
struct UnsupportedEncoderAdapter {
    message: String,
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
impl UnsupportedEncoderAdapter {
    fn new(message: String) -> Self {
        Self { message }
    }
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
impl VideoEncoder for UnsupportedEncoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        Ok(CapabilityReport {
            codec,
            decode_supported: false,
            encode_supported: false,
            hardware_acceleration: false,
        })
    }

    fn push_frame(&mut self, _frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        Err(BackendError::UnsupportedConfig(self.message.clone()))
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        Err(BackendError::UnsupportedConfig(self.message.clone()))
    }
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
fn fallback_backend_kind(requested: BackendKind) -> BackendKind {
    match requested {
        BackendKind::Auto => BackendKind::os_default(),
        concrete => concrete,
    }
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
fn preferred_backend_order() -> Vec<BackendKind> {
    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    {
        return vec![BackendKind::VideoToolbox];
    }
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    {
        vec![BackendKind::Nvidia]
    }
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
fn resolve_decoder_backend(
    requested: BackendKind,
    config: &DecoderConfig,
) -> Result<BackendKind, BackendError> {
    if requested != BackendKind::Auto {
        return Ok(requested);
    }
    let mut diagnostics = Vec::new();
    for candidate in preferred_backend_order() {
        let probe = build_decoder_inner(candidate, config.clone());
        match probe.query_capability(config.codec) {
            Ok(capability) => {
                if capability.decode_supported
                    && (!config.require_hardware || capability.hardware_acceleration)
                {
                    return Ok(candidate);
                }
                diagnostics.push(format!(
                    "{candidate:?}: decode_supported={}, hw_accel={}",
                    capability.decode_supported, capability.hardware_acceleration
                ));
            }
            Err(err) => diagnostics.push(format!("{candidate:?}: {err}")),
        }
    }
    let detail = if diagnostics.is_empty() {
        "no eligible backend candidate".to_string()
    } else {
        diagnostics.join("; ")
    };
    Err(BackendError::UnsupportedConfig(format!(
        "auto backend selection failed for decode ({:?}): {}",
        config.codec, detail
    )))
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
fn resolve_encoder_backend(
    requested: BackendKind,
    config: &EncoderConfig,
) -> Result<BackendKind, BackendError> {
    if requested != BackendKind::Auto {
        return Ok(requested);
    }
    let mut diagnostics = Vec::new();
    for candidate in preferred_backend_order() {
        let probe = build_encoder_inner(candidate, config.clone());
        match probe.query_capability(config.codec) {
            Ok(capability) => {
                if capability.encode_supported
                    && (!config.require_hardware || capability.hardware_acceleration)
                {
                    return Ok(candidate);
                }
                diagnostics.push(format!(
                    "{candidate:?}: encode_supported={}, hw_accel={}",
                    capability.encode_supported, capability.hardware_acceleration
                ));
            }
            Err(err) => diagnostics.push(format!("{candidate:?}: {err}")),
        }
    }
    let detail = if diagnostics.is_empty() {
        "no eligible backend candidate".to_string()
    } else {
        diagnostics.join("; ")
    };
    Err(BackendError::UnsupportedConfig(format!(
        "auto backend selection failed for encode ({:?}): {}",
        config.codec, detail
    )))
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
fn build_decoder_inner(kind: BackendKind, config: DecoderConfig) -> DecoderInner {
    match kind {
        BackendKind::Auto => build_decoder_inner(BackendKind::os_default(), config),
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        BackendKind::VideoToolbox => {
            DecoderInner::VideoToolbox(vt_backend::VtDecoderAdapter::new(config))
        }
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        BackendKind::Nvidia => {
            DecoderInner::Nvidia(Box::new(nv_backend::NvDecoderAdapter::new(config)))
        }
    }
}

#[cfg(not(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
)))]
fn build_decoder_inner(kind: BackendKind, _config: DecoderConfig) -> DecoderInner {
    let _ = kind;
    DecoderInner::NoBackend
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
fn build_encoder_inner(kind: BackendKind, config: EncoderConfig) -> EncoderInner {
    match kind {
        BackendKind::Auto => build_encoder_inner(BackendKind::os_default(), config),
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        BackendKind::VideoToolbox => {
            EncoderInner::VideoToolbox(vt_backend::VtEncoderAdapter::with_config(
                config.codec,
                config.fps,
                config.require_hardware,
            ))
        }
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        BackendKind::Nvidia => {
            EncoderInner::Nvidia(Box::new(nv_backend::NvEncoderAdapter::with_config(
                config.codec,
                config.fps,
                config.require_hardware,
                config.backend_options,
            )))
        }
    }
}

#[cfg(not(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
)))]
fn build_encoder_inner(kind: BackendKind, _config: EncoderConfig) -> EncoderInner {
    let _ = kind;
    EncoderInner::NoBackend
}

fn pack_access_unit_nalus_to_annexb(nalus: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in nalus {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    }
    out
}

fn unpack_length_prefixed_sample_to_annexb(sample: &[u8]) -> Result<Vec<u8>, BackendError> {
    let mut out = Vec::new();
    let mut payload = sample;
    while payload.len() >= 4 {
        let nal_len = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
        payload = &payload[4..];
        if nal_len == 0 || payload.len() < nal_len {
            return Err(BackendError::InvalidBitstream(
                "invalid length-prefixed sample payload".to_string(),
            ));
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&payload[..nal_len]);
        payload = &payload[nal_len..];
    }
    if !payload.is_empty() {
        return Err(BackendError::InvalidBitstream(
            "trailing bytes after length-prefixed sample parse".to_string(),
        ));
    }
    Ok(out)
}

fn backend_frame_to_decoded_frame(
    frame: Frame,
    mode: DecodeOutputMode,
) -> Result<DecodedFrame, BackendError> {
    match mode {
        DecodeOutputMode::Metadata => {}
        DecodeOutputMode::Nv12 => {
            let dims = dimensions_from_backend_frame(frame.width, frame.height).ok_or_else(|| {
                BackendError::InvalidInput(
                    "decoded frame dimensions are invalid for DecodeOutputMode::Nv12".to_string(),
                )
            })?;
            let argb = frame_argb_payload(&frame).ok_or_else(|| {
                BackendError::UnsupportedConfig(
                    "DecodeOutputMode::Nv12 requires backend ARGB payload".to_string(),
                )
            })?;
            let (pitch, data) = argb_to_nv12(argb, frame.width, frame.height)?;
            return Ok(DecodedFrame::Nv12 {
                dims,
                pitch,
                pts_90k: frame.pts_90k.map(Timestamp90k),
                data,
            });
        }
        DecodeOutputMode::Rgb24 => {
            let dims = dimensions_from_backend_frame(frame.width, frame.height).ok_or_else(|| {
                BackendError::InvalidInput(
                    "decoded frame dimensions are invalid for DecodeOutputMode::Rgb24"
                        .to_string(),
                )
            })?;
            let argb = frame_argb_payload(&frame).ok_or_else(|| {
                BackendError::UnsupportedConfig(
                    "DecodeOutputMode::Rgb24 requires backend ARGB payload".to_string(),
                )
            })?;
            let data = argb_to_rgb24(argb, frame.width, frame.height)?;
            return Ok(DecodedFrame::Rgb24 {
                dims,
                pts_90k: frame.pts_90k.map(Timestamp90k),
                data,
            });
        }
    }
    let dims = dimensions_from_backend_frame(frame.width, frame.height);
    let color = if frame.color_primaries.is_some()
        || frame.transfer_function.is_some()
        || frame.ycbcr_matrix.is_some()
    {
        Some(ColorMetadata {
            color_primaries: frame.color_primaries,
            transfer_function: frame.transfer_function,
            ycbcr_matrix: frame.ycbcr_matrix,
        })
    } else {
        None
    };
    Ok(DecodedFrame::Metadata {
        dims,
        pts_90k: frame.pts_90k.map(Timestamp90k),
        pixel_format: frame.pixel_format,
        decode_info_flags: frame.decode_info_flags,
        color,
    })
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
fn frame_argb_payload(frame: &Frame) -> Option<&[u8]> {
    frame.argb.as_deref()
}

#[cfg(not(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
)))]
fn frame_argb_payload(_frame: &Frame) -> Option<&[u8]> {
    None
}

fn argb_to_rgb24(argb: &[u8], width: usize, height: usize) -> Result<Vec<u8>, BackendError> {
    let expected = width
        .checked_mul(height)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| BackendError::InvalidInput("argb size overflow".to_string()))?;
    if argb.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "argb payload size mismatch: expected {}, got {}",
            expected,
            argb.len()
        )));
    }
    let mut out = Vec::with_capacity(width * height * 3);
    for px in argb.chunks_exact(4) {
        out.extend_from_slice(&[px[1], px[2], px[3]]);
    }
    Ok(out)
}

fn argb_to_nv12(argb: &[u8], width: usize, height: usize) -> Result<(usize, Vec<u8>), BackendError> {
    let expected = width
        .checked_mul(height)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| BackendError::InvalidInput("argb size overflow".to_string()))?;
    if argb.len() != expected {
        return Err(BackendError::InvalidInput(format!(
            "argb payload size mismatch: expected {}, got {}",
            expected,
            argb.len()
        )));
    }
    if width == 0 || height == 0 {
        return Err(BackendError::InvalidInput(
            "argb frame dimensions must be positive".to_string(),
        ));
    }
    let pitch = width;
    let y_size = pitch
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("nv12 luma size overflow".to_string()))?;
    let uv_size = y_size / 2;
    let mut out = vec![0_u8; y_size + uv_size];

    let mut y_plane = vec![0_u8; y_size];
    let mut u_plane = vec![0_u8; width * height];
    let mut v_plane = vec![0_u8; width * height];

    for y in 0..height {
        for x in 0..width {
            let src = (y * width + x) * 4;
            let r = argb[src + 1] as f32;
            let g = argb[src + 2] as f32;
            let b = argb[src + 3] as f32;

            let yy = (0.257 * r + 0.504 * g + 0.098 * b + 16.0).round() as i32;
            let uu = (-0.148 * r - 0.291 * g + 0.439 * b + 128.0).round() as i32;
            let vv = (0.439 * r - 0.368 * g - 0.071 * b + 128.0).round() as i32;

            y_plane[y * pitch + x] = yy.clamp(0, 255) as u8;
            u_plane[y * width + x] = uu.clamp(0, 255) as u8;
            v_plane[y * width + x] = vv.clamp(0, 255) as u8;
        }
    }

    out[..y_size].copy_from_slice(&y_plane);
    let uv_base = y_size;
    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            let idx = y * width + x;
            let idx1 = idx;
            let idx2 = idx + (x + 1 < width) as usize;
            let idx3 = idx + if y + 1 < height { width } else { 0 };
            let idx4 = idx3 + (x + 1 < width) as usize;

            let u_avg = ((u_plane[idx1] as u32
                + u_plane[idx2] as u32
                + u_plane[idx3] as u32
                + u_plane[idx4] as u32)
                / 4) as u8;
            let v_avg = ((v_plane[idx1] as u32
                + v_plane[idx2] as u32
                + v_plane[idx3] as u32
                + v_plane[idx4] as u32)
                / 4) as u8;

            let uv_row = (y / 2) * pitch;
            let uv_col = x & !1;
            let dst = uv_base + uv_row + uv_col;
            out[dst] = u_avg;
            if dst + 1 < out.len() {
                out[dst + 1] = v_avg;
            }
        }
    }

    Ok((pitch, out))
}

fn encode_frame_into_backend_frame(frame: EncodeFrame) -> Result<Frame, BackendError> {
    let EncodeFrame {
        dims,
        pts_90k,
        buffer,
        force_keyframe,
    } = frame;
    let width = dims.width.get() as usize;
    let height = dims.height.get() as usize;
    #[cfg(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        )
    ))]
    let argb = match buffer {
        RawFrameBuffer::Argb8888(data) => Some(data),
        RawFrameBuffer::Argb8888Shared(data) => Some(data.to_vec()),
        #[cfg(feature = "unstable-raw-inputs")]
        RawFrameBuffer::Nv12 { .. } => {
            return Err(BackendError::InvalidInput(
                "RawFrameBuffer::Nv12 is not supported by Encoder::push_encode_frame yet"
                    .to_string(),
            ));
        }
        #[cfg(feature = "unstable-raw-inputs")]
        RawFrameBuffer::Rgb24(_) => {
            return Err(BackendError::InvalidInput(
                "RawFrameBuffer::Rgb24 is not supported by Encoder::push_encode_frame yet"
                    .to_string(),
            ));
        }
    };
    #[cfg(not(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        )
    )))]
    match buffer {
        #[cfg(feature = "unstable-raw-inputs")]
        RawFrameBuffer::Nv12 { .. } => {
            return Err(BackendError::InvalidInput(
                "RawFrameBuffer::Nv12 is not supported by Encoder::push_encode_frame yet"
                    .to_string(),
            ));
        }
        #[cfg(feature = "unstable-raw-inputs")]
        RawFrameBuffer::Rgb24(_) => {
            return Err(BackendError::InvalidInput(
                "RawFrameBuffer::Rgb24 is not supported by Encoder::push_encode_frame yet"
                    .to_string(),
            ));
        }
        RawFrameBuffer::Argb8888(_) | RawFrameBuffer::Argb8888Shared(_) => {}
    }
    #[cfg(not(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        )
    )))]
    let _ = force_keyframe;
    Ok(Frame {
        width,
        height,
        pixel_format: None,
        pts_90k: pts_90k.map(|v| v.0),
        decode_info_flags: None,
        color_primaries: None,
        transfer_function: None,
        ycbcr_matrix: None,
        #[cfg(any(
            all(target_os = "macos", feature = "backend-vt"),
            all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            )
        ))]
        argb,
        #[cfg(any(
            all(target_os = "macos", feature = "backend-vt"),
            all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            )
        ))]
        force_keyframe,
    })
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
fn backend_packet_to_encoded_chunk(kind: BackendKind, packet: EncodedPacket) -> EncodedChunk {
    let layout = match (kind, packet.codec) {
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        (BackendKind::Auto, Codec::H264) => EncodedLayout::Avcc,
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        (BackendKind::Auto, Codec::Hevc) => EncodedLayout::Hvcc,
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        (BackendKind::VideoToolbox, Codec::H264) => EncodedLayout::Avcc,
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        (BackendKind::VideoToolbox, Codec::Hevc) => EncodedLayout::Hvcc,
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        (BackendKind::Nvidia, _) => EncodedLayout::AnnexB,
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        (BackendKind::Auto, _) => EncodedLayout::AnnexB,
    };
    EncodedChunk {
        codec: packet.codec,
        layout,
        data: packet.data,
        pts_90k: packet.pts_90k.map(Timestamp90k),
        is_keyframe: packet.is_keyframe,
    }
}

#[cfg(not(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
)))]
fn backend_packet_to_encoded_chunk(kind: BackendKind, _packet: EncodedPacket) -> EncodedChunk {
    match kind {}
}

fn dimensions_from_backend_frame(width: usize, height: usize) -> Option<Dimensions> {
    let width = u32::try_from(width)
        .ok()
        .and_then(std::num::NonZeroU32::new)?;
    let height = u32::try_from(height)
        .ok()
        .and_then(std::num::NonZeroU32::new)?;
    Some(Dimensions { width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        )
    ))]
    #[test]
    fn backend_default_is_auto() {
        assert_eq!(BackendKind::default(), BackendKind::Auto);
    }

    #[test]
    fn unpack_length_prefixed_sample_to_annexb_converts_nals() {
        let sample = [
            0, 0, 0, 2, 0x67, 0x64, //
            0, 0, 0, 3, 0x68, 0xEE, 0x3C,
        ];
        let annexb = unpack_length_prefixed_sample_to_annexb(&sample).unwrap();
        assert_eq!(
            annexb,
            vec![
                0, 0, 0, 1, 0x67, 0x64, //
                0, 0, 0, 1, 0x68, 0xEE, 0x3C
            ]
        );
    }

    #[test]
    fn decoder_config_default_output_mode_is_metadata() {
        let config = DecoderConfig::new(Codec::H264, 30, false);
        assert_eq!(config.output_mode, DecodeOutputMode::Metadata);
    }

    #[test]
    fn decode_non_metadata_mode_is_currently_unsupported() {
        let frame = Frame {
            width: 640,
            height: 360,
            pixel_format: None,
            pts_90k: Some(0),
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            #[cfg(any(
                all(target_os = "macos", feature = "backend-vt"),
                all(
                    feature = "backend-nvidia",
                    any(target_os = "linux", target_os = "windows")
                )
            ))]
            argb: None,
            #[cfg(any(
                all(target_os = "macos", feature = "backend-vt"),
                all(
                    feature = "backend-nvidia",
                    any(target_os = "linux", target_os = "windows")
                )
            ))]
            force_keyframe: false,
        };

        let err = backend_frame_to_decoded_frame(frame, DecodeOutputMode::Nv12).unwrap_err();
        assert!(matches!(err, BackendError::UnsupportedConfig(_)));
    }

    #[cfg(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        )
    ))]
    #[test]
    fn decode_rgb24_mode_converts_argb_payload() {
        let frame = Frame {
            width: 2,
            height: 1,
            pixel_format: None,
            pts_90k: Some(9000),
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            argb: Some(vec![255, 10, 20, 30, 255, 40, 50, 60]),
            force_keyframe: false,
        };

        let out = backend_frame_to_decoded_frame(frame, DecodeOutputMode::Rgb24).unwrap();
        match out {
            DecodedFrame::Rgb24 { data, .. } => assert_eq!(data, vec![10, 20, 30, 40, 50, 60]),
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    #[cfg(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        )
    ))]
    #[test]
    fn decode_nv12_mode_converts_argb_payload() {
        let frame = Frame {
            width: 2,
            height: 2,
            pixel_format: None,
            pts_90k: Some(9000),
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            argb: Some(vec![
                255, 10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120,
            ]),
            force_keyframe: false,
        };

        let out = backend_frame_to_decoded_frame(frame, DecodeOutputMode::Nv12).unwrap();
        match out {
            DecodedFrame::Nv12 { pitch, data, .. } => {
                assert_eq!(pitch, 2);
                assert_eq!(data.len(), 2 * 2 + (2 * 2 / 2));
            }
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    #[test]
    fn encoded_layout_is_inferred_from_backend_and_codec() {
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        {
            let vt_h264 = backend_packet_to_encoded_chunk(
                BackendKind::VideoToolbox,
                EncodedPacket {
                    codec: Codec::H264,
                    data: vec![1, 2, 3],
                    pts_90k: Some(9000),
                    is_keyframe: true,
                },
            );
            assert_eq!(vt_h264.layout, EncodedLayout::Avcc);

            let vt_hevc = backend_packet_to_encoded_chunk(
                BackendKind::VideoToolbox,
                EncodedPacket {
                    codec: Codec::Hevc,
                    data: vec![1, 2, 3],
                    pts_90k: None,
                    is_keyframe: false,
                },
            );
            assert_eq!(vt_hevc.layout, EncodedLayout::Hvcc);
        }

        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        {
            let nv = backend_packet_to_encoded_chunk(
                BackendKind::Nvidia,
                EncodedPacket {
                    codec: Codec::H264,
                    data: vec![1],
                    pts_90k: None,
                    is_keyframe: false,
                },
            );
            assert_eq!(nv.layout, EncodedLayout::AnnexB);
        }
    }

    #[cfg(feature = "unstable-raw-inputs")]
    #[test]
    fn encode_frame_into_backend_frame_rejects_unsupported_buffer_types() {
        let dims = Dimensions {
            width: std::num::NonZeroU32::new(640).unwrap(),
            height: std::num::NonZeroU32::new(360).unwrap(),
        };
        let result = encode_frame_into_backend_frame(EncodeFrame {
            dims,
            pts_90k: Some(Timestamp90k(0)),
            buffer: RawFrameBuffer::Rgb24(vec![0; 640 * 360 * 3]),
            force_keyframe: false,
        });
        assert!(matches!(result, Err(BackendError::InvalidInput(_))));
    }

    #[cfg(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        )
    ))]
    #[test]
    fn decode_reap_timeout_waits_until_deadline_when_empty() {
        let mut session = DecodeSession::new(Backend::Auto, DecoderConfig::new(Codec::H264, 30, false));
        let timeout = Duration::from_millis(8);
        let start = std::time::Instant::now();
        let out = session.reap_timeout(timeout).unwrap();
        let elapsed = start.elapsed();

        assert!(out.is_none());
        assert!(elapsed >= timeout);
    }

    #[cfg(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        )
    ))]
    #[test]
    fn encode_reap_timeout_waits_until_deadline_when_empty() {
        let mut session = EncodeSession::new(Backend::Auto, EncoderConfig::new(Codec::H264, 30, false));
        let timeout = Duration::from_millis(8);
        let start = std::time::Instant::now();
        let out = session.reap_timeout(timeout).unwrap();
        let elapsed = start.elapsed();

        assert!(out.is_none());
        assert!(elapsed >= timeout);
    }
}
