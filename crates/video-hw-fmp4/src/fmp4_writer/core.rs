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
    Backend, BackendEncoderOptions, BackendKind, Codec, Dimensions, EncodeFrame, EncodedChunk,
    EncodedLayout, EncoderConfig, IntelEncoderOptions, RawFrameBuffer, Timestamp90k,
};

use super::{
    config::{Fmp4WriterConfig, Fmp4WriterStatus, Fmp4WriterSummary, FragmentFrames, Pts90k},
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

    pub(crate) fn write_rgba(&mut self, frame: RgbaFrame, pts: Pts90k) -> Result<()> {
        self.inner
            .submit_rgba_frame(frame.into_inner(), to_i64_pts(pts))
    }

    pub(crate) fn write_argb(&mut self, frame: ArgbFrame, pts: Pts90k) -> Result<()> {
        self.inner
            .submit_argb_frame(frame.into_inner(), to_i64_pts(pts))
    }

    pub(crate) fn set_fragment_frames(&mut self, value: FragmentFrames) -> Result<()> {
        self.inner.set_fragment_frames(value.get().get())
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

#[derive(Debug)]
struct PendingSample {
    keyframe: bool,
    pts_90k: i64,
    data: Vec<u8>,
}

#[derive(Debug)]
struct RecorderSummaryCompat {
    output_path: PathBuf,
    segments_written: u64,
    packets_seen: u64,
    flush_packets: u64,
    bytes_written: u64,
    duration_90k: u64,
}

enum RecorderWriterCommand {
    WriteInit(Vec<u8>),
    WriteFragment {
        metadata: Vec<u8>,
        payload: Vec<u8>,
    },
    RewriteInit {
        init: Vec<u8>,
        expected_len: usize,
    },
    WriteMfra(Vec<u8>),
    Finish {
        reply_tx: std::sync::mpsc::Sender<std::result::Result<u64, String>>,
    },
}

struct RecorderFileWriter {
    command_tx: std::sync::mpsc::Sender<RecorderWriterCommand>,
    error_rx: std::sync::mpsc::Receiver<String>,
    join_handle: Option<std::thread::JoinHandle<()>>,
}

impl RecorderFileWriter {
    fn spawn(output_path: PathBuf) -> Result<Self> {
        let (command_tx, command_rx) = std::sync::mpsc::channel::<RecorderWriterCommand>();
        let (error_tx, error_rx) = std::sync::mpsc::channel::<String>();
        let thread_name = format!("video-hw-fmp4-file-writer-{}", output_path.display());
        let join_handle = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || run_recorder_file_writer(output_path, command_rx, error_tx))
            .context("failed to spawn recorder file writer thread")?;
        Ok(Self {
            command_tx,
            error_rx,
            join_handle: Some(join_handle),
        })
    }

    fn send(&mut self, command: RecorderWriterCommand) -> Result<()> {
        self.poll_error()?;
        self.command_tx
            .send(command)
            .context("failed to send recorder writer command")
    }

    fn poll_error(&mut self) -> Result<()> {
        if let Ok(message) = self.error_rx.try_recv() {
            bail!("recorder file writer failed: {message}");
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<u64> {
        self.poll_error()?;
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.command_tx
            .send(RecorderWriterCommand::Finish { reply_tx })
            .context("failed to send recorder writer finish command")?;
        let bytes_written = reply_rx
            .recv()
            .context("failed to receive recorder writer completion")?
            .map_err(anyhow::Error::msg)?;
        self.join()?;
        Ok(bytes_written)
    }

    fn join(&mut self) -> Result<()> {
        if let Some(join_handle) = self.join_handle.take() {
            join_handle
                .join()
                .map_err(|_| anyhow!("recorder file writer thread panicked"))?;
        }
        Ok(())
    }
}

fn run_recorder_file_writer(
    output_path: PathBuf,
    command_rx: std::sync::mpsc::Receiver<RecorderWriterCommand>,
    error_tx: std::sync::mpsc::Sender<String>,
) {
    let result = (|| -> Result<()> {
        let file = File::create(&output_path)
            .with_context(|| format!("failed to create {}", output_path.display()))?;
        let mut writer = BufWriter::new(file);
        let mut bytes_written = 0_u64;

        while let Ok(command) = command_rx.recv() {
            match command {
                RecorderWriterCommand::WriteInit(init) => {
                    writer
                        .write_all(&init)
                        .context("failed to write init segment")?;
                    bytes_written = bytes_written.saturating_add(init.len() as u64);
                }
                RecorderWriterCommand::WriteFragment { metadata, payload } => {
                    writer
                        .write_all(&metadata)
                        .context("failed to write fragment metadata")?;
                    writer
                        .write_all(&payload)
                        .context("failed to write fragment payload")?;
                    bytes_written =
                        bytes_written.saturating_add((metadata.len() + payload.len()) as u64);
                }
                RecorderWriterCommand::RewriteInit { init, expected_len } => {
                    writer
                        .flush()
                        .context("failed to flush before init rewrite")?;
                    writer
                        .seek(SeekFrom::Start(0))
                        .context("failed to seek to init start for rewrite")?;
                    if init.len() != expected_len {
                        bail!(
                            "patched init length mismatch: expected {}, got {}",
                            expected_len,
                            init.len()
                        );
                    }
                    writer
                        .write_all(&init)
                        .context("failed to rewrite init segment")?;
                    writer
                        .seek(SeekFrom::End(0))
                        .context("failed to seek back to output end after init rewrite")?;
                }
                RecorderWriterCommand::WriteMfra(mfra) => {
                    writer
                        .write_all(&mfra)
                        .context("failed to write mfra box")?;
                    bytes_written = bytes_written.saturating_add(mfra.len() as u64);
                }
                RecorderWriterCommand::Finish { reply_tx } => {
                    let finish_result = (|| -> Result<u64> {
                        writer
                            .flush()
                            .context("failed to flush output file during finish")?;
                        writer
                            .get_ref()
                            .sync_data()
                            .context("failed to sync output file during finish")?;
                        Ok(bytes_written)
                    })();
                    let send_result =
                        reply_tx.send(finish_result.map_err(|err| format!("{err:#}")));
                    if let Err(err) = send_result {
                        bail!("failed to return recorder finish result: {err}");
                    }
                    return Ok(());
                }
            }
        }

        bail!("recorder writer command channel closed before finish")
    })();

    if let Err(err) = result {
        let _ = error_tx.send(format!("{err:#}"));
    }
}

struct RecordingSettings {
    backend: Backend,
    codec: Codec,
    require_hardware: bool,
    intel_force_software: bool,
    fragment_frames: usize,
}

struct RecorderState {
    encoder: BackendEncoderSession,
    muxer: Fmp4SegmentMuxer,
    writer: RecorderFileWriter,
    output_path: PathBuf,
    creation_timestamp: Duration,
    dims: Dimensions,
    width_u16: u16,
    height_u16: u16,
    codec: Codec,
    timescale: NonZeroU32,
    default_duration: u32,
    first_input_pts_90k: Option<i64>,
    last_submitted_pts_90k: Option<i64>,
    next_fallback_pts_90k: i64,
    last_written_pts_90k: Option<i64>,
    pending_submitted_pts_90k: VecDeque<i64>,
    pending_force_keyframe: bool,
    submitted_frames: u64,
    vps: Option<Vec<u8>>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    av1_sequence_header: Option<Vec<u8>>,
    av1_config: Option<Av1ConfigSummary>,
    sample_entry: Option<SampleEntry>,
    sample_entry_emitted: bool,
    init_written: bool,
    init_bytes_written: Option<usize>,
    total_duration_90k: u64,
    pending_samples: VecDeque<PendingSample>,
    fragment_frames: usize,
    segments_written: u64,
    packets_seen: u64,
    bytes_written: u64,
}

impl RecorderState {
    fn new(config: &Fmp4WriterConfig) -> Result<Self> {
        let settings = RecordingSettings {
            backend: config.backend,
            codec: config.codec,
            require_hardware: config.require_hardware,
            intel_force_software: config.intel_force_software,
            fragment_frames: config.fragment_frames.get().get(),
        };
        let width_u32 = config.frame_size.width().get();
        let height_u32 = config.frame_size.height().get();
        let dims = Dimensions {
            width: config.frame_size.width(),
            height: config.frame_size.height(),
        };
        let width_u16 = u16::try_from(width_u32).context("width must fit in u16")?;
        let height_u16 = u16::try_from(height_u32).context("height must fit in u16")?;
        let fps = i32::try_from(config.frame_rate.get().get()).context("fps must fit in i32")?;
        let (resolved_backend, encoder_config) =
            resolve_recording_backend_and_config(&settings, fps)?;
        if let Some(parent) = config.output_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create output dir: {}", parent.display()))?;
        }
        let writer = RecorderFileWriter::spawn(config.output_path.clone())?;
        let creation_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let default_duration = u32::try_from((90_000 / fps.max(1)).max(1))
            .context("failed to compute default frame duration")?;
        Ok(Self {
            encoder: BackendEncoderSession::new(resolved_backend, encoder_config)?,
            muxer: Fmp4SegmentMuxer::with_options(SegmentMuxerOptions { creation_timestamp })
                .context("failed to create fMP4 muxer")?,
            writer,
            output_path: config.output_path.clone(),
            creation_timestamp,
            dims,
            width_u16,
            height_u16,
            codec: settings.codec,
            timescale: NonZeroU32::new(90_000).expect("constant non-zero"),
            default_duration,
            first_input_pts_90k: None,
            last_submitted_pts_90k: None,
            next_fallback_pts_90k: 0,
            last_written_pts_90k: None,
            pending_submitted_pts_90k: VecDeque::new(),
            pending_force_keyframe: true,
            submitted_frames: 0,
            vps: None,
            sps: None,
            pps: None,
            av1_sequence_header: None,
            av1_config: None,
            sample_entry: None,
            sample_entry_emitted: false,
            init_written: false,
            init_bytes_written: None,
            total_duration_90k: 0,
            pending_samples: VecDeque::new(),
            fragment_frames: settings.fragment_frames.max(1),
            segments_written: 0,
            packets_seen: 0,
            bytes_written: 0,
        })
    }

    fn submit_rgba_frame(&mut self, frame_rgba: Vec<u8>, pts_90k: i64) -> Result<()> {
        self.submit_argb_frame(rgba_to_argb_bytes(&frame_rgba), pts_90k)
    }

    fn submit_argb_frame(&mut self, frame_argb: Vec<u8>, pts_90k: i64) -> Result<()> {
        let fragment_interval = self.fragment_frames.max(1) as u64;
        let force_keyframe =
            self.pending_force_keyframe || self.submitted_frames.is_multiple_of(fragment_interval);
        let origin = *self.first_input_pts_90k.get_or_insert(pts_90k);
        let mut normalized_pts_90k = pts_90k.saturating_sub(origin).max(0);
        if let Some(previous) = self.last_submitted_pts_90k
            && normalized_pts_90k <= previous
        {
            normalized_pts_90k = previous.saturating_add(i64::from(self.default_duration));
        }
        self.last_submitted_pts_90k = Some(normalized_pts_90k);
        self.encoder
            .submit(EncodeFrame {
                dims: self.dims,
                pts_90k: Some(Timestamp90k(normalized_pts_90k)),
                buffer: RawFrameBuffer::Argb8888(frame_argb),
                force_keyframe,
            })
            .context("encoder submit failed")?;
        self.pending_submitted_pts_90k.push_back(normalized_pts_90k);
        self.pending_force_keyframe = false;
        self.submitted_frames = self.submitted_frames.saturating_add(1);
        while let Some(chunk) = self.encoder.try_reap().context("encoder try_reap failed")? {
            self.handle_chunk(chunk)?;
        }
        if self.encoder.requires_periodic_fragment_flush()
            && self.submitted_frames.is_multiple_of(fragment_interval)
        {
            for chunk in self
                .encoder
                .flush()
                .context("encoder periodic fragment flush failed")?
            {
                self.handle_chunk(chunk)?;
            }
        }
        Ok(())
    }

    fn set_fragment_frames(&mut self, fragment_frames: usize) -> Result<()> {
        self.fragment_frames = fragment_frames.max(1);
        self.pending_force_keyframe = true;
        self.submitted_frames = 0;
        self.flush_pending_if_ready(false)
    }

    fn finish(mut self) -> Result<RecorderSummaryCompat> {
        let mut flush_packets = 0_u64;
        for chunk in self.encoder.flush().context("encoder flush failed")? {
            flush_packets = flush_packets.saturating_add(1);
            self.handle_chunk(chunk)?;
        }
        if self.sample_entry.is_none() && !self.pending_samples.is_empty() {
            bail!("recording ended before SPS/PPS could be observed");
        }
        self.flush_pending_if_ready(true)?;
        self.rewrite_init_segment_with_final_timing()?;
        if self.segments_written > 0 {
            let mfra = self
                .muxer
                .mfra_bytes()
                .context("failed to build mfra box")?;
            self.writer.send(RecorderWriterCommand::WriteMfra(mfra))?;
        }
        self.bytes_written = match self.writer.finish() {
            Ok(bytes) => bytes,
            Err(err) => {
                let _ = self.writer.join();
                return Err(err);
            }
        };
        Ok(RecorderSummaryCompat {
            output_path: self.output_path,
            segments_written: self.segments_written,
            packets_seen: self.packets_seen,
            flush_packets,
            bytes_written: self.bytes_written,
            duration_90k: self.total_duration_90k,
        })
    }

    fn handle_chunk(&mut self, chunk: EncodedChunk) -> Result<()> {
        self.packets_seen = self.packets_seen.saturating_add(1);
        if chunk.codec != self.codec {
            bail!(
                "encoded chunk codec mismatch: expected {}, got {}",
                self.codec,
                chunk.codec
            );
        }
        let sample_data = match (self.codec, chunk.layout) {
            (Codec::H264, EncodedLayout::AnnexB) => {
                annexb_chunk_to_avcc_sample(&chunk.data, &mut self.sps, &mut self.pps)?
            }
            (Codec::H264, EncodedLayout::Avcc) => {
                avcc_chunk_to_avcc_sample(&chunk.data, &mut self.sps, &mut self.pps)?
            }
            (Codec::Hevc, EncodedLayout::AnnexB) => annexb_chunk_to_hvcc_sample(
                &chunk.data,
                &mut self.vps,
                &mut self.sps,
                &mut self.pps,
            )?,
            (Codec::Hevc, EncodedLayout::Hvcc) => {
                hvcc_chunk_to_hvcc_sample(&chunk.data, &mut self.vps, &mut self.sps, &mut self.pps)?
            }
            (Codec::Av1, EncodedLayout::Av1) => av1_chunk_to_av1_sample(
                &chunk.data,
                &mut self.av1_sequence_header,
                &mut self.av1_config,
            )?,
            (_, layout) => {
                bail!(
                    "unsupported encoded layout for fMP4 writer: {} (codec={})",
                    layout,
                    self.codec
                );
            }
        };
        if let Some(sample_data) = sample_data {
            let pts_90k = self.resolve_sample_pts_90k(chunk.pts_90k.map(|pts| pts.0));
            self.pending_samples.push_back(PendingSample {
                keyframe: chunk.is_keyframe,
                pts_90k,
                data: sample_data,
            });
        }
        self.ensure_sample_entry();
        self.flush_pending_if_ready(false)?;
        Ok(())
    }

    fn ensure_sample_entry(&mut self) {
        if self.sample_entry.is_some() {
            return;
        }
        self.sample_entry = match self.codec {
            Codec::H264 => {
                let (Some(sps), Some(pps)) = (&self.sps, &self.pps) else {
                    return;
                };
                Some(create_h264_sample_entry(
                    self.width_u16,
                    self.height_u16,
                    sps,
                    pps,
                ))
            }
            Codec::Hevc => {
                let (Some(vps), Some(sps), Some(pps)) = (&self.vps, &self.sps, &self.pps) else {
                    return;
                };
                Some(create_hevc_sample_entry(
                    self.width_u16,
                    self.height_u16,
                    vps,
                    sps,
                    pps,
                ))
            }
            Codec::Av1 => {
                let (Some(sequence_header), Some(config)) =
                    (&self.av1_sequence_header, &self.av1_config)
                else {
                    return;
                };
                Some(create_av1_sample_entry(
                    self.width_u16,
                    self.height_u16,
                    sequence_header,
                    *config,
                ))
            }
        };
    }

    fn flush_pending_if_ready(&mut self, flush_partial_fragment: bool) -> Result<()> {
        if self.sample_entry.is_none() {
            return Ok(());
        }
        let fragment_frames = self.fragment_frames.max(1);
        while self.pending_samples.len() >= fragment_frames
            || (flush_partial_fragment && !self.pending_samples.is_empty())
        {
            let take_count = if flush_partial_fragment {
                self.pending_samples.len().min(fragment_frames)
            } else {
                fragment_frames
            };
            let mut fragment_samples = Vec::with_capacity(take_count);
            for _ in 0..take_count {
                if let Some(sample) = self.pending_samples.pop_front() {
                    fragment_samples.push(sample);
                }
            }
            if fragment_samples.is_empty() {
                break;
            }
            self.write_media_fragment(fragment_samples)?;
        }
        Ok(())
    }

    fn write_media_fragment(&mut self, fragment_samples: Vec<PendingSample>) -> Result<()> {
        let include_sample_entry = !self.sample_entry_emitted;
        let mut payload = Vec::new();
        let mut samples = Vec::with_capacity(fragment_samples.len());
        for (index, pending) in fragment_samples.into_iter().enumerate() {
            let sample_entry = if include_sample_entry && index == 0 {
                self.sample_entry.clone()
            } else {
                None
            };
            let duration = self.sample_duration_for_pts(pending.pts_90k);
            self.total_duration_90k = self.total_duration_90k.saturating_add(u64::from(duration));
            let data_offset =
                u64::try_from(payload.len()).context("segment payload offset overflow")?;
            samples.push(Sample {
                track_kind: TrackKind::Video,
                sample_entry,
                keyframe: pending.keyframe,
                timescale: self.timescale,
                duration,
                composition_time_offset: None,
                data_offset,
                data_size: pending.data.len(),
            });
            payload.extend_from_slice(&pending.data);
        }
        let metadata = self
            .muxer
            .create_media_segment_metadata(&samples)
            .context("failed to create media segment metadata")?;
        if !self.init_written {
            let init = self
                .muxer
                .init_segment_bytes()
                .context("failed to build init segment")?;
            self.writer
                .send(RecorderWriterCommand::WriteInit(init.clone()))?;
            self.bytes_written = self.bytes_written.saturating_add(init.len() as u64);
            self.init_written = true;
            self.init_bytes_written = Some(init.len());
        }
        let metadata_len = metadata.len();
        let payload_len = payload.len();
        self.writer
            .send(RecorderWriterCommand::WriteFragment { metadata, payload })?;
        self.bytes_written = self
            .bytes_written
            .saturating_add((metadata_len + payload_len) as u64);
        self.segments_written = self.segments_written.saturating_add(1);
        self.sample_entry_emitted = true;
        Ok(())
    }

    fn sample_duration_for_pts(&mut self, pts_90k: i64) -> u32 {
        let duration = match self.last_written_pts_90k {
            Some(prev) => {
                let diff = pts_90k.saturating_sub(prev);
                if diff > 0 {
                    u32::try_from(diff).unwrap_or(self.default_duration)
                } else {
                    self.default_duration
                }
            }
            None => self.default_duration,
        };
        self.last_written_pts_90k = Some(pts_90k);
        duration
    }

    fn resolve_sample_pts_90k(&mut self, chunk_pts_90k: Option<i64>) -> i64 {
        let resolved = match chunk_pts_90k {
            Some(pts) => {
                let _ = self.pending_submitted_pts_90k.pop_front();
                pts
            }
            None => self
                .pending_submitted_pts_90k
                .pop_front()
                .unwrap_or_else(|| {
                    let fallback = self.next_fallback_pts_90k;
                    self.next_fallback_pts_90k = self
                        .next_fallback_pts_90k
                        .saturating_add(i64::from(self.default_duration));
                    fallback
                }),
        };
        self.next_fallback_pts_90k = resolved.saturating_add(i64::from(self.default_duration));
        resolved
    }

    fn rewrite_init_segment_with_final_timing(&mut self) -> Result<()> {
        if !self.init_written {
            return Ok(());
        }
        let expected_init_len = self
            .init_bytes_written
            .context("init segment length is missing")?;
        let init = self
            .muxer
            .init_segment_bytes()
            .context("failed to rebuild init segment")?;
        let patched_init = patch_fmp4_init_segment_timing(
            &init,
            self.creation_timestamp,
            self.total_duration_90k,
            self.timescale,
        )?;
        if patched_init.len() != expected_init_len {
            bail!(
                "finalized init segment size changed (expected {}, got {}), cannot rewrite in-place",
                expected_init_len,
                patched_init.len()
            );
        }
        self.writer.send(RecorderWriterCommand::RewriteInit {
            init: patched_init,
            expected_len: expected_init_len,
        })?;
        Ok(())
    }
}

fn resolve_recording_backend_and_config(
    settings: &RecordingSettings,
    fps: i32,
) -> Result<(BackendKind, EncoderConfig)> {
    let mut config = EncoderConfig::new(settings.codec, fps, settings.require_hardware);
    let resolved_backend = settings.backend.resolve_encoder(&config).with_context(|| {
        format!(
            "failed to resolve encoder backend (backend={}, codec={}, require_hardware={})",
            settings.backend, settings.codec, settings.require_hardware
        )
    })?;
    if backend_is_intel(resolved_backend) {
        config.backend_options = BackendEncoderOptions::Intel(IntelEncoderOptions {
            force_software: settings.intel_force_software,
            hevc_use_vpp: None,
            ..Default::default()
        });
    }
    Ok((resolved_backend, config))
}

enum BackendEncoderSession {
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    Nvidia(Box<video_hw::EncodeSession<NvEncoderAdapter>>),
    #[cfg(all(
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ))]
    Intel(Box<video_hw::EncodeSession<IntelEncoderAdapter>>),
    #[cfg(all(
        feature = "backend-vulkan",
        any(target_os = "linux", target_os = "windows")
    ))]
    Vulkan(Box<video_hw::EncodeSession<VulkanEncoderAdapter>>),
    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    VideoToolbox(Box<video_hw::EncodeSession<VtEncoderAdapter>>),
}

#[cfg(any(
    all(feature = "backend-vt", target_os = "macos"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ),
    all(
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ),
    all(
        feature = "backend-vulkan",
        any(target_os = "linux", target_os = "windows")
    )
))]
impl BackendEncoderSession {
    fn new(backend: BackendKind, config: EncoderConfig) -> Result<Self> {
        let session = match backend {
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Nvidia => Self::Nvidia(Box::new(video_hw::EncodeSession::<
                NvEncoderAdapter,
            >::new(config))),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Intel => Self::Intel(Box::new(video_hw::EncodeSession::<
                IntelEncoderAdapter,
            >::new(config))),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            BackendKind::Vulkan => Self::Vulkan(Box::new(video_hw::EncodeSession::<
                VulkanEncoderAdapter,
            >::new(config))),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            BackendKind::VideoToolbox => Self::VideoToolbox(Box::new(video_hw::EncodeSession::<
                VtEncoderAdapter,
            >::new(config))),
            #[allow(unreachable_patterns)]
            other => {
                bail!("encoder backend {other} is not compiled into video-hw-fmp4")
            }
        };
        Ok(session)
    }

    fn submit(&mut self, frame: EncodeFrame) -> Result<(), BackendError> {
        match self {
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(session) => session.submit(frame),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Intel(session) => session.submit(frame),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Vulkan(session) => session.submit(frame),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(session) => session.submit(frame),
        }
    }

    fn try_reap(&mut self) -> Result<Option<EncodedChunk>, BackendError> {
        match self {
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(session) => session.try_reap(),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Intel(session) => session.try_reap(),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Vulkan(session) => session.try_reap(),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(session) => session.try_reap(),
        }
    }

    fn flush(&mut self) -> Result<Vec<EncodedChunk>, BackendError> {
        match self {
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Nvidia(session) => session.flush(),
            #[cfg(all(
                feature = "backend-intel",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Intel(session) => session.flush(),
            #[cfg(all(
                feature = "backend-vulkan",
                any(target_os = "linux", target_os = "windows")
            ))]
            Self::Vulkan(session) => session.flush(),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            Self::VideoToolbox(session) => session.flush(),
        }
    }

    fn requires_periodic_fragment_flush(&self) -> bool {
        true
    }
}

#[cfg(not(any(
    all(feature = "backend-vt", target_os = "macos"),
    all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ),
    all(
        feature = "backend-intel",
        any(target_os = "linux", target_os = "windows")
    ),
    all(
        feature = "backend-vulkan",
        any(target_os = "linux", target_os = "windows")
    )
)))]
impl BackendEncoderSession {
    fn new(_backend: BackendKind, _config: EncoderConfig) -> Result<Self> {
        bail!("no encoder backend compiled")
    }

    fn submit(&mut self, _frame: EncodeFrame) -> Result<(), BackendError> {
        Err(BackendError::UnsupportedConfig(
            "no encoder backend compiled".to_string(),
        ))
    }

    fn try_reap(&mut self) -> Result<Option<EncodedChunk>, BackendError> {
        Err(BackendError::UnsupportedConfig(
            "no encoder backend compiled".to_string(),
        ))
    }

    fn flush(&mut self) -> Result<Vec<EncodedChunk>, BackendError> {
        Err(BackendError::UnsupportedConfig(
            "no encoder backend compiled".to_string(),
        ))
    }

    fn requires_periodic_fragment_flush(&self) -> bool {
        false
    }
}

#[cfg(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
))]
fn backend_is_intel(backend: BackendKind) -> bool {
    matches!(backend, BackendKind::Intel)
}

#[cfg(not(all(
    feature = "backend-intel",
    any(target_os = "linux", target_os = "windows")
)))]
fn backend_is_intel(_backend: BackendKind) -> bool {
    false
}

fn to_i64_pts(pts: Pts90k) -> i64 {
    i64::try_from(pts.get()).unwrap_or(i64::MAX)
}

fn create_h264_sample_entry(width: u16, height: u16, sps: &[u8], pps: &[u8]) -> SampleEntry {
    let avc_profile_indication = sps.get(1).copied().unwrap_or(66);
    let profile_compatibility = sps.get(2).copied().unwrap_or(0);
    let avc_level_indication = sps.get(3).copied().unwrap_or(30);
    SampleEntry::Avc1(Avc1Box {
        visual: VisualSampleEntryFields {
            data_reference_index: VisualSampleEntryFields::DEFAULT_DATA_REFERENCE_INDEX,
            width,
            height,
            horizresolution: VisualSampleEntryFields::DEFAULT_HORIZRESOLUTION,
            vertresolution: VisualSampleEntryFields::DEFAULT_VERTRESOLUTION,
            frame_count: VisualSampleEntryFields::DEFAULT_FRAME_COUNT,
            compressorname: VisualSampleEntryFields::NULL_COMPRESSORNAME,
            depth: VisualSampleEntryFields::DEFAULT_DEPTH,
        },
        avcc_box: AvccBox {
            avc_profile_indication,
            profile_compatibility,
            avc_level_indication,
            length_size_minus_one: Uint::new(3),
            sps_list: vec![sps.to_vec()],
            pps_list: vec![pps.to_vec()],
            chroma_format: Some(Uint::new(1)),
            bit_depth_luma_minus8: Some(Uint::new(0)),
            bit_depth_chroma_minus8: Some(Uint::new(0)),
            sps_ext_list: vec![],
        },
        unknown_boxes: vec![],
    })
}

fn create_hevc_sample_entry(
    width: u16,
    height: u16,
    vps: &[u8],
    sps: &[u8],
    pps: &[u8],
) -> SampleEntry {
    let nalu_arrays = vec![
        HvccNalUintArray {
            array_completeness: Uint::new(1),
            nal_unit_type: Uint::new(32),
            nalus: vec![vps.to_vec()],
        },
        HvccNalUintArray {
            array_completeness: Uint::new(1),
            nal_unit_type: Uint::new(33),
            nalus: vec![sps.to_vec()],
        },
        HvccNalUintArray {
            array_completeness: Uint::new(1),
            nal_unit_type: Uint::new(34),
            nalus: vec![pps.to_vec()],
        },
    ];
    SampleEntry::Hvc1(Hvc1Box {
        visual: VisualSampleEntryFields {
            data_reference_index: VisualSampleEntryFields::DEFAULT_DATA_REFERENCE_INDEX,
            width,
            height,
            horizresolution: VisualSampleEntryFields::DEFAULT_HORIZRESOLUTION,
            vertresolution: VisualSampleEntryFields::DEFAULT_VERTRESOLUTION,
            frame_count: VisualSampleEntryFields::DEFAULT_FRAME_COUNT,
            compressorname: VisualSampleEntryFields::NULL_COMPRESSORNAME,
            depth: VisualSampleEntryFields::DEFAULT_DEPTH,
        },
        hvcc_box: HvccBox {
            general_profile_space: Uint::new(0),
            general_tier_flag: Uint::new(0),
            general_profile_idc: Uint::new(1),
            general_profile_compatibility_flags: 0,
            general_constraint_indicator_flags: Uint::new(0),
            general_level_idc: 120,
            min_spatial_segmentation_idc: Uint::new(0),
            parallelism_type: Uint::new(0),
            chroma_format_idc: Uint::new(1),
            bit_depth_luma_minus8: Uint::new(0),
            bit_depth_chroma_minus8: Uint::new(0),
            avg_frame_rate: 0,
            constant_frame_rate: Uint::new(0),
            num_temporal_layers: Uint::new(0),
            temporal_id_nested: Uint::new(1),
            length_size_minus_one: Uint::new(3),
            nalu_arrays,
        },
        unknown_boxes: vec![],
    })
}

#[derive(Debug, Clone, Copy)]
struct Av1ConfigSummary {
    seq_profile: u8,
    seq_level_idx_0: u8,
    seq_tier_0: u8,
    high_bitdepth: u8,
    twelve_bit: u8,
    monochrome: u8,
    chroma_subsampling_x: u8,
    chroma_subsampling_y: u8,
    chroma_sample_position: u8,
}

impl Default for Av1ConfigSummary {
    fn default() -> Self {
        Self {
            seq_profile: 0,
            seq_level_idx_0: 31,
            seq_tier_0: 0,
            high_bitdepth: 0,
            twelve_bit: 0,
            monochrome: 0,
            chroma_subsampling_x: 1,
            chroma_subsampling_y: 1,
            chroma_sample_position: 0,
        }
    }
}

fn create_av1_sample_entry(
    width: u16,
    height: u16,
    sequence_header: &[u8],
    config: Av1ConfigSummary,
) -> SampleEntry {
    SampleEntry::Av01(Av01Box {
        visual: VisualSampleEntryFields {
            data_reference_index: VisualSampleEntryFields::DEFAULT_DATA_REFERENCE_INDEX,
            width,
            height,
            horizresolution: VisualSampleEntryFields::DEFAULT_HORIZRESOLUTION,
            vertresolution: VisualSampleEntryFields::DEFAULT_VERTRESOLUTION,
            frame_count: VisualSampleEntryFields::DEFAULT_FRAME_COUNT,
            compressorname: VisualSampleEntryFields::NULL_COMPRESSORNAME,
            depth: VisualSampleEntryFields::DEFAULT_DEPTH,
        },
        av1c_box: Av1cBox {
            seq_profile: Uint::new(config.seq_profile),
            seq_level_idx_0: Uint::new(config.seq_level_idx_0),
            seq_tier_0: Uint::new(config.seq_tier_0),
            high_bitdepth: Uint::new(config.high_bitdepth),
            twelve_bit: Uint::new(config.twelve_bit),
            monochrome: Uint::new(config.monochrome),
            chroma_subsampling_x: Uint::new(config.chroma_subsampling_x),
            chroma_subsampling_y: Uint::new(config.chroma_subsampling_y),
            chroma_sample_position: Uint::new(config.chroma_sample_position),
            initial_presentation_delay_minus_one: None,
            config_obus: sequence_header.to_vec(),
        },
        unknown_boxes: vec![],
    })
}

fn annexb_chunk_to_avcc_sample(
    annexb: &[u8],
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus = Vec::new();
    for nalu in split_annexb_nalus(annexb) {
        if nalu.is_empty() {
            continue;
        }
        match h264_nal_type(nalu) {
            7 => *sps_out = Some(nalu.to_vec()),
            8 => *pps_out = Some(nalu.to_vec()),
            _ => sample_nalus.push(nalu),
        }
    }
    if sample_nalus.is_empty() {
        return Ok(None);
    }
    repack_length_prefixed_nalus(&sample_nalus).map(Some)
}

fn annexb_chunk_to_hvcc_sample(
    annexb: &[u8],
    vps_out: &mut Option<Vec<u8>>,
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus = Vec::new();
    for nalu in split_annexb_nalus(annexb) {
        if nalu.is_empty() {
            continue;
        }
        match hevc_nal_type(nalu) {
            32 => *vps_out = Some(nalu.to_vec()),
            33 => *sps_out = Some(nalu.to_vec()),
            34 => *pps_out = Some(nalu.to_vec()),
            _ => sample_nalus.push(nalu),
        }
    }
    if sample_nalus.is_empty() {
        return Ok(None);
    }
    repack_length_prefixed_nalus(&sample_nalus).map(Some)
}

fn avcc_chunk_to_avcc_sample(
    avcc: &[u8],
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus = Vec::new();
    for nalu in split_length_prefixed_nalus(avcc)? {
        if nalu.is_empty() {
            continue;
        }
        match h264_nal_type(nalu) {
            7 => *sps_out = Some(nalu.to_vec()),
            8 => *pps_out = Some(nalu.to_vec()),
            _ => sample_nalus.push(nalu),
        }
    }
    if sample_nalus.is_empty() {
        return Ok(None);
    }
    repack_length_prefixed_nalus(&sample_nalus).map(Some)
}

fn hvcc_chunk_to_hvcc_sample(
    hvcc: &[u8],
    vps_out: &mut Option<Vec<u8>>,
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus = Vec::new();
    for nalu in split_length_prefixed_nalus(hvcc)? {
        if nalu.is_empty() {
            continue;
        }
        match hevc_nal_type(nalu) {
            32 => *vps_out = Some(nalu.to_vec()),
            33 => *sps_out = Some(nalu.to_vec()),
            34 => *pps_out = Some(nalu.to_vec()),
            _ => sample_nalus.push(nalu),
        }
    }
    if sample_nalus.is_empty() {
        return Ok(None);
    }
    repack_length_prefixed_nalus(&sample_nalus).map(Some)
}

fn av1_chunk_to_av1_sample(
    data: &[u8],
    sequence_header_out: &mut Option<Vec<u8>>,
    config_out: &mut Option<Av1ConfigSummary>,
) -> Result<Option<Vec<u8>>> {
    if data.is_empty() {
        return Ok(None);
    }
    if sequence_header_out.is_none()
        && let Some((obu, payload)) = find_av1_sequence_header_obu(data)
    {
        *sequence_header_out = Some(obu.to_vec());
        *config_out = Some(parse_av1_sequence_header(payload).unwrap_or_default());
    }
    Ok(Some(data.to_vec()))
}

fn split_annexb_nalus(data: &[u8]) -> Vec<&[u8]> {
    let mut nalus = Vec::new();
    let mut cursor = 0usize;
    while let Some((start, start_code_len)) = find_start_code(data, cursor) {
        let nalu_start = start + start_code_len;
        let nalu_end = find_start_code(data, nalu_start)
            .map(|(pos, _)| pos)
            .unwrap_or(data.len());
        if nalu_start < nalu_end {
            nalus.push(&data[nalu_start..nalu_end]);
        }
        cursor = nalu_end;
    }
    if nalus.is_empty() && !data.is_empty() {
        nalus.push(data);
    }
    nalus
}

fn split_length_prefixed_nalus(data: &[u8]) -> Result<Vec<&[u8]>> {
    let mut nalus = Vec::new();
    let mut cursor = 0usize;
    while cursor < data.len() {
        let len_bytes = data
            .get(cursor..cursor + 4)
            .context("truncated NAL length")?;
        let nalu_len = u32::from_be_bytes(len_bytes.try_into().expect("slice len")) as usize;
        cursor = cursor.saturating_add(4);
        let nalu = data
            .get(cursor..cursor + nalu_len)
            .context("truncated NAL payload")?;
        nalus.push(nalu);
        cursor = cursor.saturating_add(nalu_len);
    }
    Ok(nalus)
}

fn repack_length_prefixed_nalus(nalus: &[&[u8]]) -> Result<Vec<u8>> {
    let mut sample = Vec::new();
    for nalu in nalus {
        let len = u32::try_from(nalu.len()).context("NAL too large")?;
        sample.extend_from_slice(&len.to_be_bytes());
        sample.extend_from_slice(nalu);
    }
    Ok(sample)
}

fn find_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    if data.len() < 3 || from >= data.len() {
        return None;
    }
    let mut i = from;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            if data.get(i + 2) == Some(&1) {
                return Some((i, 3));
            }
            if i + 3 < data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                return Some((i, 4));
            }
        }
        i += 1;
    }
    None
}

fn h264_nal_type(nalu: &[u8]) -> u8 {
    nalu[0] & 0x1f
}
fn hevc_nal_type(nalu: &[u8]) -> u8 {
    (nalu[0] >> 1) & 0x3f
}

fn find_av1_sequence_header_obu(data: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut offset = 0usize;
    while offset < data.len() {
        let (obu_type, payload_start, end) = parse_av1_obu_bounds(data, offset)?;
        if obu_type == 1 {
            return Some((&data[offset..end], &data[payload_start..end]));
        }
        offset = end;
    }
    None
}

fn parse_av1_obu_bounds(data: &[u8], offset: usize) -> Option<(u8, usize, usize)> {
    let header = *data.get(offset)?;
    let obu_type = (header >> 3) & 0x0f;
    let has_extension = (header & 0x04) != 0;
    let has_size_field = (header & 0x02) != 0;
    let mut cursor = offset.checked_add(1)?;
    if has_extension {
        cursor = cursor.checked_add(1)?;
        data.get(cursor - 1)?;
    }
    if !has_size_field {
        return None;
    }
    let (payload_size, leb_len) = read_av1_leb128(&data[cursor..])?;
    cursor = cursor.checked_add(leb_len)?;
    let payload_start = cursor;
    let payload_size = usize::try_from(payload_size).ok()?;
    let end = cursor.checked_add(payload_size)?;
    if end > data.len() {
        return None;
    }
    Some((obu_type, payload_start, end))
}

fn read_av1_leb128(data: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (i, byte) in data.iter().copied().take(8).enumerate() {
        value |= u64::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

fn parse_av1_sequence_header(payload: &[u8]) -> Option<Av1ConfigSummary> {
    let mut bits = BitReader::new(payload);
    let seq_profile = bits.read_bits(3)? as u8;
    let _still_picture = bits.read_bits(1)?;
    let reduced_still_picture_header = bits.read_bits(1)? != 0;
    let mut seq_level_idx_0 = 31;
    let mut seq_tier_0 = 0;
    if reduced_still_picture_header {
        seq_level_idx_0 = bits.read_bits(5)? as u8;
    } else {
        let timing_info_present_flag = bits.read_bits(1)? != 0;
        if timing_info_present_flag {
            let _num_units_in_display_tick = bits.read_bits(32)?;
            let _time_scale = bits.read_bits(32)?;
            let equal_picture_interval = bits.read_bits(1)? != 0;
            if equal_picture_interval {
                let _num_ticks_per_picture_minus_1 = bits.read_uvlc()?;
            }
            let decoder_model_info_present_flag = bits.read_bits(1)? != 0;
            if decoder_model_info_present_flag {
                let buffer_delay_length_minus_1 = bits.read_bits(5)?;
                let _num_units_in_decoding_tick = bits.read_bits(32)?;
                let _buffer_removal_time_length_minus_1 = bits.read_bits(5)?;
                let _frame_presentation_time_length_minus_1 = bits.read_bits(5)?;
                let _ = buffer_delay_length_minus_1;
            }
        }
        let initial_display_delay_present_flag = bits.read_bits(1)? != 0;
        let operating_points_cnt_minus_1 = bits.read_bits(5)?;
        for operating_point in 0..=operating_points_cnt_minus_1 {
            let _operating_point_idc = bits.read_bits(12)?;
            let seq_level_idx = bits.read_bits(5)? as u8;
            let seq_tier = if seq_level_idx > 7 {
                bits.read_bits(1)? as u8
            } else {
                0
            };
            if initial_display_delay_present_flag {
                let initial_display_delay_present_for_this_op = bits.read_bits(1)? != 0;
                if initial_display_delay_present_for_this_op {
                    let _initial_display_delay_minus_1 = bits.read_bits(4)?;
                }
            }
            if operating_point == 0 {
                seq_level_idx_0 = seq_level_idx;
                seq_tier_0 = seq_tier;
            }
        }
    }

    Some(Av1ConfigSummary {
        seq_profile,
        seq_level_idx_0,
        seq_tier_0,
        ..Av1ConfigSummary::default()
    })
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_offset: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            bit_offset: 0,
        }
    }

    fn read_bits(&mut self, count: usize) -> Option<u64> {
        let mut value = 0u64;
        for _ in 0..count {
            let byte = *self.data.get(self.bit_offset / 8)?;
            let bit = (byte >> (7 - (self.bit_offset % 8))) & 1;
            value = (value << 1) | u64::from(bit);
            self.bit_offset = self.bit_offset.checked_add(1)?;
        }
        Some(value)
    }

    fn read_uvlc(&mut self) -> Option<u64> {
        let mut leading_zeroes = 0usize;
        while self.read_bits(1)? == 0 {
            leading_zeroes = leading_zeroes.checked_add(1)?;
        }
        if leading_zeroes >= 63 {
            return None;
        }
        let suffix = if leading_zeroes == 0 {
            0
        } else {
            self.read_bits(leading_zeroes)?
        };
        Some((1u64 << leading_zeroes) - 1 + suffix)
    }
}

fn patch_fmp4_init_segment_timing(
    init_segment: &[u8],
    creation_timestamp: Duration,
    duration_in_input_timescale: u64,
    input_timescale: NonZeroU32,
) -> Result<Vec<u8>> {
    let (ftyp_box, ftyp_size) = FtypBox::decode(init_segment).context("failed to decode ftyp")?;
    let moov_input = init_segment
        .get(ftyp_size..)
        .context("init segment missing moov")?;
    let (mut moov_box, moov_size) = MoovBox::decode(moov_input).context("failed to decode moov")?;
    let creation_time = Mp4FileTime::from_unix_time(creation_timestamp);
    let movie_duration = convert_duration_timescale(
        duration_in_input_timescale,
        input_timescale,
        moov_box.mvhd_box.timescale,
    );
    moov_box.mvhd_box.creation_time = creation_time;
    moov_box.mvhd_box.modification_time = creation_time;
    // For fragmented MP4, QuickTime/Finder double-counted playback duration when both the movie/
    // track headers and `mehd` carried the finalized duration. Keeping header durations at 0 and
    // storing the final timeline only in `mehd` avoids that discrepancy while remaining readable
    // to ffprobe and players that follow the fragment timeline.
    moov_box.mvhd_box.duration = 0;
    for trak in &mut moov_box.trak_boxes {
        trak.tkhd_box.creation_time = creation_time;
        trak.tkhd_box.modification_time = creation_time;
        trak.tkhd_box.duration = 0;
        trak.mdia_box.mdhd_box.creation_time = creation_time;
        trak.mdia_box.mdhd_box.modification_time = creation_time;
        trak.mdia_box.mdhd_box.duration = 0;
    }
    if let Some(mvex_box) = moov_box.mvex_box.as_mut()
        && let Some(mehd_box) = mvex_box.mehd_box.as_mut()
    {
        mehd_box.fragment_duration = movie_duration;
    }
    let mut patched = ftyp_box.encode_to_vec().context("failed to encode ftyp")?;
    patched.extend_from_slice(&moov_box.encode_to_vec().context("failed to encode moov")?);
    let consumed = ftyp_size.saturating_add(moov_size);
    if consumed < init_segment.len() {
        patched.extend_from_slice(&init_segment[consumed..]);
    }
    Ok(patched)
}

fn convert_duration_timescale(
    duration: u64,
    from_timescale: NonZeroU32,
    to_timescale: NonZeroU32,
) -> u64 {
    if duration == 0 || from_timescale == to_timescale {
        return duration;
    }
    let from = u64::from(from_timescale.get());
    let to = u64::from(to_timescale.get());
    duration
        .saturating_mul(to)
        .saturating_add(from / 2)
        .saturating_div(from)
}

fn rgba_to_argb_bytes(rgba: &[u8]) -> Vec<u8> {
    let mut argb = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        argb.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
    }
    argb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn av1_chunk_preserves_obu_payload_and_caches_sequence_header() {
        let sequence_header = av1_obu(1, &[0x00]);
        let frame = av1_obu(6, &[0x80]);
        let mut chunk = av1_obu(2, &[]);
        chunk.extend_from_slice(&sequence_header);
        chunk.extend_from_slice(&frame);

        let mut cached_sequence_header = None;
        let mut cached_config = None;
        let sample =
            av1_chunk_to_av1_sample(&chunk, &mut cached_sequence_header, &mut cached_config)
                .expect("AV1 sample conversion")
                .expect("sample payload");

        assert_eq!(sample, chunk);
        assert_eq!(
            cached_sequence_header.as_deref(),
            Some(sequence_header.as_slice())
        );
        assert!(cached_config.is_some());

        let second_frame = av1_obu(6, &[0x81]);
        let second_sample = av1_chunk_to_av1_sample(
            &second_frame,
            &mut cached_sequence_header,
            &mut cached_config,
        )
        .expect("second AV1 sample conversion")
        .expect("second sample payload");
        assert_eq!(second_sample, second_frame);
        assert_eq!(
            cached_sequence_header.as_deref(),
            Some(sequence_header.as_slice())
        );
    }

    #[test]
    fn av1_sample_entry_uses_sequence_header_as_av1c_config_obus() {
        let sequence_header = av1_obu(1, &[0x00]);
        let entry = create_av1_sample_entry(
            320,
            180,
            &sequence_header,
            Av1ConfigSummary {
                seq_profile: 1,
                seq_level_idx_0: 8,
                seq_tier_0: 1,
                high_bitdepth: 1,
                twelve_bit: 0,
                monochrome: 0,
                chroma_subsampling_x: 1,
                chroma_subsampling_y: 0,
                chroma_sample_position: 2,
            },
        );

        let SampleEntry::Av01(av01) = entry else {
            panic!("expected av01 sample entry");
        };
        assert_eq!(av01.visual.width, 320);
        assert_eq!(av01.visual.height, 180);
        assert_eq!(av01.av1c_box.config_obus, sequence_header);
        assert_eq!(av01.av1c_box.seq_profile.get(), 1);
        assert_eq!(av01.av1c_box.seq_level_idx_0.get(), 8);
        assert_eq!(av01.av1c_box.seq_tier_0.get(), 1);
        assert_eq!(av01.av1c_box.high_bitdepth.get(), 1);
        assert_eq!(av01.av1c_box.chroma_subsampling_y.get(), 0);
        assert_eq!(av01.av1c_box.chroma_sample_position.get(), 2);
    }

    fn av1_obu(obu_type: u8, payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() < 128);
        let mut out = Vec::with_capacity(payload.len() + 2);
        out.push((obu_type << 3) | 0x02);
        out.push(payload.len() as u8);
        out.extend_from_slice(payload);
        out
    }
}
