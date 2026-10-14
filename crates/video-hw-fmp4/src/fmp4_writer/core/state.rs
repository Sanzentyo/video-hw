//! Recorder state machine, fragment lifecycle, and initial parameter validation.

use super::*;

pub(super) struct RecordingSettings {
    pub(super) backend: Backend,
    pub(super) codec: Codec,
    pub(super) require_hardware: bool,
    pub(super) intel_force_software: bool,
    pub(super) fragment_frames: usize,
}

pub(super) struct RecorderState {
    pub(super) encoder: Option<BackendEncoderSession>,
    pub(super) muxer: Fmp4SegmentMuxer,
    pub(super) writer: RecorderFileWriter,
    pub(super) output_path: PathBuf,
    pub(super) creation_timestamp: Duration,
    pub(super) dims: Dimensions,
    pub(super) width_u16: u16,
    pub(super) height_u16: u16,
    pub(super) codec: Codec,
    pub(super) timescale: NonZeroU32,
    pub(super) default_duration: u32,
    pub(super) first_input_pts_90k: Option<i64>,
    pub(super) last_submitted_pts_90k: Option<i64>,
    pub(super) next_fallback_pts_90k: i64,
    pub(super) last_written_pts_90k: Option<i64>,
    pub(super) pending_submitted_pts_90k: VecDeque<i64>,
    pub(super) pending_force_keyframe: bool,
    pub(super) submitted_frames: u64,
    pub(super) vps: Option<Vec<u8>>,
    pub(super) sps: Option<Vec<u8>>,
    pub(super) pps: Option<Vec<u8>>,
    pub(super) av1_sequence_header: Option<Vec<u8>>,
    pub(super) av1_config: Option<Av1ConfigSummary>,
    pub(super) sample_entry: Option<SampleEntry>,
    pub(super) sample_entry_emitted: bool,
    pub(super) init_written: bool,
    pub(super) init_bytes_written: Option<usize>,
    pub(super) total_duration_90k: u64,
    pub(super) pending_samples: VecDeque<PendingSample>,
    pub(super) fragment_frames: usize,
    pub(super) segments_written: u64,
    pub(super) packets_seen: u64,
    pub(super) bytes_written: u64,
}

#[derive(Debug, Default)]
struct InitialParameterState {
    vps: Option<Vec<u8>>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    av1_sequence_header: Option<Vec<u8>>,
    av1_config: Option<Av1ConfigSummary>,
}

impl InitialParameterState {
    fn validated(codec: Codec, parameter_sets: Option<&ParameterSets>) -> Result<Self> {
        match (codec, parameter_sets) {
            (_, None) => Ok(Self::default()),
            (Codec::H264, Some(ParameterSets::H264(sets))) => {
                anyhow::ensure!(
                    sets.sps.len() <= 1 && sets.pps.len() <= 1,
                    "fMP4 writer requires at most one active H.264 SPS and PPS"
                );
                let sps = sets.sps.first().cloned();
                let pps = sets.pps.first().cloned();
                if let Some(sps) = sps.as_deref() {
                    parse_h264_sps_config(sps).context("invalid initial H.264 SPS")?;
                }
                if let Some(pps) = pps.as_deref() {
                    validate_h264_pps(pps).context("invalid initial H.264 PPS")?;
                }
                Ok(Self {
                    sps,
                    pps,
                    ..Self::default()
                })
            }
            (Codec::Hevc, Some(ParameterSets::Hevc(sets))) => {
                anyhow::ensure!(
                    sets.vps.len() <= 1 && sets.sps.len() <= 1 && sets.pps.len() <= 1,
                    "fMP4 writer requires at most one active HEVC VPS, SPS, and PPS"
                );
                let vps = sets.vps.first().cloned();
                let sps = sets.sps.first().cloned();
                let pps = sets.pps.first().cloned();
                if let Some(vps) = vps.as_deref() {
                    validate_hevc_parameter_set(vps, 32, "VPS")
                        .context("invalid initial HEVC VPS")?;
                }
                if let Some(sps) = sps.as_deref() {
                    parse_hevc_sps_config(sps).context("invalid initial HEVC SPS")?;
                }
                if let Some(pps) = pps.as_deref() {
                    validate_hevc_parameter_set(pps, 34, "PPS")
                        .context("invalid initial HEVC PPS")?;
                }
                Ok(Self {
                    vps,
                    sps,
                    pps,
                    ..Self::default()
                })
            }
            (Codec::Av1, Some(ParameterSets::Av1(sets))) => {
                let config_obus = sets.config_obus.concat();
                if config_obus.is_empty() {
                    return Ok(Self::default());
                }
                let (_, payload) = find_av1_sequence_header_obu(&config_obus)
                    .context("initial AV1 config has no sequence header OBU")?;
                let av1_config = parse_av1_sequence_header(payload)
                    .context("initial AV1 config has a malformed sequence header OBU")?;
                Ok(Self {
                    av1_sequence_header: Some(config_obus),
                    av1_config: Some(av1_config),
                    ..Self::default()
                })
            }
            (_, Some(parameter_sets)) => {
                bail!(
                    "initial parameter-set codec does not match writer codec: parameter_sets={}, writer={}",
                    parameter_sets.codec(),
                    codec
                );
            }
        }
    }
}

impl RecorderState {
    pub(super) fn new(config: &Fmp4WriterConfig) -> Result<Self> {
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
        let encoder = BackendEncoderSession::new(resolved_backend, encoder_config)?;
        let creation_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let muxer = Fmp4SegmentMuxer::with_options(SegmentMuxerOptions { creation_timestamp })
            .context("failed to create fMP4 muxer")?;
        let default_duration = u32::try_from((90_000 / fps.max(1)).max(1))
            .context("failed to compute default frame duration")?;
        if let Some(parent) = config.output_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create output dir: {}", parent.display()))?;
        }
        let writer = RecorderFileWriter::spawn(config.output_path.clone())?;
        Ok(Self {
            encoder: Some(encoder),
            muxer,
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

    pub(super) fn new_encoded(config: &EncodedTrackConfig) -> Result<Self> {
        let width_u32 = config.frame_size.width().get();
        let height_u32 = config.frame_size.height().get();
        let dims = Dimensions {
            width: config.frame_size.width(),
            height: config.frame_size.height(),
        };
        let width_u16 = u16::try_from(width_u32).context("width must fit in u16")?;
        let height_u16 = u16::try_from(height_u32).context("height must fit in u16")?;
        let initial_parameters =
            InitialParameterState::validated(config.codec, config.initial_parameter_sets.as_ref())?;
        let creation_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let muxer = Fmp4SegmentMuxer::with_options(SegmentMuxerOptions { creation_timestamp })
            .context("failed to create fMP4 muxer")?;
        if let Some(parent) = config.output_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create output dir: {}", parent.display()))?;
        }
        let writer = RecorderFileWriter::spawn(config.output_path.clone())?;
        let fps = config.frame_rate.get().get().max(1);
        let default_duration = (90_000 / fps).max(1);
        let mut state = Self {
            encoder: None,
            muxer,
            writer,
            output_path: config.output_path.clone(),
            creation_timestamp,
            dims,
            width_u16,
            height_u16,
            codec: config.codec,
            timescale: NonZeroU32::new(90_000).expect("constant non-zero"),
            default_duration,
            first_input_pts_90k: None,
            last_submitted_pts_90k: None,
            next_fallback_pts_90k: 0,
            last_written_pts_90k: None,
            pending_submitted_pts_90k: VecDeque::new(),
            pending_force_keyframe: false,
            submitted_frames: 0,
            vps: initial_parameters.vps,
            sps: initial_parameters.sps,
            pps: initial_parameters.pps,
            av1_sequence_header: initial_parameters.av1_sequence_header,
            av1_config: initial_parameters.av1_config,
            sample_entry: None,
            sample_entry_emitted: false,
            init_written: false,
            init_bytes_written: None,
            total_duration_90k: 0,
            pending_samples: VecDeque::new(),
            fragment_frames: config.fragment_frames.get().get().max(1),
            segments_written: 0,
            packets_seen: 0,
            bytes_written: 0,
        };
        state.refresh_sample_entry_from_current_parameters()?;
        Ok(state)
    }

    pub(super) fn submit_rgba_frame(&mut self, frame_rgba: Vec<u8>, pts_90k: i64) -> Result<()> {
        self.submit_argb_frame(rgba_to_argb_bytes(&frame_rgba), pts_90k)
    }

    pub(super) fn submit_argb_frame(&mut self, frame_argb: Vec<u8>, pts_90k: i64) -> Result<()> {
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
            .as_mut()
            .ok_or_else(|| {
                anyhow!("raw frame submission is not available in an encoded fMP4 session")
            })?
            .submit(EncodeFrame {
                dims: self.dims,
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
        loop {
            let chunk = self
                .encoder
                .as_mut()
                .expect("encoder checked above")
                .try_reap()
                .context("encoder try_reap failed")?;
            let Some(chunk) = chunk else {
                break;
            };
            self.handle_chunk(chunk)?;
        }
        let should_flush = self
            .encoder
            .as_ref()
            .expect("encoder checked above")
            .requires_periodic_fragment_flush()
            && self.submitted_frames.is_multiple_of(fragment_interval);
        if should_flush {
            for chunk in self
                .encoder
                .as_mut()
                .expect("encoder checked above")
                .flush()
                .context("encoder periodic fragment flush failed")?
            {
                self.handle_chunk(chunk)?;
            }
        }
        Ok(())
    }

    pub(super) fn set_fragment_frames(&mut self, fragment_frames: usize) -> Result<()> {
        self.fragment_frames = fragment_frames.max(1);
        self.pending_force_keyframe = true;
        self.submitted_frames = 0;
        self.flush_pending_if_ready(false)
    }

    pub(super) fn finish(mut self) -> Result<RecorderSummary> {
        let mut flush_packets = 0_u64;
        if self.encoder.is_some() {
            let chunks = self
                .encoder
                .as_mut()
                .expect("encoder exists")
                .flush()
                .context("encoder flush failed")?;
            for chunk in chunks {
                flush_packets = flush_packets.saturating_add(1);
                self.handle_chunk(chunk)?;
            }
        }
        if self.sample_entry.is_none() && !self.pending_samples.is_empty() {
            bail!("recording ended before codec parameter sets could be observed");
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

    pub(super) fn handle_chunk(&mut self, chunk: EncodedChunk) -> Result<()> {
        self.handle_chunk_with_duration(chunk, None)
    }

    pub(super) fn handle_chunk_with_duration(
        &mut self,
        chunk: EncodedChunk,
        duration_90k: Option<u32>,
    ) -> Result<()> {
        if chunk.codec != self.codec {
            bail!(
                "encoded chunk codec mismatch: expected {}, got {}",
                self.codec,
                chunk.codec
            );
        }
        if chunk.kind == EncodedContentKind::CodecConfig {
            anyhow::ensure!(
                duration_90k.is_none(),
                "codec-config chunks must not declare a sample duration"
            );
            anyhow::ensure!(
                chunk.pts_90k.is_none(),
                "codec-config chunks must not declare a presentation timestamp"
            );
            anyhow::ensure!(
                !chunk.is_keyframe,
                "codec-config chunks cannot be keyframes"
            );
            return self.apply_codec_config(chunk.layout, &chunk.data);
        }

        let mut vps = self.vps.clone();
        let mut sps = self.sps.clone();
        let mut pps = self.pps.clone();
        let mut av1_sequence_header = self.av1_sequence_header.clone();
        let mut av1_config = self.av1_config;
        let sample_data = match (self.codec, chunk.layout) {
            (Codec::H264, EncodedLayout::AnnexB) => {
                annexb_chunk_to_avcc_sample(&chunk.data, &mut sps, &mut pps)?
            }
            (Codec::H264, EncodedLayout::Avcc) => {
                avcc_chunk_to_avcc_sample(&chunk.data, &mut sps, &mut pps)?
            }
            (Codec::Hevc, EncodedLayout::AnnexB) => {
                annexb_chunk_to_hvcc_sample(&chunk.data, &mut vps, &mut sps, &mut pps)?
            }
            (Codec::Hevc, EncodedLayout::Hvcc) => {
                hvcc_chunk_to_hvcc_sample(&chunk.data, &mut vps, &mut sps, &mut pps)?
            }
            (Codec::Av1, EncodedLayout::Av1) => {
                av1_chunk_to_av1_sample(&chunk.data, &mut av1_sequence_header, &mut av1_config)?
            }
            (_, layout) => {
                bail!(
                    "unsupported encoded layout for fMP4 writer: {} (codec={})",
                    layout,
                    self.codec
                );
            }
        };
        let sample_data = sample_data.context(
            "frame chunk contained codec configuration but no encoded frame payload; use EncodedContentKind::CodecConfig",
        )?;
        let sample_entry = self.resolve_sample_entry(
            vps.as_deref(),
            sps.as_deref(),
            pps.as_deref(),
            av1_sequence_header.as_deref(),
            av1_config,
        )?;
        self.vps = vps;
        self.sps = sps;
        self.pps = pps;
        self.av1_sequence_header = av1_sequence_header;
        self.av1_config = av1_config;
        self.sample_entry = sample_entry;
        self.packets_seen = self.packets_seen.saturating_add(1);
        let pts_90k = self.resolve_sample_pts_90k(chunk.pts_90k.map(|pts| pts.0));
        self.pending_samples.push_back(PendingSample {
            keyframe: chunk.is_keyframe,
            pts_90k,
            duration_90k,
            data: sample_data,
        });
        self.flush_pending_if_ready(false)?;
        Ok(())
    }

    pub(super) fn apply_codec_config(&mut self, layout: EncodedLayout, data: &[u8]) -> Result<()> {
        let mut vps = self.vps.clone();
        let mut sps = self.sps.clone();
        let mut pps = self.pps.clone();
        let mut av1_sequence_header = self.av1_sequence_header.clone();
        let mut av1_config = self.av1_config;
        let frame_payload = match (self.codec, layout) {
            (Codec::H264, EncodedLayout::AnnexB) => {
                annexb_chunk_to_avcc_sample(data, &mut sps, &mut pps)?
            }
            (Codec::H264, EncodedLayout::Avcc) => {
                avcc_chunk_to_avcc_sample(data, &mut sps, &mut pps)?
            }
            (Codec::Hevc, EncodedLayout::AnnexB) => {
                annexb_chunk_to_hvcc_sample(data, &mut vps, &mut sps, &mut pps)?
            }
            (Codec::Hevc, EncodedLayout::Hvcc) => {
                hvcc_chunk_to_hvcc_sample(data, &mut vps, &mut sps, &mut pps)?
            }
            (Codec::Av1, EncodedLayout::Av1) => {
                let (sequence_header, payload) = find_av1_sequence_header_obu(data)
                    .context("AV1 codec-config chunk has no sequence header OBU")?;
                let config = parse_av1_sequence_header(payload)
                    .context("AV1 codec-config chunk has a malformed sequence header OBU")?;
                av1_sequence_header = Some(sequence_header.to_vec());
                av1_config = Some(config);
                None
            }
            (_, layout) => {
                bail!(
                    "unsupported codec-config layout for fMP4 writer: {} (codec={})",
                    layout,
                    self.codec
                );
            }
        };
        anyhow::ensure!(
            frame_payload.is_none(),
            "codec-config chunk contains encoded frame payload"
        );
        match self.codec {
            Codec::H264 => anyhow::ensure!(
                sps.is_some() && pps.is_some(),
                "H.264 codec-config chunk is missing SPS or PPS"
            ),
            Codec::Hevc => anyhow::ensure!(
                vps.is_some() && sps.is_some() && pps.is_some(),
                "HEVC codec-config chunk is missing VPS, SPS, or PPS"
            ),
            Codec::Av1 => anyhow::ensure!(
                av1_sequence_header.is_some() && av1_config.is_some(),
                "AV1 codec-config chunk is missing a sequence header"
            ),
        }
        let sample_entry = self.resolve_sample_entry(
            vps.as_deref(),
            sps.as_deref(),
            pps.as_deref(),
            av1_sequence_header.as_deref(),
            av1_config,
        )?;
        anyhow::ensure!(
            sample_entry.is_some(),
            "codec-config chunk does not contain all parameter sets required by {}",
            self.codec
        );
        self.vps = vps;
        self.sps = sps;
        self.pps = pps;
        self.av1_sequence_header = av1_sequence_header;
        self.av1_config = av1_config;
        self.sample_entry = sample_entry;
        Ok(())
    }

    fn build_sample_entry(
        &self,
        vps: Option<&[u8]>,
        sps: Option<&[u8]>,
        pps: Option<&[u8]>,
        av1_sequence_header: Option<&[u8]>,
        av1_config: Option<Av1ConfigSummary>,
    ) -> Result<Option<SampleEntry>> {
        match self.codec {
            Codec::H264 => {
                let (Some(sps), Some(pps)) = (sps, pps) else {
                    return Ok(None);
                };
                create_h264_sample_entry(self.width_u16, self.height_u16, sps, pps).map(Some)
            }
            Codec::Hevc => {
                let (Some(vps), Some(sps), Some(pps)) = (vps, sps, pps) else {
                    return Ok(None);
                };
                create_hevc_sample_entry(self.width_u16, self.height_u16, vps, sps, pps).map(Some)
            }
            Codec::Av1 => {
                let (Some(sequence_header), Some(config)) = (av1_sequence_header, av1_config)
                else {
                    return Ok(None);
                };
                Ok(Some(create_av1_sample_entry(
                    self.width_u16,
                    self.height_u16,
                    sequence_header,
                    config,
                )))
            }
        }
    }

    fn resolve_sample_entry(
        &self,
        vps: Option<&[u8]>,
        sps: Option<&[u8]>,
        pps: Option<&[u8]>,
        av1_sequence_header: Option<&[u8]>,
        av1_config: Option<Av1ConfigSummary>,
    ) -> Result<Option<SampleEntry>> {
        let candidate = self.build_sample_entry(vps, sps, pps, av1_sequence_header, av1_config)?;
        if self.sample_entry_emitted {
            if let (Some(existing), Some(candidate)) = (&self.sample_entry, &candidate) {
                anyhow::ensure!(
                    existing == candidate,
                    "codec parameter sets changed after the MP4 sample entry was emitted"
                );
            }
            return Ok(self.sample_entry.clone());
        }
        Ok(candidate.or_else(|| self.sample_entry.clone()))
    }

    fn refresh_sample_entry_from_current_parameters(&mut self) -> Result<()> {
        self.sample_entry = self.resolve_sample_entry(
            self.vps.as_deref(),
            self.sps.as_deref(),
            self.pps.as_deref(),
            self.av1_sequence_header.as_deref(),
            self.av1_config,
        )?;
        Ok(())
    }

    pub(super) fn flush_pending_if_ready(&mut self, flush_partial_fragment: bool) -> Result<()> {
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

    pub(super) fn write_media_fragment(
        &mut self,
        fragment_samples: Vec<PendingSample>,
    ) -> Result<()> {
        let include_sample_entry = !self.sample_entry_emitted;
        let mut payload = Vec::new();
        let mut samples = Vec::with_capacity(fragment_samples.len());
        for (index, pending) in fragment_samples.into_iter().enumerate() {
            let sample_entry = if include_sample_entry && index == 0 {
                self.sample_entry.clone()
            } else {
                None
            };
            let duration = pending
                .duration_90k
                .unwrap_or_else(|| self.sample_duration_for_pts(pending.pts_90k));
            if pending.duration_90k.is_some() {
                self.last_written_pts_90k = Some(pending.pts_90k);
            }
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
            self.bytes_written = self.bytes_written.saturating_add(
                u64::try_from(init.len()).context("init segment size exceeds u64")?,
            );
            self.init_written = true;
            self.init_bytes_written = Some(init.len());
        }
        let metadata_len = metadata.len();
        let payload_len = payload.len();
        self.writer
            .send(RecorderWriterCommand::WriteFragment { metadata, payload })?;
        let fragment_len = metadata_len
            .checked_add(payload_len)
            .context("fragment byte count overflow")?;
        self.bytes_written = self
            .bytes_written
            .saturating_add(u64::try_from(fragment_len).context("fragment size exceeds u64")?);
        self.segments_written = self.segments_written.saturating_add(1);
        self.sample_entry_emitted = true;
        Ok(())
    }

    pub(super) fn sample_duration_for_pts(&mut self, pts_90k: i64) -> u32 {
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

    pub(super) fn resolve_sample_pts_90k(&mut self, chunk_pts_90k: Option<i64>) -> i64 {
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

    pub(super) fn rewrite_init_segment_with_final_timing(&mut self) -> Result<()> {
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
