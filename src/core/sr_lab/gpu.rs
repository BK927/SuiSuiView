pub(crate) mod buffers;
mod executor;
pub(crate) mod kernel;
pub(crate) mod model_validation;
pub(crate) mod tiled;
mod validation;

use self::executor::SpanGpuExecutor;
use super::cpu::{self, FeatureMap};
use serde::Serialize;
use std::fs;
use std::path::Path;
use std::time::Instant;

const MAX_COMPARE_MAE: f64 = 1.0e-4;
const MAX_COMPARE_ABS_DIFF: f32 = 1.0e-3;
pub const DEFAULT_SPAN_SESSION_WARMUPS: usize = 1;
pub const DEFAULT_SPAN_SESSION_ITERATIONS: usize = 5;

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

#[derive(Debug, Serialize)]
pub struct SpanGpuSessionBenchReport {
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
    pub warmups: usize,
    pub iterations: usize,
    pub model_buffer_init_ms: f64,
    pub session_setup_ms: f64,
    pub readback: bool,
    pub reuses_model_buffers: bool,
    pub reuses_transient_buffers: bool,
    pub elapsed_ms: SpanGpuTimingStats,
}

#[derive(Debug, Default, Serialize)]
pub struct SpanGpuTimingStats {
    pub samples_ms: Vec<f64>,
    pub mean_ms: f64,
    pub min_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
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
    let weights =
        super::blob::read_checked_weights(manifest_path, &manifest, "SPAN GPU reference")?;
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

pub fn run_span_gpu_session_bench(
    manifest_path: &Path,
    input_path: &Path,
    long_edge: Option<u32>,
    warmups: usize,
    iterations: usize,
    report_path: Option<&Path>,
) -> Result<(), String> {
    if iterations == 0 {
        return Err("--sr-lab-iterations requires a positive integer".to_owned());
    }
    let manifest = super::read_manifest(manifest_path).map_err(|error| error.to_string())?;
    let weights =
        super::blob::read_checked_weights(manifest_path, &manifest, "SPAN GPU session benchmark")?;
    let (requested_long_edge, effective_long_edge) = cpu::span_reference_long_edge(long_edge);
    let input = cpu::load_input_image(input_path, effective_long_edge)?;

    let executor = SpanGpuExecutor::new()?;
    let model_started = Instant::now();
    let model = executor.upload_model(&weights);
    let model_buffer_init_ms = model_started.elapsed().as_secs_f64() * 1000.0;
    let setup_started = Instant::now();
    let session = executor.create_session(&manifest, &model, &input)?;
    let session_setup_ms = setup_started.elapsed().as_secs_f64() * 1000.0;

    for _ in 0..warmups {
        session.run()?;
    }
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        samples.push(session.run()?.elapsed_ms);
    }

    let report = SpanGpuSessionBenchReport {
        manifest: manifest_path.display().to_string(),
        input: input_path.display().to_string(),
        model: manifest.name.clone(),
        variant: manifest.variant.clone(),
        requested_long_edge,
        effective_long_edge,
        input_width: input.width,
        input_height: input.height,
        output_width: session.output_width(),
        output_height: session.output_height(),
        warmups,
        iterations,
        model_buffer_init_ms,
        session_setup_ms,
        readback: false,
        reuses_model_buffers: true,
        reuses_transient_buffers: true,
        elapsed_ms: timing_stats(samples),
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

fn timing_stats(mut samples_ms: Vec<f64>) -> SpanGpuTimingStats {
    if samples_ms.is_empty() {
        return SpanGpuTimingStats::default();
    }
    samples_ms.sort_by(|left, right| left.total_cmp(right));
    let total_ms = samples_ms.iter().sum::<f64>();
    let mean_ms = total_ms / samples_ms.len() as f64;
    let min_ms = samples_ms[0];
    let max_ms = samples_ms[samples_ms.len() - 1];
    SpanGpuTimingStats {
        p50_ms: percentile(&samples_ms, 0.50),
        p95_ms: percentile(&samples_ms, 0.95),
        p99_ms: percentile(&samples_ms, 0.99),
        samples_ms,
        mean_ms,
        min_ms,
        max_ms,
    }
}

fn percentile(samples: &[f64], percentile: f64) -> f64 {
    let last = samples.len().saturating_sub(1);
    let index = ((last as f64) * percentile.clamp(0.0, 1.0)).ceil() as usize;
    samples[index.min(last)]
}
