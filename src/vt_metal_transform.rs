use std::ffi::c_void;

use metal::{
    CompileOptions, ComputePipelineState, Device, MTLOrigin, MTLPixelFormat, MTLRegion, MTLSize,
    MTLStorageMode, MTLTextureType, MTLTextureUsage, TextureDescriptor,
};

use crate::{BackendError, Nv12Frame, RgbFrame};

const NV12_TO_RGBA_SHADER: &str = r#"
#include <metal_stdlib>
using namespace metal;

kernel void nv12_to_rgba(
    texture2d<float, access::sample> y_tex [[texture(0)]],
    texture2d<float, access::sample> uv_tex [[texture(1)]],
    texture2d<float, access::write> out_tex [[texture(2)]],
    uint2 gid [[thread_position_in_grid]]
) {
    uint width = out_tex.get_width();
    uint height = out_tex.get_height();
    if (gid.x >= width || gid.y >= height) {
        return;
    }

    constexpr sampler s(address::clamp_to_edge, filter::nearest);
    float2 coord = (float2(gid) + 0.5) / float2(width, height);
    float y = y_tex.sample(s, coord).r * 255.0;
    float2 uv = uv_tex.sample(s, coord).rg * 255.0;

    float C = max(y - 16.0, 0.0);
    float D = uv.x - 128.0;
    float E = uv.y - 128.0;

    float r = clamp((298.0 * C + 409.0 * E + 128.0) / 256.0, 0.0, 255.0);
    float g = clamp((298.0 * C - 100.0 * D - 208.0 * E + 128.0) / 256.0, 0.0, 255.0);
    float b = clamp((298.0 * C + 516.0 * D + 128.0) / 256.0, 0.0, 255.0);

    out_tex.write(float4(r / 255.0, g / 255.0, b / 255.0, 1.0), gid);
}
"#;

#[derive(Debug)]
pub struct VtMetalNv12ToRgb {
    device: Device,
    queue: metal::CommandQueue,
    pipeline: ComputePipelineState,
}

impl VtMetalNv12ToRgb {
    pub fn new() -> Result<Self, BackendError> {
        let device = Device::system_default().ok_or_else(|| {
            BackendError::DeviceLost("Metal system_default device not found".to_string())
        })?;
        let options = CompileOptions::new();
        let library = device
            .new_library_with_source(NV12_TO_RGBA_SHADER, &options)
            .map_err(|e| BackendError::Backend(format!("Metal shader compile failed: {e}")))?;
        let function = library
            .get_function("nv12_to_rgba", None)
            .map_err(|e| BackendError::Backend(format!("Metal function lookup failed: {e}")))?;
        let pipeline = device
            .new_compute_pipeline_state_with_function(&function)
            .map_err(|e| BackendError::Backend(format!("Metal pipeline creation failed: {e}")))?;
        let queue = device.new_command_queue();

        Ok(Self {
            device,
            queue,
            pipeline,
        })
    }

    pub fn convert(&self, frame: &Nv12Frame) -> Result<RgbFrame, BackendError> {
        validate_nv12(frame)?;

        let width = frame.width as u64;
        let height = frame.height as u64;
        let pitch = frame.pitch as u64;
        let luma_size = frame
            .pitch
            .checked_mul(frame.height)
            .ok_or_else(|| BackendError::InvalidInput("nv12 luma size overflow".to_string()))?;

        let y_texture = self.new_texture(width, height, MTLPixelFormat::R8Unorm, true, false);
        let uv_texture = self.new_texture(
            (width / 2).max(1),
            (height / 2).max(1),
            MTLPixelFormat::RG8Unorm,
            true,
            false,
        );
        let out_texture = self.new_texture(width, height, MTLPixelFormat::RGBA8Unorm, false, true);

        let full_y = MTLRegion {
            origin: MTLOrigin { x: 0, y: 0, z: 0 },
            size: MTLSize {
                width,
                height,
                depth: 1,
            },
        };
        let full_uv = MTLRegion {
            origin: MTLOrigin { x: 0, y: 0, z: 0 },
            size: MTLSize {
                width: (width / 2).max(1),
                height: (height / 2).max(1),
                depth: 1,
            },
        };

        y_texture.replace_region(full_y, 0, frame.data.as_ptr() as *const c_void, pitch);
        uv_texture.replace_region(
            full_uv,
            0,
            frame.data[luma_size..].as_ptr() as *const c_void,
            pitch,
        );

        let command_buffer = self.queue.new_command_buffer();
        let encoder = command_buffer.new_compute_command_encoder();
        encoder.set_compute_pipeline_state(&self.pipeline);
        encoder.set_texture(0, Some(&y_texture));
        encoder.set_texture(1, Some(&uv_texture));
        encoder.set_texture(2, Some(&out_texture));

        let threads = MTLSize {
            width,
            height,
            depth: 1,
        };
        let tg_width = self.pipeline.thread_execution_width();
        let tg_height = (self.pipeline.max_total_threads_per_threadgroup() / tg_width).max(1);
        let threads_per_group = MTLSize {
            width: tg_width,
            height: tg_height,
            depth: 1,
        };
        encoder.dispatch_threads(threads, threads_per_group);
        encoder.end_encoding();

        command_buffer.commit();
        command_buffer.wait_until_completed();

        let mut rgba = vec![0_u8; frame.width * frame.height * 4];
        let read_region = MTLRegion {
            origin: MTLOrigin { x: 0, y: 0, z: 0 },
            size: MTLSize {
                width,
                height,
                depth: 1,
            },
        };
        out_texture.get_bytes(
            rgba.as_mut_ptr() as *mut c_void,
            (frame.width * 4) as u64,
            read_region,
            0,
        );

        let mut rgb = vec![0_u8; frame.width * frame.height * 3];
        for (src, dst) in rgba.chunks_exact(4).zip(rgb.chunks_exact_mut(3)) {
            dst[0] = src[0];
            dst[1] = src[1];
            dst[2] = src[2];
        }

        Ok(RgbFrame {
            width: frame.width,
            height: frame.height,
            pts_90k: frame.pts_90k,
            data: rgb,
        })
    }

    fn new_texture(
        &self,
        width: u64,
        height: u64,
        format: MTLPixelFormat,
        shader_read: bool,
        shader_write: bool,
    ) -> metal::Texture {
        let desc = TextureDescriptor::new();
        desc.set_texture_type(MTLTextureType::D2);
        desc.set_width(width);
        desc.set_height(height);
        desc.set_depth(1);
        desc.set_pixel_format(format);
        desc.set_storage_mode(MTLStorageMode::Shared);

        let mut usage = MTLTextureUsage::Unknown;
        if shader_read {
            usage |= MTLTextureUsage::ShaderRead;
        }
        if shader_write {
            usage |= MTLTextureUsage::ShaderWrite;
        }
        desc.set_usage(usage);

        self.device.new_texture(&desc)
    }
}

fn validate_nv12(frame: &Nv12Frame) -> Result<(), BackendError> {
    if frame.width == 0 || frame.height == 0 {
        return Err(BackendError::InvalidInput(
            "nv12 frame dimensions must be positive".to_string(),
        ));
    }
    if frame.width > frame.pitch {
        return Err(BackendError::InvalidInput(
            "nv12 width exceeds pitch".to_string(),
        ));
    }

    let luma_size = frame
        .pitch
        .checked_mul(frame.height)
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
