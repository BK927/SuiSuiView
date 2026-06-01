use super::{GpuUpscaleOutput, TEXTURE_FORMAT};
use crate::core::artcnn_c4f16::{
    extent_for_size, validate_render_options, ArtcnnC4F16, ArtcnnC4F16RenderOptions,
};
use crate::core::gpu_effect::color_image_to_rgba;
use eframe::egui::ColorImage;
use std::sync::mpsc;
use std::time::Instant;

const TRANSIENT_BYTES_LIMIT: u64 = 768 * 1024 * 1024;

pub(super) struct ArtcnnBench {
    core: ArtcnnC4F16,
}

impl ArtcnnBench {
    pub(super) async fn try_new(device: &wgpu::Device) -> Option<Self> {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bench = Self::new(device);
        match device.pop_error_scope().await {
            Some(error) => {
                eprintln!("ArtCNN C4F16 bench candidate disabled: {error}");
                None
            }
            None => Some(bench),
        }
    }

    fn new(device: &wgpu::Device) -> Self {
        Self {
            core: ArtcnnC4F16::new(device),
        }
    }

    pub(super) fn apply(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &ColorImage,
        output_size: [usize; 2],
    ) -> Result<GpuUpscaleOutput, String> {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let output = self.apply_scoped(device, queue, image, output_size);
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(format!("ArtCNN C4F16 wgpu validation failed: {error}"));
        }
        output
    }

    fn apply_scoped(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &ColorImage,
        output_size: [usize; 2],
    ) -> Result<GpuUpscaleOutput, String> {
        let source_size = image.size;
        let unpadded_bytes_per_row = rgba8_bytes_per_row(output_size[0], "ArtCNN C4F16 output")?;
        let padded_bytes_per_row = align_to_checked(
            unpadded_bytes_per_row,
            wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
            "ArtCNN C4F16 output row",
        )?;
        let readback_size = readback_byte_size(padded_bytes_per_row, output_size[1])?;
        let output_rows = u32::try_from(output_size[1])
            .map_err(|_| "ArtCNN C4F16 output row count exceeds u32".to_owned())?;
        let options = ArtcnnC4F16RenderOptions {
            output_size,
            output_usage: wgpu::TextureUsages::COPY_SRC,
            transient_limit: TRANSIENT_BYTES_LIMIT,
            readback_padded_bytes_per_row: Some(padded_bytes_per_row),
        };
        let exact_output = validate_render_options(device, source_size, &options)?;

        let started = Instant::now();
        let source_texture = create_source_texture(device, queue, image)?;
        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("suisuiview-artcnn-readback"),
            size: readback_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("suisuiview-artcnn-encoder"),
        });
        let output = self.core.render_to_texture(
            device,
            &mut encoder,
            &source_view,
            source_size,
            options,
        )?;
        debug_assert!(output.size[0] <= exact_output[0] && output.size[1] <= exact_output[1]);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &output.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(output_rows),
                },
            },
            extent_for_size(output_size),
        );
        queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result.map_err(|error| error.to_string()));
        });
        device
            .poll(wgpu::PollType::Wait)
            .map_err(|error| format!("wgpu poll failed: {error}"))?;
        receiver
            .recv()
            .map_err(|error| format!("wgpu readback channel failed: {error}"))?
            .map_err(|error| format!("wgpu readback failed: {error}"))?;
        let elapsed = started.elapsed();

        let mapped = slice.get_mapped_range();
        let mut pixels = vec![0_u8; output_size[0] * output_size[1] * 4];
        for y in 0..output_size[1] {
            let src_offset = y * padded_bytes_per_row as usize;
            let dst_offset = y * output_size[0] * 4;
            pixels[dst_offset..dst_offset + output_size[0] * 4]
                .copy_from_slice(&mapped[src_offset..src_offset + output_size[0] * 4]);
        }
        drop(mapped);
        readback.unmap();

        Ok(GpuUpscaleOutput {
            image: ColorImage::from_rgba_unmultiplied(output_size, &pixels),
            elapsed,
        })
    }
}

fn create_source_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    image: &ColorImage,
) -> Result<wgpu::Texture, String> {
    let source_bytes = color_image_to_rgba(image);
    let bytes_per_row = rgba8_bytes_per_row(image.size[0], "ArtCNN C4F16 source")?;
    let rows_per_image = u32::try_from(image.size[1])
        .map_err(|_| "ArtCNN C4F16 source row count exceeds u32".to_owned())?;
    let source_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("suisuiview-artcnn-source"),
        size: extent_for_size(image.size),
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
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(rows_per_image),
        },
        extent_for_size(image.size),
    );
    Ok(source_texture)
}

fn rgba8_bytes_per_row(width: usize, label: &str) -> Result<u32, String> {
    let bytes = width
        .checked_mul(4)
        .ok_or_else(|| format!("{label} row byte size overflowed"))?;
    u32::try_from(bytes).map_err(|_| format!("{label} row byte size exceeds u32"))
}

fn align_to_checked(value: u32, alignment: u32, label: &str) -> Result<u32, String> {
    if alignment == 0 {
        return Err(format!("{label} alignment must be non-zero"));
    }
    let aligned = (value as u64)
        .div_ceil(alignment as u64)
        .checked_mul(alignment as u64)
        .ok_or_else(|| format!("{label} alignment overflowed"))?;
    u32::try_from(aligned).map_err(|_| format!("{label} alignment exceeds u32"))
}

fn readback_byte_size(padded_bytes_per_row: u32, output_height: usize) -> Result<u64, String> {
    let output_height = u64::try_from(output_height)
        .map_err(|_| "ArtCNN C4F16 readback row count exceeds u64".to_owned())?;
    (padded_bytes_per_row as u64)
        .checked_mul(output_height)
        .ok_or_else(|| "ArtCNN C4F16 readback buffer size overflowed".to_owned())
}
