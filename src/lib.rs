use std::collections::VecDeque;
use std::time::Duration;

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
mod cuda_transform;
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
    BackendDecoderOptions, BackendEncoderOptions, BackendError, BitstreamInput, CapabilityReport,
    Codec, ColorMetadata, DecodeSummary, DecodedFrame, DecoderConfig, Dimensions, EncodeFrame,
    EncodedChunk, EncodedLayout, EncoderConfig, NvidiaDecoderOptions, NvidiaEncoderOptions,
    NvidiaSessionConfig, RawFrameBuffer, SessionSwitchMode, SessionSwitchRequest, Timestamp90k,
    VtSessionConfig,
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
pub enum BackendKind {
    #[cfg(any(
        all(target_os = "macos", feature = "backend-vt"),
        all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        )
    ))]
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
impl Default for BackendKind {
    fn default() -> Self {
        BackendKind::Auto
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
    decoder_inner: Box<dyn VideoDecoder>,
    ready: VecDeque<DecodedFrame>,
}

impl DecodeSession {
    pub fn new(backend: Backend, config: DecoderConfig) -> Self {
        #[cfg(any(
            all(target_os = "macos", feature = "backend-vt"),
            all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            )
        ))]
        let decoder_inner: Box<dyn VideoDecoder> = match resolve_decoder_backend(backend, &config) {
            Ok(selected) => build_decoder_inner(selected, config),
            Err(err) => Box::new(UnsupportedDecoderAdapter::new(err.to_string())),
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
            .map(legacy_to_decoded_frame)
            .collect::<Vec<_>>();
        self.ready.extend(outputs);
        Ok(())
    }

    pub fn try_reap(&mut self) -> Result<Option<DecodedFrame>, BackendError> {
        Ok(self.ready.pop_front())
    }

    pub fn reap_timeout(
        &mut self,
        _timeout: Duration,
    ) -> Result<Option<DecodedFrame>, BackendError> {
        self.try_reap()
    }

    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>, BackendError> {
        let mut out = std::mem::take(&mut self.ready)
            .into_iter()
            .collect::<Vec<_>>();
        out.extend(
            self.decoder_inner
                .flush()?
                .into_iter()
                .map(legacy_to_decoded_frame)
                .collect::<Vec<_>>(),
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
    encoder_inner: Box<dyn VideoEncoder>,
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
        let (backend_kind, encoder_inner): (BackendKind, Box<dyn VideoEncoder>) =
            match resolve_encoder_backend(backend, &config) {
                Ok(selected) => (selected, build_encoder_inner(selected, config)),
                Err(err) => (
                    fallback_backend_kind(backend),
                    Box::new(UnsupportedEncoderAdapter::new(err.to_string())),
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
        let legacy = encode_frame_to_legacy(frame)?;
        let outputs = self
            .encoder_inner
            .push_frame(legacy)?
            .into_iter()
            .map(|packet| legacy_packet_to_encoded_chunk(self.backend_kind, packet))
            .collect::<Vec<_>>();
        self.ready.extend(outputs);
        Ok(())
    }

    pub fn try_reap(&mut self) -> Result<Option<EncodedChunk>, BackendError> {
        Ok(self.ready.pop_front())
    }

    pub fn reap_timeout(
        &mut self,
        _timeout: Duration,
    ) -> Result<Option<EncodedChunk>, BackendError> {
        self.try_reap()
    }

    pub fn flush(&mut self) -> Result<Vec<EncodedChunk>, BackendError> {
        let mut out = std::mem::take(&mut self.ready)
            .into_iter()
            .collect::<Vec<_>>();
        out.extend(
            self.encoder_inner
                .flush()?
                .into_iter()
                .map(|packet| legacy_packet_to_encoded_chunk(self.backend_kind, packet))
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
    let mut order = Vec::new();
    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    order.push(BackendKind::VideoToolbox);
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    order.push(BackendKind::Nvidia);
    order
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
fn build_decoder_inner(kind: BackendKind, config: DecoderConfig) -> Box<dyn VideoDecoder> {
    match kind {
        BackendKind::Auto => build_decoder_inner(BackendKind::os_default(), config),
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        BackendKind::VideoToolbox => Box::new(vt_backend::VtDecoderAdapter::new(config)),
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        BackendKind::Nvidia => Box::new(nv_backend::NvDecoderAdapter::new(config)),
    }
}

#[cfg(not(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
)))]
fn build_decoder_inner(kind: BackendKind, _config: DecoderConfig) -> Box<dyn VideoDecoder> {
    match kind {}
}

#[cfg(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
))]
fn build_encoder_inner(kind: BackendKind, config: EncoderConfig) -> Box<dyn VideoEncoder> {
    match kind {
        BackendKind::Auto => build_encoder_inner(BackendKind::os_default(), config),
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        BackendKind::VideoToolbox => Box::new(vt_backend::VtEncoderAdapter::with_config(
            config.codec,
            config.fps,
            config.require_hardware,
        )),
        #[cfg(all(
            feature = "backend-nvidia",
            any(target_os = "linux", target_os = "windows")
        ))]
        BackendKind::Nvidia => Box::new(nv_backend::NvEncoderAdapter::with_config(
            config.codec,
            config.fps,
            config.require_hardware,
            config.backend_options,
        )),
    }
}

#[cfg(not(any(
    all(target_os = "macos", feature = "backend-vt"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    )
)))]
fn build_encoder_inner(kind: BackendKind, _config: EncoderConfig) -> Box<dyn VideoEncoder> {
    match kind {}
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

fn legacy_to_decoded_frame(frame: Frame) -> DecodedFrame {
    let dims = dimensions_from_legacy(frame.width, frame.height);
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
    DecodedFrame::Metadata {
        dims,
        pts_90k: frame.pts_90k.map(Timestamp90k),
        pixel_format: frame.pixel_format,
        decode_info_flags: frame.decode_info_flags,
        color,
    }
}

fn encode_frame_to_legacy(frame: EncodeFrame) -> Result<Frame, BackendError> {
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
        RawFrameBuffer::Nv12 { .. } => {
            return Err(BackendError::InvalidInput(
                "RawFrameBuffer::Nv12 is not supported by Encoder::push_encode_frame yet"
                    .to_string(),
            ));
        }
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
        RawFrameBuffer::Nv12 { .. } => {
            return Err(BackendError::InvalidInput(
                "RawFrameBuffer::Nv12 is not supported by Encoder::push_encode_frame yet"
                    .to_string(),
            ));
        }
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
fn legacy_packet_to_encoded_chunk(kind: BackendKind, packet: EncodedPacket) -> EncodedChunk {
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
fn legacy_packet_to_encoded_chunk(kind: BackendKind, _packet: EncodedPacket) -> EncodedChunk {
    match kind {}
}

fn dimensions_from_legacy(width: usize, height: usize) -> Option<Dimensions> {
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
    fn encoded_layout_is_inferred_from_backend_and_codec() {
        #[cfg(all(target_os = "macos", feature = "backend-vt"))]
        {
            let vt_h264 = legacy_packet_to_encoded_chunk(
                BackendKind::VideoToolbox,
                EncodedPacket {
                    codec: Codec::H264,
                    data: vec![1, 2, 3],
                    pts_90k: Some(9000),
                    is_keyframe: true,
                },
            );
            assert_eq!(vt_h264.layout, EncodedLayout::Avcc);

            let vt_hevc = legacy_packet_to_encoded_chunk(
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
            let nv = legacy_packet_to_encoded_chunk(
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

    #[test]
    fn encode_frame_to_legacy_rejects_unsupported_buffer_types() {
        let dims = Dimensions {
            width: std::num::NonZeroU32::new(640).unwrap(),
            height: std::num::NonZeroU32::new(360).unwrap(),
        };
        let result = encode_frame_to_legacy(EncodeFrame {
            dims,
            pts_90k: Some(Timestamp90k(0)),
            buffer: RawFrameBuffer::Rgb24(vec![0; 640 * 360 * 3]),
            force_keyframe: false,
        });
        assert!(matches!(result, Err(BackendError::InvalidInput(_))));
    }
}
