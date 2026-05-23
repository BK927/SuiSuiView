use crate::core::effects::{apply_effects_to_image, ImageFilter, ViewEffects, ViewTransform};
use crate::core::gpu_effect::{image_diff, GpuEffectBench};
use crate::core::source::open_source_from_path;
use crate::core::worker::{clamp_target_long_edge, prepare_image_with_strategy, DecodeStrategy};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Serialize)]
pub struct EffectBenchReport {
    pub path: String,
    pub title: String,
    pub page_count: usize,
    pub target_long_edge: u32,
    pub gpu_available: bool,
    pub gpu_error: Option<String>,
    pub total_prepare_ms: f64,
    pub failures: usize,
    pub effects: Vec<EffectBenchSummary>,
    pub pages: Vec<PageEffectBench>,
}

#[derive(Debug, Serialize)]
pub struct EffectBenchSummary {
    pub effect: String,
    pub pages: usize,
    pub cpu_total_ms: f64,
    pub cpu_avg_ms: f64,
    pub gpu_total_ms: Option<f64>,
    pub gpu_avg_ms: Option<f64>,
    pub speedup: Option<f64>,
    pub max_channel_diff: Option<u8>,
    pub mean_abs_diff: Option<f64>,
    pub different_pixel_ratio: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct PageEffectBench {
    pub index: usize,
    pub name: String,
    pub display_width: Option<usize>,
    pub display_height: Option<usize>,
    pub prepare_ms: f64,
    pub effect_runs: Vec<EffectBenchRun>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EffectBenchRun {
    pub effect: String,
    pub cpu_ms: f64,
    pub gpu_ms: Option<f64>,
    pub max_channel_diff: Option<u8>,
    pub mean_abs_diff: Option<f64>,
    pub different_pixel_ratio: Option<f64>,
    pub gpu_error: Option<String>,
}

struct EffectCase {
    name: &'static str,
    effects: ViewEffects,
}

pub fn run_effect_bench(
    path: &Path,
    report_path: Option<&Path>,
    target_long_edge: u32,
) -> Result<(), String> {
    let report = scan_effects(path, clamp_target_long_edge(target_long_edge))?;
    print_report(&report);
    if let Some(report_path) = report_path {
        write_report(report_path, &report)?;
        println!("Report: {}", report_path.display());
    }
    Ok(())
}

pub fn scan_effects(path: &Path, target_long_edge: u32) -> Result<EffectBenchReport, String> {
    let (source, _forced_page) = open_source_from_path(path).map_err(|error| error.to_string())?;
    let gpu = match GpuEffectBench::new() {
        Ok(gpu) => Some(gpu),
        Err(error) => {
            eprintln!("WGSL effect bench disabled: {error}");
            None
        }
    };
    let gpu_error = gpu
        .is_none()
        .then(|| "WGSL bench backend unavailable".to_owned());
    let cases = effect_cases();
    let mut total_prepare = Duration::ZERO;
    let mut failures = 0usize;
    let mut pages = Vec::with_capacity(source.page_count());
    let mut summaries = cases
        .iter()
        .map(|case| (case.name.to_owned(), SummaryAccumulator::default()))
        .collect::<BTreeMap<_, _>>();

    for index in 0..source.page_count() {
        let mut page = PageEffectBench {
            index,
            name: source.page_name(index).unwrap_or("").to_owned(),
            display_width: None,
            display_height: None,
            prepare_ms: 0.0,
            effect_runs: Vec::new(),
            error: None,
        };
        match source
            .read_page(index)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                let started = Instant::now();
                let prepared =
                    prepare_image_with_strategy(&bytes, target_long_edge, DecodeStrategy::Auto)?;
                Ok((prepared, started.elapsed()))
            }) {
            Ok((prepared, prepare_elapsed)) => {
                total_prepare += prepare_elapsed;
                page.prepare_ms = millis(prepare_elapsed);
                page.display_width = Some(prepared.display_width);
                page.display_height = Some(prepared.display_height);
                for case in &cases {
                    let cpu_started = Instant::now();
                    let cpu_image = apply_effects_to_image(&prepared.image, case.effects);
                    let cpu_elapsed = cpu_started.elapsed();

                    let (gpu_ms, diff, gpu_error) = if let Some(gpu) = &gpu {
                        match gpu.apply(&prepared.image, case.effects) {
                            Ok(output) => (
                                Some(millis(output.elapsed)),
                                Some(image_diff(&cpu_image, &output.image)),
                                None,
                            ),
                            Err(error) => (None, None, Some(error)),
                        }
                    } else {
                        (
                            None,
                            None,
                            Some("WGSL bench backend unavailable".to_owned()),
                        )
                    };
                    let run = EffectBenchRun {
                        effect: case.name.to_owned(),
                        cpu_ms: millis(cpu_elapsed),
                        gpu_ms,
                        max_channel_diff: diff.as_ref().map(|diff| diff.max_channel_diff),
                        mean_abs_diff: diff.as_ref().map(|diff| diff.mean_abs_diff),
                        different_pixel_ratio: diff.as_ref().map(|diff| diff.different_pixel_ratio),
                        gpu_error,
                    };
                    summaries
                        .get_mut(case.name)
                        .expect("summary accumulator should exist for every case")
                        .push(&run);
                    page.effect_runs.push(run);
                }
            }
            Err(error) => {
                failures += 1;
                page.error = Some(error);
            }
        }
        pages.push(page);
    }

    let effects = summaries
        .into_iter()
        .map(|(effect, summary)| summary.finish(effect))
        .collect();

    Ok(EffectBenchReport {
        path: path.display().to_string(),
        title: source.title().to_owned(),
        page_count: source.page_count(),
        target_long_edge,
        gpu_available: gpu.is_some(),
        gpu_error,
        total_prepare_ms: millis(total_prepare),
        failures,
        effects,
        pages,
    })
}

fn effect_cases() -> Vec<EffectCase> {
    vec![
        EffectCase {
            name: "base",
            effects: ViewEffects::default(),
        },
        EffectCase {
            name: "rotate_flip",
            effects: ViewEffects {
                transform: ViewTransform {
                    rotation_quadrants: 1,
                    flip_horizontal: true,
                    flip_vertical: true,
                },
                ..ViewEffects::default()
            },
        },
        EffectCase {
            name: "gamma_invert",
            effects: ViewEffects {
                gamma: true,
                invert_colors: true,
                ..ViewEffects::default()
            },
        },
        EffectCase {
            name: "smooth",
            effects: ViewEffects {
                filter: ImageFilter::Smooth,
                ..ViewEffects::default()
            },
        },
        EffectCase {
            name: "smooth_sharpen",
            effects: ViewEffects {
                filter: ImageFilter::SmoothSharpen,
                ..ViewEffects::default()
            },
        },
        EffectCase {
            name: "rcas_sharpen",
            effects: ViewEffects {
                filter: ImageFilter::RcasSharpen,
                ..ViewEffects::default()
            },
        },
        EffectCase {
            name: "combined",
            effects: ViewEffects {
                transform: ViewTransform {
                    rotation_quadrants: 1,
                    flip_horizontal: true,
                    flip_vertical: true,
                },
                filter: ImageFilter::SmoothSharpen,
                gamma: true,
                invert_colors: true,
            },
        },
    ]
}

#[derive(Default)]
struct SummaryAccumulator {
    pages: usize,
    cpu_total_ms: f64,
    gpu_total_ms: f64,
    gpu_pages: usize,
    max_channel_diff: u8,
    mean_abs_diff_total: f64,
    different_pixel_ratio_total: f64,
}

impl SummaryAccumulator {
    fn push(&mut self, run: &EffectBenchRun) {
        self.pages += 1;
        self.cpu_total_ms += run.cpu_ms;
        if let Some(gpu_ms) = run.gpu_ms {
            self.gpu_total_ms += gpu_ms;
            self.gpu_pages += 1;
        }
        if let Some(max_diff) = run.max_channel_diff {
            self.max_channel_diff = self.max_channel_diff.max(max_diff);
        }
        if let Some(mean_abs_diff) = run.mean_abs_diff {
            self.mean_abs_diff_total += mean_abs_diff;
        }
        if let Some(ratio) = run.different_pixel_ratio {
            self.different_pixel_ratio_total += ratio;
        }
    }

    fn finish(self, effect: String) -> EffectBenchSummary {
        let gpu_total_ms = (self.gpu_pages > 0).then_some(self.gpu_total_ms);
        EffectBenchSummary {
            effect,
            pages: self.pages,
            cpu_total_ms: self.cpu_total_ms,
            cpu_avg_ms: average(self.cpu_total_ms, self.pages),
            gpu_total_ms,
            gpu_avg_ms: gpu_total_ms.map(|total| average(total, self.gpu_pages)),
            speedup: gpu_total_ms.and_then(|gpu| (gpu > 0.0).then_some(self.cpu_total_ms / gpu)),
            max_channel_diff: (self.gpu_pages > 0).then_some(self.max_channel_diff),
            mean_abs_diff: (self.gpu_pages > 0)
                .then_some(self.mean_abs_diff_total / self.gpu_pages as f64),
            different_pixel_ratio: (self.gpu_pages > 0)
                .then_some(self.different_pixel_ratio_total / self.gpu_pages as f64),
        }
    }
}

fn print_report(report: &EffectBenchReport) {
    println!("SuiSuiView effect bench");
    println!("Path: {}", report.path);
    println!("Book: {}", report.title);
    println!(
        "Pages: {} ok / {} failed",
        report.page_count.saturating_sub(report.failures),
        report.failures
    );
    println!("Target long edge: {}", report.target_long_edge);
    println!("Prepare total: {:.2} ms", report.total_prepare_ms);
    println!(
        "WGSL: {}",
        if report.gpu_available {
            "available"
        } else {
            report.gpu_error.as_deref().unwrap_or("unavailable")
        }
    );
    for summary in &report.effects {
        if let Some(gpu_avg) = summary.gpu_avg_ms {
            println!(
                "{:<15} CPU {:>7.2} ms/page, WGSL {:>7.2} ms/page, speedup {:>5.2}x, max diff {}, mean diff {:.4}, diff px {:.4}%",
                summary.effect,
                summary.cpu_avg_ms,
                gpu_avg,
                summary.speedup.unwrap_or(0.0),
                summary.max_channel_diff.unwrap_or(0),
                summary.mean_abs_diff.unwrap_or(0.0),
                summary.different_pixel_ratio.unwrap_or(0.0) * 100.0
            );
        } else {
            println!(
                "{:<15} CPU {:>7.2} ms/page, WGSL unavailable",
                summary.effect, summary.cpu_avg_ms
            );
        }
    }
}

fn write_report(path: &Path, report: &EffectBenchReport) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string_pretty(report).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
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

pub fn default_effect_report_path() -> PathBuf {
    PathBuf::from("perf-fixtures").join("effect-report.json")
}

#[cfg(test)]
mod tests {
    use super::scan_effects;
    use image::{ImageBuffer, ImageFormat, Rgba};
    use std::fs;
    use std::io::Cursor;

    #[test]
    fn effect_bench_scans_folder_input() {
        let dir = std::env::temp_dir().join(format!(
            "suisuiview-effect-bench-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let image = ImageBuffer::<Rgba<u8>, _>::from_fn(3, 2, |x, y| {
            Rgba([(x * 40) as u8, (y * 80) as u8, 128, 255])
        });
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        fs::write(dir.join("001.png"), bytes.into_inner()).unwrap();

        let report = scan_effects(&dir, 1024).unwrap();

        assert_eq!(report.page_count, 1);
        assert_eq!(report.failures, 0);
        assert_eq!(report.effects.len(), 7);
        let _ = fs::remove_dir_all(&dir);
    }
}
