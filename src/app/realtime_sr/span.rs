use super::span_bridge::{
    bridge_params_for_tile, checked_output_size, create_output_texture, SpanBridge,
    SpanBridgeParams,
};
use super::RealtimeSrOutput;
use crate::core::perf_trace::{self, PerfField};
use crate::core::sr_lab::{
    self,
    blob::{self, SrLabWeights},
    cpu::FeatureMap,
    gpu::{
        buffers::SpanGpuModel,
        kernel::{SpanGpuGraphPlan, SpanGpuKernel, SpanGpuWorkspace},
        model_validation::validate_span_model,
        tiled::{
            span_tile_halo, span_tile_specs, workspace_shape_count, SpanTileSpec,
            DEFAULT_SPAN_TILE_EDGE,
        },
    },
    sha256::sha256_hex,
    SrLabFamily, SrLabManifest,
};
use crossbeam_channel::{bounded, Receiver, TryRecvError};
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

const EXPERIMENT_SPAN_MANIFEST_ENV: &str = "SUISUIVIEW_EXPERIMENT_SPAN_MANIFEST";
const EXPERIMENT_SPAN_TILE_EDGE_ENV: &str = "SUISUIVIEW_EXPERIMENT_SPAN_TILE_EDGE";
const SR_LAB_SPAN_MANIFEST_ENV: &str = "SUISUIVIEW_SR_LAB_SPAN_MANIFEST";
const MAX_WEIGHT_BLOB_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DISPLAY_TRANSIENT_BYTES: u64 = 96 * 1024 * 1024;
const MAX_DISPLAY_WORKSPACE_CACHE_BYTES: u64 = 192 * 1024 * 1024;
const MAX_DISPLAY_WORKSPACE_SHAPES: usize = 32;
const MAX_DISPLAY_TILE_COUNT: usize = 256;
const MAX_DISPLAY_SOURCE_PIXELS: usize = 1_048_576;
const MIN_DISPLAY_TILE_EDGE: usize = 32;
const MAX_DISPLAY_TILE_EDGE: usize = 256;
const OUTPUT_BYTES_PER_PIXEL: usize = 4;

pub(super) struct SpanRenderer {
    state: SpanRendererState,
}

enum SpanRendererState {
    Pending,
    Loading(Receiver<Result<LoadedSpanRenderer, String>>),
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
    graph_plan: Option<SpanGpuGraphPlan>,
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
        self.start_loading_if_needed(device);
        self.finish_loading_if_ready();

        let SpanRendererState::Ready(renderer) = &mut self.state else {
            return None;
        };
        renderer.render(device, encoder, source_view, source_size)
    }

    pub(super) fn is_loading(&self) -> bool {
        matches!(self.state, SpanRendererState::Loading(_))
    }

    pub(super) fn warm_up(&mut self, device: &wgpu::Device) {
        self.start_loading_if_needed(device);
        self.finish_loading_if_ready();
    }

    fn start_loading_if_needed(&mut self, device: &wgpu::Device) {
        if !matches!(self.state, SpanRendererState::Pending) {
            return;
        }
        self.state = LoadedSpanRenderer::spawn_loader(device.clone())
            .map(SpanRendererState::Loading)
            .unwrap_or(SpanRendererState::Disabled);
    }

    fn finish_loading_if_ready(&mut self) {
        let SpanRendererState::Loading(receiver) = &self.state else {
            return;
        };
        let next_state = match receiver.try_recv() {
            Ok(Ok(renderer)) => SpanRendererState::Ready(Box::new(renderer)),
            Ok(Err(_)) | Err(TryRecvError::Disconnected) => SpanRendererState::Disabled,
            Err(TryRecvError::Empty) => return,
        };
        self.state = next_state;
    }
}

impl LoadedSpanRenderer {
    fn spawn_loader(device: wgpu::Device) -> Option<Receiver<Result<Self, String>>> {
        let manifest_path = span_manifest_path()?;
        let (sender, receiver) = bounded(1);
        let _loader = thread::Builder::new()
            .name("suisuiview-span-display-loader".to_owned())
            .spawn(move || {
                let _ = sender.send(Self::load(device, manifest_path));
            })
            .ok()?;
        Some(receiver)
    }

    fn load(device: wgpu::Device, manifest_path: PathBuf) -> Result<Self, String> {
        let manifest = sr_lab::read_manifest(&manifest_path).map_err(|error| error.to_string())?;
        sr_lab::inspect_manifest(&manifest).map_err(|error| error.to_string())?;
        validate_display_manifest(&manifest)?;
        let weights = read_checked_weights(&manifest_path, &manifest)?;
        Self::from_weights(&device, manifest, &weights)
    }

    fn from_weights(
        device: &wgpu::Device,
        manifest: SrLabManifest,
        weights: &SrLabWeights,
    ) -> Result<Self, String> {
        let kernel = SpanGpuKernel::new(device.clone());
        let model = SpanGpuModel::from_weights(device, weights);
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
        let render_start = Instant::now();
        let Some(output_size) = checked_output_size(source_size, self.manifest.scale as usize)
        else {
            record_span_display_skip("output_size_overflow", source_size, [0, 0], 0, 0, 0);
            return None;
        };
        if !fits_texture_limit(device, output_size) {
            record_span_display_skip("texture_limit", source_size, output_size, 0, 0, 0);
            return None;
        }
        let Some(source_pixels) = source_pixel_count(source_size) else {
            record_span_display_skip("source_size_overflow", source_size, output_size, 0, 0, 0);
            return None;
        };
        if source_pixels > MAX_DISPLAY_SOURCE_PIXELS {
            record_span_display_skip("source_area_limit", source_size, output_size, 0, 0, 0);
            return None;
        }
        let input_shape = input_shape(source_size);
        let halo = match span_tile_halo(&self.manifest) {
            Ok(halo) => halo,
            Err(_) => {
                record_span_display_skip("tile_halo", source_size, output_size, 0, 0, 0);
                return None;
            }
        };
        let tile_edge = span_display_tile_edge();
        let tile_specs = span_tile_specs(&input_shape, tile_edge, halo);
        let tile_count = tile_specs.len();
        let workspace_shapes = workspace_shape_count(&tile_specs);
        if tile_specs.is_empty() {
            record_span_display_skip("empty_tiles", source_size, output_size, tile_edge, 0, 0);
            return None;
        }
        if tile_count > MAX_DISPLAY_TILE_COUNT {
            record_span_display_skip(
                "tile_count_limit",
                source_size,
                output_size,
                tile_edge,
                tile_count,
                workspace_shapes,
            );
            return None;
        }
        if workspace_shapes > MAX_DISPLAY_WORKSPACE_SHAPES {
            record_span_display_skip(
                "workspace_shape_limit",
                source_size,
                output_size,
                tile_edge,
                tile_count,
                workspace_shapes,
            );
            return None;
        }
        self.reset_workspace_cache_if_source_changed(source_size);
        let tile_plans = match self.prepare_tile_plans(
            device.limits().max_storage_buffer_binding_size as u64,
            &tile_specs,
            source_size,
            output_size,
        ) {
            Ok(tile_plans) => tile_plans,
            Err(_) => {
                record_span_display_skip(
                    "workspace_limit",
                    source_size,
                    output_size,
                    tile_edge,
                    tile_count,
                    workspace_shapes,
                );
                return None;
            }
        };

        let output_texture = create_output_texture(device, output_size);
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        for plan in tile_plans {
            let slot = self.workspaces.get(plan.workspace_index)?;
            let workspace = &slot.workspace;
            let params = plan.params;
            let tile = self
                .bridge
                .bind_tile(device, source_view, workspace.input_buffer(), params);
            self.bridge.encode_input(encoder, &tile, params);

            self.kernel
                .encode_graph_plan(encoder, slot.graph_plan.as_ref()?);
            self.bridge.encode_output(
                device,
                encoder,
                &tile,
                workspace.output_buffer(),
                &output_view,
                params,
            );
        }
        record_span_display_encode(
            render_start.elapsed(),
            source_size,
            output_size,
            tile_count,
            workspace_shapes,
            self.workspaces.len(),
            self.workspace_bytes,
            tile_edge,
            estimated_dispatch_count(&self.manifest, tile_count),
        );

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
            let workspace_index = self
                .workspace_index_for_size(*size)
                .ok_or_else(|| "SPAN display workspace cache lookup failed".to_owned())?;
            let graph_plan = {
                let slot = self
                    .workspaces
                    .get(workspace_index)
                    .ok_or_else(|| "SPAN display workspace cache lookup failed".to_owned())?;
                if slot.graph_plan.is_some() {
                    None
                } else {
                    validate_span_model(
                        max_storage_buffer_binding_size,
                        &self.manifest,
                        &self.model,
                        &slot.workspace,
                    )?;
                    Some(self.kernel.create_prevalidated_graph_plan(
                        &self.manifest,
                        &self.model,
                        &slot.workspace,
                    )?)
                }
            };
            if let Some(graph_plan) = graph_plan {
                self.workspaces[workspace_index].graph_plan = Some(graph_plan);
            }
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
            graph_plan: None,
        });
        self.workspace_bytes = next_cache_bytes;
        Ok(self.workspaces.len() - 1)
    }

    fn workspace_index_for_size(&self, source_size: [usize; 2]) -> Option<usize> {
        self.workspaces
            .iter()
            .position(|slot| slot.size == source_size)
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
    let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let weights_path = checked_weights_path(manifest_dir, weights_file)?;
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

fn checked_weights_path(manifest_dir: &Path, weights_file: &str) -> Result<PathBuf, String> {
    let relative_path = safe_relative_weights_path(weights_file)?;
    let weights_path = manifest_dir.join(relative_path);
    let canonical_manifest_dir = fs::canonicalize(manifest_dir).map_err(|error| {
        format!(
            "SPAN display manifest directory cannot be resolved: {}",
            error
        )
    })?;
    let canonical_weights_path = fs::canonicalize(&weights_path)
        .map_err(|error| format!("SPAN display weight path cannot be resolved: {}", error))?;
    if !canonical_weights_path.starts_with(&canonical_manifest_dir) {
        return Err("SPAN display weight path must stay under the manifest directory".to_owned());
    }
    Ok(canonical_weights_path)
}

fn safe_relative_weights_path(weights_file: &str) -> Result<PathBuf, String> {
    let weights_file = weights_file.trim();
    if weights_file.is_empty() {
        return Err("SPAN display experiment requires a non-empty weights_file".to_owned());
    }
    let path = Path::new(weights_file);
    if path.is_absolute() {
        return Err("SPAN display weight path must be relative".to_owned());
    }
    let mut saw_normal_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => saw_normal_component = true,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(
                    "SPAN display weight path must not leave the manifest directory".to_owned(),
                );
            }
        }
    }
    if !saw_normal_component {
        return Err("SPAN display weight path must name a file".to_owned());
    }
    Ok(path.to_path_buf())
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

fn record_span_display_encode(
    duration: Duration,
    source_size: [usize; 2],
    output_size: [usize; 2],
    tile_count: usize,
    workspace_shapes: usize,
    workspace_slots: usize,
    workspace_bytes: u64,
    tile_edge: usize,
    estimated_dispatches: usize,
) {
    perf_trace::record_duration(
        "span_display_encode",
        duration,
        &[
            PerfField::Str("method", "srlab_span_x2"),
            PerfField::Usize("source_width", source_size[0]),
            PerfField::Usize("source_height", source_size[1]),
            PerfField::Usize("output_width", output_size[0]),
            PerfField::Usize("output_height", output_size[1]),
            PerfField::Usize("tile_count", tile_count),
            PerfField::Usize("workspace_shapes", workspace_shapes),
            PerfField::Usize("workspace_slots", workspace_slots),
            PerfField::Usize("tile_edge", tile_edge),
            PerfField::Usize("estimated_dispatches", estimated_dispatches),
            PerfField::Usize(
                "workspace_cache_bytes",
                usize::try_from(workspace_bytes).unwrap_or(usize::MAX),
            ),
        ],
    );
}

fn record_span_display_skip(
    reason: &'static str,
    source_size: [usize; 2],
    output_size: [usize; 2],
    tile_edge: usize,
    tile_count: usize,
    workspace_shapes: usize,
) {
    perf_trace::record_duration(
        "span_display_skip",
        Duration::ZERO,
        &[
            PerfField::Str("method", "srlab_span_x2"),
            PerfField::Str("reason", reason),
            PerfField::Usize("source_width", source_size[0]),
            PerfField::Usize("source_height", source_size[1]),
            PerfField::Usize("output_width", output_size[0]),
            PerfField::Usize("output_height", output_size[1]),
            PerfField::Usize("tile_edge", tile_edge),
            PerfField::Usize("tile_count", tile_count),
            PerfField::Usize("workspace_shapes", workspace_shapes),
        ],
    );
}

fn estimated_dispatch_count(manifest: &SrLabManifest, tile_count: usize) -> usize {
    let span_graph_dispatches = manifest
        .span
        .as_ref()
        .map(|span| 7usize.saturating_add(4usize.saturating_mul(span.block_count as usize)))
        .unwrap_or_default();
    let bridge_dispatches = 2usize;
    tile_count.saturating_mul(span_graph_dispatches.saturating_add(bridge_dispatches))
}

fn span_display_tile_edge() -> usize {
    static TILE_EDGE: OnceLock<usize> = OnceLock::new();
    *TILE_EDGE.get_or_init(|| {
        parse_span_display_tile_edge(env::var(EXPERIMENT_SPAN_TILE_EDGE_ENV).ok().as_deref())
            .unwrap_or(DEFAULT_SPAN_TILE_EDGE)
    })
}

fn parse_span_display_tile_edge(value: Option<&str>) -> Option<usize> {
    let edge = value?.trim().parse::<usize>().ok()?;
    (MIN_DISPLAY_TILE_EDGE..=MAX_DISPLAY_TILE_EDGE)
        .contains(&edge)
        .then_some(edge)
}

fn fits_texture_limit(device: &wgpu::Device, size: [usize; 2]) -> bool {
    let max = device.limits().max_texture_dimension_2d as usize;
    size[0] <= max && size[1] <= max
}

#[cfg(test)]
mod tests {
    use super::{
        distinct_workspace_sizes, estimated_dispatch_count, parse_span_display_tile_edge,
        safe_relative_weights_path,
    };
    use crate::core::sr_lab::gpu::tiled::SpanTileSpec;
    use crate::core::sr_lab::{SrLabFamily, SrLabManifest, SrLabSpanMetadata};

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

    #[test]
    fn span_display_weight_paths_must_stay_relative() {
        assert_eq!(
            safe_relative_weights_path("weights.srlab").unwrap(),
            std::path::PathBuf::from("weights.srlab")
        );
        assert!(safe_relative_weights_path("").is_err());
        assert!(safe_relative_weights_path(".").is_err());
        assert!(safe_relative_weights_path("..\\weights.srlab").is_err());
        assert!(safe_relative_weights_path("nested\\..\\weights.srlab").is_err());
        assert!(safe_relative_weights_path("C:\\models\\weights.srlab").is_err());
    }

    #[test]
    fn span_display_dispatch_estimate_includes_bridge_and_graph_passes() {
        let manifest = SrLabManifest {
            name: "SPAN-S x2".to_owned(),
            family: SrLabFamily::SpanS,
            variant: Some("SPAN-S".to_owned()),
            scale: 2,
            input_channels: 3,
            output_channels: 3,
            weights_format: "srlab01".to_owned(),
            weights_file: Some("weights.srlab".to_owned()),
            weights_sha256: "0".repeat(64),
            source: "test".to_owned(),
            source_commit: None,
            source_checkpoint_url: None,
            source_checkpoint_archive_sha256: None,
            source_checkpoint_file: None,
            source_checkpoint_sha256: None,
            license: "Apache-2.0".to_owned(),
            notes: Vec::new(),
            span: Some(SrLabSpanMetadata {
                feature_channels: 48,
                block_count: 6,
                reparameterized_conv3xc: true,
                img_range: 255.0,
                rgb_mean: [0.4488, 0.4371, 0.4040],
            }),
            layers: Vec::new(),
        };

        assert_eq!(estimated_dispatch_count(&manifest, 96), 3168);
    }

    #[test]
    fn span_display_tile_edge_override_accepts_bounded_values() {
        assert_eq!(parse_span_display_tile_edge(Some("128")), Some(128));
        assert_eq!(parse_span_display_tile_edge(Some(" 72 ")), Some(72));
        assert_eq!(parse_span_display_tile_edge(Some("31")), None);
        assert_eq!(parse_span_display_tile_edge(Some("257")), None);
        assert_eq!(parse_span_display_tile_edge(Some("wide")), None);
        assert_eq!(parse_span_display_tile_edge(None), None);
    }
}
