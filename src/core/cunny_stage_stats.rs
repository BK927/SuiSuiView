//! Per-pass activation statistics for a CuNNy variant, for checking a WGSL port.
//!
//! A mis-ported convolution chain rarely fails loudly: it renders, it looks like
//! an image, and only a quality scan reveals it is worse than doing nothing. The
//! measurement that localises such a break is the chain's own intermediates —
//! run one pass at a time and summarise every texture it wrote. A healthy CuNNy
//! layer keeps its activations spread across the byte range; a broken one pins to
//! a rail or collapses to a constant, and the first pass where that happens is
//! where the port went wrong.
//!
//! Compare a suspect variant against a known-good sibling of the same family and
//! size class — the two differ only in trained weights, so their *shape* should
//! track even though their values do not.

use crate::core::state::WgpuUpscaleMethod;
use crate::core::upscale_bench::gpu::{CunnyStageSample, GpuUpscaleBench, FINAL_OUTPUT_SLOT};
use crate::core::worker::{prepare_image_with_options, DecodeOptions};
use egui::ColorImage;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Longest edge the probe image is reduced to before the chain runs. Every pass
/// dispatches one thread per source pixel and reads back afterwards, so a full
/// page would spend minutes in stalls for no extra signal.
pub const DEFAULT_STAGE_LONG_EDGE: u32 = 256;

#[derive(Debug, Serialize)]
pub struct CunnyStageReport {
    pub image: String,
    pub method: String,
    pub source_width: usize,
    pub source_height: usize,
    pub passes: usize,
    pub stages: Vec<StageRow>,
    /// Passes whose every written texture looks degenerate. The first entry is
    /// the one to inspect in the generated WGSL.
    pub suspect_passes: Vec<usize>,
}

#[derive(Debug, Serialize)]
pub struct StageRow {
    pub pass: usize,
    /// Intermediate index, or `"output"` for the chain's visible 2x result.
    pub slot: String,
    pub mean: f64,
    pub std_dev: f64,
    pub min: u8,
    pub max: u8,
    pub zero_ratio: f64,
    pub saturated_ratio: f64,
    pub degenerate: bool,
}

/// A texture that carries no usable signal: everything at one value, or almost
/// every byte pinned to a rail. Thresholds are deliberately loose — this flags
/// candidates for a human to look at, it does not adjudicate.
fn is_degenerate(sample: &CunnyStageSample) -> bool {
    let stats = &sample.stats;
    stats.std_dev < 0.5 || stats.zero_ratio > 0.98 || stats.saturated_ratio > 0.98
}

fn slot_label(slot: usize) -> String {
    if slot == FINAL_OUTPUT_SLOT {
        "output".to_owned()
    } else {
        slot.to_string()
    }
}

pub fn run_cunny_stage_stats(
    image_path: &Path,
    method: WgpuUpscaleMethod,
    long_edge: u32,
) -> Result<CunnyStageReport, String> {
    let image = load_probe_image(image_path, long_edge)?;
    let gpu = GpuUpscaleBench::new_for_method(Some(method))?;
    let samples = gpu.cunny_stage_stats(method, &image)?;
    Ok(build_report(image_path, method, &image, &samples))
}

fn build_report(
    image_path: &Path,
    method: WgpuUpscaleMethod,
    image: &ColorImage,
    samples: &[CunnyStageSample],
) -> CunnyStageReport {
    let stages: Vec<StageRow> = samples
        .iter()
        .map(|sample| StageRow {
            pass: sample.pass,
            slot: slot_label(sample.slot),
            mean: sample.stats.mean,
            std_dev: sample.stats.std_dev,
            min: sample.stats.min,
            max: sample.stats.max,
            zero_ratio: sample.stats.zero_ratio,
            saturated_ratio: sample.stats.saturated_ratio,
            degenerate: is_degenerate(sample),
        })
        .collect();

    let passes = samples
        .iter()
        .map(|sample| sample.pass)
        .max()
        .map_or(0, |p| p + 1);
    let mut suspect_passes = Vec::new();
    for pass in 0..passes {
        let mut wrote_any = false;
        let mut all_degenerate = true;
        for sample in samples.iter().filter(|s| s.pass == pass) {
            wrote_any = true;
            all_degenerate &= is_degenerate(sample);
        }
        if wrote_any && all_degenerate {
            suspect_passes.push(pass);
        }
    }

    CunnyStageReport {
        image: image_path.display().to_string(),
        method: method.token().to_owned(),
        source_width: image.size[0],
        source_height: image.size[1],
        passes,
        stages,
        suspect_passes,
    }
}

fn load_probe_image(path: &Path, long_edge: u32) -> Result<ColorImage, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let prepared = prepare_image_with_options(&bytes, long_edge.max(16), DecodeOptions::default())
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(prepared.color_image())
}

pub fn print_cunny_stage_report(report: &CunnyStageReport) {
    println!("SuiSuiView CuNNy stage stats");
    println!("Image:  {}", report.image);
    println!("Method: {}", report.method);
    println!(
        "Source: {}x{}  ({} passes)",
        report.source_width, report.source_height, report.passes
    );
    println!();
    println!(
        "{:>4}  {:>6}  {:>8}  {:>8}  {:>4}  {:>4}  {:>7}  {:>7}",
        "pass", "slot", "mean", "stddev", "min", "max", "zero%", "sat%"
    );
    for row in &report.stages {
        println!(
            "{:>4}  {:>6}  {:>8.2}  {:>8.2}  {:>4}  {:>4}  {:>6.1}%  {:>6.1}%{}",
            row.pass,
            row.slot,
            row.mean,
            row.std_dev,
            row.min,
            row.max,
            row.zero_ratio * 100.0,
            row.saturated_ratio * 100.0,
            if row.degenerate {
                "  <-- degenerate"
            } else {
                ""
            },
        );
    }
    println!();
    if report.suspect_passes.is_empty() {
        println!("No pass wrote only degenerate textures.");
    } else {
        println!(
            "Passes whose every output is degenerate: {:?}",
            report.suspect_passes
        );
        println!("Inspect the first one in the generated WGSL; that is where the port diverges.");
    }
}

pub fn default_cunny_stage_report_path() -> PathBuf {
    PathBuf::from("bench-output").join("cunny-stage-stats.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::upscale_bench::gpu::ByteStats;

    fn sample(pass: usize, slot: usize, stats: ByteStats) -> CunnyStageSample {
        CunnyStageSample { pass, slot, stats }
    }

    fn stats(mean: f64, std_dev: f64, zero: f64, sat: f64) -> ByteStats {
        ByteStats {
            mean,
            std_dev,
            min: 0,
            max: 255,
            zero_ratio: zero,
            saturated_ratio: sat,
        }
    }

    #[test]
    fn byte_stats_describe_a_uniform_ramp() {
        let bytes: Vec<u8> = (0..=255).collect();
        let computed = ByteStats::of(&bytes);
        assert!((computed.mean - 127.5).abs() < 0.01);
        assert!(computed.std_dev > 70.0, "a full ramp is not flat");
        assert_eq!(computed.min, 0);
        assert_eq!(computed.max, 255);
        // Exactly one byte at each rail.
        assert!((computed.zero_ratio - 1.0 / 256.0).abs() < 1e-9);
        assert!((computed.saturated_ratio - 1.0 / 256.0).abs() < 1e-9);
    }

    #[test]
    fn a_constant_texture_is_degenerate() {
        assert!(is_degenerate(&sample(0, 0, ByteStats::of(&[7_u8; 64]))));
        assert!(!is_degenerate(&sample(
            0,
            0,
            ByteStats::of(&(0..=255).collect::<Vec<u8>>())
        )));
    }

    #[test]
    fn a_rail_pinned_texture_is_degenerate_even_with_spread() {
        // 99% zeros with a few live bytes still carries no usable signal.
        assert!(is_degenerate(&sample(1, 2, stats(2.0, 12.0, 0.99, 0.0))));
        assert!(is_degenerate(&sample(1, 2, stats(253.0, 12.0, 0.0, 0.99))));
    }

    #[test]
    fn only_a_pass_whose_every_output_is_degenerate_is_suspect() {
        let healthy = stats(120.0, 40.0, 0.01, 0.01);
        let dead = stats(0.0, 0.0, 1.0, 0.0);
        let samples = vec![
            sample(0, 0, healthy),
            sample(0, 1, healthy),
            // Pass 1 half-collapses: not conclusive on its own, so not flagged.
            sample(1, 0, dead),
            sample(1, 1, healthy),
            // Pass 2 is wholly dead — this is the one to look at.
            sample(2, 0, dead),
            sample(2, 1, dead),
        ];
        let image = ColorImage::new([4, 4], vec![egui::Color32::BLACK; 16]);
        let report = build_report(
            Path::new("probe.png"),
            WgpuUpscaleMethod::Cunny4x32Soft,
            &image,
            &samples,
        );
        assert_eq!(report.suspect_passes, vec![2]);
        assert_eq!(report.passes, 3);
    }

    #[test]
    fn the_final_output_row_is_labelled_rather_than_numbered() {
        let samples = vec![sample(0, FINAL_OUTPUT_SLOT, stats(120.0, 40.0, 0.0, 0.0))];
        let image = ColorImage::new([2, 2], vec![egui::Color32::BLACK; 4]);
        let report = build_report(
            Path::new("probe.png"),
            WgpuUpscaleMethod::Cunny8x32Nvl,
            &image,
            &samples,
        );
        assert_eq!(report.stages[0].slot, "output");
    }
}
