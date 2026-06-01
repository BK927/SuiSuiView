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
    SrLabFamily, SrLabManifest,
};
use crossbeam_channel::{bounded, Receiver, TryRecvError};
use std::env;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

const EXPERIMENT_SPAN_MANIFEST_ENV: &str = "SUISUIVIEW_EXPERIMENT_SPAN_MANIFEST";
const EXPERIMENT_SPAN_TILE_EDGE_ENV: &str = "SUISUIVIEW_EXPERIMENT_SPAN_TILE_EDGE";
const EXPERIMENT_SPAN_WORKSPACE_CACHE_MB_ENV: &str =
    "SUISUIVIEW_EXPERIMENT_SPAN_WORKSPACE_CACHE_MB";
const SR_LAB_SPAN_MANIFEST_ENV: &str = "SUISUIVIEW_SR_LAB_SPAN_MANIFEST";
const MIB_BYTES: u64 = 1024 * 1024;
const MAX_DISPLAY_TRANSIENT_BYTES: u64 = 96 * 1024 * 1024;
const DEFAULT_DISPLAY_WORKSPACE_CACHE_MB: u64 = 192;
const MIN_DISPLAY_WORKSPACE_CACHE_MB: u64 = 64;
const MAX_DISPLAY_WORKSPACE_CACHE_MB: u64 = 512;
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

#[derive(Clone, Copy)]
enum SpanDisplayPrepareError {
    WorkspaceLimit(SpanDisplaySkipStats),
    PrepareFailed,
}

impl SpanDisplayPrepareError {
    fn reason(self) -> &'static str {
        match self {
            Self::WorkspaceLimit(_) => "workspace_limit",
            Self::PrepareFailed => "prepare_failed",
        }
    }

    fn skip_stats(self) -> SpanDisplaySkipStats {
        match self {
            Self::WorkspaceLimit(stats) => stats,
            Self::PrepareFailed => SpanDisplaySkipStats::None,
        }
    }
}

#[derive(Clone, Copy)]
enum SpanDisplaySkipStats {
    None,
    FrameWorkspaceLimit {
        required_bytes: u64,
        limit_bytes: u64,
    },
    WorkspaceCacheLimit {
        required_bytes: u64,
        limit_bytes: u64,
    },
    TileWorkspaceLimit {
        required_bytes: u64,
        limit_bytes: u64,
    },
}

impl SpanDisplaySkipStats {
    fn frame_workspace_limit(required_bytes: u64, limit_bytes: u64) -> Self {
        Self::FrameWorkspaceLimit {
            required_bytes,
            limit_bytes,
        }
    }

    fn workspace_cache_limit(required_bytes: u64, limit_bytes: u64) -> Self {
        Self::WorkspaceCacheLimit {
            required_bytes,
            limit_bytes,
        }
    }

    fn tile_workspace_limit(required_bytes: u64, limit_bytes: u64) -> Self {
        Self::TileWorkspaceLimit {
            required_bytes,
            limit_bytes,
        }
    }
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
        let weights =
            blob::read_checked_weights(&manifest_path, &manifest, "SPAN display experiment")?;
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
        let workspace_cache_limit_bytes = span_display_workspace_cache_limit_bytes();
        self.reset_workspace_cache_if_source_changed(source_size);
        let tile_plans = match self.prepare_tile_plans(
            device.limits().max_storage_buffer_binding_size as u64,
            &tile_specs,
            source_size,
            output_size,
            workspace_cache_limit_bytes,
        ) {
            Ok(tile_plans) => tile_plans,
            Err(error) => {
                record_span_display_skip_with_stats(
                    error.reason(),
                    source_size,
                    output_size,
                    tile_edge,
                    tile_count,
                    workspace_shapes,
                    error.skip_stats(),
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
            let output =
                self.bridge
                    .bind_output(device, &tile, workspace.output_buffer(), &output_view);
            self.kernel.encode_graph_plan_with_hooks(
                encoder,
                slot.graph_plan.as_ref()?,
                |pass| self.bridge.dispatch_input(pass, &tile, params),
                |pass| self.bridge.dispatch_output(pass, &tile, &output, params),
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
            workspace_cache_limit_bytes,
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
        workspace_cache_limit_bytes: u64,
    ) -> Result<Vec<SpanTilePlan>, SpanDisplayPrepareError> {
        let workspace_sizes = distinct_workspace_sizes(specs);
        let workspace_bytes = self.workspace_byte_sizes(&workspace_sizes)?;
        let frame_workspace_bytes =
            workspace_bytes
                .iter()
                .try_fold(0u64, |total, (_size, byte_size)| {
                    total
                        .checked_add(*byte_size)
                        .ok_or(SpanDisplayPrepareError::PrepareFailed)
                })?;
        if frame_workspace_bytes > workspace_cache_limit_bytes {
            return Err(SpanDisplayPrepareError::WorkspaceLimit(
                SpanDisplaySkipStats::frame_workspace_limit(
                    frame_workspace_bytes,
                    workspace_cache_limit_bytes,
                ),
            ));
        }

        let previous_workspace_count = self.workspaces.len();
        let previous_workspace_bytes = self.workspace_bytes;
        let result = (|| {
            for (size, byte_size) in &workspace_bytes {
                self.ensure_workspace(*size, *byte_size, workspace_cache_limit_bytes)?;
            }

            for size in &workspace_sizes {
                let workspace_index = self
                    .workspace_index_for_size(*size)
                    .ok_or(SpanDisplayPrepareError::PrepareFailed)?;
                let graph_plan = {
                    let slot = self
                        .workspaces
                        .get(workspace_index)
                        .ok_or(SpanDisplayPrepareError::PrepareFailed)?;
                    if slot.graph_plan.is_some() {
                        None
                    } else {
                        validate_span_model(
                            max_storage_buffer_binding_size,
                            &self.manifest,
                            &self.model,
                            &slot.workspace,
                        )
                        .map_err(|_| SpanDisplayPrepareError::PrepareFailed)?;
                        Some(
                            self.kernel
                                .create_prevalidated_graph_plan(
                                    &self.manifest,
                                    &self.model,
                                    &slot.workspace,
                                )
                                .map_err(|_| SpanDisplayPrepareError::PrepareFailed)?,
                        )
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
                        .ok_or(SpanDisplayPrepareError::PrepareFailed)?;
                    let workspace_output_size =
                        self.workspaces[workspace_index].workspace.output_size();
                    let params = bridge_params_for_tile(
                        source_size,
                        output_size,
                        spec,
                        self.manifest.scale as usize,
                        workspace_output_size,
                    )
                    .ok_or(SpanDisplayPrepareError::PrepareFailed)?;
                    Ok(SpanTilePlan {
                        workspace_index,
                        params,
                    })
                })
                .collect::<Result<Vec<_>, SpanDisplayPrepareError>>()
        })();
        if result.is_err() {
            self.workspaces.truncate(previous_workspace_count);
            self.workspace_bytes = previous_workspace_bytes;
        }
        result
    }

    fn workspace_byte_sizes(
        &self,
        sizes: &[[usize; 2]],
    ) -> Result<Vec<([usize; 2], u64)>, SpanDisplayPrepareError> {
        sizes
            .iter()
            .map(|size| {
                let input = input_shape(*size);
                let byte_size = self
                    .kernel
                    .workspace_byte_size(&self.manifest, &input, false)
                    .map_err(|_| SpanDisplayPrepareError::PrepareFailed)?;
                if byte_size > MAX_DISPLAY_TRANSIENT_BYTES {
                    return Err(SpanDisplayPrepareError::WorkspaceLimit(
                        SpanDisplaySkipStats::tile_workspace_limit(
                            byte_size,
                            MAX_DISPLAY_TRANSIENT_BYTES,
                        ),
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
        workspace_cache_limit_bytes: u64,
    ) -> Result<usize, SpanDisplayPrepareError> {
        if let Some(index) = self.workspace_index_for_size(source_size) {
            return Ok(index);
        }
        let next_cache_bytes = self
            .workspace_bytes
            .checked_add(byte_size)
            .ok_or(SpanDisplayPrepareError::PrepareFailed)?;
        if next_cache_bytes > workspace_cache_limit_bytes {
            return Err(SpanDisplayPrepareError::WorkspaceLimit(
                SpanDisplaySkipStats::workspace_cache_limit(
                    next_cache_bytes,
                    workspace_cache_limit_bytes,
                ),
            ));
        }
        let input = input_shape(source_size);
        let workspace = self
            .kernel
            .create_workspace(&self.manifest, &input, false)
            .map_err(|_| SpanDisplayPrepareError::PrepareFailed)?;
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
    workspace_cache_limit_bytes: u64,
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
                usize_from_u64_saturating(workspace_bytes),
            ),
            PerfField::Usize(
                "workspace_cache_limit_bytes",
                usize_from_u64_saturating(workspace_cache_limit_bytes),
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
    record_span_display_skip_with_stats(
        reason,
        source_size,
        output_size,
        tile_edge,
        tile_count,
        workspace_shapes,
        SpanDisplaySkipStats::None,
    );
}

fn record_span_display_skip_with_stats(
    reason: &'static str,
    source_size: [usize; 2],
    output_size: [usize; 2],
    tile_edge: usize,
    tile_count: usize,
    workspace_shapes: usize,
    stats: SpanDisplaySkipStats,
) {
    macro_rules! record_skip {
        ($($extra:expr),* $(,)?) => {{
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
                    $($extra,)*
                ],
            );
        }};
    }

    match stats {
        SpanDisplaySkipStats::None => record_skip!(),
        SpanDisplaySkipStats::FrameWorkspaceLimit {
            required_bytes,
            limit_bytes,
        } => record_skip!(
            PerfField::Str("workspace_limit_stage", "frame_distinct_workspaces"),
            PerfField::Usize(
                "required_workspace_cache_bytes",
                usize_from_u64_saturating(required_bytes),
            ),
            PerfField::Usize(
                "workspace_cache_limit_bytes",
                usize_from_u64_saturating(limit_bytes),
            ),
        ),
        SpanDisplaySkipStats::WorkspaceCacheLimit {
            required_bytes,
            limit_bytes,
        } => record_skip!(
            PerfField::Str("workspace_limit_stage", "workspace_cache_insert"),
            PerfField::Usize(
                "required_workspace_cache_bytes",
                usize_from_u64_saturating(required_bytes),
            ),
            PerfField::Usize(
                "workspace_cache_limit_bytes",
                usize_from_u64_saturating(limit_bytes),
            ),
        ),
        SpanDisplaySkipStats::TileWorkspaceLimit {
            required_bytes,
            limit_bytes,
        } => record_skip!(
            PerfField::Str("workspace_limit_stage", "tile_transient"),
            PerfField::Usize(
                "tile_workspace_bytes",
                usize_from_u64_saturating(required_bytes),
            ),
            PerfField::Usize(
                "tile_workspace_limit_bytes",
                usize_from_u64_saturating(limit_bytes),
            ),
        ),
    }
}

fn usize_from_u64_saturating(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
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

fn span_display_workspace_cache_limit_bytes() -> u64 {
    static CACHE_LIMIT_BYTES: OnceLock<u64> = OnceLock::new();
    *CACHE_LIMIT_BYTES.get_or_init(|| {
        parse_span_display_workspace_cache_mb(
            env::var(EXPERIMENT_SPAN_WORKSPACE_CACHE_MB_ENV)
                .ok()
                .as_deref(),
        )
        .unwrap_or(DEFAULT_DISPLAY_WORKSPACE_CACHE_MB * MIB_BYTES)
    })
}

fn parse_span_display_workspace_cache_mb(value: Option<&str>) -> Option<u64> {
    let mb = value?.trim().parse::<u64>().ok()?;
    (MIN_DISPLAY_WORKSPACE_CACHE_MB..=MAX_DISPLAY_WORKSPACE_CACHE_MB)
        .contains(&mb)
        .then_some(mb.saturating_mul(MIB_BYTES))
}

fn fits_texture_limit(device: &wgpu::Device, size: [usize; 2]) -> bool {
    let max = device.limits().max_texture_dimension_2d as usize;
    size[0] <= max && size[1] <= max
}

#[cfg(test)]
mod tests {
    use super::{
        distinct_workspace_sizes, estimated_dispatch_count, parse_span_display_tile_edge,
        parse_span_display_workspace_cache_mb, MIB_BYTES,
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

    #[test]
    fn span_display_workspace_cache_override_accepts_bounded_mib() {
        assert_eq!(
            parse_span_display_workspace_cache_mb(Some("256")),
            Some(256 * MIB_BYTES)
        );
        assert_eq!(
            parse_span_display_workspace_cache_mb(Some(" 512 ")),
            Some(512 * MIB_BYTES)
        );
        assert_eq!(parse_span_display_workspace_cache_mb(Some("63")), None);
        assert_eq!(parse_span_display_workspace_cache_mb(Some("513")), None);
        assert_eq!(parse_span_display_workspace_cache_mb(Some("large")), None);
        assert_eq!(parse_span_display_workspace_cache_mb(None), None);
    }
}
