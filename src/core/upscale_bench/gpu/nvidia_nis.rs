use super::{align_to, GpuUpscaleOutput, TEXTURE_FORMAT};
use crate::core::gpu_effect::color_image_to_rgba;
use egui::ColorImage;
use std::borrow::Cow;
use std::sync::mpsc;
use std::time::Instant;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct NisParams {
    source_output: [u32; 4],
    config0: [f32; 4],
    config1: [f32; 4],
    config2: [f32; 4],
    config3: [f32; 4],
}

pub(super) struct NvidiaNisBench {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

impl NvidiaNisBench {
    pub(super) async fn try_new(device: &wgpu::Device) -> Option<Self> {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bench = Self::new(device);
        match device.pop_error_scope().await {
            Some(error) => {
                eprintln!("NVIDIA Image Scaling bench candidate disabled: {error}");
                None
            }
            None => Some(bench),
        }
    }

    fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-nvidia-nis-shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("../../nvidia_nis.wgsl"))),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("suisuiview-nvidia-nis-bind-group-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: TEXTURE_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("suisuiview-nvidia-nis-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("suisuiview-nvidia-nis-pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("cs_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            pipeline,
            bind_group_layout,
        }
    }

    pub(super) fn apply(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &ColorImage,
        output_size: [usize; 2],
    ) -> Result<GpuUpscaleOutput, String> {
        let [source_width, source_height] = image.size;
        let [output_width, output_height] = output_size;
        validate_output_size(image.size, output_size)?;

        let started = Instant::now();
        let source_bytes = color_image_to_rgba(image);
        let source_extent = wgpu::Extent3d {
            width: source_width as u32,
            height: source_height as u32,
            depth_or_array_layers: 1,
        };
        let output_extent = wgpu::Extent3d {
            width: output_width as u32,
            height: output_height as u32,
            depth_or_array_layers: 1,
        };

        let source_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-nvidia-nis-source"),
            size: source_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &source_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &source_bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((source_width * 4) as u32),
                rows_per_image: Some(source_height as u32),
            },
            source_extent,
        );

        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-nvidia-nis-output"),
            size: output_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let params = NisParams::new(
            [source_width, source_height],
            [output_width, output_height],
            0.5,
        );
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("suisuiview-nvidia-nis-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-nvidia-nis-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&output_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        let padded_bytes_per_row = align_to(
            (output_width * 4) as u32,
            wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        );
        let output_buffer_size = padded_bytes_per_row as u64 * output_height as u64;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("suisuiview-nvidia-nis-readback"),
            size: output_buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("suisuiview-nvidia-nis-encoder"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("suisuiview-nvidia-nis-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(
                (output_width as u32).div_ceil(16),
                (output_height as u32).div_ceil(16),
                1,
            );
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &output_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(output_height as u32),
                },
            },
            output_extent,
        );
        queue.submit(Some(encoder.finish()));
        let buffer_slice = readback.slice(..);
        let (tx, rx) = mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result.map_err(|error| error.to_string()));
        });
        device
            .poll(wgpu::PollType::Wait)
            .map_err(|error| format!("wgpu poll failed: {error}"))?;
        rx.recv()
            .map_err(|error| format!("wgpu readback channel failed: {error}"))?
            .map_err(|error| format!("wgpu readback failed: {error}"))?;
        let elapsed = started.elapsed();

        let mapped = buffer_slice.get_mapped_range();
        let mut output_bytes = Vec::with_capacity(output_width * output_height * 4);
        let row_bytes = output_width * 4;
        for row in 0..output_height {
            let start = row * padded_bytes_per_row as usize;
            output_bytes.extend_from_slice(&mapped[start..start + row_bytes]);
        }
        drop(mapped);
        readback.unmap();

        Ok(GpuUpscaleOutput {
            image: ColorImage::from_rgba_unmultiplied(output_size, &output_bytes),
            elapsed,
        })
    }
}

impl NisParams {
    fn new(source_size: [usize; 2], output_size: [usize; 2], sharpness: f32) -> Self {
        let sharpen_slider = sharpness.clamp(0.0, 1.0) - 0.5;
        let max_scale = if sharpen_slider >= 0.0 { 1.25 } else { 1.75 };
        let min_scale = if sharpen_slider >= 0.0 { 1.25 } else { 1.0 };
        let limit_scale = if sharpen_slider >= 0.0 { 1.25 } else { 1.0 };
        let k_sharp_start_y = 0.45;
        let k_sharp_end_y = 0.9;
        let k_sharp_strength_min = (0.4 + sharpen_slider * min_scale * 1.2).max(0.0);
        let k_sharp_strength_max = 1.6 + sharpen_slider * max_scale * 1.8;
        let k_sharp_limit_min = (0.14 + sharpen_slider * limit_scale * 0.32).max(0.1);
        let k_sharp_limit_max = 0.5 + sharpen_slider * limit_scale * 0.6;
        let k_min_contrast_ratio = 2.0;
        let k_max_contrast_ratio = 10.0;
        Self {
            source_output: [
                source_size[0] as u32,
                source_size[1] as u32,
                output_size[0] as u32,
                output_size[1] as u32,
            ],
            config0: [
                source_size[0] as f32 / output_size[0] as f32,
                source_size[1] as f32 / output_size[1] as f32,
                2.0 * 1127.0 / 1024.0,
                64.0 / 1024.0,
            ],
            config1: [
                k_min_contrast_ratio,
                1.0 / (k_max_contrast_ratio - k_min_contrast_ratio),
                1.0,
                1.0 / 255.0,
            ],
            config2: [
                k_sharp_start_y,
                1.0 / (k_sharp_end_y - k_sharp_start_y),
                k_sharp_strength_min,
                k_sharp_strength_max - k_sharp_strength_min,
            ],
            config3: [
                k_sharp_limit_min,
                k_sharp_limit_max - k_sharp_limit_min,
                0.0,
                0.0,
            ],
        }
    }
}

fn validate_output_size(source_size: [usize; 2], output_size: [usize; 2]) -> Result<(), String> {
    let [source_width, source_height] = source_size;
    let [output_width, output_height] = output_size;
    if source_width == 0 || source_height == 0 || output_width == 0 || output_height == 0 {
        return Err("cannot upscale an empty image with NVIDIA NIS".to_owned());
    }
    if !supports_near_1x_to_2x(source_width, output_width)?
        || !supports_near_1x_to_2x(source_height, output_height)?
    {
        return Err(format!(
            "NVIDIA NIS supports near-1x..2x enlargement within one pixel, got {source_width}x{source_height} -> {output_width}x{output_height}"
        ));
    }
    Ok(())
}

fn supports_near_1x_to_2x(source: usize, output: usize) -> Result<bool, String> {
    let exact_2x = source
        .checked_mul(2)
        .ok_or_else(|| "NVIDIA NIS 2x output size overflowed".to_owned())?;
    let max_output = exact_2x
        .checked_add(1)
        .ok_or_else(|| "NVIDIA NIS rounded output size overflowed".to_owned())?;
    Ok(output >= source && output <= max_output)
}

#[cfg(test)]
mod tests {
    use super::validate_output_size;

    #[test]
    fn output_size_accepts_one_pixel_rounding_over_2x() {
        assert!(validate_output_size([984, 1024], [1969, 2048]).is_ok());
    }

    #[test]
    fn output_size_rejects_sizes_outside_near_1x_to_2x() {
        assert!(validate_output_size([984, 1024], [1970, 2048]).is_err());
        assert!(validate_output_size([984, 1024], [983, 2048]).is_err());
        assert!(validate_output_size([984, 1024], [1968, 0]).is_err());
    }
}
