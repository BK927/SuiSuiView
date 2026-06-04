use crate::core::source::open_source_from_path;
use crate::core::state::{ResizeFilter, WgpuUpscaleMethod};
use crate::core::upscale_bench::{gpu::GpuUpscaleBench, gpu_methods_for_filter};
use crate::core::worker::clamp_target_long_edge;
use eframe::egui::ColorImage;
use image::{imageops::FilterType, RgbaImage};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "upscale_quality/visuals.rs"]
mod visuals;

#[derive(Debug, Serialize)]
pub struct UpscaleQualityReport {
    pub path: String,
    pub title: String,
    pub page_count: usize,
    pub source_long_edge: u32,
    pub target_long_edge: u32,
    pub gpu_available: bool,
    pub gpu_error: Option<String>,
    pub failures: usize,
    pub methods: Vec<UpscaleQualitySummary>,
    pub pages: Vec<PageUpscaleQuality>,
}

#[derive(Debug, Serialize)]
pub struct UpscaleQualitySummary {
    pub method: String,
    pub pages: usize,
    pub ssim: f64,
    pub mae: f64,
    pub rmse: f64,
    pub psnr: f64,
    pub max_abs_diff: u8,
    pub different_pixel_ratio: f64,
    pub edge_mae: f64,
    pub ringing_score: f64,
}

#[derive(Debug, Serialize)]
pub struct PageUpscaleQuality {
    pub index: usize,
    pub name: String,
    pub source_width: Option<usize>,
    pub source_height: Option<usize>,
    pub output_width: Option<usize>,
    pub output_height: Option<usize>,
    pub contact_sheet: Option<String>,
    pub visuals: Vec<UpscaleQualityVisual>,
    pub runs: Vec<UpscaleQualityRun>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UpscaleQualityVisual {
    pub method: String,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct UpscaleQualityRun {
    pub method: String,
    pub ssim: f64,
    pub mae: f64,
    pub rmse: f64,
    pub psnr: f64,
    pub max_abs_diff: u8,
    pub different_pixel_ratio: f64,
    pub edge_mae: f64,
    pub ringing_score: f64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct QualityMetrics {
    ssim: f64,
    mae: f64,
    rmse: f64,
    psnr: f64,
    max_abs_diff: u8,
    different_pixel_ratio: f64,
    edge_mae: f64,
    ringing_score: f64,
}

pub fn run_upscale_quality_scan(
    path: &Path,
    report_path: Option<&Path>,
    visual_dir: Option<&Path>,
    source_long_edge: u32,
    target_long_edge: u32,
    method_filter: Option<WgpuUpscaleMethod>,
    max_pages: Option<usize>,
) -> Result<(), String> {
    let report = scan_upscale_quality(
        path,
        clamp_upscale_source_long_edge(source_long_edge),
        clamp_target_long_edge(target_long_edge),
        visual_dir,
        method_filter,
        max_pages,
    )?;
    print_report(&report);
    if let Some(report_path) = report_path {
        write_report(report_path, &report)?;
        println!("Report: {}", report_path.display());
    }
    if let Some(method) = method_filter {
        if let Some(error) = selected_method_failure(&report, method) {
            return Err(error);
        }
    }
    Ok(())
}

pub fn scan_upscale_quality(
    path: &Path,
    source_long_edge: u32,
    target_long_edge: u32,
    visual_dir: Option<&Path>,
    method_filter: Option<WgpuUpscaleMethod>,
    max_pages: Option<usize>,
) -> Result<UpscaleQualityReport, String> {
    let (source, _forced_page) = open_source_from_path(path).map_err(|error| error.to_string())?;
    let gpu = match GpuUpscaleBench::new_for_method(method_filter) {
        Ok(gpu) => Some(gpu),
        Err(error) => {
            eprintln!("WGSL upscale quality scan disabled: {error}");
            None
        }
    };
    let gpu_error = gpu
        .is_none()
        .then(|| "WGSL upscale backend unavailable".to_owned());

    let mut failures = 0usize;
    let scanned_pages = scanned_page_count(source.page_count(), max_pages);
    let gpu_methods = gpu_methods_for_filter(method_filter);
    let mut pages = Vec::with_capacity(scanned_pages);
    let mut summaries = BTreeMap::<String, SummaryAccumulator>::new();

    for index in 0..scanned_pages {
        let mut page = PageUpscaleQuality {
            index,
            name: source.page_name(index).unwrap_or("").to_owned(),
            source_width: None,
            source_height: None,
            output_width: None,
            output_height: None,
            contact_sheet: None,
            visuals: Vec::new(),
            runs: Vec::new(),
            error: None,
        };

        let result = source
            .read_page(index)
            .map_err(|error| error.to_string())
            .and_then(|bytes| prepare_page_pair(&bytes, source_long_edge, target_long_edge));

        match result {
            Ok((input_image, baseline_image)) => {
                let output_size = baseline_image.size;
                page.source_width = Some(input_image.size[0]);
                page.source_height = Some(input_image.size[1]);
                page.output_width = Some(output_size[0]);
                page.output_height = Some(output_size[1]);

                let mut visual_images = visual_dir
                    .is_some()
                    .then(|| vec![("reference-lanczos3".to_owned(), baseline_image.clone())]);

                let bicubic = run_cpu_case(
                    &input_image,
                    &baseline_image,
                    output_size,
                    "Bicubic",
                    ResizeFilter::Bicubic,
                    &mut page,
                    &mut summaries,
                );
                if let Some(images) = &mut visual_images {
                    images.push(("cpu-bicubic".to_owned(), bicubic));
                }

                let lanczos3 = run_cpu_case(
                    &input_image,
                    &baseline_image,
                    output_size,
                    "Lanczos3",
                    ResizeFilter::Lanczos3,
                    &mut page,
                    &mut summaries,
                );
                if let Some(images) = &mut visual_images {
                    images.push(("cpu-lanczos3".to_owned(), lanczos3));
                }

                let triangle = run_cpu_case(
                    &input_image,
                    &baseline_image,
                    output_size,
                    "Fast/Triangle",
                    ResizeFilter::FastTriangle,
                    &mut page,
                    &mut summaries,
                );
                if let Some(images) = &mut visual_images {
                    images.push(("cpu-fast-triangle".to_owned(), triangle));
                }

                if let Some(gpu) = &gpu {
                    for method in &gpu_methods {
                        let image = run_gpu_case(
                            gpu,
                            &input_image,
                            &baseline_image,
                            output_size,
                            *method,
                            &mut page,
                            &mut summaries,
                        );
                        if let (Some(images), Some(image)) = (&mut visual_images, image) {
                            images.push((visuals::sanitize_name(method.label()), image));
                        }
                    }
                }

                if let (Some(root), Some(images)) = (visual_dir, visual_images) {
                    match visuals::write_page_visuals(root, index, &images) {
                        Ok((contact_sheet, visuals)) => {
                            page.contact_sheet = Some(contact_sheet);
                            page.visuals = visuals;
                        }
                        Err(error) => {
                            page.error = Some(format!("visual export failed: {error}"));
                            failures += 1;
                        }
                    }
                }
            }
            Err(error) => {
                failures += 1;
                page.error = Some(error);
            }
        }

        pages.push(page);
    }

    Ok(UpscaleQualityReport {
        path: path.display().to_string(),
        title: source.title().to_owned(),
        page_count: source.page_count(),
        source_long_edge,
        target_long_edge,
        gpu_available: gpu.is_some(),
        gpu_error,
        failures,
        methods: summaries
            .into_iter()
            .map(|(method, summary)| summary.finish(method))
            .collect(),
        pages,
    })
}

fn prepare_page_pair(
    bytes: &[u8],
    source_long_edge: u32,
    target_long_edge: u32,
) -> Result<(ColorImage, ColorImage), String> {
    let original = image::load_from_memory(bytes)
        .map_err(|error| error.to_string())?
        .to_rgba8();
    let original_width = original.width() as usize;
    let original_height = original.height() as usize;
    let (target_width, target_height) = quality_display_dimensions(
        original_width as u32,
        original_height as u32,
        target_long_edge,
        true,
    )?;
    let baseline = image::imageops::resize(
        &original,
        target_width,
        target_height,
        image_filter_type(ResizeFilter::Lanczos3),
    );
    let (source_width, source_height) = quality_display_dimensions(
        original_width as u32,
        original_height as u32,
        source_long_edge,
        false,
    )?;
    let input = image::imageops::resize(
        &original,
        source_width,
        source_height,
        image_filter_type(ResizeFilter::Lanczos3),
    );
    Ok((rgba_to_color_image(input), rgba_to_color_image(baseline)))
}

fn quality_display_dimensions(
    width: u32,
    height: u32,
    target_long_edge: u32,
    allow_upscale: bool,
) -> Result<(u32, u32), String> {
    if width == 0 || height == 0 {
        return Err("Image has zero-sized dimensions".to_owned());
    }
    let longest = width.max(height);
    if longest <= target_long_edge && !allow_upscale {
        return Ok((width, height));
    }
    let scale = target_long_edge as f64 / longest as f64;
    let scaled = |value: u32| ((value as f64 * scale).round() as u32).max(1);
    Ok((scaled(width), scaled(height)))
}

fn rgba_to_color_image(image: image::RgbaImage) -> ColorImage {
    let size = [image.width() as usize, image.height() as usize];
    ColorImage::from_rgba_unmultiplied(size, image.as_raw())
}

fn run_cpu_case(
    input: &ColorImage,
    baseline: &ColorImage,
    output_size: [usize; 2],
    label: &str,
    resize_filter: ResizeFilter,
    page: &mut PageUpscaleQuality,
    summaries: &mut BTreeMap<String, SummaryAccumulator>,
) -> ColorImage {
    let image = resize_color_image(input, output_size, resize_filter);
    push_run(label, baseline, &image, page, summaries, None);
    image
}

fn run_gpu_case(
    gpu: &GpuUpscaleBench,
    input: &ColorImage,
    baseline: &ColorImage,
    output_size: [usize; 2],
    method: WgpuUpscaleMethod,
    page: &mut PageUpscaleQuality,
    summaries: &mut BTreeMap<String, SummaryAccumulator>,
) -> Option<ColorImage> {
    match gpu.apply(input, output_size, method) {
        Ok(output) => {
            push_run(
                method.label(),
                baseline,
                &output.image,
                page,
                summaries,
                None,
            );
            Some(output.image)
        }
        Err(error) => {
            page.runs.push(UpscaleQualityRun {
                method: method.label().to_owned(),
                ssim: 0.0,
                mae: 0.0,
                rmse: 0.0,
                psnr: 0.0,
                max_abs_diff: 0,
                different_pixel_ratio: 0.0,
                edge_mae: 0.0,
                ringing_score: 0.0,
                error: Some(error),
            });
            None
        }
    }
}

fn push_run(
    label: &str,
    baseline: &ColorImage,
    image: &ColorImage,
    page: &mut PageUpscaleQuality,
    summaries: &mut BTreeMap<String, SummaryAccumulator>,
    error: Option<String>,
) {
    let result = compare_images(baseline, image);
    let (metrics, error) = match result {
        Ok(metrics) => (metrics, error),
        Err(compare_error) => (QualityMetrics::default(), Some(compare_error)),
    };
    let run = UpscaleQualityRun {
        method: label.to_owned(),
        ssim: metrics.ssim,
        mae: metrics.mae,
        rmse: metrics.rmse,
        psnr: metrics.psnr,
        max_abs_diff: metrics.max_abs_diff,
        different_pixel_ratio: metrics.different_pixel_ratio,
        edge_mae: metrics.edge_mae,
        ringing_score: metrics.ringing_score,
        error,
    };
    summaries.entry(label.to_owned()).or_default().push(&run);
    page.runs.push(run);
}

impl Default for QualityMetrics {
    fn default() -> Self {
        Self {
            ssim: 0.0,
            mae: 0.0,
            rmse: 0.0,
            psnr: 0.0,
            max_abs_diff: 0,
            different_pixel_ratio: 0.0,
            edge_mae: 0.0,
            ringing_score: 0.0,
        }
    }
}

fn resize_color_image(
    image: &ColorImage,
    output_size: [usize; 2],
    resize_filter: ResizeFilter,
) -> ColorImage {
    let bytes = color_image_to_rgba(image);
    let rgba = RgbaImage::from_raw(image.size[0] as u32, image.size[1] as u32, bytes)
        .expect("ColorImage RGBA bytes should match its dimensions");
    let resized = image::imageops::resize(
        &rgba,
        output_size[0] as u32,
        output_size[1] as u32,
        image_filter_type(resize_filter),
    );
    ColorImage::from_rgba_unmultiplied(output_size, &resized.into_raw())
}

fn image_filter_type(resize_filter: ResizeFilter) -> FilterType {
    match resize_filter {
        ResizeFilter::Bicubic => FilterType::CatmullRom,
        ResizeFilter::Lanczos3 => FilterType::Lanczos3,
        ResizeFilter::FastTriangle => FilterType::Triangle,
        ResizeFilter::Nearest => FilterType::Nearest,
    }
}

fn color_image_to_rgba(image: &ColorImage) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
    for pixel in &image.pixels {
        bytes.extend_from_slice(&[pixel.r(), pixel.g(), pixel.b(), pixel.a()]);
    }
    bytes
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
    let mut different_pixels = 0usize;
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_x2 = 0.0;
    let mut sum_y2 = 0.0;
    let mut sum_xy = 0.0;

    for (left, right) in baseline.pixels.iter().zip(candidate.pixels.iter()) {
        let left = left.to_array();
        let right = right.to_array();
        let mut pixel_differs = false;
        for channel in 0..3 {
            let diff = f64::from(left[channel].abs_diff(right[channel]));
            sum_abs += diff;
            sum_sq += diff * diff;
            max_abs_diff = max_abs_diff.max(diff as u8);
            pixel_differs |= diff != 0.0;
        }
        if pixel_differs {
            different_pixels += 1;
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
    let channel_count = pixel_count * 3.0;
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
        rmse,
        psnr,
        max_abs_diff,
        different_pixel_ratio: different_pixels as f64 / pixel_count,
        edge_mae: edge_mae(baseline, candidate),
        ringing_score: ringing_score(baseline, candidate),
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

fn ringing_score(baseline: &ColorImage, candidate: &ColorImage) -> f64 {
    let [width, height] = baseline.size;
    if width < 3 || height < 3 {
        return 0.0;
    }
    let step = edge_sample_step(width, height);
    let mut sum = 0.0;
    let mut samples: f64 = 0.0;
    for y in (1..height - 1).step_by(step) {
        for x in (1..width - 1).step_by(step) {
            let index = y * width + x;
            let candidate_center = luma(candidate.pixels[index].to_array());
            let local_min = local_luma_extreme(baseline, x, y, f64::min);
            let local_max = local_luma_extreme(baseline, x, y, f64::max);
            let undershoot = (local_min - candidate_center).max(0.0);
            let overshoot = (candidate_center - local_max).max(0.0);
            sum += undershoot + overshoot;
            samples += 1.0;
        }
    }
    sum / samples.max(1.0)
}

fn local_luma_extreme(image: &ColorImage, x: usize, y: usize, combine: fn(f64, f64) -> f64) -> f64 {
    let [width, _height] = image.size;
    let mut value = luma(image.pixels[y * width + x].to_array());
    for yy in y - 1..=y + 1 {
        for xx in x - 1..=x + 1 {
            value = combine(value, luma(image.pixels[yy * width + xx].to_array()));
        }
    }
    value
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

fn print_report(report: &UpscaleQualityReport) {
    println!("SuiSuiView upscale quality scan");
    println!("Path: {}", report.path);
    println!("Book: {}", report.title);
    println!(
        "Pages: {} scanned / {} total, {} failed",
        report.pages.len(),
        report.page_count,
        report.failures
    );
    println!("Source long edge: {}", report.source_long_edge);
    println!("Target long edge: {}", report.target_long_edge);
    println!(
        "WGSL: {}",
        if report.gpu_available {
            "available"
        } else {
            report.gpu_error.as_deref().unwrap_or("unavailable")
        }
    );
    for summary in &report.methods {
        println!(
            "{:<24} SSIM {:.5}, PSNR {:>6.2}, MAE {:>6.2}, edge {:>6.2}, ringing {:>6.2}, max {}",
            summary.method,
            summary.ssim,
            summary.psnr,
            summary.mae,
            summary.edge_mae,
            summary.ringing_score,
            summary.max_abs_diff,
        );
    }
}

fn selected_method_failure(
    report: &UpscaleQualityReport,
    method: WgpuUpscaleMethod,
) -> Option<String> {
    let label = method.label();
    if !report.gpu_available {
        return Some(format!(
            "{label} was requested but WGSL upscale quality scan is unavailable: {}",
            report.gpu_error.as_deref().unwrap_or("unknown error")
        ));
    }

    if report.failures > 0 {
        let first_error = report
            .pages
            .iter()
            .filter_map(|page| page.error.as_deref())
            .next()
            .unwrap_or("unknown page-level error");
        return Some(format!(
            "{label} quality scan had {} page-level failure(s); first error: {first_error}",
            report.failures
        ));
    }

    let mut first_error = None;
    let mut error_count = 0usize;
    for run in report
        .pages
        .iter()
        .flat_map(|page| page.runs.iter())
        .filter(|run| run.method == label)
    {
        if let Some(error) = &run.error {
            first_error.get_or_insert(error.as_str());
            error_count += 1;
        }
    }
    if error_count > 0 {
        return Some(format!(
            "{label} failed on {error_count} page(s); first error: {}",
            first_error.unwrap_or("unknown error")
        ));
    }

    let successful_pages = report
        .methods
        .iter()
        .find(|summary| summary.method == label)
        .map_or(0, |summary| summary.pages);
    if successful_pages == 0 {
        return Some(format!("{label} produced no successful quality runs"));
    }

    None
}

fn write_report(path: &Path, report: &UpscaleQualityReport) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string_pretty(report).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

#[derive(Default)]
struct SummaryAccumulator {
    pages: usize,
    ssim_sum: f64,
    mae_sum: f64,
    rmse_sum: f64,
    psnr_sum: f64,
    max_abs_diff: u8,
    different_pixel_ratio_sum: f64,
    edge_mae_sum: f64,
    ringing_score_sum: f64,
}

impl SummaryAccumulator {
    fn push(&mut self, run: &UpscaleQualityRun) {
        if run.error.is_some() {
            return;
        }
        self.pages += 1;
        self.ssim_sum += run.ssim;
        self.mae_sum += run.mae;
        self.rmse_sum += run.rmse;
        self.psnr_sum += run.psnr;
        self.max_abs_diff = self.max_abs_diff.max(run.max_abs_diff);
        self.different_pixel_ratio_sum += run.different_pixel_ratio;
        self.edge_mae_sum += run.edge_mae;
        self.ringing_score_sum += run.ringing_score;
    }

    fn finish(self, method: String) -> UpscaleQualitySummary {
        UpscaleQualitySummary {
            method,
            pages: self.pages,
            ssim: average(self.ssim_sum, self.pages),
            mae: average(self.mae_sum, self.pages),
            rmse: average(self.rmse_sum, self.pages),
            psnr: average(self.psnr_sum, self.pages),
            max_abs_diff: self.max_abs_diff,
            different_pixel_ratio: average(self.different_pixel_ratio_sum, self.pages),
            edge_mae: average(self.edge_mae_sum, self.pages),
            ringing_score: average(self.ringing_score_sum, self.pages),
        }
    }
}

fn average(total: f64, count: usize) -> f64 {
    if count == 0 {
        0.0
    } else {
        total / count as f64
    }
}

fn scanned_page_count(page_count: usize, max_pages: Option<usize>) -> usize {
    max_pages
        .filter(|pages| *pages > 0)
        .map_or(page_count, |limit| page_count.min(limit))
}

pub fn default_upscale_quality_report_path() -> PathBuf {
    PathBuf::from("perf-fixtures").join("upscale-quality-report.json")
}

fn clamp_upscale_source_long_edge(source_long_edge: u32) -> u32 {
    source_long_edge.clamp(64, 4096)
}

#[cfg(test)]
mod tests {
    use super::{
        compare_images, resize_color_image, scanned_page_count, selected_method_failure,
        PageUpscaleQuality, UpscaleQualityReport,
    };
    use crate::core::state::{ResizeFilter, WgpuUpscaleMethod};
    use eframe::egui::{Color32, ColorImage};

    #[test]
    fn identical_images_have_perfect_quality() {
        let image = ColorImage::new([2, 2], vec![Color32::WHITE; 4]);
        let metrics = compare_images(&image, &image).unwrap();
        assert_eq!(metrics.max_abs_diff, 0);
        assert_eq!(metrics.mae, 0.0);
        assert!(metrics.psnr.is_infinite());
        assert!((metrics.ssim - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn resize_quality_helper_keeps_requested_size() {
        let image = ColorImage::new([2, 2], vec![Color32::WHITE; 4]);
        let resized = resize_color_image(&image, [4, 4], ResizeFilter::Bicubic);
        assert_eq!(resized.size, [4, 4]);
    }

    #[test]
    fn selected_quality_method_requires_success() {
        let report = UpscaleQualityReport {
            path: "book.cbz".to_owned(),
            title: "book".to_owned(),
            page_count: 1,
            source_long_edge: 1024,
            target_long_edge: 2048,
            gpu_available: true,
            gpu_error: None,
            failures: 0,
            methods: Vec::new(),
            pages: Vec::new(),
        };

        let error = selected_method_failure(&report, WgpuUpscaleMethod::WgslArtcnnC4F16).unwrap();

        assert!(error.contains("ArtCNN C4F16 produced no successful quality runs"));
    }

    #[test]
    fn selected_quality_method_fails_partial_page_errors() {
        let report = UpscaleQualityReport {
            path: "book.cbz".to_owned(),
            title: "book".to_owned(),
            page_count: 2,
            source_long_edge: 1024,
            target_long_edge: 2048,
            gpu_available: true,
            gpu_error: None,
            failures: 1,
            methods: Vec::new(),
            pages: vec![PageUpscaleQuality {
                index: 0,
                name: "page-0000.png".to_owned(),
                source_width: None,
                source_height: None,
                output_width: None,
                output_height: None,
                contact_sheet: None,
                visuals: Vec::new(),
                runs: Vec::new(),
                error: Some("decode failed".to_owned()),
            }],
        };

        let error = selected_method_failure(&report, WgpuUpscaleMethod::WgslArtcnnC4F16).unwrap();

        assert!(error.contains("page-level failure"));
        assert!(error.contains("decode failed"));
    }

    #[test]
    fn zero_quality_max_pages_means_unlimited() {
        assert_eq!(scanned_page_count(4, None), 4);
        assert_eq!(scanned_page_count(4, Some(0)), 4);
        assert_eq!(scanned_page_count(4, Some(2)), 2);
        assert_eq!(scanned_page_count(4, Some(8)), 4);
    }
}
