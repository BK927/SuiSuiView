use crate::core::artcnn::{exact_output_size as artcnn_exact_output_size, ArtcnnVariant};
use crate::core::gpu_effect::{color_image_to_rgba, image_diff};
use crate::core::source::open_source_from_path;
use crate::core::state::{CpuScaleFilter, ResizeFilter, WgpuUpscaleMethod};
use crate::core::worker::{
    clamp_target_long_edge, display_dimensions_with_upscale, prepare_image_with_options,
    DecodeOptions, DecodeStrategy,
};
use egui::ColorImage;
use image::{imageops::FilterType, RgbaImage};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(crate) mod gpu;
use gpu::GpuUpscaleBench;

#[derive(Debug, Serialize)]
pub struct UpscaleBenchReport {
    pub path: String,
    pub title: String,
    pub page_count: usize,
    pub source_long_edge: u32,
    pub target_long_edge: u32,
    pub gpu_available: bool,
    pub gpu_error: Option<String>,
    pub failures: usize,
    pub methods: Vec<UpscaleBenchSummary>,
    pub pages: Vec<PageUpscaleBench>,
}

#[derive(Debug, Serialize)]
pub struct UpscaleBenchSummary {
    pub method: String,
    pub pages: usize,
    pub total_ms: f64,
    pub avg_ms: f64,
    pub max_channel_diff: u8,
    pub mean_abs_diff: f64,
    pub different_pixel_ratio: f64,
}

#[derive(Debug, Serialize)]
pub struct PageUpscaleBench {
    pub index: usize,
    pub name: String,
    pub source_width: Option<usize>,
    pub source_height: Option<usize>,
    pub output_width: Option<usize>,
    pub output_height: Option<usize>,
    pub runs: Vec<UpscaleBenchRun>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UpscaleBenchRun {
    pub method: String,
    pub ms: f64,
    pub max_channel_diff: u8,
    pub mean_abs_diff: f64,
    pub different_pixel_ratio: f64,
    pub error: Option<String>,
}

pub fn run_upscale_bench(
    path: &Path,
    report_path: Option<&Path>,
    source_long_edge: u32,
    target_long_edge: u32,
    method_filter: Option<WgpuUpscaleMethod>,
    max_pages: Option<usize>,
) -> Result<(), String> {
    let report = scan_upscalers(
        path,
        clamp_source_long_edge(source_long_edge),
        clamp_target_long_edge(target_long_edge),
        method_filter,
        max_pages,
    )?;
    let selected_method_error =
        method_filter.and_then(|method| selected_method_failure(&report, method));
    print_report(&report);
    if let Some(report_path) = report_path {
        write_report(report_path, &report)?;
        println!("Report: {}", report_path.display());
    }
    if let Some(error) = selected_method_error {
        return Err(error);
    }
    Ok(())
}

pub fn scan_upscalers(
    path: &Path,
    source_long_edge: u32,
    target_long_edge: u32,
    method_filter: Option<WgpuUpscaleMethod>,
    max_pages: Option<usize>,
) -> Result<UpscaleBenchReport, String> {
    let gpu = match GpuUpscaleBench::new_for_method(method_filter) {
        Ok(gpu) => Some(gpu),
        Err(error) => {
            eprintln!("WGSL upscale bench disabled: {error}");
            None
        }
    };
    let gpu_error = gpu
        .is_none()
        .then(|| "WGSL upscale backend unavailable".to_owned());
    scan_upscalers_with_gpu(
        path,
        source_long_edge,
        target_long_edge,
        method_filter,
        max_pages,
        gpu.as_ref(),
        gpu_error,
    )
}

pub fn run_artcnn_render(
    variant: ArtcnnVariant,
    method: WgpuUpscaleMethod,
    input_path: &Path,
    output_path: &Path,
) -> Result<(), String> {
    let input = load_color_image(input_path)?;
    let output_size = artcnn_exact_output_size(variant, input.size)?;
    let gpu = GpuUpscaleBench::new_for_method(Some(method))?;
    let output = gpu.apply(&input, output_size, method)?;
    write_color_image(output_path, &output.image)?;
    println!(
        "{}: {}x{} -> {}x{} in {:.2} ms",
        variant.label(),
        input.size[0],
        input.size[1],
        output_size[0],
        output_size[1],
        millis(output.elapsed)
    );
    println!("Output: {}", output_path.display());
    Ok(())
}

pub fn run_upscale_render(
    method: WgpuUpscaleMethod,
    input_path: &Path,
    output_path: &Path,
    output_size: [usize; 2],
) -> Result<(), String> {
    let input = load_color_image(input_path)?;
    let gpu = GpuUpscaleBench::new_for_method(Some(method))?;
    let output = gpu.apply(&input, output_size, method)?;
    write_color_image(output_path, &output.image)?;
    println!(
        "{}: {}x{} -> {}x{} in {:.2} ms",
        method.label(),
        input.size[0],
        input.size[1],
        output_size[0],
        output_size[1],
        millis(output.elapsed)
    );
    println!("Output: {}", output_path.display());
    Ok(())
}

pub(crate) fn gpu_methods_for_filter(
    method_filter: Option<WgpuUpscaleMethod>,
) -> Vec<WgpuUpscaleMethod> {
    match method_filter {
        Some(method) => vec![method],
        None => WgpuUpscaleMethod::GPU_METHODS
            .iter()
            .copied()
            .filter(|method| {
                !method.is_artcnn()
                    && !method.is_cunny()
                    && !matches!(method, WgpuUpscaleMethod::WgslSrLabSpanX2)
            })
            .collect(),
    }
}

fn scan_upscalers_with_gpu(
    path: &Path,
    source_long_edge: u32,
    target_long_edge: u32,
    method_filter: Option<WgpuUpscaleMethod>,
    max_pages: Option<usize>,
    gpu: Option<&GpuUpscaleBench>,
    gpu_error: Option<String>,
) -> Result<UpscaleBenchReport, String> {
    let (source, _forced_page) = open_source_from_path(path).map_err(|error| error.to_string())?;
    let mut failures = 0usize;
    let page_count = source.page_count();
    let scanned_page_count = max_pages
        .filter(|pages| *pages > 0)
        .map_or(page_count, |pages| page_count.min(pages));
    let mut pages = Vec::with_capacity(scanned_page_count);
    let mut summaries = BTreeMap::<String, SummaryAccumulator>::new();
    let gpu_methods = gpu_methods_for_filter(method_filter);

    for index in 0..scanned_page_count {
        let mut page = PageUpscaleBench {
            index,
            name: source.page_name(index).unwrap_or("").to_owned(),
            source_width: None,
            source_height: None,
            output_width: None,
            output_height: None,
            runs: Vec::new(),
            error: None,
        };

        let result = source
            .read_page(index)
            .map_err(|error| error.to_string())
            .and_then(|bytes| prepare_page_pair(&bytes, source_long_edge, target_long_edge));

        match result {
            Ok((input, baseline)) => {
                let input_image = input.color_image();
                let baseline_image = baseline.color_image();
                let output_size = baseline_image.size;
                page.source_width = Some(input_image.size[0]);
                page.source_height = Some(input_image.size[1]);
                page.output_width = Some(output_size[0]);
                page.output_height = Some(output_size[1]);

                run_cpu_case(
                    &input_image,
                    &baseline_image,
                    output_size,
                    "cpu_bicubic",
                    ResizeFilter::Bicubic,
                    &mut page,
                    &mut summaries,
                );
                run_cpu_case(
                    &input_image,
                    &baseline_image,
                    output_size,
                    "cpu_lanczos3",
                    ResizeFilter::Lanczos3,
                    &mut page,
                    &mut summaries,
                );
                run_cpu_case(
                    &input_image,
                    &baseline_image,
                    output_size,
                    "cpu_triangle",
                    ResizeFilter::FastTriangle,
                    &mut page,
                    &mut summaries,
                );

                if let Some(gpu) = &gpu {
                    for method in &gpu_methods {
                        run_gpu_case(
                            gpu,
                            &input_image,
                            &baseline_image,
                            output_size,
                            *method,
                            &mut page,
                            &mut summaries,
                        );
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

    Ok(UpscaleBenchReport {
        path: path.display().to_string(),
        title: source.title().to_owned(),
        page_count,
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
) -> Result<
    (
        crate::core::worker::PreparedPage,
        crate::core::worker::PreparedPage,
    ),
    String,
> {
    let source = prepare_image_with_options(
        bytes,
        source_long_edge,
        DecodeOptions {
            strategy: DecodeStrategy::Auto,
            cpu_downscale_filter: CpuScaleFilter::Lanczos3,
            allow_display_upscale: false,
            ..DecodeOptions::default()
        },
    )?;
    let (target_width, target_height) = display_dimensions_with_upscale(
        source.original_width as u32,
        source.original_height as u32,
        target_long_edge,
        true,
    )?;
    let baseline = prepare_image_with_options(
        bytes,
        target_width.max(target_height),
        DecodeOptions {
            strategy: DecodeStrategy::ImageCrate,
            cpu_upscale_filter: CpuScaleFilter::Lanczos3,
            cpu_downscale_filter: CpuScaleFilter::Lanczos3,
            allow_display_upscale: true,
            ..DecodeOptions::default()
        },
    )?;
    Ok((source, baseline))
}

fn load_color_image(path: &Path) -> Result<ColorImage, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let rgba = image::load_from_memory(&bytes)
        .map_err(|error| error.to_string())?
        .to_rgba8();
    let (width, height) = rgba.dimensions();
    let size = [
        usize::try_from(width).map_err(|_| "image width exceeds usize".to_owned())?,
        usize::try_from(height).map_err(|_| "image height exceeds usize".to_owned())?,
    ];
    Ok(ColorImage::from_rgba_unmultiplied(size, &rgba.into_raw()))
}

fn write_color_image(path: &Path, image: &ColorImage) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = color_image_to_rgba(image);
    let rgba = RgbaImage::from_raw(image.size[0] as u32, image.size[1] as u32, bytes)
        .ok_or_else(|| "ColorImage RGBA bytes did not match its dimensions".to_owned())?;
    rgba.save(path).map_err(|error| error.to_string())
}

fn run_cpu_case(
    input: &ColorImage,
    baseline: &ColorImage,
    output_size: [usize; 2],
    label: &str,
    resize_filter: ResizeFilter,
    page: &mut PageUpscaleBench,
    summaries: &mut BTreeMap<String, SummaryAccumulator>,
) {
    let started = Instant::now();
    let image = resize_color_image(input, output_size, resize_filter);
    let elapsed = started.elapsed();
    push_run(label, elapsed, baseline, image, page, summaries, None);
}

fn run_gpu_case(
    gpu: &GpuUpscaleBench,
    input: &ColorImage,
    baseline: &ColorImage,
    output_size: [usize; 2],
    method: WgpuUpscaleMethod,
    page: &mut PageUpscaleBench,
    summaries: &mut BTreeMap<String, SummaryAccumulator>,
) {
    match gpu.apply(input, output_size, method) {
        Ok(output) => push_run(
            method.label(),
            output.elapsed,
            baseline,
            output.image,
            page,
            summaries,
            None,
        ),
        Err(error) => {
            let run = UpscaleBenchRun {
                method: method.label().to_owned(),
                ms: 0.0,
                max_channel_diff: 0,
                mean_abs_diff: 0.0,
                different_pixel_ratio: 0.0,
                error: Some(error),
            };
            page.runs.push(run);
        }
    }
}

fn push_run(
    label: &str,
    elapsed: Duration,
    baseline: &ColorImage,
    image: ColorImage,
    page: &mut PageUpscaleBench,
    summaries: &mut BTreeMap<String, SummaryAccumulator>,
    error: Option<String>,
) {
    let diff = image_diff(baseline, &image);
    let run = UpscaleBenchRun {
        method: label.to_owned(),
        ms: millis(elapsed),
        max_channel_diff: diff.max_channel_diff,
        mean_abs_diff: diff.mean_abs_diff,
        different_pixel_ratio: diff.different_pixel_ratio,
        error,
    };
    summaries.entry(label.to_owned()).or_default().push(&run);
    page.runs.push(run);
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

#[derive(Default)]
struct SummaryAccumulator {
    pages: usize,
    total_ms: f64,
    max_channel_diff: u8,
    mean_abs_diff_total: f64,
    different_pixel_ratio_total: f64,
}

impl SummaryAccumulator {
    fn push(&mut self, run: &UpscaleBenchRun) {
        if run.error.is_some() {
            return;
        }
        self.pages += 1;
        self.total_ms += run.ms;
        self.max_channel_diff = self.max_channel_diff.max(run.max_channel_diff);
        self.mean_abs_diff_total += run.mean_abs_diff;
        self.different_pixel_ratio_total += run.different_pixel_ratio;
    }

    fn finish(self, method: String) -> UpscaleBenchSummary {
        UpscaleBenchSummary {
            method,
            pages: self.pages,
            total_ms: self.total_ms,
            avg_ms: average(self.total_ms, self.pages),
            max_channel_diff: self.max_channel_diff,
            mean_abs_diff: average(self.mean_abs_diff_total, self.pages),
            different_pixel_ratio: average(self.different_pixel_ratio_total, self.pages),
        }
    }
}

fn print_report(report: &UpscaleBenchReport) {
    println!("SuiSuiView upscale bench");
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
            "{:<16} {:>7.2} ms/page, max diff {}, mean diff {:.4}, diff px {:.4}%",
            summary.method,
            summary.avg_ms,
            summary.max_channel_diff,
            summary.mean_abs_diff,
            summary.different_pixel_ratio * 100.0
        );
    }
}

fn selected_method_failure(
    report: &UpscaleBenchReport,
    method: WgpuUpscaleMethod,
) -> Option<String> {
    let label = method.label();
    if !report.gpu_available {
        return Some(format!(
            "{label} was requested but WGSL upscale is unavailable: {}",
            report.gpu_error.as_deref().unwrap_or("unknown error")
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
        return Some(format!("{label} produced no successful GPU runs"));
    }

    None
}

fn write_report(path: &Path, report: &UpscaleBenchReport) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string_pretty(report).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

fn clamp_source_long_edge(source_long_edge: u32) -> u32 {
    clamp_target_long_edge(source_long_edge)
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn average(total: f64, count: usize) -> f64 {
    if count == 0 {
        0.0
    } else {
        total / count as f64
    }
}

pub fn default_upscale_report_path() -> PathBuf {
    PathBuf::from("perf-fixtures").join("upscale-report.json")
}

#[cfg(test)]
mod tests {
    use super::{
        gpu_methods_for_filter, resize_color_image, scan_upscalers_with_gpu,
        selected_method_failure, PageUpscaleBench, UpscaleBenchReport, UpscaleBenchRun,
    };
    use crate::core::state::{ResizeFilter, WgpuUpscaleMethod};
    use egui::{Color32, ColorImage};
    use image::{ImageBuffer, ImageFormat, Rgba};
    use std::fs;
    use std::io::Cursor;

    #[test]
    fn resize_color_image_returns_requested_size() {
        let image = ColorImage::new(
            [2, 1],
            vec![Color32::from_rgb(0, 0, 0), Color32::from_rgb(255, 255, 255)],
        );

        let resized = resize_color_image(&image, [4, 2], ResizeFilter::Nearest);

        assert_eq!(resized.size, [4, 2]);
        assert_eq!(resized.pixels.len(), 8);
    }

    #[test]
    fn upscale_bench_scans_folder_input() {
        let dir = std::env::temp_dir().join(format!(
            "suisuiview-upscale-bench-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let image = ImageBuffer::<Rgba<u8>, _>::from_fn(32, 20, |x, y| {
            Rgba([(x * 5) as u8, (y * 9) as u8, 128, 255])
        });
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        fs::write(dir.join("001.png"), bytes.into_inner()).unwrap();

        let report = scan_upscalers_with_gpu(
            &dir,
            1024,
            2048,
            None,
            None,
            None,
            Some("disabled in unit test".to_owned()),
        )
        .unwrap();

        assert_eq!(report.page_count, 1);
        assert!(!report.gpu_available);
        assert_eq!(report.failures, 0);
        assert!(report
            .methods
            .iter()
            .any(|method| method.method == "cpu_lanczos3"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_gpu_bench_methods_skip_heavy_explicit_matrices() {
        let default_methods = gpu_methods_for_filter(None);

        assert!(!default_methods.contains(&WgpuUpscaleMethod::WgslArtcnnC4F16));
        assert!(!default_methods.contains(&WgpuUpscaleMethod::WgslArtcnnC4F32Ds));
        assert!(!default_methods.contains(&WgpuUpscaleMethod::CunnyVeryfastSoft));
        assert!(!default_methods.contains(&WgpuUpscaleMethod::Cunny8x32Ds));
        assert_eq!(
            gpu_methods_for_filter(Some(WgpuUpscaleMethod::WgslArtcnnC4F16)),
            vec![WgpuUpscaleMethod::WgslArtcnnC4F16]
        );
        assert_eq!(
            gpu_methods_for_filter(Some(WgpuUpscaleMethod::CunnyVeryfastSoft)),
            vec![WgpuUpscaleMethod::CunnyVeryfastSoft]
        );
    }

    #[test]
    fn filtered_upscale_bench_reports_selected_method_error() {
        let method = WgpuUpscaleMethod::WgslArtcnnC4F16;
        let report = UpscaleBenchReport {
            path: "book.cbz".to_owned(),
            title: "book".to_owned(),
            page_count: 1,
            source_long_edge: 1024,
            target_long_edge: 2048,
            gpu_available: true,
            gpu_error: None,
            failures: 0,
            methods: Vec::new(),
            pages: vec![PageUpscaleBench {
                index: 0,
                name: "001.png".to_owned(),
                source_width: Some(1024),
                source_height: Some(1024),
                output_width: Some(2048),
                output_height: Some(2048),
                runs: vec![UpscaleBenchRun {
                    method: method.label().to_owned(),
                    ms: 0.0,
                    max_channel_diff: 0,
                    mean_abs_diff: 0.0,
                    different_pixel_ratio: 0.0,
                    error: Some("boom".to_owned()),
                }],
                error: None,
            }],
        };

        let error = selected_method_failure(&report, method).unwrap();
        assert!(error.contains("ArtCNN C4F16 failed"), "{error}");
        assert!(error.contains("boom"), "{error}");
    }
}
