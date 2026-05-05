use std::time::Duration;

use crate::{BackendError, ColorRequest, Frame};
#[cfg(any(test, all(target_os = "macos", feature = "backend-vt")))]
use crate::{Nv12Frame, RgbFrame, TransformJob};
use crate::{TransformDispatcher, TransformResult, should_enqueue_transform};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum DecodedUnit {
    MetadataOnly(Frame),
    #[cfg(any(test, all(target_os = "macos", feature = "backend-vt")))]
    Nv12Cpu(Nv12Frame),
    #[cfg(any(test, all(target_os = "macos", feature = "backend-vt")))]
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
#[cfg(any(test, all(target_os = "macos", feature = "backend-vt")))]
pub(crate) struct VtTransformAdapter {
    dispatcher: TransformDispatcher,
}

#[cfg(any(test, all(target_os = "macos", feature = "backend-vt")))]
impl VtTransformAdapter {
    pub fn new() -> Self {
        Self::with_config(1, 4)
    }

    pub fn with_config(worker_count: usize, queue_capacity: usize) -> Self {
        Self {
            dispatcher: TransformDispatcher::new(worker_count, queue_capacity),
        }
    }
}

#[cfg(any(test, all(target_os = "macos", feature = "backend-vt")))]
impl Default for VtTransformAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, all(target_os = "macos", feature = "backend-vt")))]
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
                self.dispatcher
                    .submit(TransformJob::Nv12ToRgb(frame))
                    .map_err(|e| BackendError::TemporaryBackpressure(format!("{e:?}")))?;
                Ok(None)
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_argb_to_nv12_dummy;

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
            nv12: None,
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
        if let Some(DecodedUnit::RgbCpu(rgb)) = output {
            assert_eq!(rgb.width, 64);
            assert_eq!(rgb.height, 36);
            return;
        }
        assert!(output.is_none());
        let reaped = adapter.recv_timeout(Duration::from_secs(1)).unwrap();
        match reaped {
            Some(DecodedUnit::RgbCpu(rgb)) => {
                assert_eq!(rgb.width, 64);
                assert_eq!(rgb.height, 36);
            }
            other => panic!("expected RGB output, got {other:?}"),
        }
    }
}
