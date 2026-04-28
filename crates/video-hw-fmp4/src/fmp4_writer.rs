mod config;
mod core;
#[cfg(feature = "async-session")]
mod session_async;
mod state;
mod video_frame;

pub use config::{
    Fmp4WriterConfig, Fmp4WriterStatus, Fmp4WriterSummary, FragmentFrames, FrameRate, FrameSize,
    Pts90k,
};
#[cfg(feature = "async-session")]
pub use session_async::AsyncWriterEvent;
#[cfg(feature = "async-session")]
pub use state::AsyncRecording;
pub use state::{Finished, Ready, SyncRecording};
pub use video_frame::{ArgbFrame, RgbaFrame};

use anyhow::Result;
use std::marker::PhantomData;

use crate::fmp4_writer::core::WriterCore;
#[cfg(feature = "async-session")]
use crate::fmp4_writer::session_async::AsyncWriterHandle;

#[derive(Debug)]
pub struct Fmp4Writer<State> {
    config: Fmp4WriterConfig,
    state: State,
    _marker: PhantomData<fn() -> State>,
}

impl Fmp4Writer<Ready> {
    pub fn new(config: Fmp4WriterConfig) -> Self {
        Self {
            config,
            state: Ready,
            _marker: PhantomData,
        }
    }

    pub fn config(&self) -> &Fmp4WriterConfig {
        &self.config
    }

    pub fn into_sync_session(self) -> Result<Fmp4Writer<SyncRecording>> {
        let core = WriterCore::open(&self.config)?;
        Ok(Fmp4Writer {
            config: self.config,
            state: SyncRecording { core },
            _marker: PhantomData,
        })
    }

    #[cfg(feature = "async-session")]
    pub fn into_async_session(self) -> Result<Fmp4Writer<AsyncRecording>> {
        let handle = AsyncWriterHandle::spawn(self.config.clone())?;
        Ok(Fmp4Writer {
            config: self.config,
            state: AsyncRecording { handle },
            _marker: PhantomData,
        })
    }
}

impl Fmp4Writer<SyncRecording> {
    pub fn write_rgba(&mut self, frame: RgbaFrame, pts: Pts90k) -> Result<()> {
        self.state.core.write_rgba(frame, pts)
    }

    pub fn write_argb(&mut self, frame: ArgbFrame, pts: Pts90k) -> Result<()> {
        self.state.core.write_argb(frame, pts)
    }

    pub fn set_fragment_frames(&mut self, value: FragmentFrames) -> Result<()> {
        self.state.core.set_fragment_frames(value)
    }

    pub fn status(&self) -> Fmp4WriterStatus {
        self.state.core.status()
    }

    pub fn finish(self) -> Result<Fmp4Writer<Finished>> {
        let summary = self.state.core.finish()?;
        Ok(Fmp4Writer {
            config: self.config,
            state: Finished { summary },
            _marker: PhantomData,
        })
    }
}

#[cfg(feature = "async-session")]
impl Fmp4Writer<AsyncRecording> {
    pub async fn write_rgba(&mut self, frame: RgbaFrame, pts: Pts90k) -> Result<()> {
        self.state.handle.write_rgba(frame, pts).await
    }

    pub async fn write_argb(&mut self, frame: ArgbFrame, pts: Pts90k) -> Result<()> {
        self.state.handle.write_argb(frame, pts).await
    }

    pub async fn set_fragment_frames(&mut self, value: FragmentFrames) -> Result<()> {
        self.state.handle.set_fragment_frames(value).await
    }

    pub async fn recv_event(&mut self) -> Option<AsyncWriterEvent> {
        self.state.handle.recv_event().await
    }

    pub fn try_recv_event(&mut self) -> Option<AsyncWriterEvent> {
        self.state.handle.try_recv_event()
    }

    pub async fn finish(self) -> Result<Fmp4Writer<Finished>> {
        let summary = self.state.handle.finish().await?;
        Ok(Fmp4Writer {
            config: self.config,
            state: Finished { summary },
            _marker: PhantomData,
        })
    }
}

impl Fmp4Writer<Finished> {
    pub fn summary(&self) -> &Fmp4WriterSummary {
        &self.state.summary
    }

    pub fn into_summary(self) -> Fmp4WriterSummary {
        self.state.summary
    }
}
