//! Camera capture and asynchronous recorder-worker channels.

use std::{path::PathBuf, sync::mpsc, thread};

use anyhow::{Context, Result};
use shiguredo_video_device::{VideoCapture, VideoCaptureConfig, VideoFrame, VideoFrameOwned};

use super::{
    cli::RecordingSettings,
    recorder::{RecorderState, RecorderSummary},
};

#[derive(Debug, Clone)]
pub(super) struct CaptureConfigState {
    pub(super) device_id: Option<String>,
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) fps: i32,
}

pub(super) enum CaptureCommand {
    Reconfigure(CaptureConfigState),
    Stop,
}

pub(super) enum CaptureEvent {
    Reconfigured(CaptureConfigState),
    Error(String),
}

#[derive(Debug)]
pub(super) struct CapturedFrame {
    pub(super) frame: VideoFrameOwned,
}

pub(super) struct CaptureRuntime {
    pub(super) frame_rx: mpsc::Receiver<CapturedFrame>,
    pub(super) event_rx: mpsc::Receiver<CaptureEvent>,
    handle: CaptureHandle,
}

pub(super) struct CaptureHandle {
    command_tx: mpsc::Sender<CaptureCommand>,
    thread: thread::JoinHandle<()>,
}

impl CaptureRuntime {
    pub(super) fn start(initial_config: CaptureConfigState) -> Result<Self> {
        let (frame_tx, frame_rx) = mpsc::channel::<CapturedFrame>();
        let (command_tx, command_rx) = mpsc::channel::<CaptureCommand>();
        let (event_tx, event_rx) = mpsc::channel::<CaptureEvent>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let thread = thread::Builder::new()
            .name("camera-capture-thread".to_string())
            .spawn(move || {
                let mut current_config = initial_config;
                let start_capture = |config: &CaptureConfigState| -> Result<VideoCapture> {
                    let frame_tx = frame_tx.clone();
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
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                let _ = event_tx.send(CaptureEvent::Reconfigured(current_config.clone()));

                while let Ok(command) = command_rx.recv() {
                    match command {
                        CaptureCommand::Reconfigure(new_config) => {
                            capture.stop();
                            match start_capture(&new_config) {
                                Ok(new_capture) => {
                                    capture = new_capture;
                                    current_config = new_config.clone();
                                    let _ = event_tx.send(CaptureEvent::Reconfigured(new_config));
                                }
                                Err(err) => {
                                    let _ = event_tx.send(CaptureEvent::Error(format!(
                                        "capture reconfigure failed: {err:#}"
                                    )));
                                    match start_capture(&current_config) {
                                        Ok(restore_capture) => {
                                            capture = restore_capture;
                                            let _ = event_tx.send(CaptureEvent::Reconfigured(
                                                current_config.clone(),
                                            ));
                                        }
                                        Err(restore_err) => {
                                            let _ = event_tx.send(CaptureEvent::Error(format!(
                                                "capture restore failed: {restore_err:#}"
                                            )));
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

        ready_rx
            .recv()
            .context("camera capture thread exited before startup status")??;

        Ok(Self {
            frame_rx,
            event_rx,
            handle: CaptureHandle { command_tx, thread },
        })
    }

    pub(super) fn into_parts(
        self,
    ) -> (
        mpsc::Receiver<CapturedFrame>,
        mpsc::Receiver<CaptureEvent>,
        CaptureHandle,
    ) {
        (self.frame_rx, self.event_rx, self.handle)
    }
}

impl CaptureHandle {
    pub(super) fn command_sender(&self) -> mpsc::Sender<CaptureCommand> {
        self.command_tx.clone()
    }

    pub(super) fn stop(self) {
        let _ = self.command_tx.send(CaptureCommand::Stop);
        if let Err(err) = self.thread.join() {
            eprintln!("capture thread join failed: {err:?}");
        }
    }
}

pub(super) enum RecorderWorkerCommand {
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

pub(super) enum RecorderWorkerEvent {
    FrameConsumed,
    Status(String),
    Error(String),
}

pub(super) struct RecorderWorker {
    command_tx: mpsc::Sender<RecorderWorkerCommand>,
    event_rx: mpsc::Receiver<RecorderWorkerEvent>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl RecorderWorker {
    pub(super) fn spawn(
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

    pub(super) fn submit_frame(&mut self, frame_rgba: Vec<u8>, pts_90k: i64) -> Result<()> {
        self.command_tx
            .send(RecorderWorkerCommand::SubmitFrame {
                frame_rgba,
                pts_90k,
            })
            .context("failed to queue recorder frame")
    }

    pub(super) fn set_fragment_frames(&mut self, fragment_frames: usize) -> Result<()> {
        self.command_tx
            .send(RecorderWorkerCommand::SetFragmentFrames { fragment_frames })
            .context("failed to queue fragment frame update")
    }

    pub(super) fn try_recv_event(&mut self) -> Option<RecorderWorkerEvent> {
        self.event_rx.try_recv().ok()
    }

    pub(super) fn finish(&mut self) -> Result<RecorderSummary> {
        let (reply_tx, reply_rx) = mpsc::channel();
        if let Err(send_err) = self
            .command_tx
            .send(RecorderWorkerCommand::Finish { reply_tx })
        {
            return Err(self.finish_channel_error(format!(
                "failed to send recorder worker finish command: {send_err}"
            )));
        }
        let summary = match reply_rx.recv() {
            Ok(summary) => summary.map_err(anyhow::Error::msg)?,
            Err(recv_err) => {
                return Err(self.finish_channel_error(format!(
                    "failed to receive recorder worker finish result: {recv_err}"
                )));
            }
        };
        self.join()
            .context("failed to join recorder worker after finish")?;
        Ok(summary)
    }

    pub(super) fn join_only(&mut self) -> Result<()> {
        self.join()
    }

    pub(super) fn join(&mut self) -> Result<()> {
        if let Some(join_handle) = self.join_handle.take() {
            join_handle
                .join()
                .map_err(|_| anyhow::anyhow!("recorder worker thread panicked"))?;
        }
        Ok(())
    }

    pub(super) fn finish_channel_error(&mut self, prefix: String) -> anyhow::Error {
        let worker_error = self.take_terminal_worker_error();
        let join_error = self.join().err();
        let mut detail = prefix;
        if let Some(worker_error) = worker_error {
            detail.push_str("; worker already terminated with error: ");
            detail.push_str(&worker_error);
        }
        if let Some(join_error) = join_error {
            detail.push_str("; failed to join recorder worker: ");
            detail.push_str(&format!("{join_error:#}"));
        }
        anyhow::anyhow!(detail)
    }

    pub(super) fn take_terminal_worker_error(&mut self) -> Option<String> {
        let mut terminal_error = None;
        while let Ok(event) = self.event_rx.try_recv() {
            if let RecorderWorkerEvent::Error(message) = event {
                terminal_error = Some(message);
            }
        }
        terminal_error
    }
}

fn run_recorder_worker(
    mut recorder: RecorderState,
    command_rx: mpsc::Receiver<RecorderWorkerCommand>,
    event_tx: mpsc::Sender<RecorderWorkerEvent>,
) {
    let mut last_segments_written = recorder.segments_written();
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
                if recorder.segments_written() != last_segments_written
                    || recorder.packets_seen().is_multiple_of(15)
                {
                    last_segments_written = recorder.segments_written();
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
