mod config;
mod core;
#[cfg(feature = "async-session")]
mod session_async;
mod state;
mod video_frame;

pub use config::{
    CompositionOffset90k, EncodedTrackConfig, Fmp4WriterConfig, Fmp4WriterStatus,
    Fmp4WriterSummary, FragmentFrames, FrameRate, FrameSize, Pts90k, SampleDuration90k,
    TrackTimescale,
};
#[cfg(feature = "async-session")]
pub use session_async::AsyncWriterEvent;
#[cfg(feature = "async-session")]
pub use state::AsyncRecording;
pub use state::{Finished, Ready, SyncEncodedRecording, SyncRecording};
pub use video_frame::{ArgbFrame, RgbaFrame};

use anyhow::Result;
use std::marker::PhantomData;
use video_hw::EncodedChunk;

use crate::fmp4_reader::EncodedSample;
use crate::fmp4_writer::core::WriterCore;
#[cfg(feature = "async-session")]
use crate::fmp4_writer::session_async::AsyncWriterHandle;

#[derive(Debug)]
pub struct Fmp4Writer<State> {
    config: Fmp4WriterConfig,
    state: State,
    _marker: PhantomData<fn() -> State>,
}

impl Fmp4Writer<Ready> {
    pub fn new(config: Fmp4WriterConfig) -> Self {
        Self {
            config,
            state: Ready,
            _marker: PhantomData,
        }
    }

    pub fn config(&self) -> &Fmp4WriterConfig {
        &self.config
    }

    pub fn into_sync_session(self) -> Result<Fmp4Writer<SyncRecording>> {
        let core = WriterCore::open(&self.config)?;
        Ok(Fmp4Writer {
            config: self.config,
            state: SyncRecording { core },
            _marker: PhantomData,
        })
    }

    pub fn into_sync_encoded_session(
        self,
        config: EncodedTrackConfig,
    ) -> Result<Fmp4Writer<SyncEncodedRecording>> {
        let core = WriterCore::open_encoded(&config)?;
        Ok(Fmp4Writer {
            config: Fmp4WriterConfig {
                output_path: config.output_path,
                frame_size: config.frame_size,
                frame_rate: config.frame_rate,
                backend: video_hw::Backend::Auto,
                codec: config.codec,
                require_hardware: false,
                intel_force_software: false,
                fragment_frames: config.fragment_frames,
            },
            state: SyncEncodedRecording { core },
            _marker: PhantomData,
        })
    }

    #[cfg(feature = "async-session")]
    pub fn into_async_session(self) -> Result<Fmp4Writer<AsyncRecording>> {
        let handle = AsyncWriterHandle::spawn(self.config.clone())?;
        Ok(Fmp4Writer {
            config: self.config,
            state: AsyncRecording { handle },
            _marker: PhantomData,
        })
    }
}

#[derive(Debug, Clone)]
pub struct EncodedSampleInput {
    pub codec: video_hw::Codec,
    pub layout: video_hw::EncodedLayout,
    pub data: Vec<u8>,
    pub pts_90k: Option<video_hw::Timestamp90k>,
    pub is_keyframe: bool,
}

impl From<EncodedChunk> for EncodedSampleInput {
    fn from(value: EncodedChunk) -> Self {
        Self {
            codec: value.codec,
            layout: value.layout,
            data: value.data,
            pts_90k: value.pts_90k,
            is_keyframe: value.is_keyframe,
        }
    }
}

impl Fmp4Writer<SyncEncodedRecording> {
    pub fn write_encoded_chunk(
        &mut self,
        chunk: EncodedChunk,
        duration_90k: Option<SampleDuration90k>,
    ) -> Result<()> {
        self.state.core.write_encoded_chunk(chunk, duration_90k)
    }

    pub fn write_encoded_sample(
        &mut self,
        sample: EncodedSampleInput,
        duration_90k: Option<SampleDuration90k>,
    ) -> Result<()> {
        self.write_encoded_chunk(
            EncodedChunk {
                codec: sample.codec,
                layout: sample.layout,
                data: sample.data,
                pts_90k: sample.pts_90k,
                is_keyframe: sample.is_keyframe,
            },
            duration_90k,
        )
    }

    pub fn write_reader_sample(
        &mut self,
        sample: EncodedSample,
        duration_90k: Option<SampleDuration90k>,
    ) -> Result<()> {
        let codec = sample
            .codec()
            .ok_or_else(|| anyhow::anyhow!("encoded sample does not have a video codec"))?;
        let layout = sample
            .encoded_layout()
            .ok_or_else(|| anyhow::anyhow!("encoded sample does not have an encoded layout"))?;
        self.write_encoded_chunk(
            EncodedChunk {
                codec,
                layout,
                data: sample.data,
                pts_90k: Some(video_hw::Timestamp90k(sample.meta.pts.ticks as i64)),
                is_keyframe: sample.meta.keyframe,
            },
            duration_90k,
        )
    }

    pub fn status(&self) -> Fmp4WriterStatus {
        self.state.core.status()
    }

    pub fn finish(self) -> Result<Fmp4Writer<Finished>> {
        let summary = self.state.core.finish()?;
        Ok(Fmp4Writer {
            config: self.config,
            state: Finished { summary },
            _marker: PhantomData,
        })
    }
}

impl Fmp4Writer<SyncRecording> {
    pub fn write_rgba(&mut self, frame: RgbaFrame, pts: Pts90k) -> Result<()> {
        self.state.core.write_rgba(frame, pts)
    }

    pub fn write_argb(&mut self, frame: ArgbFrame, pts: Pts90k) -> Result<()> {
        self.state.core.write_argb(frame, pts)
    }

    pub fn set_fragment_frames(&mut self, value: FragmentFrames) -> Result<()> {
        self.state.core.set_fragment_frames(value)
    }

    pub fn status(&self) -> Fmp4WriterStatus {
        self.state.core.status()
    }

    pub fn finish(self) -> Result<Fmp4Writer<Finished>> {
        let summary = self.state.core.finish()?;
        Ok(Fmp4Writer {
            config: self.config,
            state: Finished { summary },
            _marker: PhantomData,
        })
    }
}

#[cfg(feature = "async-session")]
impl Fmp4Writer<AsyncRecording> {
    pub async fn write_rgba(&mut self, frame: RgbaFrame, pts: Pts90k) -> Result<()> {
        self.state.handle.write_rgba(frame, pts).await
    }

    pub async fn write_argb(&mut self, frame: ArgbFrame, pts: Pts90k) -> Result<()> {
        self.state.handle.write_argb(frame, pts).await
    }

    pub async fn set_fragment_frames(&mut self, value: FragmentFrames) -> Result<()> {
        self.state.handle.set_fragment_frames(value).await
    }

    pub async fn recv_event(&mut self) -> Option<AsyncWriterEvent> {
        self.state.handle.recv_event().await
    }

    pub fn try_recv_event(&mut self) -> Option<AsyncWriterEvent> {
        self.state.handle.try_recv_event()
    }

    pub async fn finish(self) -> Result<Fmp4Writer<Finished>> {
        let summary = self.state.handle.finish().await?;
        Ok(Fmp4Writer {
            config: self.config,
            state: Finished { summary },
            _marker: PhantomData,
        })
    }
}

impl Fmp4Writer<Finished> {
    pub fn summary(&self) -> &Fmp4WriterSummary {
        &self.state.summary
    }

    pub fn into_summary(self) -> Fmp4WriterSummary {
        self.state.summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Fmp4Reader, Fmp4ReaderConfig};
    use std::{
        num::{NonZeroU32, NonZeroUsize},
        time::{SystemTime, UNIX_EPOCH},
    };
    use video_hw::{Codec, EncodedLayout, Timestamp90k, bitstream};

    #[test]
    fn sync_encoded_session_writes_h264_annexb_stream() -> Result<()> {
        let frame_size = FrameSize::new(
            NonZeroU32::new(640).expect("non-zero width"),
            NonZeroU32::new(360).expect("non-zero height"),
        );
        let output_path = std::env::temp_dir().join(format!(
            "video-hw-fmp4-encoded-session-{}.mp4",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time after epoch")
                .as_nanos()
        ));
        let ready = Fmp4Writer::<Ready>::new(Fmp4WriterConfig {
            output_path: output_path.clone(),
            frame_size,
            frame_rate: FrameRate::new(NonZeroU32::new(30).expect("non-zero fps")),
            backend: video_hw::Backend::Auto,
            codec: Codec::H264,
            require_hardware: false,
            intel_force_software: false,
            fragment_frames: FragmentFrames::new(
                NonZeroUsize::new(1).expect("non-zero fragment frames"),
            ),
        });
        let encoded_config = EncodedTrackConfig {
            output_path: output_path.clone(),
            frame_size,
            frame_rate: FrameRate::new(NonZeroU32::new(30).expect("non-zero fps")),
            codec: Codec::H264,
            fragment_frames: FragmentFrames::new(
                NonZeroUsize::new(1).expect("non-zero fragment frames"),
            ),
            initial_parameter_sets: None,
        };
        let mut writer = ready.into_sync_encoded_session(encoded_config)?;
        let mut first = Vec::new();
        bitstream::append_annexb_nalu(&mut first, &[0x67, 0x64, 0x00, 0x1f]);
        bitstream::append_annexb_nalu(&mut first, &[0x68, 0xee, 0x3c, 0x80]);
        bitstream::append_annexb_nalu(&mut first, &[0x65, 0x88]);
        writer.write_encoded_chunk(
            EncodedChunk {
                codec: Codec::H264,
                layout: EncodedLayout::AnnexB,
                data: first,
                pts_90k: Some(Timestamp90k(0)),
                is_keyframe: true,
            },
            Some(SampleDuration90k::new(3_000)),
        )?;
        let summary = writer.finish()?.into_summary();
        assert_eq!(summary.packets_seen, 1);
        assert!(summary.bytes_written > 0);

        let mut reader =
            Fmp4Reader::new(Fmp4ReaderConfig::new(output_path.clone())).into_sync_session()?;
        assert_eq!(reader.tracks().len(), 1);
        assert_eq!(reader.tracks()[0].codec(), Some(Codec::H264));
        let sample = reader
            .next_sample()?
            .expect("encoded writer should produce one sample");
        assert!(sample.meta.keyframe);
        assert_eq!(sample.encoded_layout(), Some(EncodedLayout::Avcc));
        assert!(!sample.to_annexb()?.is_empty());
        let _ = std::fs::remove_file(output_path);
        Ok(())
    }
}
