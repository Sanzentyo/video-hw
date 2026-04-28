use super::config::Fmp4WriterStatus;

use super::{
    config::{Fmp4WriterConfig, Fmp4WriterSummary, FragmentFrames, Pts90k},
    core::WriterCore,
    video_frame::{ArgbFrame, RgbaFrame},
};
use anyhow::{Context, Result, anyhow};

#[derive(Debug, Clone)]
pub enum AsyncWriterEvent {
    FrameConsumed,
    Status(Fmp4WriterStatus),
    Error(String),
}

enum AsyncWriterCommand {
    WriteRgba {
        frame: RgbaFrame,
        pts: Pts90k,
        reply: tokio::sync::oneshot::Sender<Result<()>>,
    },
    WriteArgb {
        frame: ArgbFrame,
        pts: Pts90k,
        reply: tokio::sync::oneshot::Sender<Result<()>>,
    },
    SetFragmentFrames {
        value: FragmentFrames,
        reply: tokio::sync::oneshot::Sender<Result<()>>,
    },
    Finish {
        reply: tokio::sync::oneshot::Sender<Result<Fmp4WriterSummary>>,
    },
}

pub(crate) struct AsyncWriterHandle {
    command_tx: tokio::sync::mpsc::UnboundedSender<AsyncWriterCommand>,
    event_rx: tokio::sync::mpsc::UnboundedReceiver<AsyncWriterEvent>,
}

impl AsyncWriterHandle {
    pub(crate) fn spawn(config: Fmp4WriterConfig) -> Result<Self> {
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("video-hw-fmp4-writer-worker".to_string())
            .spawn(move || {
                let mut core = match WriterCore::open(&config) {
                    Ok(core) => core,
                    Err(err) => {
                        let _ = event_tx.send(AsyncWriterEvent::Error(format!("{err:#}")));
                        return;
                    }
                };
                let _ = event_tx.send(AsyncWriterEvent::Status(core.status()));
                while let Some(command) = command_rx.blocking_recv() {
                    match command {
                        AsyncWriterCommand::WriteRgba { frame, pts, reply } => {
                            let result = core.write_rgba(frame, pts);
                            let _ = reply.send(result);
                            let _ = event_tx.send(AsyncWriterEvent::FrameConsumed);
                            let _ = event_tx.send(AsyncWriterEvent::Status(core.status()));
                        }
                        AsyncWriterCommand::WriteArgb { frame, pts, reply } => {
                            let result = core.write_argb(frame, pts);
                            let _ = reply.send(result);
                            let _ = event_tx.send(AsyncWriterEvent::FrameConsumed);
                            let _ = event_tx.send(AsyncWriterEvent::Status(core.status()));
                        }
                        AsyncWriterCommand::SetFragmentFrames { value, reply } => {
                            let result = core.set_fragment_frames(value);
                            let _ = reply.send(result);
                            let _ = event_tx.send(AsyncWriterEvent::Status(core.status()));
                        }
                        AsyncWriterCommand::Finish { reply } => {
                            let result = core.finish();
                            let _ = reply.send(result);
                            return;
                        }
                    }
                }
            })
            .context("failed to spawn writer worker thread")?;
        Ok(Self {
            command_tx,
            event_rx,
        })
    }

    pub(crate) async fn write_rgba(&mut self, frame: RgbaFrame, pts: Pts90k) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(AsyncWriterCommand::WriteRgba {
                frame,
                pts,
                reply: reply_tx,
            })
            .map_err(|_| anyhow!("writer worker command channel closed"))?;
        reply_rx.await.context("writer worker dropped rgba reply")?
    }

    pub(crate) async fn write_argb(&mut self, frame: ArgbFrame, pts: Pts90k) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(AsyncWriterCommand::WriteArgb {
                frame,
                pts,
                reply: reply_tx,
            })
            .map_err(|_| anyhow!("writer worker command channel closed"))?;
        reply_rx.await.context("writer worker dropped argb reply")?
    }

    pub(crate) async fn set_fragment_frames(&mut self, value: FragmentFrames) -> Result<()> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(AsyncWriterCommand::SetFragmentFrames {
                value,
                reply: reply_tx,
            })
            .map_err(|_| anyhow!("writer worker command channel closed"))?;
        reply_rx
            .await
            .context("writer worker dropped fragment reply")?
    }

    pub(crate) async fn finish(mut self) -> Result<Fmp4WriterSummary> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(AsyncWriterCommand::Finish { reply: reply_tx })
            .map_err(|_| anyhow!("writer worker command channel closed"))?;
        while self.event_rx.try_recv().is_ok() {}
        reply_rx
            .await
            .context("writer worker dropped finish reply")?
    }

    pub(crate) async fn recv_event(&mut self) -> Option<AsyncWriterEvent> {
        self.event_rx.recv().await
    }

    pub(crate) fn try_recv_event(&mut self) -> Option<AsyncWriterEvent> {
        self.event_rx.try_recv().ok()
    }
}
