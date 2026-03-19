use super::{config::Fmp4ReaderStatus, core::ReaderCore};
#[cfg(feature = "async-session")]
use super::{config::Fmp4Track, session_async::AsyncReaderHandle};

#[derive(Debug, Clone, Copy, Default)]
pub struct ReaderReady;

#[derive(Debug)]
pub struct SyncReading {
    pub(crate) core: ReaderCore,
}

#[cfg(feature = "async-session")]
pub struct AsyncReading {
    pub(crate) handle: AsyncReaderHandle,
    pub(crate) tracks: Vec<Fmp4Track>,
}

#[cfg(feature = "async-session")]
impl core::fmt::Debug for AsyncReading {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AsyncReading").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct Finished {
    pub(crate) status: Fmp4ReaderStatus,
}
