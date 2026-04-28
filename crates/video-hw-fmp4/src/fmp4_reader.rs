mod config;
mod core;
#[cfg(feature = "async-session")]
mod session_async;
mod state;

pub use config::{Fmp4ReadSample, Fmp4ReaderConfig, Fmp4ReaderStatus, Fmp4Track};
#[cfg(feature = "async-session")]
pub use session_async::AsyncReaderEvent;
#[cfg(feature = "async-session")]
pub use state::AsyncReading;
pub use state::{Finished, ReaderReady, SyncReading};

use anyhow::Result;
use std::marker::PhantomData;

use self::core::ReaderCore;
#[cfg(feature = "async-session")]
use self::session_async::AsyncReaderHandle;

#[derive(Debug)]
pub struct Fmp4Reader<State> {
    config: Fmp4ReaderConfig,
    state: State,
    _marker: PhantomData<fn() -> State>,
}

impl Fmp4Reader<ReaderReady> {
    pub fn new(config: Fmp4ReaderConfig) -> Self {
        Self {
            config,
            state: ReaderReady,
            _marker: PhantomData,
        }
    }

    pub fn config(&self) -> &Fmp4ReaderConfig {
        &self.config
    }

    pub fn into_sync_session(self) -> Result<Fmp4Reader<SyncReading>> {
        let core = ReaderCore::open(&self.config)?;
        Ok(Fmp4Reader {
            config: self.config,
            state: SyncReading { core },
            _marker: PhantomData,
        })
    }

    #[cfg(feature = "async-session")]
    pub fn into_async_session(self) -> Result<Fmp4Reader<AsyncReading>> {
        let (handle, tracks) = AsyncReaderHandle::spawn(self.config.clone())?;
        Ok(Fmp4Reader {
            config: self.config,
            state: AsyncReading { handle, tracks },
            _marker: PhantomData,
        })
    }
}

impl Fmp4Reader<SyncReading> {
    pub fn tracks(&self) -> &[Fmp4Track] {
        self.state.core.tracks()
    }

    pub fn next_sample(&mut self) -> Result<Option<Fmp4ReadSample>> {
        self.state.core.next_sample()
    }

    pub fn status(&self) -> Fmp4ReaderStatus {
        self.state.core.status()
    }

    pub fn finish(self) -> Fmp4Reader<Finished> {
        Fmp4Reader {
            config: self.config,
            state: Finished {
                status: self.state.core.status(),
            },
            _marker: PhantomData,
        }
    }
}

#[cfg(feature = "async-session")]
impl Fmp4Reader<AsyncReading> {
    pub fn tracks(&self) -> &[Fmp4Track] {
        &self.state.tracks
    }

    pub async fn next_sample(&mut self) -> Result<Option<Fmp4ReadSample>> {
        self.state.handle.next_sample().await
    }

    pub async fn recv_event(&mut self) -> Option<AsyncReaderEvent> {
        self.state.handle.recv_event().await
    }

    pub fn try_recv_event(&mut self) -> Option<AsyncReaderEvent> {
        self.state.handle.try_recv_event()
    }

    pub async fn finish(self) -> Result<Fmp4Reader<Finished>> {
        let status = self.state.handle.finish().await?;
        Ok(Fmp4Reader {
            config: self.config,
            state: Finished { status },
            _marker: PhantomData,
        })
    }
}

impl Fmp4Reader<Finished> {
    pub fn status(&self) -> &Fmp4ReaderStatus {
        &self.state.status
    }
}
