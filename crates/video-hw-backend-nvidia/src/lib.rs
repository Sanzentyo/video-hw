pub use video_hw_core::*;

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod pipeline;

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub use pipeline::{
    BoundedQueueRx, BoundedQueueTx, InFlightCredits, QueueRecvError, QueueSendError, QueueStats,
    bounded_queue,
};

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod transform;

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub use transform::{
    ColorRequest, Nv12Frame, RgbFrame, TransformDispatcher, TransformJob, TransformResult,
    make_argb_to_nv12_dummy, nv12_to_rgb24, should_enqueue_transform,
};

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod backend_transform_adapter;

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod bitstream;

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod pipeline_scheduler;

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod nv_meta_decoder;

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod nv_backend;

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub use nv_backend::{AnnexBPacker, NvDecoderAdapter, NvEncoderAdapter};
