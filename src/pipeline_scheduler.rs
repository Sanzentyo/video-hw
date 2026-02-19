use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::backend_transform_adapter::{BackendTransformAdapter, DecodedUnit};
use crate::pipeline::{BoundedQueueRx, BoundedQueueTx, QueueRecvError, QueueSendError, bounded_queue};
use crate::{BackendError, ColorRequest};

#[derive(Debug)]
enum SchedulerTask {
    Frame {
        input: DecodedUnit,
        color: ColorRequest,
        resize: Option<(u32, u32)>,
    },
    Shutdown,
}

#[derive(Debug)]
pub struct PipelineScheduler {
    in_tx: BoundedQueueTx<SchedulerTask>,
    out_rx: BoundedQueueRx<Result<DecodedUnit, BackendError>>,
    worker: Option<JoinHandle<()>>,
}

impl PipelineScheduler {
    pub fn new<A>(adapter: A, queue_capacity: usize) -> Self
    where
        A: BackendTransformAdapter + Send + 'static,
    {
        let (in_tx, in_rx) = bounded_queue(queue_capacity.max(1));
        let (out_tx, out_rx) = bounded_queue(queue_capacity.max(1));
        let worker = thread::spawn(move || run_scheduler(adapter, in_rx, out_tx));
        Self {
            in_tx,
            out_rx,
            worker: Some(worker),
        }
    }

    pub fn submit(
        &self,
        input: DecodedUnit,
        color: ColorRequest,
        resize: Option<(u32, u32)>,
    ) -> Result<(), BackendError> {
        self.in_tx
            .try_send(SchedulerTask::Frame {
                input,
                color,
                resize,
            })
            .map_err(map_send_err)
    }

    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<Result<DecodedUnit, BackendError>>, BackendError> {
        match self.out_rx.recv_timeout(timeout) {
            Ok(v) => Ok(Some(v)),
            Err(QueueRecvError::Timeout) | Err(QueueRecvError::Empty) => Ok(None),
            Err(err) => Err(BackendError::Backend(format!(
                "pipeline output receive failed: {err:?}"
            ))),
        }
    }
}

impl Drop for PipelineScheduler {
    fn drop(&mut self) {
        let _ = self.in_tx.send(SchedulerTask::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_scheduler<A>(
    adapter: A,
    in_rx: BoundedQueueRx<SchedulerTask>,
    out_tx: BoundedQueueTx<Result<DecodedUnit, BackendError>>,
) where
    A: BackendTransformAdapter,
{
    while let Ok(task) = in_rx.recv() {
        match task {
            SchedulerTask::Shutdown => break,
            SchedulerTask::Frame {
                input,
                color,
                resize,
            } => {
                let submit_result = adapter.submit(input, color, resize);
                match submit_result {
                    Ok(Some(output)) => {
                        let _ = out_tx.send(Ok(output));
                    }
                    Ok(None) => {
                        // Adapter accepted async work; reap until one result appears or fails.
                        loop {
                            match adapter.recv_timeout(Duration::from_millis(5)) {
                                Ok(Some(output)) => {
                                    let _ = out_tx.send(Ok(output));
                                    break;
                                }
                                Ok(None) => continue,
                                Err(err) => {
                                    let _ = out_tx.send(Err(err));
                                    break;
                                }
                            }
                        }
                    }
                    Err(err) => {
                        let _ = out_tx.send(Err(err));
                    }
                }
            }
        }
    }
}

fn map_send_err(err: QueueSendError) -> BackendError {
    match err {
        QueueSendError::Full => {
            BackendError::TemporaryBackpressure("pipeline input queue is full".to_string())
        }
        QueueSendError::Disconnected => {
            BackendError::Backend("pipeline input queue disconnected".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Frame, Nv12Frame, backend_transform_adapter::NvidiaTransformAdapter};

    #[test]
    fn keep_native_frame_passes_through_scheduler() {
        let scheduler = PipelineScheduler::new(NvidiaTransformAdapter::new(1, 4), 4);
        scheduler
            .submit(
                DecodedUnit::MetadataOnly(Frame {
                    width: 64,
                    height: 36,
                    pixel_format: None,
                    pts_90k: Some(0),
                    argb: None,
                }),
                ColorRequest::KeepNative,
                None,
            )
            .unwrap();
        let output = scheduler
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(output, DecodedUnit::MetadataOnly(_)));
    }

    #[test]
    fn rgb_request_reaps_async_result() {
        let scheduler = PipelineScheduler::new(NvidiaTransformAdapter::new(1, 4), 4);
        let nv12 = Nv12Frame {
            width: 32,
            height: 18,
            pitch: 32,
            pts_90k: Some(1000),
            data: vec![128; 32 * 18 + (32 * 18 / 2)],
        };
        scheduler
            .submit(DecodedUnit::Nv12Cpu(nv12), ColorRequest::Rgb8, None)
            .unwrap();

        let output = scheduler
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(output, DecodedUnit::RgbCpu(_)));
    }
}

