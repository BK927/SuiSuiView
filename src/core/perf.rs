use crate::core::source::open_source_from_path;
use crate::core::worker::{clamp_target_long_edge, prepare_image_with_strategy, DecodeStrategy};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Serialize)]
pub struct PerfReport {
    pub path: String,
    pub title: String,
    pub book_id: String,
    pub page_count: usize,
    pub target_long_edge: u32,
    pub decode_strategy: String,
    pub open_ms: f64,
    pub total_read_ms: f64,
    pub total_prepare_ms: f64,
    pub prepare_p50_ms: f64,
    pub prepare_p95_ms: f64,
    pub max_display_bytes: usize,
    pub total_source_bytes: u64,
    pub total_display_bytes: u64,
    pub failures: usize,
    pub backend_counts: BTreeMap<String, usize>,
    pub pages: Vec<PagePerf>,
}

#[derive(Debug, Serialize)]
pub struct PagePerf {
    pub index: usize,
    pub name: String,
    pub source_bytes: u64,
    pub original_width: Option<usize>,
    pub original_height: Option<usize>,
    pub display_width: Option<usize>,
    pub display_height: Option<usize>,
    pub display_bytes: Option<usize>,
    pub decode_backend: Option<String>,
    pub read_ms: f64,
    pub prepare_ms: f64,
    pub error: Option<String>,
}

pub fn run_perf_scan(
    path: &Path,
    report_path: Option<&Path>,
    target_long_edge: u32,
    decode_strategy: DecodeStrategy,
) -> Result<(), String> {
    let report = scan_path(
        path,
        clamp_target_long_edge(target_long_edge),
        decode_strategy,
    )?;
    print_report(&report);

    if let Some(report_path) = report_path {
        write_report(report_path, &report)?;
        println!("Report: {}", report_path.display());
    }

    Ok(())
}

pub fn scan_path(
    path: &Path,
    target_long_edge: u32,
    decode_strategy: DecodeStrategy,
) -> Result<PerfReport, String> {
    let open_started = Instant::now();
    let (source, forced_page) = open_source_from_path(path).map_err(|error| error.to_string())?;
    let open_elapsed = open_started.elapsed();
    let mut pages = Vec::with_capacity(source.page_count());
    let mut total_read = Duration::ZERO;
    let mut total_prepare = Duration::ZERO;
    let mut total_source_bytes = 0u64;
    let mut total_display_bytes = 0u64;
    let mut failures = 0usize;
    let mut backend_counts = BTreeMap::new();
    let mut successful_prepare_ms = Vec::new();
    let mut max_display_bytes = 0usize;

    for index in 0..source.page_count() {
        let read_started = Instant::now();
        let bytes_result = source.read_page(index).map_err(|error| error.to_string());
        let read_elapsed = read_started.elapsed();
        total_read += read_elapsed;

        let mut page = PagePerf {
            index,
            name: source.page_name(index).unwrap_or("").to_owned(),
            source_bytes: 0,
            original_width: None,
            original_height: None,
            display_width: None,
            display_height: None,
            display_bytes: None,
            decode_backend: None,
            read_ms: millis(read_elapsed),
            prepare_ms: 0.0,
            error: None,
        };

        match bytes_result {
            Ok(bytes) => {
                page.source_bytes = bytes.len() as u64;
                total_source_bytes += bytes.len() as u64;

                let prepare_started = Instant::now();
                let prepared =
                    prepare_image_with_strategy(&bytes, target_long_edge, decode_strategy);
                let prepare_elapsed = prepare_started.elapsed();
                total_prepare += prepare_elapsed;
                page.prepare_ms = millis(prepare_elapsed);

                match prepared {
                    Ok(prepared) => {
                        page.original_width = Some(prepared.original_width);
                        page.original_height = Some(prepared.original_height);
                        page.display_width = Some(prepared.display_width);
                        page.display_height = Some(prepared.display_height);
                        page.display_bytes = Some(prepared.byte_size);
                        successful_prepare_ms.push(page.prepare_ms);
                        max_display_bytes = max_display_bytes.max(prepared.byte_size);
                        let backend = prepared.decode_backend.as_str().to_owned();
                        *backend_counts.entry(backend.clone()).or_default() += 1;
                        page.decode_backend = Some(backend);
                        total_display_bytes += prepared.byte_size as u64;
                    }
                    Err(error) => {
                        failures += 1;
                        page.error = Some(error);
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

    let (prepare_p50_ms, prepare_p95_ms) = prepare_percentiles(successful_prepare_ms);

    let mut report = PerfReport {
        path: path.display().to_string(),
        title: source.title().to_owned(),
        book_id: source.book_id().to_owned(),
        page_count: source.page_count(),
        target_long_edge,
        decode_strategy: decode_strategy.as_str().to_owned(),
        open_ms: millis(open_elapsed),
        total_read_ms: millis(total_read),
        total_prepare_ms: millis(total_prepare),
        prepare_p50_ms,
        prepare_p95_ms,
        max_display_bytes,
        total_source_bytes,
        total_display_bytes,
        failures,
        backend_counts,
        pages,
    };

    if let Some(forced_page) = forced_page {
        report.title = format!("{} (starts at page {})", report.title, forced_page + 1);
    }

    Ok(report)
}

fn print_report(report: &PerfReport) {
    let successful = report.page_count.saturating_sub(report.failures);
    println!("SuiSuiView perf scan");
    println!("Path: {}", report.path);
    println!("Book: {}", report.title);
    println!("Pages: {} ok / {} failed", successful, report.failures);
    println!("Decode strategy: {}", report.decode_strategy);
    println!("Open/index: {:.2} ms", report.open_ms);
    println!(
        "Read: {:.2} ms total, {:.2} ms/page",
        report.total_read_ms,
        average(report.total_read_ms, report.page_count)
    );
    println!(
        "Prepare: {:.2} ms total, {:.2} ms/page",
        report.total_prepare_ms,
        average(report.total_prepare_ms, successful)
    );
    println!(
        "Prepare percentiles: p50 {:.2} ms, p95 {:.2} ms",
        report.prepare_p50_ms, report.prepare_p95_ms
    );
    println!(
        "Bytes: source {:.1} MB, display {:.1} MB",
        mib(report.total_source_bytes),
        mib(report.total_display_bytes)
    );
    if !report.backend_counts.is_empty() {
        let backends = report
            .backend_counts
            .iter()
            .map(|(backend, count)| format!("{backend}={count}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("Backends: {backends}");
    }

    let mut slowest = report
        .pages
        .iter()
        .filter(|page| page.error.is_none())
        .collect::<Vec<_>>();
    slowest.sort_by(|left, right| {
        right
            .prepare_ms
            .partial_cmp(&left.prepare_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    for page in slowest.into_iter().take(5) {
        println!(
            "Slow page {:>4}: read {:>7.2} ms, prepare {:>7.2} ms, {}",
            page.index + 1,
            page.read_ms,
            page.prepare_ms,
            page.name
        );
    }
}

fn prepare_percentiles(mut values: Vec<f64>) -> (f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    (percentile(&values, 0.50), percentile(&values, 0.95))
}

fn percentile(values: &[f64], quantile: f64) -> f64 {
    let index = ((values.len() - 1) as f64 * quantile.clamp(0.0, 1.0)).round() as usize;
    values[index]
}

fn write_report(path: &Path, report: &PerfReport) -> Result<(), String> {
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

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

pub fn default_report_path() -> PathBuf {
    PathBuf::from("perf-fixtures").join("report.json")
}
