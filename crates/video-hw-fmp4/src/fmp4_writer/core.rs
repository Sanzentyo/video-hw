//! fMP4 writer facade backed by private file, state, encoder, sample, and timing modules.

use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{BufWriter, Seek, SeekFrom, Write},
    num::NonZeroU32,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use shiguredo_mp4::{
    Decode, Encode, Mp4FileTime, TrackKind, Uint,
    boxes::{
        Av01Box, Av1cBox, Avc1Box, AvccBox, FtypBox, Hvc1Box, HvccBox, HvccNalUintArray, MoovBox,
        SampleEntry, VisualSampleEntryFields,
    },
    mux::{Fmp4SegmentMuxer, Sample, SegmentMuxerOptions},
};
use video_hw::BackendError;
#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
use video_hw::IntelEncoderAdapter;
#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
use video_hw::NvEncoderAdapter;
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use video_hw::VtEncoderAdapter;
#[cfg(all(
    feature = "backend-vulkan",
    any(target_os = "linux", target_os = "windows")
))]
use video_hw::VulkanEncoderAdapter;
use video_hw::{
    Backend, BackendEncoderOptions, BackendKind, Codec, Dimensions, EncodeFrame, EncodeInputFormat,
    EncodedChunk, EncodedContentKind, EncodedLayout, EncoderConfig, IntelEncoderOptions,
    RawFrameBuffer, Timestamp90k, bitstream::ParameterSets,
};

use super::{
    config::{
        EncodedTrackConfig, Fmp4WriterConfig, Fmp4WriterStatus, Fmp4WriterSummary, FragmentFrames,
        Pts90k, SampleDuration90k,
    },
    video_frame::{ArgbFrame, RgbaFrame},
};

pub(crate) struct WriterCore {
    inner: RecorderState,
}

impl WriterCore {
    pub(crate) fn open(config: &Fmp4WriterConfig) -> Result<Self> {
        Ok(Self {
            inner: RecorderState::new(config)?,
        })
    }

    pub(crate) fn open_encoded(config: &EncodedTrackConfig) -> Result<Self> {
        Ok(Self {
            inner: RecorderState::new_encoded(config)?,
        })
    }

    pub(crate) fn write_rgba(&mut self, frame: RgbaFrame, pts: Pts90k) -> Result<()> {
        let pts = pts
            .try_into_timestamp()
            .context("PTS in 90 kHz units exceeds the signed timestamp range")?;
        self.inner.submit_rgba_frame(frame.into_inner(), pts.0)
    }

    pub(crate) fn write_argb(&mut self, frame: ArgbFrame, pts: Pts90k) -> Result<()> {
        let pts = pts
            .try_into_timestamp()
            .context("PTS in 90 kHz units exceeds the signed timestamp range")?;
        self.inner.submit_argb_frame(frame.into_inner(), pts.0)
    }

    pub(crate) fn set_fragment_frames(&mut self, value: FragmentFrames) -> Result<()> {
        self.inner.set_fragment_frames(value.get().get())
    }

    pub(crate) fn write_encoded_chunk(
        &mut self,
        chunk: EncodedChunk,
        duration_90k: Option<SampleDuration90k>,
    ) -> Result<()> {
        self.inner
            .handle_chunk_with_duration(chunk, duration_90k.map(SampleDuration90k::get))
    }

    pub(crate) fn status(&self) -> Fmp4WriterStatus {
        Fmp4WriterStatus {
            output_path: self.inner.output_path.clone(),
            segments_written: self.inner.segments_written,
            packets_seen: self.inner.packets_seen,
            bytes_written: self.inner.bytes_written,
            fragment_frames: FragmentFrames::new(
                std::num::NonZeroUsize::new(self.inner.fragment_frames.max(1))
                    .expect("fragment_frames is always non-zero"),
            ),
        }
    }

    pub(crate) fn finish(self) -> Result<Fmp4WriterSummary> {
        let summary = self.inner.finish()?;
        Ok(Fmp4WriterSummary {
            output_path: summary.output_path,
            segments_written: summary.segments_written,
            packets_seen: summary.packets_seen,
            flush_packets: summary.flush_packets,
            bytes_written: summary.bytes_written,
            duration_90k: summary.duration_90k,
        })
    }
}

mod encoder;
mod file_writer;
mod sample;
mod state;
mod timing;

#[cfg(test)]
mod tests;

use encoder::*;
use file_writer::*;
use sample::*;
use state::*;
use timing::*;
