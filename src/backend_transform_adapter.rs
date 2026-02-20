use std::time::Duration;

#[cfg(all(feature = "backend-nvidia", any(target_os = "linux", target_os = "windows")))]
use crate::cuda_transform::CudaNv12ToRgb;
#[cfg(all(target_os = "macos", feature = "backend-vt"))]
use crate::vt_metal_transform::VtMetalNv12ToRgb;
use crate::{
    BackendError, ColorRequest, Frame, Nv12Frame, RgbFrame, TransformDispatcher, TransformJob,
    TransformResult, should_enqueue_transform,
};

#[derive(Debug, Clone)]
pub enum DecodedUnit {
    MetadataOnly(Frame),
    Nv12Cpu(Nv12Frame),
    RgbCpu(RgbFrame),
}

pub trait BackendTransformAdapter {
    fn submit(
        &self,
        input: DecodedUnit,
        color: ColorRequest,
        resize: Option<(u32, u32)>,
    ) -> Result<Option<DecodedUnit>, BackendError>;

    fn recv_timeout(&self, timeout: Duration) -> Result<Option<DecodedUnit>, BackendError>;
}

#[derive(Debug)]
pub struct NvidiaTransformAdapter {
    dispatcher: TransformDispatcher,
    #[cfg(all(feature = "backend-nvidia", any(target_os = "linux", target_os = "windows")))]
    cuda: Option<CudaNv12ToRgb>,
}

impl NvidiaTransformAdapter {
    pub fn new(worker_count: usize, queue_capacity: usize) -> Self {
        Self {
            dispatcher: TransformDispatcher::new(worker_count, queue_capacity),
            #[cfg(all(feature = "backend-nvidia", any(target_os = "linux", target_os = "windows")))]
            cuda: CudaNv12ToRgb::new().ok(),
        }
    }
}

impl BackendTransformAdapter for NvidiaTransformAdapter {
    fn submit(
        &self,
        input: DecodedUnit,
        color: ColorRequest,
        resize: Option<(u32, u32)>,
    ) -> Result<Option<DecodedUnit>, BackendError> {
        if !should_enqueue_transform(color, resize) {
            return Ok(Some(input));
        }

        match (input, color) {
            (DecodedUnit::Nv12Cpu(frame), ColorRequest::Rgb8 | ColorRequest::Rgba8) => {
                #[cfg(all(feature = "backend-nvidia", any(target_os = "linux", target_os = "windows")))]
                if let Some(cuda) = &self.cuda
                    && let Ok(rgb) = cuda.convert(&frame)
                {
                    return Ok(Some(DecodedUnit::RgbCpu(rgb)));
                }
                self.dispatcher
                    .submit(TransformJob::Nv12ToRgb(frame))
                    .map_err(|e| BackendError::TemporaryBackpressure(format!("{e:?}")))?;
                Ok(None)
            }
            (DecodedUnit::MetadataOnly(frame), _) => Ok(Some(DecodedUnit::MetadataOnly(frame))),
            (other, _) => Ok(Some(other)),
        }
    }

    fn recv_timeout(&self, timeout: Duration) -> Result<Option<DecodedUnit>, BackendError> {
        match self.dispatcher.recv_timeout(timeout) {
            Ok(Ok(TransformResult::Rgb(rgb))) => Ok(Some(DecodedUnit::RgbCpu(rgb))),
            Ok(Err(err)) => Err(err),
            Err(crate::QueueRecvError::Timeout) | Err(crate::QueueRecvError::Empty) => Ok(None),
            Err(err) => Err(BackendError::Backend(format!(
                "transform recv failed: {err:?}"
            ))),
        }
    }
}

#[derive(Debug)]
pub struct VtTransformAdapter {
    dispatcher: TransformDispatcher,
    #[cfg(all(target_os = "macos", feature = "backend-vt"))]
    metal: Option<VtMetalNv12ToRgb>,
}

impl VtTransformAdapter {
    pub fn new() -> Self {
        Self::with_config(1, 4)
    }

    pub fn with_config(worker_count: usize, queue_capacity: usize) -> Self {
        Self {
            dispatcher: TransformDispatcher::new(worker_count, queue_capacity),
            #[cfg(all(target_os = "macos", feature = "backend-vt"))]
            metal: if vt_gpu_transform_enabled() {
                VtMetalNv12ToRgb::new().ok()
            } else {
                None
            },
        }
    }
}

impl Default for VtTransformAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl BackendTransformAdapter for VtTransformAdapter {
    fn submit(
        &self,
        input: DecodedUnit,
        color: ColorRequest,
        resize: Option<(u32, u32)>,
    ) -> Result<Option<DecodedUnit>, BackendError> {
        if !should_enqueue_transform(color, resize) {
            return Ok(Some(input));
        }

        match (input, color) {
            (DecodedUnit::Nv12Cpu(frame), ColorRequest::Rgb8 | ColorRequest::Rgba8) => {
                #[cfg(all(target_os = "macos", feature = "backend-vt"))]
                if let Some(metal) = &self.metal
                    && let Ok(rgb) = metal.convert(&frame)
                {
                    return Ok(Some(DecodedUnit::RgbCpu(rgb)));
                }
                self.dispatcher
                    .submit(TransformJob::Nv12ToRgb(frame))
                    .map_err(|e| BackendError::TemporaryBackpressure(format!("{e:?}")))?;
                Ok(None)
            }
            (DecodedUnit::MetadataOnly(frame), _) => Ok(Some(DecodedUnit::MetadataOnly(frame))),
            (other, _) => Ok(Some(other)),
        }
    }

    fn recv_timeout(&self, timeout: Duration) -> Result<Option<DecodedUnit>, BackendError> {
        match self.dispatcher.recv_timeout(timeout) {
            Ok(Ok(TransformResult::Rgb(rgb))) => Ok(Some(DecodedUnit::RgbCpu(rgb))),
            Ok(Err(err)) => Err(err),
            Err(crate::QueueRecvError::Timeout) | Err(crate::QueueRecvError::Empty) => Ok(None),
            Err(err) => Err(BackendError::Backend(format!(
                "transform recv failed: {err:?}"
            ))),
        }
    }
}

#[cfg(all(target_os = "macos", feature = "backend-vt"))]
fn vt_gpu_transform_enabled() -> bool {
    match std::env::var("VIDEO_HW_VT_GPU_TRANSFORM") {
        Ok(raw) => {
            let v = raw.trim().to_ascii_lowercase();
            !matches!(v.as_str(), "0" | "false" | "off" | "no")
        }
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_argb_to_nv12_dummy;

    #[test]
    fn keep_native_fast_path_returns_input() {
        let adapter = NvidiaTransformAdapter::new(1, 4);
        let input = DecodedUnit::MetadataOnly(Frame {
            width: 64,
            height: 36,
            pixel_format: None,
            pts_90k: Some(0),
            argb: None,
            force_keyframe: false,
        });
        let output = adapter
            .submit(input, ColorRequest::KeepNative, None)
            .unwrap();
        assert!(matches!(output, Some(DecodedUnit::MetadataOnly(_))));
    }

    #[test]
    fn nv12_rgb_request_runs_worker() {
        let adapter = NvidiaTransformAdapter::new(1, 4);
        let nv12 = make_argb_to_nv12_dummy(64, 36);
        let output = adapter
            .submit(DecodedUnit::Nv12Cpu(nv12), ColorRequest::Rgb8, None)
            .unwrap();
        if let Some(DecodedUnit::RgbCpu(_)) = output {
            return;
        }
        assert!(output.is_none());
        let reaped = adapter.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(reaped, Some(DecodedUnit::RgbCpu(_))));
    }

    #[test]
    fn vt_keep_native_fast_path_returns_input() {
        let adapter = VtTransformAdapter::new();
        let input = DecodedUnit::MetadataOnly(Frame {
            width: 64,
            height: 36,
            pixel_format: None,
            pts_90k: Some(0),
            argb: None,
            force_keyframe: false,
        });
        let output = adapter
            .submit(input, ColorRequest::KeepNative, None)
            .unwrap();
        assert!(matches!(output, Some(DecodedUnit::MetadataOnly(_))));
    }

    #[test]
    fn vt_nv12_rgb_request_runs_worker() {
        let adapter = VtTransformAdapter::new();
        let nv12 = make_argb_to_nv12_dummy(64, 36);
        let output = adapter
            .submit(DecodedUnit::Nv12Cpu(nv12), ColorRequest::Rgb8, None)
            .unwrap();
        if let Some(DecodedUnit::RgbCpu(_)) = output {
            return;
        }
        assert!(output.is_none());
        let reaped = adapter.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(reaped, Some(DecodedUnit::RgbCpu(_))));
    }
}
