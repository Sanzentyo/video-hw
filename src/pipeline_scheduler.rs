use std::thread::{self, JoinHandle};
use std::time::Duration;
use std::{
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::backend_transform_adapter::{BackendTransformAdapter, DecodedUnit};
use crate::pipeline::{
    BoundedQueueRx, BoundedQueueTx, QueueRecvError, QueueSendError, bounded_queue,
};
use crate::{BackendError, ColorRequest};

#[derive(Debug)]
enum SchedulerTask {
    Frame {
        generation: u64,
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
    generation: Arc<AtomicU64>,
    worker: Option<JoinHandle<()>>,
}

impl PipelineScheduler {
    pub fn new<A>(adapter: A, queue_capacity: usize) -> Self
    where
        A: BackendTransformAdapter + Send + 'static,
    {
        let (in_tx, in_rx) = bounded_queue(queue_capacity.max(1));
        let (out_tx, out_rx) = bounded_queue(queue_capacity.max(1));
        let generation = Arc::new(AtomicU64::new(1));
        let worker_generation = Arc::clone(&generation);
        let worker =
            thread::spawn(move || run_scheduler(adapter, in_rx, out_tx, worker_generation));
        Self {
            in_tx,
            out_rx,
            generation,
            worker: Some(worker),
        }
    }

    pub fn submit(
        &self,
        input: DecodedUnit,
        color: ColorRequest,
        resize: Option<(u32, u32)>,
    ) -> Result<(), BackendError> {
        self.submit_with_generation(self.generation(), input, color, resize)
    }

    pub fn submit_with_generation(
        &self,
        generation: u64,
        input: DecodedUnit,
        color: ColorRequest,
        resize: Option<(u32, u32)>,
    ) -> Result<(), BackendError> {
        self.in_tx
            .try_send(SchedulerTask::Frame {
                generation,
                input,
                color,
                resize,
            })
            .map_err(map_send_err)
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub fn set_generation(&self, generation: u64) {
        self.generation.store(generation.max(1), Ordering::Relaxed);
    }

    pub fn advance_generation(&self) -> u64 {
        self.generation
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1)
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
    generation: Arc<AtomicU64>,
) where
    A: BackendTransformAdapter,
{
    while let Ok(task) = in_rx.recv() {
        match task {
            SchedulerTask::Shutdown => break,
            SchedulerTask::Frame {
                generation: task_generation,
                input,
                color,
                resize,
            } => {
                let active_generation = generation.load(Ordering::Relaxed);
                if task_generation != active_generation {
                    let _ = out_tx.send(Err(BackendError::TemporaryBackpressure(format!(
                        "stale pipeline generation dropped: task={task_generation}, active={active_generation}"
                    ))));
                    continue;
                }
                let submit_result = adapter.submit(input, color, resize);
                match submit_result {
                    Ok(Some(output)) => {
                        let latest_generation = generation.load(Ordering::Relaxed);
                        if task_generation == latest_generation {
                            let _ = out_tx.send(Ok(output));
                        } else {
                            let _ = out_tx.send(Err(BackendError::TemporaryBackpressure(format!(
                                "stale pipeline generation dropped after submit: task={task_generation}, active={latest_generation}"
                            ))));
                        }
                    }
                    Ok(None) => {
                        // Adapter accepted async work; reap until one result appears or fails.
                        loop {
                            match adapter.recv_timeout(Duration::from_millis(5)) {
                                Ok(Some(output)) => {
                                    let latest_generation = generation.load(Ordering::Relaxed);
                                    if task_generation == latest_generation {
                                        let _ = out_tx.send(Ok(output));
                                    } else {
                                        let _ = out_tx.send(Err(
                                            BackendError::TemporaryBackpressure(format!(
                                                "stale pipeline generation dropped after reap: task={task_generation}, active={latest_generation}"
                                            )),
                                        ));
                                    }
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
                    force_keyframe: false,
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

    #[test]
    fn stale_generation_is_dropped() {
        let scheduler = PipelineScheduler::new(NvidiaTransformAdapter::new(1, 4), 4);
        let stale_generation = scheduler.generation();
        let _ = scheduler.advance_generation();
        scheduler
            .submit_with_generation(
                stale_generation,
                DecodedUnit::MetadataOnly(Frame {
                    width: 64,
                    height: 36,
                    pixel_format: None,
                    pts_90k: Some(0),
                    argb: None,
                    force_keyframe: false,
                }),
                ColorRequest::KeepNative,
                None,
            )
            .unwrap();
        let output = scheduler
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert!(matches!(
            output,
            Err(BackendError::TemporaryBackpressure(_))
        ));
    }
}
