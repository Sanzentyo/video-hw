use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::BackendError;
use crate::pipeline::{BoundedQueueRx, QueueRecvError, QueueSendError, bounded_queue};

#[derive(Debug, Clone)]
pub struct Nv12Frame {
    pub width: usize,
    pub height: usize,
    pub pitch: usize,
    pub pts_90k: Option<i64>,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RgbFrame {
    pub width: usize,
    pub height: usize,
    pub pts_90k: Option<i64>,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorRequest {
    KeepNative,
    Rgb8,
    Rgba8,
}

impl ColorRequest {
    pub fn needs_transform(self) -> bool {
        !matches!(self, Self::KeepNative)
    }
}

#[derive(Debug, Clone)]
pub enum TransformJob {
    Nv12ToRgb(Nv12Frame),
}

#[derive(Debug, Clone)]
pub enum TransformResult {
    Rgb(RgbFrame),
}

#[derive(Debug)]
pub struct TransformDispatcher {
    jobs_tx: Option<mpsc::Sender<TransformJob>>,
    results_rx: BoundedQueueRx<Result<TransformResult, BackendError>>,
    workers: Vec<JoinHandle<()>>,
}

impl TransformDispatcher {
    pub fn new(worker_count: usize, result_queue_capacity: usize) -> Self {
        let (jobs_tx, jobs_rx) = mpsc::channel::<TransformJob>();
        let jobs_rx = Arc::new(Mutex::new(jobs_rx));
        let (results_tx, results_rx) = bounded_queue(result_queue_capacity.max(1));

        let mut workers = Vec::new();
        for _ in 0..worker_count.max(1) {
            let jobs = Arc::clone(&jobs_rx);
            let results = results_tx.clone();
            workers.push(thread::spawn(move || {
                loop {
                    let job = {
                        let lock = jobs.lock();
                        let Ok(receiver) = lock else {
                            break;
                        };
                        receiver.recv()
                    };
                    let Ok(job) = job else {
                        break;
                    };
                    let result = run_job(job);
                    let _ = results.send(result);
                }
            }));
        }

        Self {
            jobs_tx: Some(jobs_tx),
            results_rx,
            workers,
        }
    }

    pub fn submit(&self, job: TransformJob) -> Result<(), QueueSendError> {
        let Some(tx) = &self.jobs_tx else {
            return Err(QueueSendError::Disconnected);
        };
        tx.send(job).map_err(|_| QueueSendError::Disconnected)
    }

    pub fn recv(&self) -> Result<Result<TransformResult, BackendError>, QueueRecvError> {
        self.results_rx.recv()
    }

    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Result<TransformResult, BackendError>, QueueRecvError> {
        self.results_rx.recv_timeout(timeout)
    }

    pub fn try_recv(&self) -> Result<Result<TransformResult, BackendError>, QueueRecvError> {
        self.results_rx.try_recv()
    }
}

impl Drop for TransformDispatcher {
    fn drop(&mut self) {
        let _ = self.jobs_tx.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn run_job(job: TransformJob) -> Result<TransformResult, BackendError> {
    match job {
        TransformJob::Nv12ToRgb(frame) => {
            let rgb = nv12_to_rgb24(&frame)?;
            Ok(TransformResult::Rgb(rgb))
        }
    }
}

pub fn nv12_to_rgb24(frame: &Nv12Frame) -> Result<RgbFrame, BackendError> {
    let width = frame.width;
    let height = frame.height;
    let pitch = frame.pitch.max(width);
    if width == 0 || height == 0 {
        return Err(BackendError::InvalidInput(
            "nv12 frame dimensions must be positive".to_string(),
        ));
    }
    if width > pitch {
        return Err(BackendError::InvalidInput(
            "nv12 width exceeds pitch".to_string(),
        ));
    }
    let luma_size = pitch
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("nv12 luma size overflow".to_string()))?;
    let total_size = luma_size
        .checked_add(luma_size / 2)
        .ok_or_else(|| BackendError::InvalidInput("nv12 total size overflow".to_string()))?;
    if frame.data.len() < total_size {
        return Err(BackendError::InvalidInput(
            "nv12 data is smaller than expected".to_string(),
        ));
    }

    let uv_base = luma_size;
    let mut rgb = vec![0_u8; width.saturating_mul(height).saturating_mul(3)];
    for y in 0..height {
        let y_row = y * pitch;
        let uv_row = uv_base + (y / 2) * pitch;
        let dst_row = y * width * 3;
        for x in 0..width {
            let y_value = i32::from(frame.data[y_row + x]);
            let uv_index = uv_row + (x & !1);
            let u_value = i32::from(frame.data[uv_index]);
            let v_value = i32::from(frame.data[uv_index + 1]);

            let c = (y_value - 16).max(0);
            let d = u_value - 128;
            let e = v_value - 128;
            let r = clip_to_u8((298 * c + 409 * e + 128) >> 8);
            let g = clip_to_u8((298 * c - 100 * d - 208 * e + 128) >> 8);
            let b = clip_to_u8((298 * c + 516 * d + 128) >> 8);

            let dst = dst_row + x * 3;
            rgb[dst] = r;
            rgb[dst + 1] = g;
            rgb[dst + 2] = b;
        }
    }

    Ok(RgbFrame {
        width,
        height,
        pts_90k: frame.pts_90k,
        data: rgb,
    })
}

#[inline]
fn clip_to_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

pub fn make_argb_to_nv12_dummy(width: usize, height: usize) -> Nv12Frame {
    let pitch = width.max(1);
    let luma_size = pitch * height.max(1);
    let chroma_size = luma_size / 2;
    let mut data = vec![0_u8; luma_size + chroma_size];
    for y in 0..height {
        for x in 0..width {
            data[y * pitch + x] = ((x + y) % 256) as u8;
        }
    }
    for i in 0..chroma_size {
        data[luma_size + i] = 128;
    }
    Nv12Frame {
        width,
        height,
        pitch,
        pts_90k: None,
        data,
    }
}

pub fn should_enqueue_transform(color: ColorRequest, resize: Option<(u32, u32)>) -> bool {
    color.needs_transform() || resize.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nv12_to_rgb_returns_expected_size() {
        let frame = make_argb_to_nv12_dummy(64, 36);
        let rgb = nv12_to_rgb24(&frame).unwrap();
        assert_eq!(rgb.width, 64);
        assert_eq!(rgb.height, 36);
        assert_eq!(rgb.data.len(), 64 * 36 * 3);
    }

    #[test]
    fn dispatcher_runs_transform_job() {
        let dispatcher = TransformDispatcher::new(2, 8);
        let frame = make_argb_to_nv12_dummy(32, 18);
        dispatcher.submit(TransformJob::Nv12ToRgb(frame)).unwrap();
        let result = dispatcher
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        match result {
            TransformResult::Rgb(rgb) => {
                assert_eq!(rgb.width, 32);
                assert_eq!(rgb.height, 18);
            }
        }
    }

    #[test]
    fn keep_native_fast_path_bypasses_transform() {
        assert!(!should_enqueue_transform(ColorRequest::KeepNative, None));
        assert!(should_enqueue_transform(ColorRequest::Rgb8, None));
        assert!(should_enqueue_transform(
            ColorRequest::KeepNative,
            Some((640, 360))
        ));
    }
}
