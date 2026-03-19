//! Camera preview + fragmented MP4 recorder example.
//!
//! This example captures camera frames via `shiguredo_video_device`,
//! previews them in an `eframe` window, and can toggle recording ON/OFF.
//! Recorded chunks are encoded with `video-hw` (H.264) and muxed into
//! fragmented MP4 (`ftyp+moov` + repeated `moof+mdat`) via `shiguredo_mp4`.

use std::{
    borrow::Cow,
    collections::VecDeque,
    fs::{self, File},
    io::{BufWriter, Seek, SeekFrom, Write},
    num::NonZeroU32,
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use clap::Parser;
use eframe::egui::{self, ColorImage};
use shiguredo_mp4::{
    Decode, Encode, Mp4FileTime, TrackKind, Uint,
    boxes::{
        Avc1Box, AvccBox, FtypBox, Hvc1Box, HvccBox, HvccNalUintArray, MoovBox, SampleEntry,
        VisualSampleEntryFields,
    },
    mux::{Fmp4SegmentMuxer, Sample, SegmentMuxerOptions},
};
use shiguredo_video_device::{
    PixelFormat, VideoCapture, VideoCaptureConfig, VideoDeviceList, VideoFrame, VideoFrameOwned,
};
use video_hw::{
    AnyEncodeSession, Backend, BackendEncoderOptions, BackendKind, Codec, Dimensions, EncodeFrame,
    EncodedChunk, EncodedLayout, EncoderConfig, IntelEncoderOptions, RawFrameBuffer, Timestamp90k,
};

#[derive(Debug, Parser)]
#[command(about = "Preview camera and record fragmented MP4 (fMP4) with recording ON/OFF")]
struct CliArgs {
    #[arg(long)]
    list_devices: bool,
    #[arg(long)]
    video_device_id: Option<String>,
    #[arg(long, default_value = "1280x720")]
    resolution: String,
    #[arg(long, default_value_t = 30)]
    fps: i32,
    #[arg(long)]
    duration: Option<f64>,
    #[arg(long, default_value_t = false)]
    auto_start_recording: bool,
    #[arg(long, default_value = "auto")]
    backend: String,
    #[arg(long, default_value = "h264")]
    codec: String,
    #[arg(long, default_value_t = false)]
    require_hardware: bool,
    #[arg(long, default_value_t = false)]
    intel_force_software: bool,
    #[arg(long, default_value_t = 30)]
    fragment_frames: u32,
    #[arg(long, default_value = "output/camera-fmp4")]
    output_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct RecordingSettings {
    backend: Backend,
    codec: Codec,
    require_hardware: bool,
    intel_force_software: bool,
    fragment_frames: usize,
}

#[derive(Debug, Clone)]
struct CaptureConfigState {
    device_id: Option<String>,
    width: i32,
    height: i32,
    fps: i32,
}

enum CaptureCommand {
    Reconfigure(CaptureConfigState),
    Stop,
}

enum CaptureEvent {
    Reconfigured(CaptureConfigState),
    Error(String),
}

#[derive(Debug)]
struct Args {
    list_devices: bool,
    video_device_id: Option<String>,
    width: i32,
    height: i32,
    fps: i32,
    auto_start_recording: bool,
    fragment_frames: usize,
    duration: Option<Duration>,
    output_dir: PathBuf,
    recording_settings: RecordingSettings,
}

#[derive(Debug, Clone)]
struct UiFrame {
    width: usize,
    height: usize,
    timestamp_us: i64,
    rgba: Vec<u8>,
}

#[derive(Debug)]
struct CapturedFrame {
    frame: VideoFrameOwned,
}

#[derive(Debug)]
struct PendingSample {
    keyframe: bool,
    pts_90k: i64,
    data: Vec<u8>,
}

#[derive(Debug)]
struct RecorderSummary {
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
        reply_tx: mpsc::Sender<std::result::Result<u64, String>>,
    },
}

struct RecorderFileWriter {
    command_tx: mpsc::Sender<RecorderWriterCommand>,
    error_rx: mpsc::Receiver<String>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl RecorderFileWriter {
    fn spawn(output_path: PathBuf) -> Result<Self> {
        let (command_tx, command_rx) = mpsc::channel::<RecorderWriterCommand>();
        let (error_tx, error_rx) = mpsc::channel::<String>();
        let thread_name = format!("camera-record-file-writer-{}", output_path.display());
        let join_handle = thread::Builder::new()
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
            anyhow::bail!("recorder file writer failed: {message}");
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<u64> {
        self.poll_error()?;
        let (reply_tx, reply_rx) = mpsc::channel();
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
                .map_err(|_| anyhow::anyhow!("recorder file writer thread panicked"))?;
        }
        Ok(())
    }
}

fn run_recorder_file_writer(
    output_path: PathBuf,
    command_rx: mpsc::Receiver<RecorderWriterCommand>,
    error_tx: mpsc::Sender<String>,
) {
    let result = (|| -> Result<()> {
        let file = File::create(&output_path)
            .with_context(|| format!("failed to create {}", output_path.display()))?;
        let mut writer = BufWriter::new(file);
        let mut init_bytes_written = None::<usize>;
        let mut bytes_written = 0_u64;

        while let Ok(command) = command_rx.recv() {
            match command {
                RecorderWriterCommand::WriteInit(init) => {
                    writer
                        .write_all(&init)
                        .context("failed to write init segment")?;
                    init_bytes_written = Some(init.len());
                    bytes_written = bytes_written.saturating_add(init.len() as u64);
                }
                RecorderWriterCommand::WriteFragment { metadata, payload } => {
                    writer
                        .write_all(&metadata)
                        .context("failed to write media segment metadata")?;
                    writer
                        .write_all(&payload)
                        .context("failed to write media segment payload")?;
                    writer
                        .flush()
                        .context("failed to flush media fragment writer")?;
                    writer
                        .get_ref()
                        .sync_data()
                        .context("failed to sync media fragment to disk")?;
                    bytes_written =
                        bytes_written.saturating_add((metadata.len() + payload.len()) as u64);
                }
                RecorderWriterCommand::RewriteInit { init, expected_len } => {
                    if init.len() != expected_len {
                        anyhow::bail!(
                            "rewrite init size mismatch (expected {}, got {})",
                            expected_len,
                            init.len()
                        );
                    }
                    if let Some(written_len) = init_bytes_written
                        && written_len != expected_len
                    {
                        anyhow::bail!(
                            "rewrite init expected {} bytes but writer initialized with {} bytes",
                            expected_len,
                            written_len
                        );
                    }
                    writer
                        .flush()
                        .context("failed to flush before init rewrite")?;
                    writer
                        .seek(SeekFrom::Start(0))
                        .context("failed to seek to init segment start")?;
                    writer
                        .write_all(&init)
                        .context("failed to rewrite init segment timing metadata")?;
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
                        anyhow::bail!("failed to return recorder finish result: {err}");
                    }
                    return Ok(());
                }
            }
        }

        anyhow::bail!("recorder writer command channel closed before finish")
    })();

    if let Err(err) = result {
        let _ = error_tx.send(format!("{err:#}"));
    }
}

struct RecorderState {
    encoder: AnyEncodeSession,
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
            ..Default::default()
        });
    }
    Ok((resolved_backend, config))
}

fn probe_recording_backend_path(
    settings: &RecordingSettings,
    width: i32,
    height: i32,
    fps: i32,
) -> Result<BackendKind> {
    let _ = width;
    let _ = height;
    let (resolved_backend, _config) = resolve_recording_backend_and_config(settings, fps)?;
    Ok(resolved_backend)
}

impl RecorderState {
    fn new(
        settings: &RecordingSettings,
        width: i32,
        height: i32,
        fps: i32,
        output_path: PathBuf,
    ) -> Result<Self> {
        let width_u32 = u32::try_from(width).context("width must be >= 0")?;
        let height_u32 = u32::try_from(height).context("height must be >= 0")?;
        let dims = dims(width_u32, height_u32)?;
        let width_u16 =
            u16::try_from(width).context("width must fit in u16 for mp4 sample entry")?;
        let height_u16 =
            u16::try_from(height).context("height must fit in u16 for mp4 sample entry")?;

        let (resolved_backend, config) = resolve_recording_backend_and_config(settings, fps)?;

        if let Some(parent) = output_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create output dir: {}", parent.display()))?;
        }
        let writer = RecorderFileWriter::spawn(output_path.clone())?;
        let creation_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();

        let default_duration = u32::try_from((90_000 / fps.max(1)).max(1))
            .context("failed to compute default frame duration")?;

        Ok(Self {
            encoder: AnyEncodeSession::with_backend_kind(resolved_backend, config)?,
            muxer: Fmp4SegmentMuxer::with_options(SegmentMuxerOptions { creation_timestamp })
                .context("failed to create fMP4 muxer")?,
            writer,
            output_path,
            creation_timestamp,
            dims,
            width_u16,
            height_u16,
            codec: settings.codec,
            timescale: NonZeroU32::new(90_000).expect("90_000 is non-zero"),
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
        let dims = self.dims;
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
                dims,
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

    fn finish(mut self) -> Result<RecorderSummary> {
        let mut flush_packets = 0_u64;
        for chunk in self.encoder.flush().context("encoder flush failed")? {
            flush_packets = flush_packets.saturating_add(1);
            self.handle_chunk(chunk)?;
        }

        if self.sample_entry.is_none() && !self.pending_samples.is_empty() {
            anyhow::bail!("recording ended before SPS/PPS could be observed");
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
        Ok(RecorderSummary {
            output_path: self.output_path,
            segments_written: self.segments_written,
            packets_seen: self.packets_seen,
            flush_packets,
            bytes_written: self.bytes_written,
            duration_90k: self.total_duration_90k,
        })
    }

    fn progress_status(&self) -> String {
        format!(
            "recording ON: {} (segments={}, packets={}, bytes={}, fragment_frames={})",
            self.output_path.display(),
            self.segments_written,
            self.packets_seen,
            self.bytes_written,
            self.fragment_frames
        )
    }

    fn set_fragment_frames(&mut self, fragment_frames: usize) -> Result<()> {
        self.fragment_frames = fragment_frames.max(1);
        self.pending_force_keyframe = true;
        self.submitted_frames = 0;
        self.flush_pending_if_ready(false)
    }

    fn handle_chunk(&mut self, chunk: EncodedChunk) -> Result<()> {
        self.packets_seen = self.packets_seen.saturating_add(1);
        if chunk.codec != self.codec {
            anyhow::bail!(
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
            (_, layout) => {
                anyhow::bail!(
                    "unsupported encoded layout for fMP4 recorder: {} (codec={})",
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
            anyhow::bail!(
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

enum RecorderWorkerCommand {
    SubmitFrame {
        frame_rgba: Vec<u8>,
        pts_90k: i64,
    },
    SetFragmentFrames {
        fragment_frames: usize,
    },
    Finish {
        reply_tx: mpsc::Sender<std::result::Result<RecorderSummary, String>>,
    },
}

enum RecorderWorkerEvent {
    FrameConsumed,
    Status(String),
    Error(String),
}

struct RecorderWorker {
    command_tx: mpsc::Sender<RecorderWorkerCommand>,
    event_rx: mpsc::Receiver<RecorderWorkerEvent>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl RecorderWorker {
    fn spawn(
        settings: RecordingSettings,
        width: i32,
        height: i32,
        fps: i32,
        output_path: PathBuf,
    ) -> Result<Self> {
        let (command_tx, command_rx) = mpsc::channel::<RecorderWorkerCommand>();
        let (event_tx, event_rx) = mpsc::channel::<RecorderWorkerEvent>();
        let (startup_tx, startup_rx) = mpsc::channel::<std::result::Result<(), String>>();
        let join_handle = thread::Builder::new()
            .name("camera-record-worker-thread".to_string())
            .spawn(move || {
                let recorder = match RecorderState::new(&settings, width, height, fps, output_path)
                {
                    Ok(state) => state,
                    Err(err) => {
                        let _ = startup_tx.send(Err(format!("{err:#}")));
                        return;
                    }
                };
                let _ = startup_tx.send(Ok(()));
                run_recorder_worker(recorder, command_rx, event_tx);
            })
            .context("failed to spawn recorder worker thread")?;
        match startup_rx
            .recv()
            .context("recorder worker exited before startup status")?
        {
            Ok(()) => {}
            Err(message) => {
                let _ = join_handle.join();
                anyhow::bail!("{message}");
            }
        }
        Ok(Self {
            command_tx,
            event_rx,
            join_handle: Some(join_handle),
        })
    }

    fn submit_frame(&mut self, frame_rgba: Vec<u8>, pts_90k: i64) -> Result<()> {
        self.command_tx
            .send(RecorderWorkerCommand::SubmitFrame {
                frame_rgba,
                pts_90k,
            })
            .context("failed to queue recorder frame")
    }

    fn set_fragment_frames(&mut self, fragment_frames: usize) -> Result<()> {
        self.command_tx
            .send(RecorderWorkerCommand::SetFragmentFrames { fragment_frames })
            .context("failed to queue fragment frame update")
    }

    fn try_recv_event(&mut self) -> Option<RecorderWorkerEvent> {
        self.event_rx.try_recv().ok()
    }

    fn finish(&mut self) -> Result<RecorderSummary> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.command_tx
            .send(RecorderWorkerCommand::Finish { reply_tx })
            .context("failed to send recorder worker finish command")?;
        let summary = reply_rx
            .recv()
            .context("failed to receive recorder worker finish result")?
            .map_err(anyhow::Error::msg)?;
        self.join()?;
        Ok(summary)
    }

    fn join_only(&mut self) -> Result<()> {
        self.join()
    }

    fn join(&mut self) -> Result<()> {
        if let Some(join_handle) = self.join_handle.take() {
            join_handle
                .join()
                .map_err(|_| anyhow::anyhow!("recorder worker thread panicked"))?;
        }
        Ok(())
    }
}

fn run_recorder_worker(
    mut recorder: RecorderState,
    command_rx: mpsc::Receiver<RecorderWorkerCommand>,
    event_tx: mpsc::Sender<RecorderWorkerEvent>,
) {
    let mut last_segments_written = recorder.segments_written;
    let _ = event_tx.send(RecorderWorkerEvent::Status(recorder.progress_status()));
    while let Ok(command) = command_rx.recv() {
        match command {
            RecorderWorkerCommand::SubmitFrame {
                frame_rgba,
                pts_90k,
            } => {
                let submit_result = recorder.submit_rgba_frame(frame_rgba, pts_90k);
                let _ = event_tx.send(RecorderWorkerEvent::FrameConsumed);
                if let Err(err) = submit_result {
                    let _ = event_tx.send(RecorderWorkerEvent::Error(format!(
                        "failed to submit frame: {err:#}"
                    )));
                    break;
                }
                if recorder.segments_written != last_segments_written
                    || recorder.packets_seen.is_multiple_of(15)
                {
                    last_segments_written = recorder.segments_written;
                    let _ = event_tx.send(RecorderWorkerEvent::Status(recorder.progress_status()));
                }
            }
            RecorderWorkerCommand::SetFragmentFrames { fragment_frames } => {
                if let Err(err) = recorder.set_fragment_frames(fragment_frames) {
                    let _ = event_tx.send(RecorderWorkerEvent::Error(format!(
                        "failed to update fragment frequency: {err:#}"
                    )));
                    break;
                }
                let _ = event_tx.send(RecorderWorkerEvent::Status(recorder.progress_status()));
            }
            RecorderWorkerCommand::Finish { reply_tx } => {
                let result = recorder.finish().map_err(|err| format!("{err:#}"));
                let _ = reply_tx.send(result);
                return;
            }
        }
    }

    if let Err(err) = recorder.finish() {
        let _ = event_tx.send(RecorderWorkerEvent::Error(format!(
            "recorder worker shutdown failed: {err:#}"
        )));
    }
}

#[derive(Debug, Clone)]
struct BackendProbeStatus {
    backend: Backend,
    available: bool,
    detail: String,
}

struct CameraRecordApp {
    rx: mpsc::Receiver<CapturedFrame>,
    capture_cmd_tx: mpsc::Sender<CaptureCommand>,
    capture_event_rx: mpsc::Receiver<CaptureEvent>,
    latest: Option<UiFrame>,
    texture: Option<egui::TextureHandle>,
    displayed_timestamp_us: Option<i64>,
    started_at: Option<Instant>,
    duration: Option<Duration>,
    video_device_id: Option<String>,
    width: i32,
    height: i32,
    fps: i32,
    output_dir: PathBuf,
    recording_settings: RecordingSettings,
    selected_backend: Backend,
    selected_codec: Codec,
    backend_probe_statuses: Vec<BackendProbeStatus>,
    pending_width: i32,
    pending_height: i32,
    pending_fps: i32,
    fragment_frames: usize,
    pending_fragment_frames: usize,
    recording_seq: u64,
    recording_submitted_frames: u64,
    recording: Option<RecorderWorker>,
    recording_queue_depth: usize,
    controls_collapsed: bool,
    total_received: u64,
    total_rendered: u64,
    status_message: String,
}

struct CameraAppInit {
    video_device_id: Option<String>,
    width: i32,
    height: i32,
    fps: i32,
    auto_start_recording: bool,
    fragment_frames: usize,
    duration: Option<Duration>,
    output_dir: PathBuf,
    recording_settings: RecordingSettings,
}

impl CameraRecordApp {
    fn new(
        rx: mpsc::Receiver<CapturedFrame>,
        capture_cmd_tx: mpsc::Sender<CaptureCommand>,
        capture_event_rx: mpsc::Receiver<CaptureEvent>,
        init: CameraAppInit,
    ) -> Self {
        let mut app = Self {
            rx,
            capture_cmd_tx,
            capture_event_rx,
            latest: None,
            texture: None,
            displayed_timestamp_us: None,
            started_at: None,
            duration: init.duration,
            video_device_id: init.video_device_id,
            width: init.width,
            height: init.height,
            fps: init.fps,
            output_dir: init.output_dir,
            selected_backend: init.recording_settings.backend,
            selected_codec: init.recording_settings.codec,
            backend_probe_statuses: Vec::new(),
            pending_width: init.width,
            pending_height: init.height,
            pending_fps: init.fps,
            fragment_frames: init.fragment_frames.max(1),
            pending_fragment_frames: init.fragment_frames.max(1),
            recording_settings: init.recording_settings,
            recording_seq: 0,
            recording_submitted_frames: 0,
            recording: None,
            recording_queue_depth: 0,
            controls_collapsed: false,
            total_received: 0,
            total_rendered: 0,
            status_message: "ready".to_string(),
        };
        app.refresh_backend_probe_statuses();
        if init.auto_start_recording {
            app.start_recording();
        }
        app
    }

    fn current_recording_settings(&self) -> RecordingSettings {
        let mut settings = self.recording_settings.clone();
        settings.backend = self.selected_backend;
        settings.codec = self.selected_codec;
        settings.fragment_frames = self.fragment_frames.max(1);
        settings
    }

    fn refresh_backend_probe_statuses(&mut self) {
        if self.recording.is_some() {
            return;
        }
        let mut statuses = Vec::new();
        for backend in Backend::supported() {
            let mut settings = self.current_recording_settings();
            settings.backend = backend;
            let status =
                match probe_recording_backend_path(&settings, self.width, self.height, self.fps) {
                    Ok(resolved_backend) => BackendProbeStatus {
                        backend,
                        available: true,
                        detail: format!("available (resolved={resolved_backend})"),
                    },
                    Err(err) => BackendProbeStatus {
                        backend,
                        available: false,
                        detail: format!("{err:#}"),
                    },
                };
            statuses.push(status);
        }
        self.backend_probe_statuses = statuses;
    }

    fn selected_backend_probe(&self) -> Option<&BackendProbeStatus> {
        self.backend_probe_statuses
            .iter()
            .find(|status| status.backend == self.selected_backend)
    }

    fn handle_owned_frame(&mut self, captured: CapturedFrame) -> Result<()> {
        let Some(ui_frame) = frame_to_rgba(&captured.frame.as_frame()) else {
            return Ok(());
        };
        self.total_received = self.total_received.saturating_add(1);

        if let Some(recorder) = self.recording.as_mut() {
            let started_at = self.started_at.get_or_insert_with(Instant::now);
            // Recording PTS is fixed-FPS based on submit order. Using capture wallclock here made
            // playback duration drift and, combined with fragmented MP4 metadata, confused players.
            let pts_90k = i64::try_from(
                self.recording_submitted_frames
                    .saturating_mul(fps_frame_duration_90k(self.fps)),
            )
            .unwrap_or(i64::MAX);
            recorder.submit_frame(ui_frame.rgba.clone(), pts_90k)?;
            self.recording_submitted_frames = self.recording_submitted_frames.saturating_add(1);
            self.recording_queue_depth = self.recording_queue_depth.saturating_add(1);
            let _ = started_at;
        }

        self.latest = Some(ui_frame);
        Ok(())
    }

    fn update_texture_if_needed(&mut self, ui: &egui::Ui) {
        let Some(frame) = &self.latest else {
            return;
        };
        if self.displayed_timestamp_us == Some(frame.timestamp_us) {
            return;
        }
        let expected_rgba_len = frame.width.saturating_mul(frame.height).saturating_mul(4);
        if frame.rgba.len() != expected_rgba_len {
            self.status_message = format!(
                "preview frame dropped: rgba size mismatch (expected {}, got {})",
                expected_rgba_len,
                frame.rgba.len()
            );
            self.displayed_timestamp_us = Some(frame.timestamp_us);
            return;
        }
        let image = ColorImage::from_rgba_unmultiplied([frame.width, frame.height], &frame.rgba);
        match &mut self.texture {
            Some(texture) if texture.size() == [frame.width, frame.height] => {
                texture.set(image, egui::TextureOptions::LINEAR);
            }
            _ => {
                self.texture = Some(ui.ctx().load_texture(
                    "camera-preview-recorder",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        self.displayed_timestamp_us = Some(frame.timestamp_us);
        self.total_rendered = self.total_rendered.saturating_add(1);
    }

    fn poll_capture_events(&mut self) {
        while let Ok(event) = self.capture_event_rx.try_recv() {
            match event {
                CaptureEvent::Reconfigured(config) => {
                    self.width = config.width;
                    self.height = config.height;
                    self.fps = config.fps;
                    self.pending_width = config.width;
                    self.pending_height = config.height;
                    self.pending_fps = config.fps;
                    self.latest = None;
                    self.texture = None;
                    self.displayed_timestamp_us = None;
                    self.refresh_backend_probe_statuses();
                    self.status_message = format!(
                        "capture reconfigured: {}x{} @ {} fps",
                        config.width, config.height, config.fps
                    );
                }
                CaptureEvent::Error(message) => {
                    self.status_message = format!("capture error: {message}");
                }
            }
        }
    }

    fn apply_capture_settings(&mut self) {
        if self.pending_width <= 0 || self.pending_height <= 0 || self.pending_fps <= 0 {
            self.status_message = "capture settings must be positive".to_string();
            return;
        }
        if self.recording.is_some() {
            self.stop_recording();
        }

        let config = CaptureConfigState {
            device_id: self.video_device_id.clone(),
            width: self.pending_width,
            height: self.pending_height,
            fps: self.pending_fps,
        };
        if let Err(err) = self
            .capture_cmd_tx
            .send(CaptureCommand::Reconfigure(config.clone()))
        {
            self.status_message = format!("failed to send capture reconfigure command: {err}");
        } else {
            self.status_message = format!(
                "reconfiguring capture to {}x{} @ {} fps...",
                config.width, config.height, config.fps
            );
        }
    }

    fn apply_fragment_settings(&mut self) {
        self.fragment_frames = self.pending_fragment_frames.max(1);
        self.recording_settings.fragment_frames = self.fragment_frames;
        if let Some(recorder) = self.recording.as_mut() {
            if let Err(err) = recorder.set_fragment_frames(self.fragment_frames) {
                self.status_message = format!("failed to apply fragment settings: {err:#}");
                return;
            }
            self.status_message = format!(
                "fragment settings queued: {} frame(s)/fragment",
                self.fragment_frames
            );
        } else {
            self.status_message = format!(
                "fragment settings applied: {} frame(s)/fragment",
                self.fragment_frames
            );
        }
    }

    fn start_recording(&mut self) {
        if self.recording.is_some() {
            return;
        }
        if let Some(status) = self.selected_backend_probe()
            && !status.available
        {
            self.status_message = format!("record start blocked: {}", status.detail);
            return;
        }

        self.recording_seq = self.recording_seq.saturating_add(1);
        let output_path = match next_recording_path(&self.output_dir, self.recording_seq) {
            Ok(path) => path,
            Err(err) => {
                self.status_message = format!("record start failed: {err:#}");
                return;
            }
        };

        let settings = self.current_recording_settings();

        match RecorderWorker::spawn(
            settings,
            self.width,
            self.height,
            self.fps,
            output_path.clone(),
        ) {
            Ok(worker) => {
                self.started_at = None;
                self.recording_submitted_frames = 0;
                self.recording_queue_depth = 0;
                self.status_message = "recording ON: worker started".to_string();
                self.recording = Some(worker);
            }
            Err(err) => {
                self.status_message = format!("record start failed: {err:#}");
            }
        }
    }

    fn stop_recording(&mut self) {
        if let Some(mut recorder) = self.recording.take() {
            self.recording_queue_depth = 0;
            self.recording_submitted_frames = 0;
            match recorder.finish() {
                Ok(summary) => {
                    self.status_message = format!(
                        "recording OFF: {} (segments={}, packets={}, flush_packets={}, bytes={}, duration_s={:.3})",
                        summary.output_path.display(),
                        summary.segments_written,
                        summary.packets_seen,
                        summary.flush_packets,
                        summary.bytes_written,
                        duration_90k_to_seconds(summary.duration_90k)
                    );
                }
                Err(err) => {
                    self.status_message = format!("record stop failed: {err:#}");
                }
            }
            self.refresh_backend_probe_statuses();
        }
    }

    fn handle_recording_worker_error(&mut self, message: String) {
        self.recording_queue_depth = 0;
        self.recording_submitted_frames = 0;
        let join_result = if let Some(mut recorder) = self.recording.take() {
            let result = recorder.join_only();
            self.refresh_backend_probe_statuses();
            result
        } else {
            Ok(())
        };
        self.status_message = match join_result {
            Ok(()) => format!("recording worker error: {message}"),
            Err(err) => format!("recording worker error: {message}; join failed: {err:#}"),
        };
    }

    fn poll_recording_events(&mut self) {
        let mut worker_error = None::<String>;
        if let Some(recorder) = self.recording.as_mut() {
            while let Some(event) = recorder.try_recv_event() {
                match event {
                    RecorderWorkerEvent::FrameConsumed => {
                        self.recording_queue_depth = self.recording_queue_depth.saturating_sub(1);
                    }
                    RecorderWorkerEvent::Status(message) => {
                        self.status_message = message;
                    }
                    RecorderWorkerEvent::Error(message) => {
                        worker_error = Some(message);
                        break;
                    }
                }
            }
        }

        if let Some(message) = worker_error {
            self.handle_recording_worker_error(message);
        }
    }

    fn show_recording_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(format!(
                "recording: {}",
                if self.recording.is_some() {
                    "ON"
                } else {
                    "OFF"
                }
            ));
            if self.recording.is_some() {
                if ui.button("Stop Recording").clicked() {
                    self.stop_recording();
                }
            } else if ui.button("Start Recording").clicked() {
                self.start_recording();
            }
        });
        if let Some(status) = self.selected_backend_probe()
            && !status.available
            && self.recording.is_none()
        {
            ui.label(format!("start preflight warning: {}", status.detail));
        }
        ui.label(format!("status: {}", self.status_message));
    }

    fn show_controls_scroll_contents(&mut self, ui: &mut egui::Ui) {
        ui.heading("Camera Controls");
        ui.label(format!(
            "capture: {}x{} @ {} fps",
            self.width, self.height, self.fps
        ));
        ui.label(format!(
            "frames: received={} rendered={}",
            self.total_received, self.total_rendered
        ));
        if self.recording.is_some() {
            ui.label(format!(
                "record queue: pending={}",
                self.recording_queue_depth
            ));
        }

        if let Some(latest) = &self.latest {
            ui.label(format!("timestamp_us: {}", latest.timestamp_us));
        } else {
            ui.label("waiting for frames...");
        }

        let previous_backend = self.selected_backend;
        let previous_codec = self.selected_codec;
        ui.add_enabled_ui(self.recording.is_none(), |ui| {
            ui.horizontal(|ui| {
                ui.label("backend:");
                egui::ComboBox::from_id_salt("record-backend")
                    .selected_text(self.selected_backend.to_string())
                    .show_ui(ui, |ui| {
                        for backend in Backend::supported() {
                            ui.selectable_value(
                                &mut self.selected_backend,
                                backend,
                                backend.to_string(),
                            );
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("codec:");
                egui::ComboBox::from_id_salt("record-codec")
                    .selected_text(match self.selected_codec {
                        Codec::H264 => "h264",
                        Codec::Hevc => "hevc",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.selected_codec, Codec::H264, "h264");
                        ui.selectable_value(&mut self.selected_codec, Codec::Hevc, "hevc");
                    });
            });
        });
        if previous_backend != self.selected_backend || previous_codec != self.selected_codec {
            self.refresh_backend_probe_statuses();
        }

        egui::CollapsingHeader::new("backend availability")
            .default_open(true)
            .show(ui, |ui| {
                for status in &self.backend_probe_statuses {
                    let availability = if status.available {
                        "available"
                    } else {
                        "unavailable"
                    };
                    let selected_mark = if status.backend == self.selected_backend {
                        "*"
                    } else {
                        " "
                    };
                    ui.label(format!(
                        "{selected_mark} {}: {} ({})",
                        status.backend, availability, status.detail
                    ));
                }
            });

        ui.horizontal(|ui| {
            ui.label("capture settings:");
            ui.add(egui::DragValue::new(&mut self.pending_width).range(160..=3840));
            ui.label("x");
            ui.add(egui::DragValue::new(&mut self.pending_height).range(120..=2160));
            ui.label("@");
            ui.add(egui::DragValue::new(&mut self.pending_fps).range(1..=120));
            ui.label("fps");
            if ui.button("Apply Capture").clicked() {
                self.apply_capture_settings();
            }
        });

        ui.horizontal(|ui| {
            ui.label("fragment frequency:");
            ui.add(egui::DragValue::new(&mut self.pending_fragment_frames).range(1..=300));
            ui.label("frame(s)/fragment");
            if ui.button("Apply Fragment").clicked() {
                self.apply_fragment_settings();
            }
        });
    }
}

impl Drop for CameraRecordApp {
    fn drop(&mut self) {
        if let Some(mut recorder) = self.recording.take()
            && let Err(err) = recorder.finish()
        {
            eprintln!("failed to finalize recording on drop: {err:#}");
        }
    }
}

impl eframe::App for CameraRecordApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(16));
        self.poll_capture_events();
        self.poll_recording_events();

        if let Some(duration) = self.duration
            && let Some(started_at) = self.started_at
            && self.recording.is_some()
            && started_at.elapsed() >= duration
        {
            self.stop_recording();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        const MAX_FRAMES_PER_TICK: usize = 8;
        for _ in 0..MAX_FRAMES_PER_TICK {
            let Ok(captured) = self.rx.try_recv() else {
                break;
            };
            if let Err(err) = self.handle_owned_frame(captured) {
                self.status_message = format!("recording error: {err:#}");
                self.stop_recording();
                break;
            }
        }

        if let Some(duration) = self.duration
            && let Some(started_at) = self.started_at
            && self.recording.is_some()
            && started_at.elapsed() >= duration
        {
            self.stop_recording();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        egui::SidePanel::left("camera-controls")
            .resizable(true)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let toggle_label = if self.controls_collapsed {
                        ">> expand controls"
                    } else {
                        "<< collapse controls"
                    };
                    if ui.button(toggle_label).clicked() {
                        self.controls_collapsed = !self.controls_collapsed;
                    }
                });
                self.show_recording_toolbar(ui);
                if !self.controls_collapsed {
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt("camera-controls-scroll")
                        .show(ui, |ui| {
                            self.show_controls_scroll_contents(ui);
                        });
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Camera Preview");
            self.update_texture_if_needed(ui);
            if let Some(texture) = &self.texture {
                ui.add(
                    egui::Image::from_texture(egui::load::SizedTexture::from_handle(texture))
                        .shrink_to_fit(),
                );
            } else {
                ui.label("waiting for frames...");
            }
        });
    }
}

fn main() -> Result<()> {
    let args = parse_args()?;

    if args.list_devices {
        list_devices();
        return Ok(());
    }

    if args.auto_start_recording {
        let mut preflight_settings = args.recording_settings.clone();
        preflight_settings.fragment_frames = args.fragment_frames;
        probe_recording_backend_path(&preflight_settings, args.width, args.height, args.fps)
            .context("auto-start preflight failed")?;
    }

    let (tx, rx) = mpsc::channel::<CapturedFrame>();
    let (capture_cmd_tx, capture_cmd_rx) = mpsc::channel::<CaptureCommand>();
    let (capture_event_tx, capture_event_rx) = mpsc::channel::<CaptureEvent>();
    let (capture_ready_tx, capture_ready_rx) = mpsc::channel::<Result<()>>();
    let initial_capture_config = CaptureConfigState {
        device_id: args.video_device_id.clone(),
        width: args.width,
        height: args.height,
        fps: args.fps,
    };
    let capture_thread = thread::Builder::new()
        .name("camera-capture-thread".to_string())
        .spawn(move || {
            let mut current_config = initial_capture_config;
            let start_capture = |config: &CaptureConfigState| -> Result<VideoCapture> {
                let frame_tx = tx.clone();
                let mut capture = VideoCapture::new(
                    VideoCaptureConfig {
                        device_id: config.device_id.clone(),
                        width: config.width,
                        height: config.height,
                        fps: config.fps,
                        pixel_format: None,
                    },
                    move |frame: VideoFrame<'_>| {
                        let _ = frame_tx.send(CapturedFrame {
                            frame: frame.to_owned(),
                        });
                    },
                )
                .map_err(|err| anyhow::anyhow!("failed to create video capture: {err}"))?;
                capture
                    .start()
                    .map_err(|err| anyhow::anyhow!("failed to start video capture: {err}"))?;
                Ok(capture)
            };

            let mut capture = match start_capture(&current_config) {
                Ok(capture) => capture,
                Err(err) => {
                    let _ = capture_ready_tx.send(Err(err));
                    return;
                }
            };
            let _ = capture_ready_tx.send(Ok(()));
            let _ = capture_event_tx.send(CaptureEvent::Reconfigured(current_config.clone()));

            while let Ok(command) = capture_cmd_rx.recv() {
                match command {
                    CaptureCommand::Reconfigure(new_config) => {
                        capture.stop();
                        match start_capture(&new_config) {
                            Ok(new_capture) => {
                                capture = new_capture;
                                current_config = new_config.clone();
                                let _ =
                                    capture_event_tx.send(CaptureEvent::Reconfigured(new_config));
                            }
                            Err(err) => {
                                let _ = capture_event_tx.send(CaptureEvent::Error(format!(
                                    "capture reconfigure failed: {err:#}"
                                )));
                                match start_capture(&current_config) {
                                    Ok(restore_capture) => {
                                        capture = restore_capture;
                                        let _ = capture_event_tx.send(CaptureEvent::Reconfigured(
                                            current_config.clone(),
                                        ));
                                    }
                                    Err(restore_err) => {
                                        let _ = capture_event_tx.send(CaptureEvent::Error(
                                            format!("capture restore failed: {restore_err:#}"),
                                        ));
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    CaptureCommand::Stop => {
                        capture.stop();
                        break;
                    }
                }
            }
        })
        .context("failed to spawn camera capture thread")?;

    capture_ready_rx
        .recv()
        .context("camera capture thread exited before startup status")??;

    println!(
        "camera preview started: {}x{} @ {}",
        args.width, args.height, args.fps
    );
    println!("recording output dir: {}", args.output_dir.display());
    println!(
        "initial recording backend: {}",
        args.recording_settings.backend
    );
    println!("initial recording codec: {}", args.recording_settings.codec);
    println!(
        "initial fragment frequency: {} frame(s)/fragment",
        args.fragment_frames
    );
    if args.auto_start_recording {
        println!("auto-start recording: enabled");
    }
    println!("backend/codec/resolution/fragment frequency can be changed in GUI");
    println!("toggle recording with Start/Stop buttons");

    let mut options = eframe::NativeOptions::default();
    if cfg!(target_os = "macos") {
        options.run_and_return = false;
    }
    let app_capture_cmd_tx = capture_cmd_tx.clone();
    let app_video_device_id = args.video_device_id.clone();
    let app = move |_cc: &eframe::CreationContext<'_>| {
        Ok(Box::new(CameraRecordApp::new(
            rx,
            app_capture_cmd_tx,
            capture_event_rx,
            CameraAppInit {
                video_device_id: app_video_device_id,
                width: args.width,
                height: args.height,
                fps: args.fps,
                auto_start_recording: args.auto_start_recording,
                fragment_frames: args.fragment_frames,
                duration: args.duration,
                output_dir: args.output_dir.clone(),
                recording_settings: args.recording_settings.clone(),
            },
        )) as Box<dyn eframe::App>)
    };

    if let Err(err) = eframe::run_native("Camera fMP4 Recorder", options, Box::new(app)) {
        eprintln!("eframe failed: {err}");
    }

    let _ = capture_cmd_tx.send(CaptureCommand::Stop);
    if let Err(err) = capture_thread.join() {
        eprintln!("capture thread join failed: {err:?}");
    }
    println!("camera preview stopped");
    Ok(())
}

fn parse_args() -> Result<Args> {
    let cli = CliArgs::parse();
    let fragment_frames = usize::try_from(cli.fragment_frames).unwrap_or(1).max(1);
    let (width, height) = parse_resolution(&cli.resolution)
        .with_context(|| format!("invalid resolution: {}", cli.resolution))?;
    let recording_settings = if cli.list_devices {
        RecordingSettings {
            backend: Backend::Auto,
            codec: Codec::H264,
            require_hardware: cli.require_hardware,
            intel_force_software: cli.intel_force_software,
            fragment_frames,
        }
    } else {
        RecordingSettings {
            backend: cli.backend.parse()?,
            codec: parse_codec(&cli.codec)?,
            require_hardware: cli.require_hardware,
            intel_force_software: cli.intel_force_software,
            fragment_frames,
        }
    };

    Ok(Args {
        list_devices: cli.list_devices,
        video_device_id: cli.video_device_id,
        width,
        height,
        fps: cli.fps,
        auto_start_recording: cli.auto_start_recording,
        fragment_frames,
        duration: cli.duration.map(Duration::from_secs_f64),
        output_dir: cli.output_dir,
        recording_settings,
    })
}

fn parse_resolution(s: &str) -> Option<(i32, i32)> {
    match s.to_lowercase().as_str() {
        "4k" | "2160p" => Some((3840, 2160)),
        "1080p" => Some((1920, 1080)),
        "720p" => Some((1280, 720)),
        "540p" => Some((960, 540)),
        _ => {
            let parts: Vec<&str> = s.split('x').collect();
            if parts.len() == 2 {
                let w = parts[0].parse().ok()?;
                let h = parts[1].parse().ok()?;
                Some((w, h))
            } else {
                None
            }
        }
    }
}

fn parse_codec(raw: &str) -> Result<Codec> {
    match raw.to_ascii_lowercase().as_str() {
        "h264" => Ok(Codec::H264),
        "hevc" | "h265" => Ok(Codec::Hevc),
        other => anyhow::bail!("unsupported codec: {other}"),
    }
}

fn list_devices() {
    println!("=== video devices ===");
    match VideoDeviceList::enumerate() {
        Ok(devices) => {
            if devices.is_empty() {
                println!("no video devices found");
            } else {
                for device in devices.devices() {
                    let name = device.name().unwrap_or_else(|_| "Unknown".to_string());
                    let id = device.unique_id().unwrap_or_else(|_| "Unknown".to_string());
                    println!("{name}");
                    println!("  id: {id}");
                    for fmt in device.formats() {
                        println!(
                            "  {}x{} @ {:.0}-{:.0} fps ({})",
                            fmt.width,
                            fmt.height,
                            fmt.min_fps,
                            fmt.max_fps,
                            fmt.pixel_format.name()
                        );
                    }
                }
            }
        }
        Err(err) => eprintln!("failed to enumerate video devices: {err:?}"),
    }
}

fn next_recording_path(output_dir: &PathBuf, seq: u64) -> Result<PathBuf> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    Ok(output_dir.join(format!("camera-recording-{epoch_ms}-{seq:03}.mp4")))
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
    // Conservative baseline values. The VPS/SPS/PPS arrays carry decoder-critical details.
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

fn annexb_chunk_to_avcc_sample(
    annexb: &[u8],
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus: Vec<&[u8]> = Vec::new();
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

    let mut avcc = Vec::new();
    for nalu in sample_nalus {
        let len = u32::try_from(nalu.len()).context("NAL too large for AVCC")?;
        avcc.extend_from_slice(&len.to_be_bytes());
        avcc.extend_from_slice(nalu);
    }
    Ok(Some(avcc))
}

fn annexb_chunk_to_hvcc_sample(
    annexb: &[u8],
    vps_out: &mut Option<Vec<u8>>,
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus: Vec<&[u8]> = Vec::new();
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

    let mut hvcc = Vec::new();
    for nalu in sample_nalus {
        let len = u32::try_from(nalu.len()).context("NAL too large for hvcC sample")?;
        hvcc.extend_from_slice(&len.to_be_bytes());
        hvcc.extend_from_slice(nalu);
    }
    Ok(Some(hvcc))
}

fn avcc_chunk_to_avcc_sample(
    avcc: &[u8],
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus: Vec<&[u8]> = Vec::new();
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

    Ok(Some(repack_length_prefixed_nalus(&sample_nalus)?))
}

fn hvcc_chunk_to_hvcc_sample(
    hvcc: &[u8],
    vps_out: &mut Option<Vec<u8>>,
    sps_out: &mut Option<Vec<u8>>,
    pps_out: &mut Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>> {
    let mut sample_nalus: Vec<&[u8]> = Vec::new();
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

    Ok(Some(repack_length_prefixed_nalus(&sample_nalus)?))
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
            .context("length-prefixed sample is truncated before NAL length")?;
        let nalu_len =
            u32::from_be_bytes(len_bytes.try_into().expect("length slice size")) as usize;
        cursor = cursor.saturating_add(4);
        let nalu = data
            .get(cursor..cursor + nalu_len)
            .context("length-prefixed sample is truncated inside NAL payload")?;
        nalus.push(nalu);
        cursor = cursor.saturating_add(nalu_len);
    }
    Ok(nalus)
}

fn repack_length_prefixed_nalus(nalus: &[&[u8]]) -> Result<Vec<u8>> {
    let mut sample = Vec::new();
    for nalu in nalus {
        let len = u32::try_from(nalu.len()).context("NAL too large for length-prefixed sample")?;
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

fn fps_frame_duration_90k(fps: i32) -> u64 {
    90_000 / u64::try_from(fps.max(1)).unwrap_or(1)
}

fn duration_90k_to_seconds(duration_90k: u64) -> f64 {
    duration_90k as f64 / 90_000.0
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

fn patch_fmp4_init_segment_timing(
    init_segment: &[u8],
    creation_timestamp: Duration,
    duration_in_input_timescale: u64,
    input_timescale: NonZeroU32,
) -> Result<Vec<u8>> {
    let (ftyp_box, ftyp_size) =
        FtypBox::decode(init_segment).context("failed to decode ftyp from init segment")?;
    let moov_input = init_segment
        .get(ftyp_size..)
        .context("init segment is missing moov payload")?;
    let (mut moov_box, moov_size) =
        MoovBox::decode(moov_input).context("failed to decode moov from init segment")?;
    let creation_time = Mp4FileTime::from_unix_time(creation_timestamp);

    let movie_duration = convert_duration_timescale(
        duration_in_input_timescale,
        input_timescale,
        moov_box.mvhd_box.timescale,
    );
    moov_box.mvhd_box.creation_time = creation_time;
    moov_box.mvhd_box.modification_time = creation_time;
    // For fragmented MP4, QuickTime/Finder showed about double duration if both the header boxes
    // and `mehd` carried the finalized length. Keep the real duration only in `mehd`.
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

    let mut patched = ftyp_box
        .encode_to_vec()
        .context("failed to encode ftyp for patched init segment")?;
    patched.extend_from_slice(
        &moov_box
            .encode_to_vec()
            .context("failed to encode moov for patched init segment")?,
    );

    let consumed = ftyp_size.saturating_add(moov_size);
    if consumed < init_segment.len() {
        patched.extend_from_slice(&init_segment[consumed..]);
    }
    Ok(patched)
}

fn rgba_to_argb_bytes(rgba: &[u8]) -> Vec<u8> {
    let mut argb = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        argb.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
    }
    argb
}

fn strip_stride<'a>(
    data: &'a [u8],
    row_bytes: usize,
    height: usize,
    stride: usize,
) -> Cow<'a, [u8]> {
    if stride == row_bytes {
        Cow::Borrowed(&data[..row_bytes * height])
    } else {
        let mut result = Vec::with_capacity(row_bytes * height);
        for row in 0..height {
            let start = row * stride;
            result.extend_from_slice(&data[start..start + row_bytes]);
        }
        Cow::Owned(result)
    }
}

fn frame_to_rgba(frame: &VideoFrame<'_>) -> Option<UiFrame> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let stride = frame.stride as usize;
    let stride_uv = frame.stride_uv as usize;

    let rgba = match frame.pixel_format {
        PixelFormat::Nv12 => {
            let y = strip_stride(frame.data, w, h, stride);
            let uv_data = frame.uv_data?;
            let uv = strip_stride(uv_data, w, h / 2, stride_uv);
            nv12_to_rgba(&y, &uv, w, h)
        }
        PixelFormat::I420 => {
            let y = strip_stride(frame.data, w, h, stride);
            let uv_data = frame.uv_data?;
            let uv_w = w / 2;
            let uv_h = h / 2;
            let u_plane_size = stride_uv * uv_h;
            if uv_data.len() < u_plane_size * 2 {
                return None;
            }
            let u = strip_stride(&uv_data[..u_plane_size], uv_w, uv_h, stride_uv);
            let v = strip_stride(
                &uv_data[u_plane_size..u_plane_size * 2],
                uv_w,
                uv_h,
                stride_uv,
            );
            i420_to_rgba(&y, &u, &v, w, h)
        }
        PixelFormat::Yuy2 => {
            let data = strip_stride(frame.data, w * 2, h, stride);
            yuy2_to_rgba(&data, w, h)
        }
        PixelFormat::Unknown(_) => {
            use std::sync::Once;
            static WARN: Once = Once::new();
            WARN.call_once(|| {
                eprintln!(
                    "warning: unsupported pixel format for preview/recording: {}",
                    frame.pixel_format.name()
                );
            });
            return None;
        }
    };

    Some(UiFrame {
        width: w,
        height: h,
        timestamp_us: frame.timestamp_us,
        rgba,
    })
}

fn nv12_to_rgba(y_plane: &[u8], uv_plane: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let y_row = row * w;
        let uv_row = (row / 2) * w;
        for col in 0..w {
            let y = y_plane[y_row + col];
            let uv_index = uv_row + (col / 2) * 2;
            let u = uv_plane[uv_index];
            let v = uv_plane[uv_index + 1];
            let (r, g, b) = yuv_to_rgb(y, u, v);
            out.extend_from_slice(&[r, g, b, 255]);
        }
    }
    out
}

fn i420_to_rgba(y_plane: &[u8], u_plane: &[u8], v_plane: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    let uv_w = w / 2;
    for row in 0..h {
        let y_row = row * w;
        let uv_row = (row / 2) * uv_w;
        for col in 0..w {
            let y = y_plane[y_row + col];
            let uv_index = uv_row + (col / 2);
            let u = u_plane[uv_index];
            let v = v_plane[uv_index];
            let (r, g, b) = yuv_to_rgb(y, u, v);
            out.extend_from_slice(&[r, g, b, 255]);
        }
    }
    out
}

fn yuy2_to_rgba(data: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let row_start = row * w * 2;
        let row_data = &data[row_start..row_start + w * 2];
        for chunk in row_data.chunks_exact(4) {
            let y0 = chunk[0];
            let u = chunk[1];
            let y1 = chunk[2];
            let v = chunk[3];
            let (r0, g0, b0) = yuv_to_rgb(y0, u, v);
            let (r1, g1, b1) = yuv_to_rgb(y1, u, v);
            out.extend_from_slice(&[r0, g0, b0, 255]);
            out.extend_from_slice(&[r1, g1, b1, 255]);
        }
    }
    out
}

fn yuv_to_rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let c = (y as i32 - 16).max(0);
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    let r = (298 * c + 409 * e + 128) >> 8;
    let g = (298 * c - 100 * d - 208 * e + 128) >> 8;
    let b = (298 * c + 516 * d + 128) >> 8;
    (clamp_u8(r), clamp_u8(g), clamp_u8(b))
}

fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
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

fn dims(width: u32, height: u32) -> Result<Dimensions> {
    let width = NonZeroU32::new(width).context("width must be > 0")?;
    let height = NonZeroU32::new(height).context("height must be > 0")?;
    Ok(Dimensions { width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h264_sample_entry_sets_avcc_extended_fields() {
        let sample_entry = create_h264_sample_entry(
            1280,
            720,
            &[0x67, 0x64, 0x00, 0x1f],
            &[0x68, 0xee, 0x3c, 0x80],
        );

        let SampleEntry::Avc1(avc1) = sample_entry else {
            panic!("expected avc1 sample entry");
        };

        assert!(avc1.avcc_box.chroma_format.is_some());
        assert!(avc1.avcc_box.bit_depth_luma_minus8.is_some());
        assert!(avc1.avcc_box.bit_depth_chroma_minus8.is_some());
    }

    #[test]
    fn h264_sample_entry_allows_init_segment_build() {
        let sample_entry = create_h264_sample_entry(
            1280,
            720,
            &[0x67, 0x64, 0x00, 0x1f],
            &[0x68, 0xee, 0x3c, 0x80],
        );
        let mut muxer = Fmp4SegmentMuxer::new().expect("muxer should initialize");
        let sample = Sample {
            track_kind: TrackKind::Video,
            sample_entry: Some(sample_entry),
            keyframe: true,
            timescale: NonZeroU32::new(90_000).expect("constant timescale is non-zero"),
            duration: 3_000,
            composition_time_offset: None,
            data_offset: 0,
            data_size: 4,
        };
        muxer
            .create_media_segment_metadata(&[sample])
            .expect("sample metadata should initialize track");

        let init = muxer
            .init_segment_bytes()
            .expect("init segment should be buildable");

        assert!(!init.is_empty());
    }

    #[test]
    fn patch_fmp4_init_segment_timing_sets_creation_and_duration() {
        let sample_entry = create_h264_sample_entry(
            1280,
            720,
            &[0x67, 0x64, 0x00, 0x1f],
            &[0x68, 0xee, 0x3c, 0x80],
        );
        let mut muxer = Fmp4SegmentMuxer::new().expect("muxer should initialize");
        let first_sample = Sample {
            track_kind: TrackKind::Video,
            sample_entry: Some(sample_entry),
            keyframe: true,
            timescale: NonZeroU32::new(90_000).expect("constant timescale is non-zero"),
            duration: 3_000,
            composition_time_offset: None,
            data_offset: 0,
            data_size: 4,
        };
        let second_sample = Sample {
            sample_entry: None,
            data_offset: 4,
            ..first_sample.clone()
        };
        muxer
            .create_media_segment_metadata(&[first_sample, second_sample])
            .expect("sample metadata should initialize track");
        let init = muxer
            .init_segment_bytes()
            .expect("init segment should be buildable");

        let creation_timestamp = Duration::from_secs(1_700_000_000);
        let patched = patch_fmp4_init_segment_timing(
            &init,
            creation_timestamp,
            6_000,
            NonZeroU32::new(90_000).expect("constant timescale is non-zero"),
        )
        .expect("init patch should succeed");

        let (_, ftyp_size) = FtypBox::decode(&patched).expect("ftyp should decode");
        let (moov, _) = MoovBox::decode(&patched[ftyp_size..]).expect("moov should decode");
        let expected_creation = Mp4FileTime::from_unix_time(creation_timestamp).as_secs();

        assert_eq!(moov.mvhd_box.creation_time.as_secs(), expected_creation);
        assert_eq!(moov.mvhd_box.modification_time.as_secs(), expected_creation);
        assert_eq!(moov.mvhd_box.duration, 0);

        for trak in &moov.trak_boxes {
            assert_eq!(trak.tkhd_box.creation_time.as_secs(), expected_creation);
            assert_eq!(trak.tkhd_box.modification_time.as_secs(), expected_creation);
            assert_eq!(trak.tkhd_box.duration, 0);
            assert_eq!(
                trak.mdia_box.mdhd_box.creation_time.as_secs(),
                expected_creation
            );
            assert_eq!(
                trak.mdia_box.mdhd_box.modification_time.as_secs(),
                expected_creation
            );
            assert_eq!(trak.mdia_box.mdhd_box.duration, 0);
        }

        let mehd_duration = moov
            .mvex_box
            .and_then(|mvex| mvex.mehd_box)
            .map(|mehd| mehd.fragment_duration)
            .expect("mehd should exist");
        assert_eq!(
            mehd_duration,
            convert_duration_timescale(
                6_000,
                NonZeroU32::new(90_000).expect("constant timescale is non-zero"),
                moov.mvhd_box.timescale,
            )
        );
    }
}
