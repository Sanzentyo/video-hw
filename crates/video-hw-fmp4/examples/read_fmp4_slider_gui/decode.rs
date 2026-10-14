use std::{
    collections::HashSet,
    sync::{Arc, mpsc},
    thread::JoinHandle,
};

use anyhow::{Context, Result};
use tracing::{debug, warn};
use video_hw::{
    Backend, BackendKind, ColorConvertOptions, DecodeOutputMode, DecodedFrame, PixelOutputLayout,
};
use video_hw_fmp4::{
    Fmp4Reader, Fmp4ReaderConfig, FrameDecodeRangeRequest, FrameDecoder, SampleRange,
};

use super::media::LoadedVideo;

#[derive(Debug, Clone)]
pub(super) struct DecodedPreview {
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) rgba: Arc<[u8]>,
}

impl DecodedPreview {
    pub(super) fn resident_bytes(&self) -> usize {
        self.rgba.len()
    }
}

pub(super) struct DecodeAttempt {
    pub(super) preview: Option<DecodedPreview>,
    pub(super) frame_count: usize,
    pub(super) decode_backend: Backend,
    pub(super) resolved_backend: BackendKind,
    pub(super) output_mode: DecodeOutputMode,
    pub(super) fallback_used: bool,
    pub(super) fallback_reason: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DecodeWorkerRequest {
    pub(super) request_id: u64,
    pub(super) generation: u64,
    pub(super) sample_index: usize,
    pub(super) backend: Backend,
    pub(super) require_hardware: bool,
    pub(super) strict_backend: bool,
}

pub(super) enum DecodeWorkerCommand {
    Decode(DecodeWorkerRequest),
    Shutdown,
}

pub(super) struct DecodeWorkerEvent {
    pub(super) request: DecodeWorkerRequest,
    pub(super) result: Result<DecodeAttempt, String>,
}

pub(super) struct DecodeWorker {
    command_tx: Option<mpsc::SyncSender<DecodeWorkerCommand>>,
    event_rx: mpsc::Receiver<DecodeWorkerEvent>,
    join_handle: Option<JoinHandle<()>>,
}

impl DecodeWorker {
    pub(super) fn try_send_decode(
        &self,
        request: DecodeWorkerRequest,
    ) -> Result<(), mpsc::TrySendError<DecodeWorkerRequest>> {
        let Some(command_tx) = self.command_tx.as_ref() else {
            return Err(mpsc::TrySendError::Disconnected(request));
        };
        command_tx
            .try_send(DecodeWorkerCommand::Decode(request))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(DecodeWorkerCommand::Decode(request)) => {
                    mpsc::TrySendError::Full(request)
                }
                mpsc::TrySendError::Disconnected(DecodeWorkerCommand::Decode(request)) => {
                    mpsc::TrySendError::Disconnected(request)
                }
                mpsc::TrySendError::Full(DecodeWorkerCommand::Shutdown)
                | mpsc::TrySendError::Disconnected(DecodeWorkerCommand::Shutdown) => {
                    unreachable!("decode dispatch cannot return a shutdown command")
                }
            })
    }

    pub(super) fn try_recv(&self) -> Result<DecodeWorkerEvent, mpsc::TryRecvError> {
        self.event_rx.try_recv()
    }
}

impl Drop for DecodeWorker {
    fn drop(&mut self) {
        if let Some(command_tx) = self.command_tx.take() {
            // If the sole channel slot still contains a decode request, dropping
            // the sender makes the worker exit after that request instead of
            // deadlocking this join while waiting for another command.
            let _ = command_tx.try_send(DecodeWorkerCommand::Shutdown);
            drop(command_tx);
        }
        if let Some(join_handle) = self.join_handle.take()
            && join_handle.join().is_err()
        {
            warn!("decode worker thread panicked during shutdown");
        }
    }
}

fn decoded_frame_to_preview(frame: DecodedFrame) -> Result<Option<DecodedPreview>> {
    if matches!(frame, DecodedFrame::Metadata { .. }) {
        return Ok(None);
    }
    let pixels = frame
        .to_pixel_buffer(PixelOutputLayout::Rgba8888, ColorConvertOptions::default())
        .map_err(anyhow::Error::new)
        .context("failed to convert decoded frame to RGBA preview")?;
    let width =
        usize::try_from(pixels.dims.width.get()).context("preview width overflows usize")?;
    let height =
        usize::try_from(pixels.dims.height.get()).context("preview height overflows usize")?;
    Ok(Some(DecodedPreview {
        width,
        height,
        rgba: pixels.data.into(),
    }))
}

fn enforce_strict_backend_result(
    strict_backend: bool,
    requested_backend: Backend,
    resolved_backend: BackendKind,
    fallback_used: bool,
    fallback_reason: Option<&str>,
) -> Result<()> {
    if strict_backend && fallback_used {
        anyhow::bail!(
            "strict backend {} resolved to fallback backend {}: {}",
            requested_backend,
            resolved_backend,
            fallback_reason.unwrap_or("fallback reason unavailable")
        );
    }
    Ok(())
}

fn decode_attempt_for_mode(
    video: &LoadedVideo,
    decode_backend: Backend,
    require_hardware: bool,
    sample_index: usize,
    output_mode: DecodeOutputMode,
    strict_backend: bool,
) -> Result<DecodeAttempt> {
    let mut reader = Fmp4Reader::new(Fmp4ReaderConfig::new(video.input_path.clone()))
        .into_sync_session()
        .with_context(|| format!("failed to open {}", video.input_path.display()))?;
    let target_sample = video
        .sample(sample_index)
        .with_context(|| format!("sample index {sample_index} is out of range"))?;
    let decode_segment = reader
        .gop_for_sample(target_sample.sample_id)?
        .with_context(|| {
            format!(
                "no GOP contains requested sample {}",
                target_sample.sample_id
            )
        })?;
    let mut request = FrameDecodeRangeRequest::new(SampleRange {
        track_id: video.track.track_id,
        start_sample: target_sample.sample_id,
        end_sample_exclusive: decode_segment.end_sample_exclusive,
    });
    request.backend = decode_backend;
    request.require_hardware = require_hardware;
    request.output_mode = output_mode;
    request.fps = Some(video.estimated_fps.max(1));

    let mut decoder = FrameDecoder::new(&mut reader);
    let mut decoded = decoder.decode_range_iter(request).with_context(|| {
        format!(
            "failed to decode GOP for sample {} (backend={decode_backend}, output_mode={output_mode})",
            target_sample.sample_id
        )
    })?;
    let diagnostics = decoded.diagnostics().clone();
    #[cfg(any(
        feature = "backend-vt",
        feature = "backend-nvidia",
        feature = "backend-intel",
        feature = "backend-vulkan"
    ))]
    let fallback_used = diagnostics.fallback_used;
    #[cfg(not(any(
        feature = "backend-vt",
        feature = "backend-nvidia",
        feature = "backend-intel",
        feature = "backend-vulkan"
    )))]
    let fallback_used = false;
    #[cfg(any(
        feature = "backend-vt",
        feature = "backend-nvidia",
        feature = "backend-intel",
        feature = "backend-vulkan"
    ))]
    let fallback_reason = diagnostics.fallback_reason.clone();
    #[cfg(not(any(
        feature = "backend-vt",
        feature = "backend-nvidia",
        feature = "backend-intel",
        feature = "backend-vulkan"
    )))]
    let fallback_reason = None;
    let resolved_backend = diagnostics.resolved_backend;
    enforce_strict_backend_result(
        strict_backend,
        decode_backend,
        resolved_backend,
        fallback_used,
        fallback_reason.as_deref(),
    )?;
    let mut frame_count = 0;
    let mut target_frame = None;
    for frame in &mut decoded {
        let frame = frame.with_context(|| {
            format!(
                "streaming decode failed for sample {}",
                target_sample.sample_id
            )
        })?;
        frame_count += 1;
        if frame.sample_id == Some(target_sample.sample_id) {
            target_frame = Some(frame);
        }
    }
    let target_frame = target_frame.with_context(|| {
        format!(
            "decoder did not return the requested sample {}",
            target_sample.sample_id
        )
    })?;
    let target_preview = decoded_frame_to_preview(target_frame.frame)?;

    Ok(DecodeAttempt {
        preview: target_preview,
        frame_count,
        decode_backend,
        resolved_backend,
        output_mode,
        fallback_used,
        fallback_reason,
    })
}

pub(super) fn decode_sample_index_sync(
    video: &LoadedVideo,
    requested_backend: Backend,
    require_hardware: bool,
    sample_index: usize,
    strict_backend: bool,
) -> Result<DecodeAttempt> {
    let mut fallbacks = Vec::<Backend>::new();
    let mut fallback_seen = HashSet::<String>::new();
    let mut push_fallback = |backend: Backend| {
        let label = backend.to_string();
        if fallback_seen.insert(label) {
            fallbacks.push(backend);
        }
    };
    if requested_backend == Backend::Auto {
        for backend in Backend::supported() {
            if backend != Backend::Auto {
                push_fallback(backend);
            }
        }
    } else if !strict_backend {
        push_fallback(requested_backend);
        for backend in Backend::supported() {
            if backend != Backend::Auto && backend != requested_backend {
                push_fallback(backend);
            }
        }
    } else {
        push_fallback(requested_backend);
    }
    anyhow::ensure!(
        !fallbacks.is_empty(),
        "no decode backend available for requested backend {requested_backend}"
    );
    debug!(
        requested_backend = %requested_backend,
        require_hardware,
        strict_backend,
        fallback_chain = ?fallbacks,
        sample_index,
        "built decode fallback chain"
    );
    let mut last_mode_error = None::<anyhow::Error>;
    for &decode_backend in &fallbacks {
        for output_mode in [DecodeOutputMode::Rgb24, DecodeOutputMode::Nv12] {
            match decode_attempt_for_mode(
                video,
                decode_backend,
                require_hardware,
                sample_index,
                output_mode,
                strict_backend,
            ) {
                Ok(attempt) if attempt.preview.is_some() => return Ok(attempt),
                Ok(_) => {}
                Err(err) => {
                    warn!(
                        backend = %decode_backend,
                        mode = %output_mode,
                        sample_index,
                        "decoder mode attempt failed; trying the next preview mode: {err:#}"
                    );
                    last_mode_error = Some(err);
                }
            }
        }
    }
    for &decode_backend in &fallbacks {
        match decode_attempt_for_mode(
            video,
            decode_backend,
            require_hardware,
            sample_index,
            DecodeOutputMode::Metadata,
            strict_backend,
        ) {
            Ok(attempt) => return Ok(attempt),
            Err(err) => {
                last_mode_error = Some(err);
            }
        }
    }
    if let Some(err) = last_mode_error {
        Err(err)
    } else {
        anyhow::bail!(
            "no decode backend could decode sample {} for requested backend {}",
            sample_index,
            requested_backend
        );
    }
}

pub(super) fn spawn_decode_worker(video: LoadedVideo) -> DecodeWorker {
    // The UI scheduler permits one outstanding request. A bounded channel keeps
    // that invariant intact even if a future caller bypasses the UI guard.
    let (command_tx, command_rx) = mpsc::sync_channel::<DecodeWorkerCommand>(1);
    let (event_tx, event_rx) = mpsc::channel::<DecodeWorkerEvent>();
    let join_handle = std::thread::spawn(move || {
        while let Ok(command) = command_rx.recv() {
            match command {
                DecodeWorkerCommand::Decode(request) => {
                    debug!(
                        request_id = request.request_id,
                        sample_index = request.sample_index,
                        backend = %request.backend,
                        require_hardware = request.require_hardware,
                        strict_backend = request.strict_backend,
                        "decode worker received request"
                    );
                    let result = decode_sample_index_sync(
                        &video,
                        request.backend,
                        request.require_hardware,
                        request.sample_index,
                        request.strict_backend,
                    )
                    .map_err(|err| format!("{err:#}"));
                    if let Err(err) = &result {
                        warn!(
                            request_id = request.request_id,
                            sample_index = request.sample_index,
                            "decode worker request failed: {err}"
                        );
                    }
                    let _ = event_tx.send(DecodeWorkerEvent { request, result });
                }
                DecodeWorkerCommand::Shutdown => break,
            }
        }
    });
    DecodeWorker {
        command_tx: Some(command_tx),
        event_rx,
        join_handle: Some(join_handle),
    }
}

#[cfg(test)]
#[path = "decode_tests.rs"]
mod tests;
