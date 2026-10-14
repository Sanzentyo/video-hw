mod config;
mod core;
#[cfg(feature = "async-session")]
mod session_async;
mod state;
mod video_frame;

pub use config::{
    EncodedTrackConfig, Fmp4WriterConfig, Fmp4WriterStatus, Fmp4WriterSummary, FragmentFrames,
    FrameRate, FrameSize, Pts90k, SampleDuration90k,
};
#[cfg(feature = "async-session")]
pub use state::AsyncRecording;
pub use state::{Finished, Ready, SyncEncodedRecording, SyncRecording};
pub use video_frame::{ArgbFrame, RgbaFrame};

use anyhow::Result;
use std::marker::PhantomData;
use video_hw::EncodedChunk;

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

    /// Opens a synchronous encoded-sample session without constructing an
    /// unused raw-frame writer configuration.
    pub fn new_sync_encoded(
        config: EncodedTrackConfig,
    ) -> Result<Fmp4Writer<SyncEncodedRecording>> {
        let core = WriterCore::open_encoded(&config)?;
        Ok(Fmp4Writer {
            config: Fmp4WriterConfig {
                output_path: config.output_path.clone(),
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

impl Fmp4Writer<SyncEncodedRecording> {
    /// Writes an encoded frame or updates codec configuration.
    ///
    /// `EncodedContentKind::CodecConfig` updates the MP4 sample entry and does
    /// not create a media sample. Codec-config chunks must not carry PTS,
    /// duration, keyframe state, or frame payload.
    pub fn write_encoded_chunk(
        &mut self,
        chunk: EncodedChunk,
        duration_90k: Option<SampleDuration90k>,
    ) -> Result<()> {
        self.state.core.write_encoded_chunk(chunk, duration_90k)
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

    /// Returns worker-owned writer status without an accumulating event queue.
    pub async fn status(&mut self) -> Result<Fmp4WriterStatus> {
        self.state.handle.status().await
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
    use video_hw::{Codec, EncodedContentKind, EncodedLayout, Timestamp90k, bitstream};

    #[test]
    fn pts_90k_rejects_values_outside_video_hw_timestamp_range() {
        assert_eq!(
            Pts90k::new(i64::MAX as u64)
                .try_into_timestamp()
                .expect("i64::MAX is representable"),
            Timestamp90k(i64::MAX)
        );
        assert!(Pts90k::new(u64::MAX).try_into_timestamp().is_err());
    }

    #[test]
    fn encoded_session_rejects_mismatched_initial_parameter_sets_before_creating_output() {
        let output_path = std::env::temp_dir().join(format!(
            "video-hw-fmp4-mismatched-config-{}.mp4",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time after epoch")
                .as_nanos()
        ));
        let config = EncodedTrackConfig {
            output_path: output_path.clone(),
            frame_size: FrameSize::new(
                NonZeroU32::new(640).expect("non-zero width"),
                NonZeroU32::new(360).expect("non-zero height"),
            ),
            frame_rate: FrameRate::new(NonZeroU32::new(30).expect("non-zero fps")),
            codec: Codec::H264,
            fragment_frames: FragmentFrames::new(
                NonZeroUsize::new(1).expect("non-zero fragment frames"),
            ),
            initial_parameter_sets: Some(bitstream::ParameterSets::Hevc(Default::default())),
        };

        let error = match Fmp4Writer::<Ready>::new_sync_encoded(config) {
            Ok(_) => panic!("mismatched initial parameter sets must be rejected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("does not match writer codec"));
        assert!(!output_path.exists());
    }

    #[test]
    fn encoded_session_rejects_malformed_h264_parameter_sets_before_creating_output() {
        let output_path = std::env::temp_dir().join(format!(
            "video-hw-fmp4-malformed-h264-config-{}.mp4",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time after epoch")
                .as_nanos()
        ));
        let config = EncodedTrackConfig {
            output_path: output_path.clone(),
            frame_size: FrameSize::new(
                NonZeroU32::new(640).expect("non-zero width"),
                NonZeroU32::new(360).expect("non-zero height"),
            ),
            frame_rate: FrameRate::new(NonZeroU32::new(30).expect("non-zero fps")),
            codec: Codec::H264,
            fragment_frames: FragmentFrames::new(
                NonZeroUsize::new(1).expect("non-zero fragment frames"),
            ),
            initial_parameter_sets: Some(bitstream::ParameterSets::H264(
                bitstream::H264ParameterSets {
                    sps: vec![vec![0x67]],
                    pps: vec![vec![0x68]],
                },
            )),
        };

        let error = match Fmp4Writer::<Ready>::new_sync_encoded(config) {
            Ok(_) => panic!("malformed initial parameter sets must be rejected"),
            Err(error) => error,
        };

        assert!(
            error.to_string().contains("invalid initial H.264 SPS"),
            "unexpected error: {error:#}"
        );
        assert!(!output_path.exists());
    }

    #[test]
    fn encoded_session_reports_output_creation_failure_during_open() {
        let output_path = std::env::temp_dir().join(format!(
            "video-hw-fmp4-output-is-directory-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time after epoch")
                .as_nanos()
        ));
        std::fs::create_dir(&output_path).expect("temporary output directory should be created");
        let config = EncodedTrackConfig {
            output_path: output_path.clone(),
            frame_size: FrameSize::new(
                NonZeroU32::new(640).expect("non-zero width"),
                NonZeroU32::new(360).expect("non-zero height"),
            ),
            frame_rate: FrameRate::new(NonZeroU32::new(30).expect("non-zero fps")),
            codec: Codec::H264,
            fragment_frames: FragmentFrames::new(
                NonZeroUsize::new(1).expect("non-zero fragment frames"),
            ),
            initial_parameter_sets: None,
        };

        let error = match Fmp4Writer::<Ready>::new_sync_encoded(config) {
            Ok(_) => panic!("opening a directory as the output file must fail"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("failed to initialize recorder file writer")
        );
        std::fs::remove_dir(output_path).expect("temporary output directory should be removed");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_roundtrips_writer_status_and_summary() {
        let fragment_frames =
            FragmentFrames::new(NonZeroUsize::new(30).expect("non-zero fragment frames"));
        let status = Fmp4WriterStatus {
            output_path: "output/video.mp4".into(),
            segments_written: 2,
            packets_seen: 60,
            bytes_written: 1_024,
            fragment_frames,
        };
        let status_json = serde_json::to_string(&status).expect("serialize writer status");
        let status_roundtrip: Fmp4WriterStatus =
            serde_json::from_str(&status_json).expect("deserialize writer status");
        assert_eq!(status_roundtrip, status);

        let summary = Fmp4WriterSummary {
            output_path: "output/video.mp4".into(),
            segments_written: 2,
            packets_seen: 60,
            flush_packets: 1,
            bytes_written: 1_024,
            duration_90k: 180_000,
        };
        let summary_json = serde_json::to_string(&summary).expect("serialize writer summary");
        let summary_roundtrip: Fmp4WriterSummary =
            serde_json::from_str(&summary_json).expect("deserialize writer summary");
        assert_eq!(summary_roundtrip, summary);
    }

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
        let mut writer = Fmp4Writer::<Ready>::new_sync_encoded(encoded_config)?;
        let mut codec_config = Vec::new();
        bitstream::append_annexb_nalu(&mut codec_config, &[0x67, 66, 0, 30, 0x80]);
        bitstream::append_annexb_nalu(&mut codec_config, &[0x68, 0xc0]);
        let invalid_config_error = writer
            .write_encoded_chunk(
                EncodedChunk {
                    codec: Codec::H264,
                    layout: EncodedLayout::AnnexB,
                    kind: EncodedContentKind::CodecConfig,
                    data: codec_config.clone(),
                    pts_90k: None,
                    is_keyframe: false,
                },
                Some(SampleDuration90k::new(
                    NonZeroU32::new(3_000).expect("non-zero sample duration"),
                )),
            )
            .expect_err("codec config with a sample duration must be rejected");
        assert!(
            invalid_config_error
                .to_string()
                .contains("must not declare a sample duration")
        );
        writer.write_encoded_chunk(
            EncodedChunk {
                codec: Codec::H264,
                layout: EncodedLayout::AnnexB,
                kind: EncodedContentKind::CodecConfig,
                data: codec_config,
                pts_90k: None,
                is_keyframe: false,
            },
            None,
        )?;
        assert_eq!(
            writer.status().packets_seen,
            0,
            "codec configuration must not count as a media packet"
        );
        let mut first_frame = Vec::new();
        bitstream::append_annexb_nalu(&mut first_frame, &[0x65, 0x88]);
        writer.write_encoded_chunk(
            EncodedChunk {
                codec: Codec::H264,
                layout: EncodedLayout::AnnexB,
                kind: EncodedContentKind::Frame,
                data: first_frame,
                pts_90k: Some(Timestamp90k(0)),
                is_keyframe: true,
            },
            Some(SampleDuration90k::new(
                NonZeroU32::new(3_000).expect("non-zero sample duration"),
            )),
        )?;
        assert_eq!(writer.status().packets_seen, 1);

        let mut changed_codec_config = Vec::new();
        bitstream::append_annexb_nalu(&mut changed_codec_config, &[0x67, 66, 0, 31, 0x80]);
        bitstream::append_annexb_nalu(&mut changed_codec_config, &[0x68, 0xc0]);
        let changed_config_error = writer
            .write_encoded_chunk(
                EncodedChunk {
                    codec: Codec::H264,
                    layout: EncodedLayout::AnnexB,
                    kind: EncodedContentKind::CodecConfig,
                    data: changed_codec_config,
                    pts_90k: None,
                    is_keyframe: false,
                },
                None,
            )
            .expect_err("parameter sets cannot change after the sample entry is emitted");
        assert!(
            changed_config_error
                .to_string()
                .contains("changed after the MP4 sample entry was emitted")
        );

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

    #[cfg(feature = "async-session")]
    #[test]
    fn async_session_reports_initialization_failure_before_returning() {
        let invalid_size = FrameSize::new(
            NonZeroU32::new(u32::from(u16::MAX) + 1).expect("non-zero width"),
            NonZeroU32::new(1).expect("non-zero height"),
        );
        let output_path = std::env::temp_dir().join("video-hw-fmp4-invalid-async-init.mp4");
        let result = Fmp4Writer::<Ready>::new(Fmp4WriterConfig {
            output_path,
            frame_size: invalid_size,
            frame_rate: FrameRate::new(NonZeroU32::new(30).expect("non-zero fps")),
            backend: video_hw::Backend::Auto,
            codec: Codec::H264,
            require_hardware: false,
            intel_force_software: false,
            fragment_frames: FragmentFrames::new(
                NonZeroUsize::new(1).expect("non-zero fragment frames"),
            ),
        })
        .into_async_session();

        let error = result.expect_err("invalid dimensions must fail session initialization");
        assert!(
            error
                .to_string()
                .contains("failed to initialize writer worker")
        );
        assert!(
            error
                .chain()
                .any(|cause| cause.to_string().contains("width must fit in u16"))
        );
    }
}
