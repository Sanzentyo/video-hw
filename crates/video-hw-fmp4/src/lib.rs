//! Typestate-based fragmented MP4 writer/reader built on top of `video-hw`.
//!
//! Notes from camera-recorder validation on macOS/QuickTime:
//!
//! - Recording PTS should be generated from the configured FPS cadence, not from capture wallclock.
//!   Using capture timestamps made duration unstable across devices and could stretch playback.
//! - For fragmented MP4 intended for QuickTime/Finder, keep the final duration in `mehd` and leave
//!   `mvhd`/`tkhd`/`mdhd` duration at `0`. Setting both caused QuickTime/Finder to show roughly
//!   double duration even though `ffprobe` reported the correct fragment timeline.
//!
//! The camera examples in `examples/` follow those rules.

pub mod fmp4_reader;
pub mod fmp4_writer;

#[cfg(test)]
mod tests;

pub use fmp4_reader::{
    Finished as ReaderFinished, Fmp4ReadSample, Fmp4Reader, Fmp4ReaderConfig, Fmp4ReaderStatus,
    Fmp4Track, ReaderReady as Fmp4ReaderReady, SyncReading,
};
pub use fmp4_writer::{
    ArgbFrame, Finished, Fmp4Writer, Fmp4WriterConfig, Fmp4WriterStatus, Fmp4WriterSummary,
    FragmentFrames, FrameRate, FrameSize, Pts90k, Ready, RgbaFrame, SyncRecording,
};

#[cfg(feature = "async-session")]
pub use fmp4_reader::{AsyncReaderEvent, AsyncReading};
#[cfg(feature = "async-session")]
pub use fmp4_writer::{AsyncRecording, AsyncWriterEvent};
