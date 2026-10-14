use super::{
    config::{Fmp4WriterConfig, Fmp4WriterStatus, Fmp4WriterSummary, FragmentFrames, Pts90k},
    core::WriterCore,
    video_frame::{ArgbFrame, RgbaFrame},
};
use anyhow::{Context, Result, anyhow};

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
    Status {
        reply: tokio::sync::oneshot::Sender<Result<Fmp4WriterStatus>>,
    },
    Finish {
        reply: tokio::sync::oneshot::Sender<Result<Fmp4WriterSummary>>,
    },
    Abort,
}

pub(crate) struct AsyncWriterHandle {
    command_tx: tokio::sync::mpsc::UnboundedSender<AsyncWriterCommand>,
    join_handle: Option<std::thread::JoinHandle<()>>,
}

impl AsyncWriterHandle {
    pub(crate) fn spawn(config: Fmp4WriterConfig) -> Result<Self> {
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel(1);
        let join_handle = std::thread::Builder::new()
            .name("video-hw-fmp4-writer-worker".to_string())
            .spawn(move || {
                let mut core = match WriterCore::open(&config) {
                    Ok(core) => core,
                    Err(err) => {
                        let _ = init_tx.send(Err(err));
                        return;
                    }
                };
                if init_tx.send(Ok(())).is_err() {
                    return;
                }
                while let Some(command) = command_rx.blocking_recv() {
                    match command {
                        AsyncWriterCommand::WriteRgba { frame, pts, reply } => {
                            let result = core.write_rgba(frame, pts);
                            let _ = reply.send(result);
                        }
                        AsyncWriterCommand::WriteArgb { frame, pts, reply } => {
                            let result = core.write_argb(frame, pts);
                            let _ = reply.send(result);
                        }
                        AsyncWriterCommand::SetFragmentFrames { value, reply } => {
                            let result = core.set_fragment_frames(value);
                            let _ = reply.send(result);
                        }
                        AsyncWriterCommand::Status { reply } => {
                            let _ = reply.send(Ok(core.status()));
                        }
                        AsyncWriterCommand::Finish { reply } => {
                            let result = core.finish();
                            let _ = reply.send(result);
                            return;
                        }
                        AsyncWriterCommand::Abort => return,
                    }
                }
            })
            .context("failed to spawn writer worker thread")?;
        match init_rx.recv() {
            Err(error) => {
                let panicked = join_handle.join().is_err();
                if panicked {
                    return Err(anyhow!(
                        "writer worker thread panicked during initialization"
                    ));
                }
                return Err(error).context("writer worker exited before reporting initialization");
            }
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                join_handle
                    .join()
                    .map_err(|_| anyhow!("writer worker thread panicked during initialization"))?;
                return Err(err.context("failed to initialize writer worker"));
            }
        }
        Ok(Self {
            command_tx,
            join_handle: Some(join_handle),
        })
    }

    pub(crate) async fn write_rgba(&mut self, frame: RgbaFrame, pts: Pts90k) -> Result<()> {
        self.request(
            |reply| AsyncWriterCommand::WriteRgba { frame, pts, reply },
            "write_rgba",
        )
        .await
    }

    pub(crate) async fn write_argb(&mut self, frame: ArgbFrame, pts: Pts90k) -> Result<()> {
        self.request(
            |reply| AsyncWriterCommand::WriteArgb { frame, pts, reply },
            "write_argb",
        )
        .await
    }

    pub(crate) async fn set_fragment_frames(&mut self, value: FragmentFrames) -> Result<()> {
        self.request(
            |reply| AsyncWriterCommand::SetFragmentFrames { value, reply },
            "set_fragment_frames",
        )
        .await
    }

    pub(crate) async fn status(&mut self) -> Result<Fmp4WriterStatus> {
        self.request(|reply| AsyncWriterCommand::Status { reply }, "status")
            .await
    }

    pub(crate) async fn finish(mut self) -> Result<Fmp4WriterSummary> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        if self
            .command_tx
            .send(AsyncWriterCommand::Finish { reply: reply_tx })
            .is_err()
        {
            self.join_worker()?;
            return Err(anyhow!("writer worker command channel closed"));
        }
        let result = reply_rx.await.context("writer worker dropped finish reply");
        self.join_worker()?;
        result?
    }

    async fn request<T>(
        &self,
        make_command: impl FnOnce(tokio::sync::oneshot::Sender<Result<T>>) -> AsyncWriterCommand,
        label: &'static str,
    ) -> Result<T> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(make_command(reply_tx))
            .map_err(|_| anyhow!("writer worker command channel closed"))?;
        reply_rx
            .await
            .with_context(|| format!("writer worker dropped {label} reply"))?
    }

    fn join_worker(&mut self) -> Result<()> {
        if let Some(handle) = self.join_handle.take() {
            handle
                .join()
                .map_err(|_| anyhow!("writer worker thread panicked"))?;
        }
        Ok(())
    }
}

impl Drop for AsyncWriterHandle {
    fn drop(&mut self) {
        if self.join_handle.is_some() {
            let _ = self.command_tx.send(AsyncWriterCommand::Abort);
        }
        if self
            .join_handle
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            let _ = self.join_worker();
        }
    }
}
