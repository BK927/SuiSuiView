use super::{
    buffers::SpanGpuModel, compare_features, timing_stats, validate_comparison, validation,
    SpanGpuComparison, SpanGpuExecutor, SpanGpuTimingStats,
};
use crate::core::sr_lab::cpu::{self, FeatureMap};
use crate::core::sr_lab::{blob::SrLabWeights, SrLabManifest};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

const MAX_TILED_WORKSPACE_SHAPES: usize = 32;
pub const DEFAULT_SPAN_TILE_EDGE: usize = 64;

#[derive(Debug, Serialize)]
pub struct SpanGpuTiledReferenceReport {
    pub manifest: String,
    pub input: String,
    pub model: String,
    pub variant: Option<String>,
    pub requested_long_edge: u32,
    pub effective_long_edge: u32,
    pub input_width: usize,
    pub input_height: usize,
    pub output_width: usize,
    pub output_height: usize,
    pub tile_edge: usize,
    pub halo: usize,
    pub tile_count: usize,
    pub workspace_shape_count: usize,
    pub model_buffer_init_ms: f64,
    pub reuses_model_buffers: bool,
    pub reuses_transient_buffers: bool,
    pub total_cpu_orchestrated_elapsed_ms: f64,
    pub tile_elapsed_ms: SpanGpuTimingStats,
    pub comparison: Option<SpanGpuComparison>,
    pub tiles: Vec<SpanGpuTileReport>,
}

#[derive(Debug, Serialize)]
pub struct SpanGpuTileReport {
    pub tile_index: usize,
    pub input_x: usize,
    pub input_y: usize,
    pub input_width: usize,
    pub input_height: usize,
    pub crop_x: usize,
    pub crop_y: usize,
    pub crop_width: usize,
    pub crop_height: usize,
    pub gpu_elapsed_ms: f64,
}

pub(crate) struct SpanGpuTiledRunner {
    executor: SpanGpuExecutor,
    model: SpanGpuModel,
}

pub(crate) struct SpanGpuTiledRun {
    pub(crate) output: FeatureMap,
}

impl SpanGpuTiledRunner {
    pub(crate) fn new(weights: &SrLabWeights) -> Result<Self, String> {
        let executor = SpanGpuExecutor::new()?;
        let model = executor.upload_model(weights);
        Ok(Self { executor, model })
    }

    pub(crate) fn run(
        &self,
        manifest: &SrLabManifest,
        input: &FeatureMap,
        tile_edge: usize,
    ) -> Result<SpanGpuTiledRun, String> {
        if tile_edge == 0 {
            return Err("SPAN tile edge must be positive".to_owned());
        }
        validation::validate_span_manifest(manifest, input)?;
        let halo = span_tile_halo(manifest)?;
        let scale = manifest.scale as usize;
        let output_channels = manifest.output_channels as usize;
        let tile_specs = span_tile_specs(input, tile_edge, halo);
        let workspace_shape_count = workspace_shape_count(&tile_specs);
        if workspace_shape_count > MAX_TILED_WORKSPACE_SHAPES {
            return Err(format!(
                "SPAN tile edge produced {workspace_shape_count} distinct workspace shapes; increase tile edge or keep shapes at {MAX_TILED_WORKSPACE_SHAPES} or fewer"
            ));
        }

        let mut stitched = FeatureMap::zeros(
            output_channels,
            checked_scaled(input.height, scale)?,
            checked_scaled(input.width, scale)?,
        );
        let mut sessions = HashMap::new();
        for spec in tile_specs.iter().copied() {
            let crop = crop_input(input, spec);
            let shape = (crop.width, crop.height);
            let gpu = match sessions.entry(shape) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let session =
                        self.executor
                            .create_readback_session(manifest, &self.model, &crop)?;
                    entry.insert(session)
                }
            }
            .run(&crop)?;
            stitch_tile_output(&mut stitched, &gpu.output, spec, scale)?;
        }

        Ok(SpanGpuTiledRun { output: stitched })
    }
}

// established call surface; a params struct would be pure boilerplate
#[allow(clippy::too_many_arguments)]
pub fn run_span_gpu_tiled_reference(
    manifest_path: &Path,
    input_path: &Path,
    long_edge: Option<u32>,
    max_long_edge: Option<u32>,
    tile_edge: usize,
    output_path: Option<&Path>,
    report_path: Option<&Path>,
    compare_cpu: bool,
) -> Result<(), String> {
    if tile_edge == 0 {
        return Err("--sr-lab-tile-edge requires a positive integer".to_owned());
    }
    let manifest = super::super::read_manifest(manifest_path).map_err(|error| error.to_string())?;
    let weights = super::super::blob::read_checked_weights(
        manifest_path,
        &manifest,
        "SPAN GPU tiled reference",
    )?;
    let (requested_long_edge, effective_long_edge) =
        cpu::span_reference_long_edge(long_edge, max_long_edge);
    let input = cpu::load_input_image(input_path, effective_long_edge)?;
    validation::validate_span_manifest(&manifest, &input)?;
    let halo = span_tile_halo(&manifest)?;
    let scale = manifest.scale as usize;
    let output_channels = manifest.output_channels as usize;
    let tile_specs = span_tile_specs(&input, tile_edge, halo);
    let workspace_shape_count = workspace_shape_count(&tile_specs);
    if workspace_shape_count > MAX_TILED_WORKSPACE_SHAPES {
        return Err(format!(
            "--sr-lab-tile-edge produced {workspace_shape_count} distinct SPAN GPU workspace shapes; increase --sr-lab-tile-edge or keep shapes at {MAX_TILED_WORKSPACE_SHAPES} or fewer"
        ));
    }
    let mut stitched = FeatureMap::zeros(
        output_channels,
        checked_scaled(input.height, scale)?,
        checked_scaled(input.width, scale)?,
    );

    let executor = SpanGpuExecutor::new()?;
    let model_started = Instant::now();
    let model = executor.upload_model(&weights);
    let model_buffer_init_ms = model_started.elapsed().as_secs_f64() * 1000.0;

    let tiled_started = Instant::now();
    let mut sessions = HashMap::new();
    let mut tile_reports = Vec::with_capacity(tile_specs.len());
    let mut samples = Vec::with_capacity(tile_specs.len());
    for (tile_index, spec) in tile_specs.iter().copied().enumerate() {
        let crop = crop_input(&input, spec);
        let shape = (crop.width, crop.height);
        let gpu = match sessions.entry(shape) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let session = executor.create_readback_session(&manifest, &model, &crop)?;
                entry.insert(session)
            }
        }
        .run(&crop)?;
        stitch_tile_output(&mut stitched, &gpu.output, spec, scale)?;
        samples.push(gpu.elapsed_ms);
        tile_reports.push(SpanGpuTileReport {
            tile_index,
            input_x: spec.x,
            input_y: spec.y,
            input_width: spec.width,
            input_height: spec.height,
            crop_x: spec.crop_x,
            crop_y: spec.crop_y,
            crop_width: spec.crop_width,
            crop_height: spec.crop_height,
            gpu_elapsed_ms: gpu.elapsed_ms,
        });
    }
    let total_cpu_orchestrated_elapsed_ms = tiled_started.elapsed().as_secs_f64() * 1000.0;
    debug_assert_eq!(workspace_shape_count, sessions.len());
    drop(sessions);

    if let Some(output_path) = output_path {
        cpu::write_output_image(output_path, &stitched)?;
    }
    let comparison = if compare_cpu {
        let cpu_output = cpu::span_forward(&manifest, &weights, &input)?;
        let comparison = compare_features(&cpu_output, &stitched)?;
        validate_comparison(&comparison)?;
        Some(comparison)
    } else {
        None
    };

    let report = SpanGpuTiledReferenceReport {
        manifest: manifest_path.display().to_string(),
        input: input_path.display().to_string(),
        model: manifest.name.clone(),
        variant: manifest.variant.clone(),
        requested_long_edge,
        effective_long_edge,
        input_width: input.width,
        input_height: input.height,
        output_width: stitched.width,
        output_height: stitched.height,
        tile_edge,
        halo,
        tile_count: tile_reports.len(),
        workspace_shape_count,
        model_buffer_init_ms,
        reuses_model_buffers: true,
        reuses_transient_buffers: workspace_shape_count < tile_reports.len(),
        total_cpu_orchestrated_elapsed_ms,
        tile_elapsed_ms: timing_stats(samples),
        comparison,
        tiles: tile_reports,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
    );
    if let Some(report_path) = report_path {
        if let Some(parent) = report_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(
            report_path,
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) struct SpanTileSpec {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) crop_x: usize,
    pub(crate) crop_y: usize,
    pub(crate) crop_width: usize,
    pub(crate) crop_height: usize,
}

pub(crate) fn span_tile_halo(manifest: &SrLabManifest) -> Result<usize, String> {
    let block_count = manifest
        .span
        .as_ref()
        .ok_or_else(|| "SPAN GPU tiled reference requires span metadata".to_owned())?
        .block_count as usize;
    block_count
        .checked_mul(3)
        .and_then(|radius| radius.checked_add(3))
        .ok_or_else(|| "SPAN tiled halo radius overflowed".to_owned())
}

pub(crate) fn span_tile_specs(
    input: &FeatureMap,
    tile_edge: usize,
    halo: usize,
) -> Vec<SpanTileSpec> {
    if tile_edge == 0 {
        return Vec::new();
    }
    let mut specs = Vec::new();
    let mut y = 0;
    while y < input.height {
        let height = tile_edge.min(input.height - y);
        let crop_y = y.saturating_sub(halo);
        let crop_y_end = (y + height).saturating_add(halo).min(input.height);
        let mut x = 0;
        while x < input.width {
            let width = tile_edge.min(input.width - x);
            let crop_x = x.saturating_sub(halo);
            let crop_x_end = (x + width).saturating_add(halo).min(input.width);
            specs.push(SpanTileSpec {
                x,
                y,
                width,
                height,
                crop_x,
                crop_y,
                crop_width: crop_x_end - crop_x,
                crop_height: crop_y_end - crop_y,
            });
            x += tile_edge;
        }
        y += tile_edge;
    }
    specs
}

pub(crate) fn workspace_shape_count(tile_specs: &[SpanTileSpec]) -> usize {
    let mut shapes = Vec::new();
    for spec in tile_specs {
        let shape = (spec.crop_width, spec.crop_height);
        if !shapes.contains(&shape) {
            shapes.push(shape);
        }
    }
    shapes.len()
}

fn checked_scaled(value: usize, scale: usize) -> Result<usize, String> {
    value
        .checked_mul(scale)
        .ok_or_else(|| "SPAN GPU tiled output size overflowed".to_owned())
}

fn crop_input(input: &FeatureMap, spec: SpanTileSpec) -> FeatureMap {
    let mut crop = FeatureMap::zeros(input.channels, spec.crop_height, spec.crop_width);
    for channel in 0..input.channels {
        for y in 0..spec.crop_height {
            for x in 0..spec.crop_width {
                let value = input.get(
                    channel,
                    (spec.crop_y + y) as isize,
                    (spec.crop_x + x) as isize,
                );
                crop.set(channel, y, x, value);
            }
        }
    }
    crop
}

fn stitch_tile_output(
    stitched: &mut FeatureMap,
    tile_output: &FeatureMap,
    spec: SpanTileSpec,
    scale: usize,
) -> Result<(), String> {
    let expected_width = spec.crop_width * scale;
    let expected_height = spec.crop_height * scale;
    if tile_output.channels != stitched.channels
        || tile_output.width != expected_width
        || tile_output.height != expected_height
    {
        return Err(format!(
            "SPAN GPU tile output shape mismatch: expected {}x{}x{}, got {}x{}x{}",
            stitched.channels,
            expected_width,
            expected_height,
            tile_output.channels,
            tile_output.width,
            tile_output.height
        ));
    }
    let source_x = (spec.x - spec.crop_x) * scale;
    let source_y = (spec.y - spec.crop_y) * scale;
    let dest_x = spec.x * scale;
    let dest_y = spec.y * scale;
    let copy_width = spec.width * scale;
    let copy_height = spec.height * scale;
    for channel in 0..stitched.channels {
        for y in 0..copy_height {
            for x in 0..copy_width {
                let value =
                    tile_output.get(channel, (source_y + y) as isize, (source_x + x) as isize);
                stitched.set(channel, dest_y + y, dest_x + x, value);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{crop_input, span_tile_specs, stitch_tile_output, workspace_shape_count};
    use crate::core::sr_lab::cpu::FeatureMap;

    #[test]
    fn tiled_specs_include_halo_and_clamp_to_image_edges() {
        let input = FeatureMap::zeros(3, 5, 7);
        let specs = span_tile_specs(&input, 3, 1);

        assert_eq!(specs.len(), 6);
        assert_eq!(specs[0].x, 0);
        assert_eq!(specs[0].y, 0);
        assert_eq!(specs[0].width, 3);
        assert_eq!(specs[0].height, 3);
        assert_eq!(specs[0].crop_x, 0);
        assert_eq!(specs[0].crop_y, 0);
        assert_eq!(specs[0].crop_width, 4);
        assert_eq!(specs[0].crop_height, 4);

        let last = specs.last().unwrap();
        assert_eq!(last.x, 6);
        assert_eq!(last.y, 3);
        assert_eq!(last.width, 1);
        assert_eq!(last.height, 2);
        assert_eq!(last.crop_x, 5);
        assert_eq!(last.crop_y, 2);
        assert_eq!(last.crop_width, 2);
        assert_eq!(last.crop_height, 3);
    }

    #[test]
    fn tiled_crop_and_stitch_copy_only_the_interior_region() {
        let mut input = FeatureMap::zeros(1, 4, 4);
        for y in 0..input.height {
            for x in 0..input.width {
                input.set(0, y, x, (y * 10 + x) as f32);
            }
        }

        let spec = span_tile_specs(&input, 2, 1)[3];
        let crop = crop_input(&input, spec);
        assert_eq!(crop.width, 3);
        assert_eq!(crop.height, 3);
        assert_eq!(crop.get(0, 0, 0), 11.0);
        assert_eq!(crop.get(0, 2, 2), 33.0);

        let scale = 2;
        let mut tile_output = FeatureMap::zeros(1, crop.height * scale, crop.width * scale);
        for y in 0..tile_output.height {
            for x in 0..tile_output.width {
                tile_output.set(0, y, x, (y * 100 + x) as f32);
            }
        }
        let mut stitched = FeatureMap::zeros(1, input.height * scale, input.width * scale);

        stitch_tile_output(&mut stitched, &tile_output, spec, scale).unwrap();

        assert_eq!(stitched.get(0, 4, 4), 202.0);
        assert_eq!(stitched.get(0, 5, 5), 303.0);
        assert_eq!(stitched.get(0, 3, 4), 0.0);
        assert_eq!(stitched.get(0, 4, 3), 0.0);
    }

    #[test]
    fn workspace_shape_count_deduplicates_crop_dimensions() {
        let input = FeatureMap::zeros(3, 6, 8);
        let specs = span_tile_specs(&input, 4, 21);

        assert_eq!(workspace_shape_count(&specs), 1);
    }

    #[test]
    fn tiled_specs_reject_zero_tile_edge() {
        let input = FeatureMap::zeros(3, 6, 8);
        assert!(span_tile_specs(&input, 0, 21).is_empty());
    }
}
