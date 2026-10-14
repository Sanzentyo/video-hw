//! Dedicated output thread, initialization handshake, and abort-on-drop ownership.

use super::*;

#[derive(Debug)]
pub(super) struct PendingSample {
    pub(super) keyframe: bool,
    pub(super) pts_90k: i64,
    pub(super) duration_90k: Option<u32>,
    pub(super) data: Vec<u8>,
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

pub(super) enum RecorderWriterCommand {
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
    Abort,
}

pub(super) struct RecorderFileWriter {
    pub(super) command_tx: std::sync::mpsc::Sender<RecorderWriterCommand>,
    pub(super) error_rx: std::sync::mpsc::Receiver<String>,
    pub(super) join_handle: Option<std::thread::JoinHandle<()>>,
}

impl RecorderFileWriter {
    pub(super) fn spawn(output_path: PathBuf) -> Result<Self> {
        let (command_tx, command_rx) = std::sync::mpsc::channel::<RecorderWriterCommand>();
        let (error_tx, error_rx) = std::sync::mpsc::channel::<String>();
        let (init_tx, init_rx) =
            std::sync::mpsc::sync_channel::<std::result::Result<(), String>>(1);
        let thread_name = format!("video-hw-fmp4-file-writer-{}", output_path.display());
        let mut join_handle = Some(
            std::thread::Builder::new()
                .name(thread_name)
                .spawn(move || run_recorder_file_writer(output_path, command_rx, error_tx, init_tx))
                .context("failed to spawn recorder file writer thread")?,
        );
        match init_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(message)) => {
                if let Some(handle) = join_handle.take() {
                    let _ = handle.join();
                }
                bail!("failed to initialize recorder file writer: {message}");
            }
            Err(error) => {
                let panicked = join_handle
                    .take()
                    .is_some_and(|handle| handle.join().is_err());
                if panicked {
                    bail!("recorder file writer thread panicked during initialization");
                }
                return Err(error).context("recorder file writer stopped during initialization");
            }
        }
        Ok(Self {
            command_tx,
            error_rx,
            join_handle,
        })
    }

    pub(super) fn send(&mut self, command: RecorderWriterCommand) -> Result<()> {
        self.poll_error()?;
        self.command_tx
            .send(command)
            .context("failed to send recorder writer command")
    }

    pub(super) fn poll_error(&mut self) -> Result<()> {
        if let Ok(message) = self.error_rx.try_recv() {
            bail!("recorder file writer failed: {message}");
        }
        Ok(())
    }

    pub(super) fn finish(&mut self) -> Result<u64> {
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

    pub(super) fn join(&mut self) -> Result<()> {
        if let Some(join_handle) = self.join_handle.take() {
            join_handle
                .join()
                .map_err(|_| anyhow!("recorder file writer thread panicked"))?;
        }
        Ok(())
    }
}

impl Drop for RecorderFileWriter {
    fn drop(&mut self) {
        if self.join_handle.is_some() {
            let _ = self.command_tx.send(RecorderWriterCommand::Abort);
            let _ = self.join();
        }
    }
}

pub(super) fn run_recorder_file_writer(
    output_path: PathBuf,
    command_rx: std::sync::mpsc::Receiver<RecorderWriterCommand>,
    error_tx: std::sync::mpsc::Sender<String>,
    init_tx: std::sync::mpsc::SyncSender<std::result::Result<(), String>>,
) {
    let result = (|| -> Result<()> {
        let file = match File::create(&output_path) {
            Ok(file) => file,
            Err(error) => {
                let message = format!("failed to create {}: {error}", output_path.display());
                let _ = init_tx.send(Err(message.clone()));
                bail!(message);
            }
        };
        init_tx
            .send(Ok(()))
            .map_err(|_| anyhow!("recorder initialization receiver was dropped"))?;
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
                RecorderWriterCommand::Abort => return Ok(()),
            }
        }

        bail!("recorder writer command channel closed before finish")
    })();

    if let Err(err) = result {
        let _ = error_tx.send(format!("{err:#}"));
    }
}
