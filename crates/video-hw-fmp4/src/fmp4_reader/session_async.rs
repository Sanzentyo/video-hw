use super::{
    config::{
        EncodedSample, Fmp4ReaderConfig, Fmp4ReaderStatus, Fmp4Track, GopSegment, MediaTime,
        Mp4IndexSnapshot, RangeCacheStats, SampleId, SampleLookup, SampleMeta, TrackId,
    },
    core::ReaderCore,
};
use anyhow::{Context, Result, anyhow};

#[derive(Debug, Clone)]
pub enum AsyncReaderEvent {
    Status(Fmp4ReaderStatus),
    Error(String),
}

enum AsyncReaderCommand {
    Samples {
        track: TrackId,
        reply: tokio::sync::oneshot::Sender<Result<Vec<SampleMeta>>>,
    },
    SampleMeta {
        sample: SampleId,
        reply: tokio::sync::oneshot::Sender<Result<Option<SampleMeta>>>,
    },
    SampleAtPts {
        track: TrackId,
        pts: MediaTime,
        reply: tokio::sync::oneshot::Sender<Result<Option<SampleId>>>,
    },
    SampleAtPtsWithDelta {
        track: TrackId,
        pts: MediaTime,
        reply: tokio::sync::oneshot::Sender<Result<Option<SampleLookup>>>,
    },
    KeyframeBefore {
        sample: SampleId,
        reply: tokio::sync::oneshot::Sender<Result<Option<SampleId>>>,
    },
    GopForSample {
        sample: SampleId,
        reply: tokio::sync::oneshot::Sender<Result<Option<GopSegment>>>,
    },
    ReadSample {
        sample: SampleId,
        reply: tokio::sync::oneshot::Sender<Result<EncodedSample>>,
    },
    ReadGop {
        sample: SampleId,
        reply: tokio::sync::oneshot::Sender<Result<Vec<EncodedSample>>>,
    },
    ReadSegment {
        segment: GopSegment,
        reply: tokio::sync::oneshot::Sender<Result<Vec<EncodedSample>>>,
    },
    NextSample {
        reply: tokio::sync::oneshot::Sender<Result<Option<EncodedSample>>>,
    },
    IndexSnapshot {
        reply: tokio::sync::oneshot::Sender<Result<Mp4IndexSnapshot>>,
    },
    Status {
        reply: tokio::sync::oneshot::Sender<Result<Fmp4ReaderStatus>>,
    },
    CacheStats {
        reply: tokio::sync::oneshot::Sender<Result<RangeCacheStats>>,
    },
    ClearCache {
        reply: tokio::sync::oneshot::Sender<Result<()>>,
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
                        AsyncReaderCommand::Samples { track, reply } => {
                            let result = core.samples(track).map(<[_]>::to_vec);
                            let _ = reply.send(result);
                            let _ = event_tx.send(AsyncReaderEvent::Status(core.status()));
                        }
                        AsyncReaderCommand::SampleMeta { sample, reply } => {
                            let result = Ok(core.sample_meta(sample).cloned());
                            let _ = reply.send(result);
                            let _ = event_tx.send(AsyncReaderEvent::Status(core.status()));
                        }
                        AsyncReaderCommand::SampleAtPts { track, pts, reply } => {
                            let result = Ok(core.sample_at_pts(track, pts));
                            let _ = reply.send(result);
                            let _ = event_tx.send(AsyncReaderEvent::Status(core.status()));
                        }
                        AsyncReaderCommand::SampleAtPtsWithDelta { track, pts, reply } => {
                            let result = Ok(core.sample_at_pts_with_delta(track, pts));
                            let _ = reply.send(result);
                            let _ = event_tx.send(AsyncReaderEvent::Status(core.status()));
                        }
                        AsyncReaderCommand::KeyframeBefore { sample, reply } => {
                            let result = Ok(core.keyframe_before(sample));
                            let _ = reply.send(result);
                            let _ = event_tx.send(AsyncReaderEvent::Status(core.status()));
                        }
                        AsyncReaderCommand::GopForSample { sample, reply } => {
                            let result = Ok(core.gop_for_sample(sample));
                            let _ = reply.send(result);
                            let _ = event_tx.send(AsyncReaderEvent::Status(core.status()));
                        }
                        AsyncReaderCommand::ReadSample { sample, reply } => {
                            let result = core.read_sample(sample);
                            let _ = reply.send(result);
                            let _ = event_tx.send(AsyncReaderEvent::Status(core.status()));
                        }
                        AsyncReaderCommand::ReadGop { sample, reply } => {
                            let result = core.read_gop(sample);
                            let _ = reply.send(result);
                            let _ = event_tx.send(AsyncReaderEvent::Status(core.status()));
                        }
                        AsyncReaderCommand::ReadSegment { segment, reply } => {
                            let result = core.read_segment(segment);
                            let _ = reply.send(result);
                            let _ = event_tx.send(AsyncReaderEvent::Status(core.status()));
                        }
                        AsyncReaderCommand::NextSample { reply } => {
                            let result = core.next_sample();
                            let _ = reply.send(result);
                            let _ = event_tx.send(AsyncReaderEvent::Status(core.status()));
                        }
                        AsyncReaderCommand::IndexSnapshot { reply } => {
                            let result = core.index_snapshot();
                            let _ = reply.send(result);
                            let _ = event_tx.send(AsyncReaderEvent::Status(core.status()));
                        }
                        AsyncReaderCommand::Status { reply } => {
                            let _ = reply.send(Ok(core.status()));
                        }
                        AsyncReaderCommand::CacheStats { reply } => {
                            let _ = reply.send(Ok(core.cache_stats()));
                        }
                        AsyncReaderCommand::ClearCache { reply } => {
                            core.clear_cache();
                            let _ = reply.send(Ok(()));
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

    pub(crate) async fn samples(&mut self, track: TrackId) -> Result<Vec<SampleMeta>> {
        self.request(
            |reply| AsyncReaderCommand::Samples { track, reply },
            "samples",
        )
        .await
    }

    pub(crate) async fn sample_meta(&mut self, sample: SampleId) -> Result<Option<SampleMeta>> {
        self.request(
            |reply| AsyncReaderCommand::SampleMeta { sample, reply },
            "sample_meta",
        )
        .await
    }

    pub(crate) async fn sample_at_pts(
        &mut self,
        track: TrackId,
        pts: MediaTime,
    ) -> Result<Option<SampleId>> {
        self.request(
            |reply| AsyncReaderCommand::SampleAtPts { track, pts, reply },
            "sample_at_pts",
        )
        .await
    }

    pub(crate) async fn sample_at_pts_with_delta(
        &mut self,
        track: TrackId,
        pts: MediaTime,
    ) -> Result<Option<SampleLookup>> {
        self.request(
            |reply| AsyncReaderCommand::SampleAtPtsWithDelta { track, pts, reply },
            "sample_at_pts_with_delta",
        )
        .await
    }

    pub(crate) async fn keyframe_before(&mut self, sample: SampleId) -> Result<Option<SampleId>> {
        self.request(
            |reply| AsyncReaderCommand::KeyframeBefore { sample, reply },
            "keyframe_before",
        )
        .await
    }

    pub(crate) async fn gop_for_sample(&mut self, sample: SampleId) -> Result<Option<GopSegment>> {
        self.request(
            |reply| AsyncReaderCommand::GopForSample { sample, reply },
            "gop_for_sample",
        )
        .await
    }

    pub(crate) async fn read_sample(&mut self, sample: SampleId) -> Result<EncodedSample> {
        self.request(
            |reply| AsyncReaderCommand::ReadSample { sample, reply },
            "read_sample",
        )
        .await
    }

    pub(crate) async fn read_gop(&mut self, sample: SampleId) -> Result<Vec<EncodedSample>> {
        self.request(
            |reply| AsyncReaderCommand::ReadGop { sample, reply },
            "read_gop",
        )
        .await
    }

    pub(crate) async fn read_segment(&mut self, segment: GopSegment) -> Result<Vec<EncodedSample>> {
        self.request(
            |reply| AsyncReaderCommand::ReadSegment { segment, reply },
            "read_segment",
        )
        .await
    }

    pub(crate) async fn next_sample(&mut self) -> Result<Option<EncodedSample>> {
        self.request(
            |reply| AsyncReaderCommand::NextSample { reply },
            "next_sample",
        )
        .await
    }

    pub(crate) async fn index_snapshot(&mut self) -> Result<Mp4IndexSnapshot> {
        self.request(
            |reply| AsyncReaderCommand::IndexSnapshot { reply },
            "index_snapshot",
        )
        .await
    }

    pub(crate) async fn status(&mut self) -> Result<Fmp4ReaderStatus> {
        self.request(|reply| AsyncReaderCommand::Status { reply }, "status")
            .await
    }

    pub(crate) async fn cache_stats(&mut self) -> Result<RangeCacheStats> {
        self.request(
            |reply| AsyncReaderCommand::CacheStats { reply },
            "cache_stats",
        )
        .await
    }

    pub(crate) async fn clear_cache(&mut self) -> Result<()> {
        self.request(
            |reply| AsyncReaderCommand::ClearCache { reply },
            "clear_cache",
        )
        .await
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

    async fn request<T>(
        &mut self,
        make_command: impl FnOnce(tokio::sync::oneshot::Sender<Result<T>>) -> AsyncReaderCommand,
        label: &'static str,
    ) -> Result<T> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(make_command(reply_tx))
            .map_err(|_| anyhow!("reader worker command channel closed"))?;
        reply_rx
            .await
            .with_context(|| format!("reader worker dropped {label} reply"))?
    }

    pub(crate) async fn recv_event(&mut self) -> Option<AsyncReaderEvent> {
        self.event_rx.recv().await
    }

    pub(crate) fn try_recv_event(&mut self) -> Option<AsyncReaderEvent> {
        self.event_rx.try_recv().ok()
    }
}
