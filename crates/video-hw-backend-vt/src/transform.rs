use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use metal::{CompileOptions, Device, MTLResourceOptions, MTLSize};

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
            let rgb = match nv12_to_rgb24_metal(&frame) {
                Ok(rgb) => rgb,
                Err(_) => nv12_to_rgb24(&frame)?,
            };
            Ok(TransformResult::Rgb(rgb))
        }
    }
}

pub fn nv12_to_rgb24_metal(frame: &Nv12Frame) -> Result<RgbFrame, BackendError> {
    validate_nv12_frame(frame)?;
    let device = Device::system_default().ok_or_else(|| {
        BackendError::UnsupportedConfig("Metal device is not available".to_string())
    })?;
    let options = CompileOptions::new();
    let library = device
        .new_library_with_source(NV12_TO_RGB_METAL, &options)
        .map_err(|err| BackendError::Backend(format!("metal library compile failed: {err}")))?;
    let function = library
        .get_function("nv12_to_rgb", None)
        .map_err(|err| BackendError::Backend(format!("metal function lookup failed: {err}")))?;
    let pipeline = device
        .new_compute_pipeline_state_with_function(&function)
        .map_err(|err| BackendError::Backend(format!("metal pipeline creation failed: {err}")))?;
    let command_queue = device.new_command_queue();
    let command_buffer = command_queue.new_command_buffer();
    let encoder = command_buffer.new_compute_command_encoder();

    let input_buffer = device.new_buffer_with_data(
        frame.data.as_ptr().cast(),
        frame.data.len() as u64,
        MTLResourceOptions::StorageModeShared,
    );
    let output_len = frame.width.saturating_mul(frame.height).saturating_mul(3);
    let output_buffer = device.new_buffer(output_len as u64, MTLResourceOptions::StorageModeShared);
    let params = [
        frame.width as u32,
        frame.height as u32,
        frame.pitch as u32,
        0_u32,
    ];
    let params_buffer = device.new_buffer_with_data(
        params.as_ptr().cast(),
        std::mem::size_of_val(&params) as u64,
        MTLResourceOptions::StorageModeShared,
    );

    encoder.set_compute_pipeline_state(&pipeline);
    encoder.set_buffer(0, Some(&input_buffer), 0);
    encoder.set_buffer(1, Some(&output_buffer), 0);
    encoder.set_buffer(2, Some(&params_buffer), 0);
    encoder.dispatch_threads(
        MTLSize::new(frame.width as u64, frame.height as u64, 1),
        MTLSize::new(16, 16, 1),
    );
    encoder.end_encoding();
    command_buffer.commit();
    command_buffer.wait_until_completed();

    let mut data = vec![0_u8; output_len];
    if output_len > 0 {
        let src = output_buffer.contents().cast::<u8>();
        if src.is_null() {
            return Err(BackendError::Backend(
                "metal output buffer contents is null".to_string(),
            ));
        }
        // Metal shared buffers expose CPU-readable memory after the command buffer completes.
        unsafe {
            data.copy_from_slice(std::slice::from_raw_parts(src, output_len));
        }
    }

    Ok(RgbFrame {
        width: frame.width,
        height: frame.height,
        pts_90k: frame.pts_90k,
        data,
    })
}

pub fn nv12_to_rgb24(frame: &Nv12Frame) -> Result<RgbFrame, BackendError> {
    validate_nv12_frame(frame)?;
    let width = frame.width;
    let height = frame.height;
    let pitch = frame.pitch.max(width);
    let luma_size = pitch
        .checked_mul(height)
        .ok_or_else(|| BackendError::InvalidInput("nv12 luma size overflow".to_string()))?;

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

fn validate_nv12_frame(frame: &Nv12Frame) -> Result<(), BackendError> {
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

    Ok(())
}

#[inline]
fn clip_to_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

const NV12_TO_RGB_METAL: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct Params {
    uint width;
    uint height;
    uint pitch;
    uint _pad;
};

static inline uchar clip_to_uchar(int value) {
    return (uchar)clamp(value, 0, 255);
}

kernel void nv12_to_rgb(
    device const uchar* nv12 [[buffer(0)]],
    device uchar* rgb [[buffer(1)]],
    constant Params& params [[buffer(2)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= params.width || gid.y >= params.height) {
        return;
    }

    uint y_index = gid.y * params.pitch + gid.x;
    uint uv_base = params.pitch * params.height;
    uint uv_index = uv_base + (gid.y / 2) * params.pitch + (gid.x & ~1u);
    int y_value = int(nv12[y_index]);
    int u_value = int(nv12[uv_index]);
    int v_value = int(nv12[uv_index + 1]);

    int c = max(y_value - 16, 0);
    int d = u_value - 128;
    int e = v_value - 128;
    uint dst = (gid.y * params.width + gid.x) * 3;
    rgb[dst] = clip_to_uchar((298 * c + 409 * e + 128) >> 8);
    rgb[dst + 1] = clip_to_uchar((298 * c - 100 * d - 208 * e + 128) >> 8);
    rgb[dst + 2] = clip_to_uchar((298 * c + 516 * d + 128) >> 8);
}
"#;

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
    fn metal_nv12_to_rgb_matches_cpu_path() {
        let frame = make_argb_to_nv12_dummy(32, 18);
        let cpu = nv12_to_rgb24(&frame).unwrap();
        let gpu = match nv12_to_rgb24_metal(&frame) {
            Ok(gpu) => gpu,
            Err(err) if err.is_runtime_unavailable() => {
                eprintln!("skip: Metal unavailable: {err}");
                return;
            }
            Err(err) => panic!("unexpected Metal transform error: {err:?}"),
        };

        assert_eq!(gpu.width, cpu.width);
        assert_eq!(gpu.height, cpu.height);
        assert_eq!(gpu.data, cpu.data);
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
