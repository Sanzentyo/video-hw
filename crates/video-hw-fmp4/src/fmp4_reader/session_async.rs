use super::config::Fmp4ReaderStatus;

use super::{
    config::{Fmp4ReadSample, Fmp4ReaderConfig, Fmp4Track},
    core::ReaderCore,
};
use anyhow::{Context, Result, anyhow};

#[derive(Debug, Clone)]
pub enum AsyncReaderEvent {
    Status(Fmp4ReaderStatus),
    Error(String),
}

enum AsyncReaderCommand {
    NextSample {
        reply: tokio::sync::oneshot::Sender<Result<Option<Fmp4ReadSample>>>,
    },
    Finish {
        reply: tokio::sync::oneshot::Sender<Result<Fmp4ReaderStatus>>,
    },
}

pub(crate) struct AsyncReaderHandle {
    command_tx: tokio::sync::mpsc::UnboundedSender<AsyncReaderCommand>,
    event_rx: tokio::sync::mpsc::UnboundedReceiver<AsyncReaderEvent>,
}

impl AsyncReaderHandle {
    pub(crate) fn spawn(config: Fmp4ReaderConfig) -> Result<(Self, Vec<Fmp4Track>)> {
        let core = ReaderCore::open(&config)?;
        let tracks = core.tracks().to_vec();
        let initial_status = core.status();
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("video-hw-fmp4-reader-worker".to_string())
            .spawn(move || {
                let mut core = core;
                let _ = event_tx.send(AsyncReaderEvent::Status(initial_status));
                while let Some(command) = command_rx.blocking_recv() {
                    match command {
                        AsyncReaderCommand::NextSample { reply } => {
                            let result = core.next_sample();
                            let _ = reply.send(result);
                            let _ = event_tx.send(AsyncReaderEvent::Status(core.status()));
                        }
                        AsyncReaderCommand::Finish { reply } => {
                            let _ = reply.send(Ok(core.status()));
                            return;
                        }
                    }
                }
            })
            .context("failed to spawn reader worker thread")?;
        Ok((
            Self {
                command_tx,
                event_rx,
            },
            tracks,
        ))
    }

    pub(crate) async fn next_sample(&mut self) -> Result<Option<Fmp4ReadSample>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(AsyncReaderCommand::NextSample { reply: reply_tx })
            .map_err(|_| anyhow!("reader worker command channel closed"))?;
        reply_rx
            .await
            .context("reader worker dropped next_sample reply")?
    }

    pub(crate) async fn finish(self) -> Result<Fmp4ReaderStatus> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(AsyncReaderCommand::Finish { reply: reply_tx })
            .map_err(|_| anyhow!("reader worker command channel closed"))?;
        reply_rx
            .await
            .context("reader worker dropped finish reply")?
    }

    pub(crate) async fn recv_event(&mut self) -> Option<AsyncReaderEvent> {
        self.event_rx.recv().await
    }

    pub(crate) fn try_recv_event(&mut self) -> Option<AsyncReaderEvent> {
        self.event_rx.try_recv().ok()
    }
}
