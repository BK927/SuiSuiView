use super::span_bridge::{
    bridge_params_for_tile, checked_output_size, create_output_texture, SpanBridge,
    SpanBridgeParams,
};
use super::RealtimeSrOutput;
use crate::core::sr_lab::{
    self,
    blob::{self, SrLabWeights},
    cpu::FeatureMap,
    gpu::{
        buffers::SpanGpuModel,
        kernel::{SpanGpuKernel, SpanGpuWorkspace},
        model_validation::validate_span_model,
        tiled::{
            span_tile_halo, span_tile_specs, workspace_shape_count, SpanTileSpec,
            DEFAULT_SPAN_TILE_EDGE,
        },
    },
    sha256::sha256_hex,
    SrLabFamily, SrLabManifest,
};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const EXPERIMENT_SPAN_MANIFEST_ENV: &str = "SUISUIVIEW_EXPERIMENT_SPAN_MANIFEST";
const SR_LAB_SPAN_MANIFEST_ENV: &str = "SUISUIVIEW_SR_LAB_SPAN_MANIFEST";
const MAX_WEIGHT_BLOB_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DISPLAY_TRANSIENT_BYTES: u64 = 96 * 1024 * 1024;
const MAX_DISPLAY_WORKSPACE_CACHE_BYTES: u64 = 192 * 1024 * 1024;
const MAX_DISPLAY_WORKSPACE_SHAPES: usize = 32;
const MAX_DISPLAY_TILE_COUNT: usize = 256;
const MAX_DISPLAY_SOURCE_PIXELS: usize = 1_048_576;
const OUTPUT_BYTES_PER_PIXEL: usize = 4;

pub(super) struct SpanRenderer {
    state: SpanRendererState,
}

enum SpanRendererState {
    Pending,
    Ready(Box<LoadedSpanRenderer>),
    Disabled,
}

struct LoadedSpanRenderer {
    manifest: SrLabManifest,
    model: SpanGpuModel,
    kernel: SpanGpuKernel,
    bridge: SpanBridge,
    workspace_source_size: Option<[usize; 2]>,
    workspaces: Vec<SpanWorkspaceSlot>,
    workspace_bytes: u64,
}

struct SpanWorkspaceSlot {
    size: [usize; 2],
    workspace: SpanGpuWorkspace,
}

struct SpanTilePlan {
    workspace_index: usize,
    params: SpanBridgeParams,
}

impl SpanRenderer {
    pub(super) fn new() -> Self {
        Self {
            state: SpanRendererState::Pending,
        }
    }

    pub(super) fn render(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        source_size: [usize; 2],
    ) -> Option<RealtimeSrOutput> {
        if matches!(self.state, SpanRendererState::Pending) {
            self.state = match LoadedSpanRenderer::new(device) {
                Ok(renderer) => SpanRendererState::Ready(Box::new(renderer)),
                Err(_error) => SpanRendererState::Disabled,
            };
        }

        let SpanRendererState::Ready(renderer) = &mut self.state else {
            return None;
        };
        renderer.render(device, encoder, source_view, source_size)
    }
}

impl LoadedSpanRenderer {
    fn new(device: &wgpu::Device) -> Result<Self, String> {
        let manifest_path = span_manifest_path()
            .ok_or_else(|| "SPAN display experiment requires a manifest env var".to_owned())?;
        let manifest = sr_lab::read_manifest(&manifest_path).map_err(|error| error.to_string())?;
        sr_lab::inspect_manifest(&manifest).map_err(|error| error.to_string())?;
        validate_display_manifest(&manifest)?;
        let weights = read_checked_weights(&manifest_path, &manifest)?;

        let kernel = SpanGpuKernel::new(device.clone());
        let model = SpanGpuModel::from_weights(device, &weights);
        let bridge = SpanBridge::new(device);

        Ok(Self {
            manifest,
            model,
            kernel,
            bridge,
            workspace_source_size: None,
            workspaces: Vec::new(),
            workspace_bytes: 0,
        })
    }

    fn render(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        source_size: [usize; 2],
    ) -> Option<RealtimeSrOutput> {
        let output_size = checked_output_size(source_size, self.manifest.scale as usize)?;
        if !fits_texture_limit(device, output_size) {
            return None;
        }
        if source_pixel_count(source_size)? > MAX_DISPLAY_SOURCE_PIXELS {
            return None;
        }
        let input_shape = input_shape(source_size);
        let halo = span_tile_halo(&self.manifest).ok()?;
        let tile_specs = span_tile_specs(&input_shape, DEFAULT_SPAN_TILE_EDGE, halo);
        if tile_specs.is_empty()
            || tile_specs.len() > MAX_DISPLAY_TILE_COUNT
            || workspace_shape_count(&tile_specs) > MAX_DISPLAY_WORKSPACE_SHAPES
        {
            return None;
        }
        self.reset_workspace_cache_if_source_changed(source_size);
        let tile_plans = self
            .prepare_tile_plans(
                device.limits().max_storage_buffer_binding_size as u64,
                &tile_specs,
                source_size,
                output_size,
            )
            .ok()?;

        let output_texture = create_output_texture(device, output_size);
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        for plan in tile_plans {
            let workspace = &self.workspaces.get(plan.workspace_index)?.workspace;
            let params = plan.params;
            let tile = self
                .bridge
                .bind_tile(device, source_view, workspace.input_buffer(), params);
            self.bridge.encode_input(encoder, &tile, params);

            self.kernel
                .encode_workspace(encoder, &self.manifest, &self.model, workspace)
                .ok()?;
            self.bridge.encode_output(
                device,
                encoder,
                &tile,
                workspace.output_buffer(),
                &output_view,
                params,
            );
        }

        Some(RealtimeSrOutput {
            texture: output_texture,
            view: output_view,
            size: output_size,
            byte_size: output_size[0]
                .saturating_mul(output_size[1])
                .saturating_mul(OUTPUT_BYTES_PER_PIXEL),
        })
    }

    fn reset_workspace_cache_if_source_changed(&mut self, source_size: [usize; 2]) {
        if self.workspace_source_size == Some(source_size) {
            return;
        }
        self.workspace_source_size = Some(source_size);
        self.workspaces.clear();
        self.workspace_bytes = 0;
    }

    fn prepare_tile_plans(
        &mut self,
        max_storage_buffer_binding_size: u64,
        specs: &[SpanTileSpec],
        source_size: [usize; 2],
        output_size: [usize; 2],
    ) -> Result<Vec<SpanTilePlan>, String> {
        let workspace_sizes = distinct_workspace_sizes(specs);
        let workspace_bytes = self.workspace_byte_sizes(&workspace_sizes)?;
        let frame_workspace_bytes =
            workspace_bytes
                .iter()
                .try_fold(0u64, |total, (_size, byte_size)| {
                    total
                        .checked_add(*byte_size)
                        .ok_or_else(|| "SPAN display frame workspace size overflowed".to_owned())
                })?;
        if frame_workspace_bytes > MAX_DISPLAY_WORKSPACE_CACHE_BYTES {
            return Err(format!(
                "SPAN display frame would pin about {} MiB of tile workspaces, above the {} MiB display limit",
                frame_workspace_bytes.div_ceil(1024 * 1024),
                MAX_DISPLAY_WORKSPACE_CACHE_BYTES.div_ceil(1024 * 1024)
            ));
        }

        for (size, byte_size) in &workspace_bytes {
            self.ensure_workspace(*size, *byte_size)?;
        }

        for size in &workspace_sizes {
            let workspace = self.workspace_for_size(*size)?;
            validate_span_model(
                max_storage_buffer_binding_size,
                &self.manifest,
                &self.model,
                workspace,
            )?;
        }

        specs
            .iter()
            .copied()
            .map(|spec| {
                let workspace_index = self
                    .workspace_index_for_size([spec.crop_width, spec.crop_height])
                    .ok_or_else(|| "SPAN display workspace cache lookup failed".to_owned())?;
                let workspace_output_size =
                    self.workspaces[workspace_index].workspace.output_size();
                let params = bridge_params_for_tile(
                    source_size,
                    output_size,
                    spec,
                    self.manifest.scale as usize,
                    workspace_output_size,
                )
                .ok_or_else(|| "SPAN display tile bridge params overflowed".to_owned())?;
                Ok(SpanTilePlan {
                    workspace_index,
                    params,
                })
            })
            .collect()
    }

    fn workspace_byte_sizes(&self, sizes: &[[usize; 2]]) -> Result<Vec<([usize; 2], u64)>, String> {
        sizes
            .iter()
            .map(|size| {
                let input = input_shape(*size);
                let byte_size = self
                    .kernel
                    .workspace_byte_size(&self.manifest, &input, false)?;
                if byte_size > MAX_DISPLAY_TRANSIENT_BYTES {
                    return Err(format!(
                        "SPAN display tile would allocate about {} MiB of transient buffers, above the {} MiB tile limit",
                        byte_size.div_ceil(1024 * 1024),
                        MAX_DISPLAY_TRANSIENT_BYTES.div_ceil(1024 * 1024)
                    ));
                }
                Ok((*size, byte_size))
            })
            .collect()
    }

    fn ensure_workspace(
        &mut self,
        source_size: [usize; 2],
        byte_size: u64,
    ) -> Result<usize, String> {
        if let Some(index) = self.workspace_index_for_size(source_size) {
            return Ok(index);
        }
        let next_cache_bytes = self
            .workspace_bytes
            .checked_add(byte_size)
            .ok_or_else(|| "SPAN display workspace cache size overflowed".to_owned())?;
        if next_cache_bytes > MAX_DISPLAY_WORKSPACE_CACHE_BYTES {
            return Err(format!(
                "SPAN display workspace cache would grow to about {} MiB, above the {} MiB display limit",
                next_cache_bytes.div_ceil(1024 * 1024),
                MAX_DISPLAY_WORKSPACE_CACHE_BYTES.div_ceil(1024 * 1024)
            ));
        }
        let input = input_shape(source_size);
        let workspace = self
            .kernel
            .create_workspace(&self.manifest, &input, false)?;
        self.workspaces.push(SpanWorkspaceSlot {
            size: source_size,
            workspace,
        });
        self.workspace_bytes = next_cache_bytes;
        Ok(self.workspaces.len() - 1)
    }

    fn workspace_index_for_size(&self, source_size: [usize; 2]) -> Option<usize> {
        self.workspaces
            .iter()
            .position(|slot| slot.size == source_size)
    }

    fn workspace_for_size(&self, source_size: [usize; 2]) -> Result<&SpanGpuWorkspace, String> {
        self.workspace_index_for_size(source_size)
            .and_then(|index| self.workspaces.get(index))
            .map(|slot| &slot.workspace)
            .ok_or_else(|| "SPAN display workspace cache lookup failed".to_owned())
    }
}

fn span_manifest_path() -> Option<PathBuf> {
    env::var_os(EXPERIMENT_SPAN_MANIFEST_ENV)
        .or_else(|| env::var_os(SR_LAB_SPAN_MANIFEST_ENV))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn validate_display_manifest(manifest: &SrLabManifest) -> Result<(), String> {
    if !matches!(manifest.family, SrLabFamily::Span | SrLabFamily::SpanS) {
        return Err("SPAN display experiment requires a SPAN-family manifest".to_owned());
    }
    if manifest.scale != 2 || manifest.input_channels != 3 || manifest.output_channels != 3 {
        return Err("SPAN display experiment requires a 3-channel x2 RGB manifest".to_owned());
    }
    if !manifest.license.eq_ignore_ascii_case("Apache-2.0") {
        return Err(format!(
            "SPAN display experiment only accepts Apache-2.0 local lab weights, got {}",
            manifest.license
        ));
    }
    Ok(())
}

fn read_checked_weights(
    manifest_path: &Path,
    manifest: &SrLabManifest,
) -> Result<SrLabWeights, String> {
    let weights_file = manifest
        .weights_file
        .as_deref()
        .ok_or_else(|| "SPAN display experiment requires manifest weights_file".to_owned())?;
    let weights_path = manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(weights_file);
    let byte_len = fs::metadata(&weights_path)
        .map_err(|error| error.to_string())?
        .len();
    if byte_len > MAX_WEIGHT_BLOB_BYTES {
        return Err(format!(
            "SPAN display weight blob is too large: {} bytes",
            byte_len
        ));
    }
    let bytes = fs::read(&weights_path).map_err(|error| error.to_string())?;
    let actual_sha256 = sha256_hex(&bytes);
    if !actual_sha256.eq_ignore_ascii_case(&manifest.weights_sha256) {
        return Err(format!(
            "SPAN display weight SHA-256 mismatch for {}",
            weights_path.display()
        ));
    }
    blob::parse_weights(&bytes)
}

fn input_shape(size: [usize; 2]) -> FeatureMap {
    FeatureMap {
        channels: 3,
        height: size[1],
        width: size[0],
        values: Vec::new(),
    }
}

fn source_pixel_count(size: [usize; 2]) -> Option<usize> {
    size[0].checked_mul(size[1])
}

fn distinct_workspace_sizes(specs: &[SpanTileSpec]) -> Vec<[usize; 2]> {
    let mut sizes = Vec::new();
    for spec in specs {
        let size = [spec.crop_width, spec.crop_height];
        if !sizes.contains(&size) {
            sizes.push(size);
        }
    }
    sizes
}

fn fits_texture_limit(device: &wgpu::Device, size: [usize; 2]) -> bool {
    let max = device.limits().max_texture_dimension_2d as usize;
    size[0] <= max && size[1] <= max
}

#[cfg(test)]
mod tests {
    use super::distinct_workspace_sizes;
    use crate::core::sr_lab::gpu::tiled::SpanTileSpec;

    #[test]
    fn distinct_workspace_sizes_preserve_first_seen_shapes() {
        let specs = [
            SpanTileSpec {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
                crop_x: 0,
                crop_y: 0,
                crop_width: 8,
                crop_height: 6,
            },
            SpanTileSpec {
                x: 4,
                y: 0,
                width: 4,
                height: 4,
                crop_x: 0,
                crop_y: 0,
                crop_width: 8,
                crop_height: 6,
            },
            SpanTileSpec {
                x: 8,
                y: 0,
                width: 2,
                height: 4,
                crop_x: 6,
                crop_y: 0,
                crop_width: 4,
                crop_height: 6,
            },
        ];

        assert_eq!(distinct_workspace_sizes(&specs), vec![[8, 6], [4, 6]]);
    }
}
