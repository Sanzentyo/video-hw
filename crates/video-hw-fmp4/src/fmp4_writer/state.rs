#[cfg(feature = "async-session")]
use super::session_async::AsyncWriterHandle;
use super::{config::Fmp4WriterSummary, core::WriterCore};

#[derive(Debug, Clone, Copy, Default)]
pub struct Ready;

pub struct SyncRecording {
    pub(crate) core: WriterCore,
}

impl core::fmt::Debug for SyncRecording {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SyncRecording").finish_non_exhaustive()
    }
}

#[cfg(feature = "async-session")]
pub struct AsyncRecording {
    pub(crate) handle: AsyncWriterHandle,
}

#[cfg(feature = "async-session")]
impl core::fmt::Debug for AsyncRecording {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AsyncRecording").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct Finished {
    pub(crate) summary: Fmp4WriterSummary,
}
