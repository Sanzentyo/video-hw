use std::time::Duration;

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
use crate::cuda_transform::CudaNv12ToRgb;
use crate::{BackendError, ColorRequest, Frame};
#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
use crate::{
    Nv12Frame, RgbFrame, TransformDispatcher, TransformJob, TransformResult,
    should_enqueue_transform,
};

#[derive(Debug, Clone)]
pub(crate) enum DecodedUnit {
    MetadataOnly(Frame),
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    Nv12Cpu(Nv12Frame),
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    RgbCpu(RgbFrame),
}

pub(crate) trait BackendTransformAdapter {
    fn submit(
        &self,
        input: DecodedUnit,
        color: ColorRequest,
        resize: Option<(u32, u32)>,
    ) -> Result<Option<DecodedUnit>, BackendError>;

    fn recv_timeout(&self, timeout: Duration) -> Result<Option<DecodedUnit>, BackendError>;
}

#[derive(Debug)]
#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
pub(crate) struct NvidiaTransformAdapter {
    dispatcher: TransformDispatcher,
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    cuda: Option<CudaNv12ToRgb>,
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
impl NvidiaTransformAdapter {
    pub fn new(worker_count: usize, queue_capacity: usize) -> Self {
        Self {
            dispatcher: TransformDispatcher::new(worker_count, queue_capacity),
            #[cfg(all(
                feature = "backend-nvidia",
                any(target_os = "linux", target_os = "windows")
            ))]
            cuda: CudaNv12ToRgb::new().ok(),
        }
    }
}

#[cfg(all(
    feature = "backend-nvidia",
    any(target_os = "linux", target_os = "windows")
))]
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
                #[cfg(all(
                    feature = "backend-nvidia",
                    any(target_os = "linux", target_os = "windows")
                ))]
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
pub(crate) struct VtTransformAdapter {
    _private: (),
}

impl VtTransformAdapter {
    pub fn new() -> Self {
        Self::with_config(1, 4)
    }

    pub fn with_config(worker_count: usize, queue_capacity: usize) -> Self {
        let _ = worker_count;
        let _ = queue_capacity;
        Self { _private: () }
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
        let _ = color;
        let _ = resize;
        Ok(Some(input))
    }

    fn recv_timeout(&self, timeout: Duration) -> Result<Option<DecodedUnit>, BackendError> {
        let _ = timeout;
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    use crate::make_argb_to_nv12_dummy;

    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
    #[test]
    fn keep_native_fast_path_returns_input() {
        let adapter = NvidiaTransformAdapter::new(1, 4);
        let input = DecodedUnit::MetadataOnly(Frame {
            width: 64,
            height: 36,
            pixel_format: None,
            pts_90k: Some(0),
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            argb: None,
            force_keyframe: false,
        });
        let output = adapter
            .submit(input, ColorRequest::KeepNative, None)
            .unwrap();
        assert!(matches!(output, Some(DecodedUnit::MetadataOnly(_))));
    }

    #[cfg(all(
        feature = "backend-nvidia",
        any(target_os = "linux", target_os = "windows")
    ))]
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
            decode_info_flags: None,
            color_primaries: None,
            transfer_function: None,
            ycbcr_matrix: None,
            argb: None,
            force_keyframe: false,
        });
        let output = adapter
            .submit(input, ColorRequest::KeepNative, None)
            .unwrap();
        assert!(matches!(output, Some(DecodedUnit::MetadataOnly(_))));
    }
}
