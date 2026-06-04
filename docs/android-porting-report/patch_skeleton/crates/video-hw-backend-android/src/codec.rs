//! Android MediaCodec adapter skeleton.
//!
//! This file intentionally omits most unsafe details. It shows how the backend
//! should fit the video-hw-core VideoDecoder / VideoEncoder contracts.

use std::collections::VecDeque;

use video_hw_core::{
    BackendError, CapabilityReport, Codec, DecodeSummary, DecoderConfig, EncodedPacket,
    EncoderConfig, Frame, VideoDecoder, VideoEncoder,
};

use crate::capability::android_capability_report;

#[derive(Debug)]
pub struct AndroidDecoderAdapter {
    config: DecoderConfig,
    ready: VecDeque<Frame>,
    summary: DecodeSummary,
}

impl AndroidDecoderAdapter {
    #[must_use]
    pub fn new(config: DecoderConfig) -> Self {
        Self {
            config,
            ready: VecDeque::new(),
            summary: DecodeSummary {
                decoded_frames: 0,
                width: None,
                height: None,
                pixel_format: None,
            },
        }
    }

    fn ensure_started(&mut self) -> Result<(), BackendError> {
        // TODO:
        // - Map Codec to MIME.
        // - Create AMediaCodec decoder.
        // - Create AMediaFormat with mime/width/height/csd.
        // - Configure and start.
        Ok(())
    }

    fn drain_output(&mut self, _timeout_us: i64) -> Result<Vec<Frame>, BackendError> {
        // TODO:
        // - dequeueOutputBuffer loop.
        // - handle format changed.
        // - normalize CPU YUV output to Frame.nv12 when possible.
        // - otherwise return metadata-only frames.
        Ok(Vec::new())
    }
}

impl VideoDecoder for AndroidDecoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        android_capability_report(codec, false)
    }

    fn push_bitstream_chunk(
        &mut self,
        chunk: &[u8],
        pts_90k: Option<i64>,
    ) -> Result<Vec<Frame>, BackendError> {
        self.ensure_started()?;
        let _ = (chunk, pts_90k);
        // TODO:
        // - dequeue input buffer.
        // - copy access unit.
        // - queue with pts_us = pts_90k * 1_000_000 / 90_000.
        self.drain_output(0)
    }

    fn flush(&mut self) -> Result<Vec<Frame>, BackendError> {
        // TODO:
        // - queue EOS.
        // - drain until EOS.
        Ok(self.ready.drain(..).collect())
    }

    fn try_reap(&mut self) -> Result<Vec<Frame>, BackendError> {
        self.drain_output(0)
    }

    fn decode_summary(&self) -> DecodeSummary {
        self.summary
    }
}

#[derive(Debug)]
pub struct AndroidEncoderAdapter {
    config: EncoderConfig,
    ready: VecDeque<EncodedPacket>,
}

impl AndroidEncoderAdapter {
    #[must_use]
    pub fn with_config(config: EncoderConfig) -> Self {
        Self {
            config,
            ready: VecDeque::new(),
        }
    }

    fn ensure_started(&mut self, _first_frame: &Frame) -> Result<(), BackendError> {
        // TODO:
        // - Create AMediaCodec encoder.
        // - Configure mime/width/height/bitrate/fps/i-frame interval/color format.
        // - Start.
        Ok(())
    }

    fn drain_output(&mut self, _timeout_us: i64) -> Result<Vec<EncodedPacket>, BackendError> {
        // TODO:
        // - dequeue output buffers.
        // - collect codec-specific-data.
        // - convert H.264/HEVC samples to Annex B EncodedPacket.
        Ok(Vec::new())
    }
}

impl VideoEncoder for AndroidEncoderAdapter {
    fn query_capability(&self, codec: Codec) -> Result<CapabilityReport, BackendError> {
        android_capability_report(codec, true)
    }

    fn push_frame(&mut self, frame: Frame) -> Result<Vec<EncodedPacket>, BackendError> {
        self.ensure_started(&frame)?;
        // TODO:
        // - Convert ARGB to NV12 if config.input_format requires it.
        // - Copy NV12/YUV into input buffer.
        // - Queue with PTS.
        self.drain_output(0)
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        // TODO: queue EOS and drain.
        Ok(self.ready.drain(..).collect())
    }

    fn try_reap(&mut self) -> Result<Vec<EncodedPacket>, BackendError> {
        self.drain_output(0)
    }
}
