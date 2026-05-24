use crate::core::source::open_source_from_path;
use crate::core::worker::{
    clamp_target_long_edge, prepare_image_with_strategy, DecodeBackend, DecodeStrategy,
};
use eframe::egui::ColorImage;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Serialize)]
pub struct QualityReport {
    pub path: String,
    pub page_count: usize,
    pub target_long_edge: u32,
    pub failures: usize,
    pub pages: Vec<PageQuality>,
}

#[derive(Debug, Serialize)]
pub struct PageQuality {
    pub index: usize,
    pub name: String,
    pub backend: String,
    pub ssim: f64,
    pub mae: f64,
    pub premultiplied_mae: f64,
    pub edge_mae: f64,
    pub rmse: f64,
    pub psnr: f64,
    pub max_abs_diff: u8,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct QualityMetrics {
    ssim: f64,
    mae: f64,
    premultiplied_mae: f64,
    edge_mae: f64,
    rmse: f64,
    psnr: f64,
    max_abs_diff: u8,
}

pub fn run_quality_scan(
    path: &Path,
    target_long_edge: u32,
    report_path: Option<&Path>,
) -> Result<(), String> {
    let report = scan_path(path, clamp_target_long_edge(target_long_edge))?;
    print_report(&report);
    if let Some(report_path) = report_path {
        write_report(report_path, &report)?;
        println!("Report: {}", report_path.display());
    }
    Ok(())
}

pub fn scan_path(path: &Path, target_long_edge: u32) -> Result<QualityReport, String> {
    let started = Instant::now();
    let (source, _forced_page) = open_source_from_path(path).map_err(|error| error.to_string())?;
    let mut pages = Vec::with_capacity(source.page_count());
    let mut failures = 0usize;

    for index in 0..source.page_count() {
        let name = source.page_name(index).unwrap_or("").to_owned();
        let mut page = PageQuality {
            index,
            name,
            backend: DecodeBackend::ImageCrate.as_str().to_owned(),
            ssim: 0.0,
            mae: 0.0,
            premultiplied_mae: 0.0,
            edge_mae: 0.0,
            rmse: 0.0,
            psnr: 0.0,
            max_abs_diff: 0,
            error: None,
        };

        let result = source
            .read_page(index)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                let baseline = prepare_image_with_strategy(
                    &bytes,
                    target_long_edge,
                    DecodeStrategy::ImageCrate,
                )?;
                let candidate =
                    prepare_image_with_strategy(&bytes, target_long_edge, DecodeStrategy::Auto)?;
                let metrics = compare_images(&baseline.color_image(), &candidate.color_image())?;
                Ok((candidate.decode_backend, metrics))
            });

        match result {
            Ok((backend, metrics)) => {
                page.backend = backend.as_str().to_owned();
                page.ssim = metrics.ssim;
                page.mae = metrics.mae;
                page.premultiplied_mae = metrics.premultiplied_mae;
                page.edge_mae = metrics.edge_mae;
                page.rmse = metrics.rmse;
                page.psnr = metrics.psnr;
                page.max_abs_diff = metrics.max_abs_diff;
            }
            Err(error) => {
                failures += 1;
                page.error = Some(error);
            }
        }

        pages.push(page);
    }

    println!(
        "Quality scan elapsed: {:.2} s",
        started.elapsed().as_secs_f64()
    );
    Ok(QualityReport {
        path: path.display().to_string(),
        page_count: source.page_count(),
        target_long_edge,
        failures,
        pages,
    })
}

fn compare_images(baseline: &ColorImage, candidate: &ColorImage) -> Result<QualityMetrics, String> {
    if baseline.size != candidate.size {
        return Err(format!(
            "Image sizes differ: baseline {:?}, candidate {:?}",
            baseline.size, candidate.size
        ));
    }
    if baseline.pixels.is_empty() {
        return Err("Image has no pixels".to_owned());
    }

    let mut sum_abs = 0.0;
    let mut sum_sq = 0.0;
    let mut max_abs_diff = 0u8;
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_x2 = 0.0;
    let mut sum_y2 = 0.0;
    let mut sum_xy = 0.0;
    let mut premultiplied_abs = 0.0;
    let mut premultiplied_channels = 0.0;

    let mut compared_channels = 0.0;
    for (left, right) in baseline.pixels.iter().zip(candidate.pixels.iter()) {
        let left = left.to_array();
        let right = right.to_array();

        for channel in 0..3 {
            let left_value = f64::from(left[channel]) * f64::from(left[3]) / 255.0;
            let right_value = f64::from(right[channel]) * f64::from(right[3]) / 255.0;
            premultiplied_abs += (left_value - right_value).abs();
            premultiplied_channels += 1.0;
        }
        premultiplied_abs += (f64::from(left[3]) - f64::from(right[3])).abs();
        premultiplied_channels += 1.0;

        if left[3] != 0 || right[3] != 0 {
            for channel in 0..3 {
                let diff = (f64::from(left[channel]) - f64::from(right[channel])).abs();
                sum_abs += diff;
                sum_sq += diff * diff;
                compared_channels += 1.0;
                max_abs_diff = max_abs_diff.max(diff as u8);
            }
        }

        let x = luma(left);
        let y = luma(right);
        sum_x += x;
        sum_y += y;
        sum_x2 += x * x;
        sum_y2 += y * y;
        sum_xy += x * y;
    }

    let pixel_count = baseline.pixels.len() as f64;
    let channel_count = if compared_channels == 0.0 {
        pixel_count * 3.0
    } else {
        compared_channels
    };
    let mae = sum_abs / channel_count;
    let mse = sum_sq / channel_count;
    let rmse = mse.sqrt();
    let psnr = if mse == 0.0 {
        f64::INFINITY
    } else {
        20.0 * (255.0 / rmse).log10()
    };

    let mean_x = sum_x / pixel_count;
    let mean_y = sum_y / pixel_count;
    let var_x = (sum_x2 / pixel_count) - mean_x * mean_x;
    let var_y = (sum_y2 / pixel_count) - mean_y * mean_y;
    let cov_xy = (sum_xy / pixel_count) - mean_x * mean_y;
    let c1 = 6.5025;
    let c2 = 58.5225;
    let denominator = (mean_x * mean_x + mean_y * mean_y + c1) * (var_x + var_y + c2);
    let ssim = if denominator == 0.0 {
        1.0
    } else {
        ((2.0 * mean_x * mean_y + c1) * (2.0 * cov_xy + c2)) / denominator
    };

    Ok(QualityMetrics {
        ssim,
        mae,
        premultiplied_mae: premultiplied_abs / premultiplied_channels,
        edge_mae: edge_mae(baseline, candidate),
        rmse,
        psnr,
        max_abs_diff,
    })
}

fn luma(rgba: [u8; 4]) -> f64 {
    0.2126 * f64::from(rgba[0]) + 0.7152 * f64::from(rgba[1]) + 0.0722 * f64::from(rgba[2])
}

fn edge_mae(baseline: &ColorImage, candidate: &ColorImage) -> f64 {
    let [width, height] = baseline.size;
    if width < 2 || height < 2 {
        return 0.0;
    }

    let step = edge_sample_step(width, height);
    let mut sum_abs = 0.0;
    let mut samples: f64 = 0.0;
    for y in (0..height - 1).step_by(step) {
        for x in (0..width - 1).step_by(step) {
            let index = y * width + x;
            let right = index + 1;
            let down = index + width;
            let baseline_edge = edge_strength(
                baseline.pixels[index].to_array(),
                baseline.pixels[right].to_array(),
                baseline.pixels[down].to_array(),
            );
            let candidate_edge = edge_strength(
                candidate.pixels[index].to_array(),
                candidate.pixels[right].to_array(),
                candidate.pixels[down].to_array(),
            );
            sum_abs += (baseline_edge - candidate_edge).abs();
            samples += 1.0;
        }
    }
    sum_abs / samples.max(1.0)
}

fn edge_sample_step(width: usize, height: usize) -> usize {
    let pixels = width.saturating_mul(height);
    if pixels > 4_000_000 {
        4
    } else if pixels > 1_000_000 {
        2
    } else {
        1
    }
}

fn edge_strength(pixel: [u8; 4], right: [u8; 4], down: [u8; 4]) -> f64 {
    let center = luma(pixel);
    let dx = luma(right) - center;
    let dy = luma(down) - center;
    (dx * dx + dy * dy).sqrt()
}

fn print_report(report: &QualityReport) {
    let successful = report.page_count.saturating_sub(report.failures);
    println!("SuiSuiView quality scan");
    println!("Path: {}", report.path);
    println!("Pages: {} ok / {} failed", successful, report.failures);
    println!("Target long edge: {}", report.target_long_edge);

    let mut summaries: BTreeMap<&str, BackendSummary> = BTreeMap::new();
    for page in report.pages.iter().filter(|page| page.error.is_none()) {
        summaries
            .entry(page.backend.as_str())
            .or_default()
            .push(page);
    }

    for (backend, summary) in summaries {
        println!(
            "{backend}: count {}, avg SSIM {:.5}, worst SSIM {:.5}, avg MAE {:.2}, avg edge MAE {:.2}, max diff {}",
            summary.count,
            summary.ssim_sum / summary.count as f64,
            summary.worst_ssim,
            summary.mae_sum / summary.count as f64,
            summary.edge_mae_sum / summary.count as f64,
            summary.max_abs_diff
        );
    }

    let mut worst = report
        .pages
        .iter()
        .filter(|page| page.error.is_none())
        .collect::<Vec<_>>();
    worst.sort_by(|left, right| {
        left.ssim
            .partial_cmp(&right.ssim)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    for page in worst.into_iter().take(10) {
        println!(
            "Worst page {:>4}: {:<12} SSIM {:.5}, MAE {:>6.2}, RMSE {:>6.2}, PSNR {:>6.2}, max {}, {}",
            page.index + 1,
            page.backend,
            page.ssim,
            page.mae,
            page.rmse,
            page.psnr,
            page.max_abs_diff,
            page.name
        );
    }

    for page in report
        .pages
        .iter()
        .filter(|page| page.error.is_some())
        .take(5)
    {
        println!(
            "Failed page {:>4}: {}",
            page.index + 1,
            page.error.as_deref().unwrap_or("unknown error")
        );
    }
}

fn write_report(path: &Path, report: &QualityReport) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string_pretty(report).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

#[derive(Default)]
struct BackendSummary {
    count: usize,
    ssim_sum: f64,
    mae_sum: f64,
    edge_mae_sum: f64,
    worst_ssim: f64,
    max_abs_diff: u8,
}

impl BackendSummary {
    fn push(&mut self, page: &PageQuality) {
        self.count += 1;
        self.ssim_sum += page.ssim;
        self.mae_sum += page.mae;
        self.edge_mae_sum += page.edge_mae;
        self.worst_ssim = if self.count == 1 {
            page.ssim
        } else {
            self.worst_ssim.min(page.ssim)
        };
        self.max_abs_diff = self.max_abs_diff.max(page.max_abs_diff);
    }
}
