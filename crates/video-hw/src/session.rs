use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[cfg(all(target_os = "android", feature = "backend-android"))]
use crate::{AndroidDecoderAdapter, AndroidEncoderAdapter};
use crate::{
    Backend, BackendError, BackendKind, BitstreamInput, CapabilityReport, Codec, DecodeOutputMode,
    DecodeSummary, DecodedFrame, DecoderBackend, DecoderConfig, EncodeFrame, EncodeInputFormat,
    EncodedChunk, EncoderBackend, EncoderConfig, SessionSwitchRequest,
    SessionSwitchingEncoderBackend, VideoDecoder, VideoEncoder, backend_frame_to_decoded_frame,
    backend_packet_to_encoded_chunk, backend_supports_output_mode, encode_frame_into_backend_frame,
    normalize_bitstream_input, preflight_decoder_capability, preflight_encoder_capability,
    resolve_explicit_backend,
};
#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
use crate::{IntelDecoderAdapter, IntelEncoderAdapter};
#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
use crate::{NvDecoderAdapter, NvEncoderAdapter};
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use crate::{VtDecoderAdapter, VtEncoderAdapter};
#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
use crate::{VulkanDecoderAdapter, VulkanEncoderAdapter};

trait DynDecodeSession {
    fn backend_kind(&self) -> BackendKind;
    fn submit(&mut self, input: BitstreamInput) -> Result<(), BackendError>;
    fn try_reap(&mut self) -> Result<Option<DecodedFrame>, BackendError>;
    fn flush(&mut self) -> Result<Vec<DecodedFrame>, BackendError>;
    fn summary(&self) -> DecodeSummary;
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError>;
}

trait DynEncodeSession {
    fn backend_kind(&self) -> BackendKind;
    fn submit(&mut self, frame: EncodeFrame) -> Result<(), BackendError>;
    fn try_reap(&mut self) -> Result<Option<EncodedChunk>, BackendError>;
    fn flush(&mut self) -> Result<Vec<EncodedChunk>, BackendError>;
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError>;
    fn request_session_switch(
        &mut self,
        request: SessionSwitchRequest,
    ) -> Result<Vec<EncodedChunk>, BackendError>;
    fn requires_periodic_fragment_flush(&self) -> bool;
}

pub struct AnyDecodeSession {
    inner: Box<dyn DynDecodeSession>,
}

impl AnyDecodeSession {
    pub fn new(backend: Backend, config: DecoderConfig) -> Result<Self, BackendError> {
        let kind = backend.resolve_decoder(&config)?;
        Self::with_backend_kind(kind, config)
    }

    pub fn with_backend_kind(
        kind: BackendKind,
        config: DecoderConfig,
    ) -> Result<Self, BackendError> {
        resolve_explicit_backend(kind)?;
        if !backend_supports_output_mode(kind, config.output_mode) {
            return Err(BackendError::UnsupportedConfig(format!(
                "{kind:?} decoder does not support DecodeOutputMode::{}",
                config.output_mode
            )));
        }
        let capability = preflight_decoder_capability(kind, &config)?;
        if !capability.decode_is_usable(config.require_hardware)
            || !capability.supports_decode_output_mode(config.output_mode)
        {
            return Err(BackendError::UnsupportedConfig(format!(
                "{kind} decoder is not usable for codec={}, output_mode={}, require_hardware={}: contract_supported={}, runtime={:?}, hardware_acceleration={}",
                config.codec,
                config.output_mode,
                config.require_hardware,
                capability.contract.decode.supported,
                capability.decode_runtime.status,
                capability.decode_runtime.hardware_acceleration
            )));
        }
        match kind {
            #[cfg(all(target_os = "android", feature = "backend-android"))]
            BackendKind::Android => Ok(Self {
                inner: Box::new(DecodeSession::<AndroidDecoderAdapter>::new(config)),
            }),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            BackendKind::VideoToolbox => Ok(Self {
                inner: Box::new(DecodeSession::<VtDecoderAdapter>::new(config)),
            }),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Nvidia => Ok(Self {
                inner: Box::new(DecodeSession::<NvDecoderAdapter>::new(config)),
            }),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Intel => Ok(Self {
                inner: Box::new(DecodeSession::<IntelDecoderAdapter>::new(config)),
            }),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Vulkan => Ok(Self {
                inner: Box::new(DecodeSession::<VulkanDecoderAdapter>::new(config)),
            }),
            _ => Err(BackendError::UnsupportedConfig(format!(
                "{kind} decoder backend is not compiled for this target"
            ))),
        }
    }

    #[must_use]
    pub fn backend_kind(&self) -> BackendKind {
        self.inner.backend_kind()
    }

    pub fn submit(&mut self, input: BitstreamInput) -> Result<(), BackendError> {
        self.inner.submit(input)
    }

    pub fn try_reap(&mut self) -> Result<Option<DecodedFrame>, BackendError> {
        self.inner.try_reap()
    }

    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>, BackendError> {
        self.inner.flush()
    }

    pub fn summary(&self) -> DecodeSummary {
        self.inner.summary()
    }

    pub fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        self.inner.query_capability(codec)
    }
}

pub struct AnyEncodeSession {
    inner: Box<dyn DynEncodeSession>,
}

impl AnyEncodeSession {
    pub fn new(backend: Backend, config: EncoderConfig) -> Result<Self, BackendError> {
        let kind = backend.resolve_encoder(&config)?;
        Self::with_backend_kind(kind, config)
    }

    pub fn with_backend_kind(
        kind: BackendKind,
        config: EncoderConfig,
    ) -> Result<Self, BackendError> {
        resolve_explicit_backend(kind)?;
        let capability = preflight_encoder_capability(kind, &config)?;
        if !capability.encode_is_usable(config.require_hardware)
            || !capability.supports_encode_input_format(config.input_format)
        {
            return Err(BackendError::UnsupportedConfig(format!(
                "{kind} encoder is not usable for codec={}, input_format={}, require_hardware={}: contract_supported={}, runtime={:?}, hardware_acceleration={}",
                config.codec,
                config.input_format,
                config.require_hardware,
                capability.contract.encode.supported,
                capability.encode_runtime.status,
                capability.encode_runtime.hardware_acceleration
            )));
        }
        match kind {
            #[cfg(all(target_os = "android", feature = "backend-android"))]
            BackendKind::Android => Ok(Self {
                inner: Box::new(EncodeSession::<AndroidEncoderAdapter>::new(config)),
            }),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            BackendKind::VideoToolbox => Ok(Self {
                inner: Box::new(EncodeSession::<VtEncoderAdapter>::new(config)),
            }),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Nvidia => Ok(Self {
                inner: Box::new(EncodeSession::<NvEncoderAdapter>::new(config)),
            }),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Intel => Ok(Self {
                inner: Box::new(EncodeSession::<IntelEncoderAdapter>::new(config)),
            }),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Vulkan => Ok(Self {
                inner: Box::new(EncodeSession::<VulkanEncoderAdapter>::new(config)),
            }),
            _ => Err(BackendError::UnsupportedConfig(format!(
                "{kind} encoder backend is not compiled for this target"
            ))),
        }
    }

    #[must_use]
    pub fn backend_kind(&self) -> BackendKind {
        self.inner.backend_kind()
    }

    pub fn submit(&mut self, frame: EncodeFrame) -> Result<(), BackendError> {
        self.inner.submit(frame)
    }

    pub fn try_reap(&mut self) -> Result<Option<EncodedChunk>, BackendError> {
        self.inner.try_reap()
    }

    pub fn flush(&mut self) -> Result<Vec<EncodedChunk>, BackendError> {
        self.inner.flush()
    }

    pub fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        self.inner.query_capability(codec)
    }

    pub fn request_session_switch(
        &mut self,
        request: SessionSwitchRequest,
    ) -> Result<Vec<EncodedChunk>, BackendError> {
        self.inner.request_session_switch(request)
    }

    #[must_use]
    pub fn requires_periodic_fragment_flush(&self) -> bool {
        self.inner.requires_periodic_fragment_flush()
    }
}

pub struct DecodeSession<D: VideoDecoder> {
    decoder_inner: D,
    codec: Codec,
    output_mode: DecodeOutputMode,
    ready: VecDeque<DecodedFrame>,
}

impl<D: VideoDecoder> DecodeSession<D> {
    #[must_use]
    pub fn from_decoder(codec: Codec, output_mode: DecodeOutputMode, decoder: D) -> Self {
        Self {
            decoder_inner: decoder,
            codec,
            output_mode,
            ready: VecDeque::new(),
        }
    }

    pub fn submit(&mut self, input: BitstreamInput) -> Result<(), BackendError> {
        let (annexb, pts_90k) = normalize_bitstream_input(input, self.codec)?;
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
        let flushed = self
            .decoder_inner
            .flush()?
            .into_iter()
            .map(|frame| backend_frame_to_decoded_frame(frame, self.output_mode))
            .collect::<Result<Vec<_>, _>>()?;
        let mut out = std::mem::take(&mut self.ready)
            .into_iter()
            .collect::<Vec<_>>();
        out.extend(flushed);
        Ok(out)
    }

    pub fn summary(&self) -> DecodeSummary {
        self.decoder_inner.decode_summary()
    }

    pub fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        self.decoder_inner.query_capability(codec)
    }
}

impl<D> DecodeSession<D>
where
    D: DecoderBackend,
{
    pub fn new(config: DecoderConfig) -> Self {
        let codec = config.codec;
        let output_mode = config.output_mode;
        let decoder = D::from_decoder_config(config);
        Self::from_decoder(codec, output_mode, decoder)
    }

    #[must_use]
    pub fn backend_kind_static() -> BackendKind {
        D::BACKEND_KIND
    }
}

impl<D> DynDecodeSession for DecodeSession<D>
where
    D: DecoderBackend + 'static,
{
    fn backend_kind(&self) -> BackendKind {
        D::BACKEND_KIND
    }

    fn submit(&mut self, input: BitstreamInput) -> Result<(), BackendError> {
        DecodeSession::submit(self, input)
    }

    fn try_reap(&mut self) -> Result<Option<DecodedFrame>, BackendError> {
        DecodeSession::try_reap(self)
    }

    fn flush(&mut self) -> Result<Vec<DecodedFrame>, BackendError> {
        DecodeSession::flush(self)
    }

    fn summary(&self) -> DecodeSummary {
        DecodeSession::summary(self)
    }

    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        DecodeSession::query_capability(self, codec)
    }
}

pub struct EncodeSession<E: VideoEncoder> {
    backend_kind: BackendKind,
    input_format: EncodeInputFormat,
    encoder_inner: E,
    ready: VecDeque<EncodedChunk>,
}

impl<E: VideoEncoder> EncodeSession<E> {
    #[must_use]
    pub fn from_encoder(
        backend_kind: BackendKind,
        input_format: EncodeInputFormat,
        encoder: E,
    ) -> Self {
        Self {
            backend_kind,
            input_format,
            encoder_inner: encoder,
            ready: VecDeque::new(),
        }
    }

    #[must_use]
    pub fn backend_kind(&self) -> BackendKind {
        self.backend_kind
    }

    pub fn submit(&mut self, frame: EncodeFrame) -> Result<(), BackendError> {
        frame.validate()?;
        let actual_format = frame.buffer.input_format();
        if actual_format != self.input_format {
            return Err(BackendError::InvalidInput(format!(
                "encoder configured for {} input, got {}",
                self.input_format, actual_format
            )));
        }
        let backend_frame = encode_frame_into_backend_frame(frame)?;
        let outputs = self
            .encoder_inner
            .push_frame(backend_frame)?
            .into_iter()
            .map(backend_packet_to_encoded_chunk)
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
            .map(backend_packet_to_encoded_chunk)
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
        let flushed = self
            .encoder_inner
            .flush()?
            .into_iter()
            .map(backend_packet_to_encoded_chunk)
            .collect::<Vec<_>>();
        let mut out = std::mem::take(&mut self.ready)
            .into_iter()
            .collect::<Vec<_>>();
        out.extend(flushed);
        Ok(out)
    }

    pub fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        self.encoder_inner.query_capability(codec)
    }

    pub fn request_session_switch(
        &mut self,
        request: SessionSwitchRequest,
    ) -> Result<Vec<EncodedChunk>, BackendError> {
        let switched = self
            .encoder_inner
            .request_session_switch(request)?
            .into_iter()
            .map(backend_packet_to_encoded_chunk)
            .collect::<Vec<_>>();
        let mut out = std::mem::take(&mut self.ready)
            .into_iter()
            .collect::<Vec<_>>();
        out.extend(switched);
        Ok(out)
    }
}

impl<E> EncodeSession<E>
where
    E: EncoderBackend,
{
    pub fn new(config: EncoderConfig) -> Self {
        let input_format = config.input_format;
        let encoder = E::from_encoder_config(config);
        Self::from_encoder(E::BACKEND_KIND, input_format, encoder)
    }

    #[must_use]
    pub fn backend_kind_static() -> BackendKind {
        E::BACKEND_KIND
    }
}

impl<E> EncodeSession<E>
where
    E: SessionSwitchingEncoderBackend,
{
    pub fn request_session_switch_strict(
        &mut self,
        request: SessionSwitchRequest,
    ) -> Result<Vec<EncodedChunk>, BackendError> {
        self.request_session_switch(request)
    }
}

impl<E> DynEncodeSession for EncodeSession<E>
where
    E: EncoderBackend + 'static,
{
    fn backend_kind(&self) -> BackendKind {
        EncodeSession::backend_kind(self)
    }

    fn submit(&mut self, frame: EncodeFrame) -> Result<(), BackendError> {
        EncodeSession::submit(self, frame)
    }

    fn try_reap(&mut self) -> Result<Option<EncodedChunk>, BackendError> {
        EncodeSession::try_reap(self)
    }

    fn flush(&mut self) -> Result<Vec<EncodedChunk>, BackendError> {
        EncodeSession::flush(self)
    }

    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        EncodeSession::query_capability(self, codec)
    }

    fn request_session_switch(
        &mut self,
        request: SessionSwitchRequest,
    ) -> Result<Vec<EncodedChunk>, BackendError> {
        EncodeSession::request_session_switch(self, request)
    }

    fn requires_periodic_fragment_flush(&self) -> bool {
        true
    }
}

#[cfg(all(target_os = "android", feature = "backend-android"))]
impl DecoderBackend for AndroidDecoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Android;

    fn from_decoder_config(config: DecoderConfig) -> Self {
        Self::new(config)
    }

    fn supports_output_mode(mode: DecodeOutputMode) -> bool {
        matches!(mode, DecodeOutputMode::Metadata)
    }
}

#[cfg(all(target_os = "android", feature = "backend-android"))]
impl EncoderBackend for AndroidEncoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Android;

    fn from_encoder_config(config: EncoderConfig) -> Self {
        Self::with_config(config)
    }
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
impl DecoderBackend for VtDecoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::VideoToolbox;

    fn from_decoder_config(config: DecoderConfig) -> Self {
        Self::new(config)
    }
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
impl EncoderBackend for VtEncoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::VideoToolbox;

    fn from_encoder_config(config: EncoderConfig) -> Self {
        Self::new(config)
    }
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
impl SessionSwitchingEncoderBackend for VtEncoderAdapter {}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
impl DecoderBackend for NvDecoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Nvidia;

    fn from_decoder_config(config: DecoderConfig) -> Self {
        Self::new(config)
    }

    fn supports_output_mode(mode: DecodeOutputMode) -> bool {
        matches!(
            mode,
            DecodeOutputMode::Metadata | DecodeOutputMode::Nv12 | DecodeOutputMode::Rgb24
        )
    }
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
impl EncoderBackend for NvEncoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Nvidia;

    fn from_encoder_config(config: EncoderConfig) -> Self {
        Self::new(config)
    }
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
impl SessionSwitchingEncoderBackend for NvEncoderAdapter {}

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
impl DecoderBackend for IntelDecoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Intel;

    fn from_decoder_config(config: DecoderConfig) -> Self {
        Self::new(config)
    }

    fn supports_output_mode(mode: DecodeOutputMode) -> bool {
        matches!(
            mode,
            DecodeOutputMode::Metadata | DecodeOutputMode::Nv12 | DecodeOutputMode::Rgb24
        )
    }
}

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
impl EncoderBackend for IntelEncoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Intel;

    fn from_encoder_config(config: EncoderConfig) -> Self {
        Self::new(config)
    }
}

#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
impl DecoderBackend for VulkanDecoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Vulkan;

    fn from_decoder_config(config: DecoderConfig) -> Self {
        Self::new(config)
    }

    fn supports_output_mode(mode: DecodeOutputMode) -> bool {
        matches!(
            mode,
            DecodeOutputMode::Metadata | DecodeOutputMode::Nv12 | DecodeOutputMode::Rgb24
        )
    }
}

#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
impl EncoderBackend for VulkanEncoderAdapter {
    const BACKEND_KIND: BackendKind = BackendKind::Vulkan;

    fn from_encoder_config(config: EncoderConfig) -> Self {
        Self::with_config(
            config.codec,
            config.fps,
            config.require_hardware,
            config.backend_options,
        )
    }
}
