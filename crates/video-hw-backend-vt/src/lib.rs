pub use video_hw_core::*;

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
mod pipeline;

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
pub use pipeline::{
    BoundedQueueRx, BoundedQueueTx, InFlightCredits, QueueRecvError, QueueSendError, QueueStats,
    bounded_queue,
};

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
mod transform;

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
pub use transform::{
    ColorRequest, Nv12Frame, RgbFrame, TransformDispatcher, TransformJob, TransformResult,
    make_argb_to_nv12_dummy, nv12_to_rgb24, should_enqueue_transform,
};

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
mod backend_transform_adapter;
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
mod bitstream;
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
mod pipeline_scheduler;

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
mod vt_backend;

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
pub use vt_backend::{VtDecoderAdapter, VtEncoderAdapter};
