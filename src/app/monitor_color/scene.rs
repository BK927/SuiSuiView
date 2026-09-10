//! FP16 egui composition borrowing textures from the normal surface renderer.
//! The surface renderer remains their only owner, including across toggles.
use std::collections::{HashMap, HashSet};

pub(super) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

struct BorrowedTexture {
    source: wgpu::Texture,
    options: egui::TextureOptions,
    alias: egui::TextureId,
}

pub(super) struct SceneRenderer {
    pub renderer: egui_wgpu::Renderer,
    textures: HashMap<egui::TextureId, BorrowedTexture>,
}

impl SceneRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            // The scene stores gamma-encoded sRGB in FP16. Quantization dither
            // belongs exclusively to the later display-profile output pass.
            renderer: egui_wgpu::Renderer::new(device, FORMAT, None, 1, false),
            textures: HashMap::new(),
        }
    }

    pub fn borrow_textures(
        &mut self,
        device: &wgpu::Device,
        source: &egui_wgpu::Renderer,
        primitives: &mut [egui::ClippedPrimitive],
    ) {
        let mut used = HashSet::new();
        for primitive in primitives {
            let egui::epaint::Primitive::Mesh(mesh) = &mut primitive.primitive else {
                continue;
            };
            let id = mesh.texture_id;
            let Some(texture) = source.texture(&id) else {
                continue;
            };
            let Some(image) = &texture.texture else {
                // App textures are egui-owned; custom GPU images use callbacks.
                // No native bind-group-only images are registered by this host.
                continue;
            };
            let options = texture.options.unwrap_or_default();
            let stale = self
                .textures
                .get(&id)
                .is_some_and(|borrowed| borrowed.source != *image || borrowed.options != options);
            if stale {
                let old = self.textures.remove(&id).unwrap();
                self.renderer.free_texture(&old.alias);
            }
            let borrowed = self.textures.entry(id).or_insert_with(|| {
                let filter = |value| match value {
                    egui::TextureFilter::Nearest => wgpu::FilterMode::Nearest,
                    egui::TextureFilter::Linear => wgpu::FilterMode::Linear,
                };
                let wrap = match options.wrap_mode {
                    egui::TextureWrapMode::ClampToEdge => wgpu::AddressMode::ClampToEdge,
                    egui::TextureWrapMode::Repeat => wgpu::AddressMode::Repeat,
                    egui::TextureWrapMode::MirroredRepeat => wgpu::AddressMode::MirrorRepeat,
                };
                let alias = self.renderer.register_native_texture_with_sampler_options(
                    device,
                    &image.create_view(&Default::default()),
                    wgpu::SamplerDescriptor {
                        label: Some("display-color-borrowed-egui-texture"),
                        mag_filter: filter(options.magnification),
                        min_filter: filter(options.minification),
                        mipmap_filter: filter(options.mipmap_mode.unwrap_or(options.minification)),
                        address_mode_u: wrap,
                        address_mode_v: wrap,
                        ..Default::default()
                    },
                );
                BorrowedTexture {
                    source: image.clone(),
                    options,
                    alias,
                }
            });
            mesh.texture_id = borrowed.alias;
            used.insert(id);
        }
        // Keep only this frame's bindings. Native aliases never own/destroy the
        // source allocation, so pruning cannot invalidate the surface renderer.
        self.textures.retain(|id, texture| {
            if used.contains(id) {
                true
            } else {
                self.renderer.free_texture(&texture.alias);
                false
            }
        });
    }
}
