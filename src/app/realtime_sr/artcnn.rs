use super::RealtimeSrOutput;
use crate::core::artcnn_c4f16::{exact_output_size, ArtcnnC4F16, ArtcnnC4F16RenderOptions};

const TRANSIENT_BYTES_LIMIT: u64 = 256 * 1024 * 1024;

pub(super) struct ArtcnnRenderer {
    core: ArtcnnC4F16,
}

impl ArtcnnRenderer {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        Self {
            core: ArtcnnC4F16::new(device),
        }
    }

    pub(super) fn render(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        source_size: [usize; 2],
    ) -> Option<RealtimeSrOutput> {
        let output_size = exact_output_size(source_size).ok()?;
        let output = self
            .core
            .render_to_texture(
                device,
                encoder,
                source_view,
                source_size,
                ArtcnnC4F16RenderOptions {
                    output_size,
                    output_usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    transient_limit: TRANSIENT_BYTES_LIMIT,
                    readback_padded_bytes_per_row: None,
                },
            )
            .ok()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        Some(RealtimeSrOutput {
            texture: output.texture,
            view,
            size: output.size,
            byte_size: output.size[0]
                .saturating_mul(output.size[1])
                .saturating_mul(4),
        })
    }
}
