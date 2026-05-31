mod executor;

use self::executor::SpanGpuExecutor;
use super::cpu::{self, FeatureMap};
use serde::Serialize;
use std::fs;
use std::path::Path;
use std::time::Instant;

const MAX_COMPARE_MAE: f64 = 1.0e-4;
const MAX_COMPARE_ABS_DIFF: f32 = 1.0e-3;

#[derive(Debug, Serialize)]
pub struct SpanGpuReferenceReport {
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
    pub model_upload_ms: f64,
    pub gpu_elapsed_ms: f64,
    pub comparison: Option<SpanGpuComparison>,
}

#[derive(Debug, Serialize)]
pub struct SpanGpuComparison {
    pub mae: f64,
    pub rmse: f64,
    pub max_abs_diff: f32,
    pub psnr: Option<f64>,
}

pub fn run_span_gpu_reference(
    manifest_path: &Path,
    input_path: &Path,
    long_edge: Option<u32>,
    output_path: Option<&Path>,
    report_path: Option<&Path>,
    compare_cpu: bool,
) -> Result<(), String> {
    let manifest = super::read_manifest(manifest_path).map_err(|error| error.to_string())?;
    let weights_file = manifest
        .weights_file
        .as_deref()
        .ok_or_else(|| "SPAN GPU reference requires manifest weights_file".to_owned())?;
    let weights_path = manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(weights_file);
    let weights = super::blob::read_weights(&weights_path)?;
    let (requested_long_edge, effective_long_edge) = cpu::span_reference_long_edge(long_edge);
    let input = cpu::load_input_image(input_path, effective_long_edge)?;

    let executor = SpanGpuExecutor::new()?;
    let upload_started = Instant::now();
    let model = executor.upload_model(&weights);
    let model_upload_ms = upload_started.elapsed().as_secs_f64() * 1000.0;
    let gpu = executor.run(&manifest, &model, &input)?;
    if let Some(output_path) = output_path {
        cpu::write_output_image(output_path, &gpu.output)?;
    }
    let comparison = if compare_cpu {
        let cpu_output = cpu::span_forward(&manifest, &weights, &input)?;
        let comparison = compare_features(&cpu_output, &gpu.output)?;
        validate_comparison(&comparison)?;
        Some(comparison)
    } else {
        None
    };

    let report = SpanGpuReferenceReport {
        manifest: manifest_path.display().to_string(),
        input: input_path.display().to_string(),
        model: manifest.name.clone(),
        variant: manifest.variant.clone(),
        requested_long_edge,
        effective_long_edge,
        input_width: input.width,
        input_height: input.height,
        output_width: gpu.output.width,
        output_height: gpu.output.height,
        model_upload_ms,
        gpu_elapsed_ms: gpu.elapsed_ms,
        comparison,
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

fn compare_features(cpu: &FeatureMap, gpu: &FeatureMap) -> Result<SpanGpuComparison, String> {
    if cpu.channels != gpu.channels || cpu.height != gpu.height || cpu.width != gpu.width {
        return Err(format!(
            "CPU/GPU output shape mismatch: CPU {}x{}x{}, GPU {}x{}x{}",
            cpu.channels, cpu.width, cpu.height, gpu.channels, gpu.width, gpu.height
        ));
    }
    let mut abs_total = 0.0f64;
    let mut square_total = 0.0f64;
    let mut max_abs_diff = 0.0f32;
    for (left, right) in cpu.values.iter().zip(&gpu.values) {
        let diff = (*left - *right).abs();
        max_abs_diff = max_abs_diff.max(diff);
        abs_total += diff as f64;
        square_total += (diff as f64) * (diff as f64);
    }
    let count = cpu.values.len() as f64;
    let mae = abs_total / count;
    let rmse = (square_total / count).sqrt();
    let psnr = (rmse > 0.0).then(|| 20.0 * (255.0 / rmse).log10());
    Ok(SpanGpuComparison {
        mae,
        rmse,
        max_abs_diff,
        psnr,
    })
}

fn validate_comparison(comparison: &SpanGpuComparison) -> Result<(), String> {
    if comparison.mae > MAX_COMPARE_MAE || comparison.max_abs_diff > MAX_COMPARE_ABS_DIFF {
        return Err(format!(
            "SPAN GPU reference diverged from CPU reference: MAE {:.8}, max abs diff {:.8}",
            comparison.mae, comparison.max_abs_diff
        ));
    }
    Ok(())
}
