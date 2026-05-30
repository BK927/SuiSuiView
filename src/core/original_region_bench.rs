use crate::core::source::open_source_from_path;
use crate::core::worker::{
    prepare_original_region_with_options, DecodeOptions, OriginalRegion, PreparedRegion,
};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Serialize)]
pub struct OriginalRegionBenchReport {
    pub path: String,
    pub title: String,
    pub book_id: String,
    pub page_count: usize,
    pub page_index: usize,
    pub page_name: String,
    pub source_bytes: u64,
    pub region: OriginalRegion,
    pub iterations: usize,
    pub open_ms: f64,
    pub read_ms: f64,
    pub prepare_p50_ms: f64,
    pub prepare_p95_ms: f64,
    pub prepare_max_ms: f64,
    pub original_width: Option<u32>,
    pub original_height: Option<u32>,
    pub region_bytes: Option<usize>,
    pub decode_backend: Option<String>,
    pub error: Option<String>,
    pub samples: Vec<OriginalRegionBenchSample>,
}

#[derive(Debug, Serialize)]
pub struct OriginalRegionBenchSample {
    pub iteration: usize,
    pub prepare_ms: f64,
    pub region_bytes: Option<usize>,
    pub error: Option<String>,
}

pub fn run_original_region_bench(
    path: &Path,
    report_path: Option<&Path>,
    page_index: usize,
    region: OriginalRegion,
    iterations: usize,
) -> Result<(), String> {
    let report = bench_original_region(path, page_index, region, iterations.max(1))?;
    print_report(&report);

    if let Some(report_path) = report_path {
        write_report(report_path, &report)?;
        println!("Report: {}", report_path.display());
    }

    Ok(())
}

pub fn bench_original_region(
    path: &Path,
    page_index: usize,
    region: OriginalRegion,
    iterations: usize,
) -> Result<OriginalRegionBenchReport, String> {
    let open_started = Instant::now();
    let (source, forced_page) = open_source_from_path(path).map_err(|error| error.to_string())?;
    let open_elapsed = open_started.elapsed();
    let page_count = source.page_count();
    if page_count == 0 {
        return Err("source has no pages".to_owned());
    }
    if page_index >= page_count {
        return Err(format!(
            "page index {page_index} is out of range for {page_count} pages"
        ));
    }

    let read_started = Instant::now();
    let bytes = source
        .read_page(page_index)
        .map_err(|error| error.to_string())?;
    let read_elapsed = read_started.elapsed();
    let mut samples = Vec::with_capacity(iterations);
    let mut successful_prepare_ms = Vec::new();
    let mut original_width = None;
    let mut original_height = None;
    let mut region_bytes = None;
    let mut decode_backend = None;
    let mut aggregate_error = None;

    for iteration in 0..iterations {
        let prepare_started = Instant::now();
        let prepared: Result<Option<PreparedRegion>, String> =
            prepare_original_region_with_options(&bytes, region, DecodeOptions::default());
        let prepare_elapsed = prepare_started.elapsed();
        let prepare_ms = millis(prepare_elapsed);

        match prepared {
            Ok(Some(prepared)) => {
                original_width.get_or_insert(prepared.original_width);
                original_height.get_or_insert(prepared.original_height);
                region_bytes.get_or_insert(prepared.byte_size);
                decode_backend.get_or_insert_with(|| prepared.decode_backend.as_str().to_owned());
                successful_prepare_ms.push(prepare_ms);
                samples.push(OriginalRegionBenchSample {
                    iteration,
                    prepare_ms,
                    region_bytes: Some(prepared.byte_size),
                    error: None,
                });
            }
            Ok(None) => {
                let error = "region decode unsupported for this page/options".to_owned();
                aggregate_error.get_or_insert_with(|| error.clone());
                samples.push(OriginalRegionBenchSample {
                    iteration,
                    prepare_ms,
                    region_bytes: None,
                    error: Some(error),
                });
            }
            Err(error) => {
                aggregate_error.get_or_insert_with(|| error.clone());
                samples.push(OriginalRegionBenchSample {
                    iteration,
                    prepare_ms,
                    region_bytes: None,
                    error: Some(error),
                });
            }
        }
    }

    let (prepare_p50_ms, prepare_p95_ms, prepare_max_ms) =
        prepare_percentiles(successful_prepare_ms);
    let mut title = source.title().to_owned();
    if let Some(forced_page) = forced_page {
        title = format!("{title} (starts at page {})", forced_page + 1);
    }

    Ok(OriginalRegionBenchReport {
        path: path.display().to_string(),
        title,
        book_id: source.book_id().to_owned(),
        page_count,
        page_index,
        page_name: source.page_name(page_index).unwrap_or("").to_owned(),
        source_bytes: bytes.len() as u64,
        region,
        iterations,
        open_ms: millis(open_elapsed),
        read_ms: millis(read_elapsed),
        prepare_p50_ms,
        prepare_p95_ms,
        prepare_max_ms,
        original_width,
        original_height,
        region_bytes,
        decode_backend,
        error: aggregate_error,
        samples,
    })
}

fn print_report(report: &OriginalRegionBenchReport) {
    println!("SuiSuiView original region bench");
    println!("Path: {}", report.path);
    println!("Book: {}", report.title);
    println!(
        "Page: {} / {} ({})",
        report.page_index + 1,
        report.page_count,
        report.page_name
    );
    println!(
        "Region: {},{} {}x{}",
        report.region.x, report.region.y, report.region.width, report.region.height
    );
    println!("Iterations: {}", report.iterations);
    println!("Open/index: {:.2} ms", report.open_ms);
    println!("Read: {:.2} ms", report.read_ms);
    if let Some(error) = report.error.as_deref() {
        println!("Error: {error}");
    }
    if let Some(backend) = report.decode_backend.as_deref() {
        println!("Backend: {backend}");
    }
    println!(
        "Prepare percentiles: p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms",
        report.prepare_p50_ms, report.prepare_p95_ms, report.prepare_max_ms
    );
    if let Some(bytes) = report.region_bytes {
        println!("Region bytes: {:.2} MiB", mib(bytes as u64));
    }
}

fn prepare_percentiles(mut values: Vec<f64>) -> (f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let max = values.last().copied().unwrap_or_default();
    (percentile(&values, 0.50), percentile(&values, 0.95), max)
}

fn percentile(values: &[f64], quantile: f64) -> f64 {
    let index = ((values.len() - 1) as f64 * quantile.clamp(0.0, 1.0)).round() as usize;
    values[index]
}

fn write_report(path: &Path, report: &OriginalRegionBenchReport) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string_pretty(report).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

pub fn default_original_region_report_path() -> PathBuf {
    PathBuf::from("perf-fixtures").join("original-region-report.json")
}

#[cfg(test)]
mod tests {
    use super::bench_original_region;
    use crate::core::worker::OriginalRegion;
    use image::{ImageBuffer, ImageFormat, Rgba};
    use std::fs;
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn original_region_bench_scans_png_region() {
        let dir = unique_temp_dir("region-bench");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("page-000.png"), encoded_xy_png(8, 6)).unwrap();

        let report = bench_original_region(
            &dir,
            0,
            OriginalRegion {
                x: 2,
                y: 1,
                width: 3,
                height: 2,
            },
            3,
        )
        .unwrap();

        assert_eq!(report.page_count, 1);
        assert_eq!(report.iterations, 3);
        assert_eq!(report.original_width, Some(8));
        assert_eq!(report.original_height, Some(6));
        assert_eq!(report.region_bytes, Some(3 * 2 * 4));
        assert_eq!(report.decode_backend.as_deref(), Some("png-exact-rows"));
        assert!(report.error.is_none());
        assert_eq!(report.samples.len(), 3);
        assert!(report.samples.iter().all(|sample| sample.error.is_none()));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn original_region_bench_rejects_out_of_range_page_index() {
        let dir = unique_temp_dir("region-bench-page-index");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("page-000.png"), encoded_xy_png(8, 6)).unwrap();

        let error = bench_original_region(
            &dir,
            1,
            OriginalRegion {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            1,
        )
        .unwrap_err();

        assert!(error.contains("out of range"), "{error}");
        let _ = fs::remove_dir_all(dir);
    }

    fn encoded_xy_png(width: u32, height: u32) -> Vec<u8> {
        let image = ImageBuffer::from_fn(width, height, |x, y| {
            Rgba([(x * 17) as u8, (y * 31) as u8, ((x + y) * 13) as u8, 255])
        });
        let mut cursor = Cursor::new(Vec::new());
        image
            .write_to(&mut cursor, ImageFormat::Png)
            .expect("encode PNG fixture");
        cursor.into_inner()
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("suisuiview-{name}-{stamp}"))
    }
}
