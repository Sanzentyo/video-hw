//! Encoder session, fMP4 mux state, and asynchronous file-writer ownership.

use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{BufWriter, Seek, SeekFrom, Write},
    num::NonZeroU32,
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use shiguredo_mp4::{
    TrackKind,
    boxes::SampleEntry,
    mux::{Fmp4SegmentMuxer, Sample, SegmentMuxerOptions},
};
use video_hw::{
    AnyEncodeSession, BackendEncoderOptions, BackendKind, Codec, Dimensions, EncodeFrame,
    EncodeInputFormat, EncodedChunk, EncodedLayout, EncoderConfig, IntelEncoderOptions,
    RawFrameBuffer, Timestamp90k,
};

use super::{
    cli::RecordingSettings,
    codec_samples::{
        annexb_chunk_to_avcc_sample, annexb_chunk_to_hvcc_sample, avcc_chunk_to_avcc_sample,
        create_h264_sample_entry, create_hevc_sample_entry, hvcc_chunk_to_hvcc_sample,
        patch_fmp4_init_segment_timing,
    },
    pixel::{backend_is_intel, dims, rgba_to_argb_bytes},
};

#[derive(Debug)]
struct PendingSample {
    keyframe: bool,
    pts_90k: i64,
    data: Vec<u8>,
}

#[derive(Debug)]
pub(super) struct RecorderSummary {
    pub(super) output_path: PathBuf,
    pub(super) segments_written: u64,
    pub(super) packets_seen: u64,
    pub(super) flush_packets: u64,
    pub(super) bytes_written: u64,
    pub(super) duration_90k: u64,
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

    pub(super) fn finish(&mut self) -> Result<u64> {
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

pub(super) struct RecorderState {
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

pub(super) fn resolve_recording_backend_and_config(
    settings: &RecordingSettings,
    fps: i32,
) -> Result<(BackendKind, EncoderConfig)> {
    let mut config = EncoderConfig::new(
        settings.codec,
        fps,
        settings.require_hardware,
        EncodeInputFormat::Argb8888,
    );
    let resolved_backend = settings.backend.resolve_encoder(&config).with_context(|| {
        format!(
            "failed to resolve encoder backend (backend={}, codec={}, require_hardware={})",
            settings.backend, settings.codec, settings.require_hardware
        )
    })?;
    if backend_is_intel(resolved_backend) {
        config.backend_options = BackendEncoderOptions::Intel(IntelEncoderOptions {
            force_software: settings.intel_force_software,
            hevc_use_vpp: if settings.codec == Codec::Hevc {
                Some(true)
            } else {
                None
            },
            ..Default::default()
        });
    }
    Ok((resolved_backend, config))
}

pub(super) fn probe_recording_backend_path(
    settings: &RecordingSettings,
    width: i32,
    height: i32,
    fps: i32,
) -> Result<BackendKind> {
    let width_u32 = u32::try_from(width).context("width must be >= 0")?;
    let height_u32 = u32::try_from(height).context("height must be >= 0")?;
    let dims = dims(width_u32, height_u32)?;
    let (resolved_backend, config) = resolve_recording_backend_and_config(settings, fps)?;
    let mut session =
        AnyEncodeSession::with_backend_kind(resolved_backend, config).with_context(|| {
            format!("failed to create encoder session (resolved_backend={resolved_backend})")
        })?;
    let pixel_count = usize::try_from(width_u32)
        .ok()
        .and_then(|w| {
            usize::try_from(height_u32)
                .ok()
                .and_then(|h| w.checked_mul(h))
        })
        .context("failed to compute preflight frame pixel count")?;
    let frame_len = pixel_count
        .checked_mul(4)
        .context("failed to compute preflight argb frame size")?;
    let preflight_frame = vec![0_u8; frame_len];
    let mut preflight_produced_chunk = false;
    let preflight_pts_step = i64::from((90_000 / fps.max(1)).max(1));
    let preflight_frame_count: i64 =
        if settings.codec == Codec::Hevc && backend_is_intel(resolved_backend) {
            30
        } else {
            6
        };
    for frame_index in 0..preflight_frame_count {
        session
            .submit(EncodeFrame {
                dims,
                pts_90k: Some(Timestamp90k(preflight_pts_step.saturating_mul(frame_index))),
                buffer: RawFrameBuffer::Argb8888(preflight_frame.clone()),
                force_keyframe: frame_index == 0,
            })
            .with_context(|| format!("encoder preflight submit failed at frame={frame_index}"))?;
        while let Some(_chunk) = session
            .try_reap()
            .context("encoder preflight try_reap failed")?
        {
            preflight_produced_chunk = true;
        }
    }
    for _chunk in session.flush().context("encoder preflight flush failed")? {
        preflight_produced_chunk = true;
    }
    if !preflight_produced_chunk {
        anyhow::bail!(
            "encoder preflight produced no chunks (backend={resolved_backend}, codec={}, frames={preflight_frame_count})",
            settings.codec,
        );
    }
    Ok(resolved_backend)
}

impl RecorderState {
    pub(super) fn new(
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

    pub(super) fn submit_rgba_frame(&mut self, frame_rgba: Vec<u8>, pts_90k: i64) -> Result<()> {
        self.submit_argb_frame(rgba_to_argb_bytes(&frame_rgba), pts_90k)
    }

    fn submit_argb_frame(&mut self, frame_argb: Vec<u8>, pts_90k: i64) -> Result<()> {
        let dims = self.dims;
        let fragment_interval = self.fragment_frames.max(1) as u64;
        let force_keyframe =
            self.pending_force_keyframe || self.submitted_frames.is_multiple_of(fragment_interval);
        let origin = self.first_input_pts_90k.unwrap_or(pts_90k);
        let mut normalized_pts_90k = pts_90k.saturating_sub(origin).max(0);
        if let Some(previous) = self.last_submitted_pts_90k
            && normalized_pts_90k <= previous
        {
            normalized_pts_90k = previous.saturating_add(i64::from(self.default_duration));
        }
        self.encoder
            .submit(EncodeFrame {
                dims,
                pts_90k: Some(Timestamp90k(normalized_pts_90k)),
                buffer: RawFrameBuffer::Argb8888(frame_argb),
                force_keyframe,
            })
            .context("encoder submit failed")?;
        self.first_input_pts_90k = Some(origin);
        self.last_submitted_pts_90k = Some(normalized_pts_90k);
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

    pub(super) fn finish(mut self) -> Result<RecorderSummary> {
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

    pub(super) fn progress_status(&self) -> String {
        format!(
            "recording ON: {} (segments={}, packets={}, bytes={}, fragment_frames={})",
            self.output_path.display(),
            self.segments_written,
            self.packets_seen,
            self.bytes_written,
            self.fragment_frames
        )
    }

    pub(super) fn segments_written(&self) -> u64 {
        self.segments_written
    }

    pub(super) fn packets_seen(&self) -> u64 {
        self.packets_seen
    }

    pub(super) fn set_fragment_frames(&mut self, fragment_frames: usize) -> Result<()> {
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
            Codec::Av1 => None,
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
