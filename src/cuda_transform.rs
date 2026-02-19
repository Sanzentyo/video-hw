use std::sync::Arc;

use cudarc::driver::{CudaContext, LaunchConfig, PushKernelArg};
use cudarc::nvrtc::compile_ptx;

use crate::{BackendError, Nv12Frame, RgbFrame};

const NV12_TO_RGB_KERNEL: &str = r#"
extern "C" __global__ void nv12_to_rgb_kernel(
    const unsigned char* nv12,
    unsigned int pitch,
    unsigned int width,
    unsigned int height,
    unsigned char* rgb
) {
    unsigned int x = blockIdx.x * blockDim.x + threadIdx.x;
    unsigned int y = blockIdx.y * blockDim.y + threadIdx.y;
    if (x >= width || y >= height) {
        return;
    }

    unsigned int y_idx = y * pitch + x;
    unsigned int uv_base = pitch * height;
    unsigned int uv_idx = uv_base + (y >> 1) * pitch + (x & ~1);

    int Y = (int)nv12[y_idx];
    int U = (int)nv12[uv_idx];
    int V = (int)nv12[uv_idx + 1];

    int C = Y - 16;
    if (C < 0) C = 0;
    int D = U - 128;
    int E = V - 128;

    int R = (298 * C + 409 * E + 128) >> 8;
    int G = (298 * C - 100 * D - 208 * E + 128) >> 8;
    int B = (298 * C + 516 * D + 128) >> 8;

    if (R < 0) R = 0; else if (R > 255) R = 255;
    if (G < 0) G = 0; else if (G > 255) G = 255;
    if (B < 0) B = 0; else if (B > 255) B = 255;

    unsigned int dst = (y * width + x) * 3;
    rgb[dst + 0] = (unsigned char)R;
    rgb[dst + 1] = (unsigned char)G;
    rgb[dst + 2] = (unsigned char)B;
}
"#;

#[derive(Debug, Clone)]
pub struct CudaNv12ToRgb {
    ctx: Arc<CudaContext>,
    stream: Arc<cudarc::driver::CudaStream>,
    kernel: cudarc::driver::CudaFunction,
}

impl CudaNv12ToRgb {
    pub fn new() -> Result<Self, BackendError> {
        let ctx = CudaContext::new(0)
            .map_err(|e| BackendError::UnsupportedConfig(format!("cuda init failed: {e}")))?;
        let ptx = compile_ptx(NV12_TO_RGB_KERNEL)
            .map_err(|e| BackendError::UnsupportedConfig(format!("nvrtc compile failed: {e}")))?;
        let module = ctx
            .load_module(ptx)
            .map_err(|e| BackendError::Backend(format!("cuda module load failed: {e}")))?;
        let kernel = module
            .load_function("nv12_to_rgb_kernel")
            .map_err(|e| BackendError::Backend(format!("cuda kernel load failed: {e}")))?;
        let stream = ctx.default_stream();
        Ok(Self {
            ctx,
            stream,
            kernel,
        })
    }

    pub fn convert(&self, frame: &Nv12Frame) -> Result<RgbFrame, BackendError> {
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

        self.ctx
            .bind_to_thread()
            .map_err(|e| BackendError::Backend(format!("cuda bind failed: {e}")))?;

        let input = self
            .stream
            .clone_htod(&frame.data[..total_size])
            .map_err(|e| BackendError::Backend(format!("cuda htod failed: {e}")))?;
        let mut output = self
            .stream
            .alloc_zeros::<u8>(width.saturating_mul(height).saturating_mul(3))
            .map_err(|e| BackendError::Backend(format!("cuda alloc failed: {e}")))?;

        let width_u32 = width as u32;
        let height_u32 = height as u32;
        let pitch_u32 = pitch as u32;
        let cfg = LaunchConfig {
            grid_dim: (width_u32.div_ceil(16), height_u32.div_ceil(16), 1),
            block_dim: (16, 16, 1),
            shared_mem_bytes: 0,
        };

        unsafe {
            self.stream
                .launch_builder(&self.kernel)
                .arg(&input)
                .arg(&pitch_u32)
                .arg(&width_u32)
                .arg(&height_u32)
                .arg(&mut output)
                .launch(cfg)
        }
        .map_err(|e| BackendError::Backend(format!("cuda launch failed: {e}")))?;

        self.stream
            .synchronize()
            .map_err(|e| BackendError::Backend(format!("cuda sync failed: {e}")))?;
        let rgb = self
            .stream
            .clone_dtoh(&output)
            .map_err(|e| BackendError::Backend(format!("cuda dtoh failed: {e}")))?;

        Ok(RgbFrame {
            width,
            height,
            pts_90k: frame.pts_90k,
            data: rgb,
        })
    }
}
