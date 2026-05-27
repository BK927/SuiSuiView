mod animated;
mod candidates;
mod source;
mod stats;
#[cfg(feature = "bench-native-wuffs")]
mod wuffs;

use candidates::{candidate_decoders, deferred_candidates, detect_format, BenchFormat};
use serde::Serialize;
use source::DecoderBenchSource;
use stats::TimingStats;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const DEFAULT_DECODER_BENCH_ITERATIONS: usize = 3;
pub const DEFAULT_DECODER_BENCH_MAX_PAGES: usize = 12;
const DECODER_BENCH_WARMUP_RUNS: usize = 1;

#[derive(Debug, Serialize)]
pub struct DecoderBenchReport {
    pub path: String,
    pub title: String,
    pub book_id: String,
    pub source_page_count: usize,
    pub pages_tested: usize,
    pub iterations: usize,
    pub max_pages: usize,
    pub total_source_bytes: u64,
    pub features: DecoderBenchFeatures,
    pub corpus: Vec<DecoderCorpusFormatSummary>,
    pub summaries: Vec<DecoderCandidateSummary>,
    pub pages: Vec<DecoderBenchPage>,
    pub deferred_candidates: Vec<DeferredCandidate>,
}

#[derive(Debug, Serialize)]
pub struct DecoderBenchFeatures {
    pub bench_native: bool,
    pub bench_native_jpeg_turbo: bool,
    pub bench_native_webp: bool,
    pub bench_native_wuffs: bool,
    pub bench_avif_native: bool,
    pub bench_libavif_native: bool,
    pub bench_svg: bool,
}

#[derive(Debug, Serialize)]
pub struct DecoderCorpusFormatSummary {
    pub format: String,
    pub pages: usize,
    pub source_bytes: u64,
    pub decoded_pixels: u64,
}

#[derive(Debug, Serialize)]
pub struct DecoderCandidateSummary {
    pub format: String,
    pub candidate: String,
    pub note: String,
    pub output_pixel_format: String,
    pub allocation_note: String,
    pub pages_ok: usize,
    pub failures: usize,
    pub cold_runs: usize,
    pub runs: usize,
    pub cold_timing: TimingStats,
    pub warm_timing: TimingStats,
    pub throughput_mpix_s: f64,
}

#[derive(Debug, Serialize)]
pub struct DecoderBenchPage {
    pub index: usize,
    pub name: String,
    pub format: String,
    pub source_bytes: u64,
    pub results: Vec<DecoderCandidateResult>,
}

#[derive(Debug, Serialize)]
pub struct DecoderCandidateResult {
    pub candidate: String,
    pub note: String,
    pub output_pixel_format: String,
    pub allocation_note: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frames_decoded: Option<u32>,
    pub total_duration_ms: Option<u64>,
    pub rgba_bytes: Option<usize>,
    pub checksum16: Option<String>,
    pub reference_match: Option<bool>,
    pub reference_note: Option<String>,
    pub cold_timing: TimingStats,
    pub runs: usize,
    pub warm_timing: TimingStats,
    pub throughput_mpix_s: f64,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DeferredCandidate {
    pub format: &'static str,
    pub candidate: &'static str,
    pub reason: &'static str,
}

#[derive(Default)]
struct SummaryAccumulator {
    note: String,
    output_pixel_format: String,
    allocation_note: String,
    pages_ok: usize,
    failures: usize,
    cold_times_ms: Vec<f64>,
    runs: usize,
    warm_times_ms: Vec<f64>,
    decoded_pixels: u64,
}

struct ReferenceImage {
    width: u32,
    height: u32,
    frames_decoded: u32,
    total_duration_ms: u64,
    rgba_bytes: usize,
    checksum16: String,
    accepted_checksums: BTreeSet<String>,
    exact: bool,
}

#[derive(Default)]
struct CorpusAccumulator {
    pages: usize,
    source_bytes: u64,
    decoded_pixels: u64,
}

pub fn run_decoder_bench(
    path: &Path,
    report_path: Option<&Path>,
    iterations: usize,
    max_pages: usize,
) -> Result<(), String> {
    let report = bench_path(path, iterations.max(1), max_pages.max(1))?;
    print_report(&report);

    if let Some(report_path) = report_path {
        write_report(report_path, &report)?;
        println!("Report: {}", report_path.display());
    }

    Ok(())
}

pub fn bench_path(
    path: &Path,
    iterations: usize,
    max_pages: usize,
) -> Result<DecoderBenchReport, String> {
    let source = DecoderBenchSource::open(path)?;
    let page_limit = source.page_count().min(max_pages);
    let mut total_source_bytes = 0u64;
    let mut corpus: BTreeMap<String, CorpusAccumulator> = BTreeMap::new();
    let mut pages = Vec::with_capacity(page_limit);
    let mut summaries: BTreeMap<(String, String), SummaryAccumulator> = BTreeMap::new();

    for index in 0..page_limit {
        let bytes = source.read_page(index)?;
        total_source_bytes += bytes.len() as u64;
        let format = detect_format(&bytes);
        let format_name = format
            .map(BenchFormat::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let mut page = DecoderBenchPage {
            index,
            name: source.page_name(index).unwrap_or("").to_owned(),
            format: format_name.clone(),
            source_bytes: bytes.len() as u64,
            results: Vec::new(),
        };

        let Some(format) = format else {
            record_corpus(&mut corpus, &format_name, bytes.len() as u64, None);
            page.results.push(unsupported_result(
                "format-sniff",
                "No decoder benchmark candidate recognized this byte signature.",
            ));
            pages.push(page);
            continue;
        };

        let candidates = candidate_decoders(format);
        if candidates.is_empty() {
            record_corpus(&mut corpus, &format_name, bytes.len() as u64, None);
            page.results.push(unsupported_result(
                "candidate-table",
                "No decoder benchmark candidate is wired for this format.",
            ));
            pages.push(page);
            continue;
        }

        let mut candidate_results = Vec::with_capacity(candidates.len());
        for candidate in rotated_candidates(candidates, index) {
            candidate_results.push(run_candidate(candidate, &bytes, iterations));
        }

        let reference = reference_from_results(format, &candidate_results);
        apply_reference_validation(&mut candidate_results, reference.as_ref());
        record_corpus(
            &mut corpus,
            &format_name,
            bytes.len() as u64,
            decoded_pixels_for_corpus(reference.as_ref(), &candidate_results),
        );

        for result in candidate_results {
            let key = (format.as_str().to_owned(), result.candidate.clone());
            let summary = summaries.entry(key).or_insert_with(|| SummaryAccumulator {
                note: result.note.clone(),
                output_pixel_format: result.output_pixel_format.clone(),
                allocation_note: result.allocation_note.clone(),
                ..SummaryAccumulator::default()
            });
            summary.runs += result.runs;
            summary
                .cold_times_ms
                .extend(result.cold_timing.samples.iter());
            summary
                .warm_times_ms
                .extend(result.warm_timing.samples.iter());
            if result.error.is_some() || result.reference_match == Some(false) {
                summary.failures += 1;
            } else {
                summary.pages_ok += 1;
            }
            if result.error.is_none() {
                if let (Some(width), Some(height), Some(frames_decoded)) =
                    (result.width, result.height, result.frames_decoded)
                {
                    summary.decoded_pixels += u64::from(width)
                        * u64::from(height)
                        * u64::from(frames_decoded.max(1))
                        * result.runs as u64;
                }
            }
            page.results.push(result);
        }

        pages.push(page);
    }

    let report_title = source.title().to_owned();

    Ok(DecoderBenchReport {
        path: path.display().to_string(),
        title: report_title,
        book_id: source.book_id().to_owned(),
        source_page_count: source.page_count(),
        pages_tested: page_limit,
        iterations,
        max_pages,
        total_source_bytes,
        features: DecoderBenchFeatures {
            bench_native: cfg!(feature = "bench-native"),
            bench_native_jpeg_turbo: cfg!(feature = "bench-native-jpeg-turbo"),
            bench_native_webp: cfg!(feature = "bench-native-webp"),
            bench_native_wuffs: cfg!(feature = "bench-native-wuffs"),
            bench_avif_native: cfg!(feature = "bench-avif-native"),
            bench_libavif_native: cfg!(feature = "bench-libavif-native"),
            bench_svg: cfg!(feature = "bench-svg"),
        },
        corpus: corpus
            .into_iter()
            .map(|(format, summary)| DecoderCorpusFormatSummary {
                format,
                pages: summary.pages,
                source_bytes: summary.source_bytes,
                decoded_pixels: summary.decoded_pixels,
            })
            .collect(),
        summaries: summaries
            .into_iter()
            .map(|((format, candidate), summary)| {
                let cold_timing = TimingStats::from_samples(summary.cold_times_ms);
                let warm_timing = TimingStats::from_samples(summary.warm_times_ms);
                let throughput_mpix_s =
                    throughput_mpix_s(summary.decoded_pixels, warm_timing.total_ms);
                DecoderCandidateSummary {
                    format,
                    candidate,
                    note: summary.note,
                    output_pixel_format: summary.output_pixel_format,
                    allocation_note: summary.allocation_note,
                    pages_ok: summary.pages_ok,
                    failures: summary.failures,
                    cold_runs: cold_timing.samples.len(),
                    runs: summary.runs,
                    cold_timing,
                    warm_timing,
                    throughput_mpix_s,
                }
            })
            .collect(),
        pages,
        deferred_candidates: deferred_candidates(),
    })
}

fn run_candidate(
    candidate: &candidates::CandidateDecoder,
    bytes: &[u8],
    iterations: usize,
) -> DecoderCandidateResult {
    let cold_started = Instant::now();
    let cold_decoded = (candidate.decode)(bytes);
    let cold_elapsed = cold_started.elapsed();
    let mut samples = Vec::with_capacity(iterations);
    let mut width = None;
    let mut height = None;
    let mut frames_decoded = None;
    let mut total_duration_ms = None;
    let mut rgba_bytes = None;
    let mut checksum = None;
    let reference_match = None;
    let reference_note = None;
    let mut decoded_pixels = 0u64;

    match cold_decoded {
        Ok(decoded) => {
            black_box(decoded.pixels.as_ptr());
            width = Some(decoded.width);
            height = Some(decoded.height);
            frames_decoded = Some(decoded.frames_decoded);
            total_duration_ms = Some(decoded.total_duration_ms);
            rgba_bytes = Some(decoded.pixels.len());
            checksum = Some(checksum16(&decoded.pixels));
        }
        Err(error) => {
            let cold_timing = TimingStats::from_samples(vec![cold_elapsed.as_secs_f64() * 1000.0]);
            return DecoderCandidateResult {
                candidate: candidate.name.to_owned(),
                note: candidate.note.to_owned(),
                output_pixel_format: candidate.output_pixel_format.to_owned(),
                allocation_note: candidate.allocation_note.to_owned(),
                width,
                height,
                frames_decoded,
                total_duration_ms,
                rgba_bytes,
                checksum16: checksum,
                reference_match,
                reference_note,
                cold_timing,
                runs: 0,
                warm_timing: TimingStats::default(),
                throughput_mpix_s: 0.0,
                error: Some(error),
            };
        }
    }

    for _ in 0..DECODER_BENCH_WARMUP_RUNS {
        if let Ok(decoded) = (candidate.decode)(bytes) {
            black_box(decoded.pixels.as_ptr());
        }
    }

    for _ in 0..iterations {
        let started = Instant::now();
        let decoded = (candidate.decode)(bytes);
        let elapsed = started.elapsed();

        match decoded {
            Ok(decoded) => {
                samples.push(elapsed.as_secs_f64() * 1000.0);
                black_box(decoded.pixels.as_ptr());
                decoded_pixels += decoded.decoded_pixels();
            }
            Err(error) => {
                let timing = TimingStats::from_samples(samples);
                return DecoderCandidateResult {
                    candidate: candidate.name.to_owned(),
                    note: candidate.note.to_owned(),
                    output_pixel_format: candidate.output_pixel_format.to_owned(),
                    allocation_note: candidate.allocation_note.to_owned(),
                    width,
                    height,
                    frames_decoded,
                    total_duration_ms,
                    rgba_bytes,
                    checksum16: checksum,
                    reference_match,
                    reference_note,
                    cold_timing: TimingStats::from_samples(vec![
                        cold_elapsed.as_secs_f64() * 1000.0,
                    ]),
                    runs: timing.samples.len(),
                    throughput_mpix_s: throughput_mpix_s(decoded_pixels, timing.total_ms),
                    warm_timing: timing,
                    error: Some(error),
                };
            }
        }
    }

    let timing = TimingStats::from_samples(samples);
    DecoderCandidateResult {
        candidate: candidate.name.to_owned(),
        note: candidate.note.to_owned(),
        output_pixel_format: candidate.output_pixel_format.to_owned(),
        allocation_note: candidate.allocation_note.to_owned(),
        width,
        height,
        frames_decoded,
        total_duration_ms,
        rgba_bytes,
        checksum16: checksum,
        reference_match,
        reference_note,
        cold_timing: TimingStats::from_samples(vec![cold_elapsed.as_secs_f64() * 1000.0]),
        runs: timing.samples.len(),
        throughput_mpix_s: throughput_mpix_s(decoded_pixels, timing.total_ms),
        warm_timing: timing,
        error: None,
    }
}

fn unsupported_result(candidate: &str, error: &str) -> DecoderCandidateResult {
    DecoderCandidateResult {
        candidate: candidate.to_owned(),
        note: String::new(),
        output_pixel_format: String::new(),
        allocation_note: String::new(),
        width: None,
        height: None,
        frames_decoded: None,
        total_duration_ms: None,
        rgba_bytes: None,
        checksum16: None,
        reference_match: None,
        reference_note: None,
        cold_timing: TimingStats::default(),
        runs: 0,
        warm_timing: TimingStats::default(),
        throughput_mpix_s: 0.0,
        error: Some(error.to_owned()),
    }
}

fn reference_from_results(
    format: BenchFormat,
    results: &[DecoderCandidateResult],
) -> Option<ReferenceImage> {
    let baseline = preferred_reference_result(format, results)?;
    let exact_by_format = format.exact_reference();
    let animated = baseline.frames_decoded.unwrap_or(1) > 1;
    let checksum_consensus_allowed =
        format.allows_checksum_consensus() || (matches!(format, BenchFormat::Webp) && animated);
    let (checksum16, accepted_checksums, has_checksum_consensus) =
        if exact_by_format || checksum_consensus_allowed {
            let baseline_checksum = baseline.checksum16.clone()?;
            let (accepted, has_consensus) = accepted_exact_checksums(
                checksum_consensus_allowed,
                results,
                baseline,
                &baseline_checksum,
            );
            (baseline_checksum, accepted, has_consensus)
        } else {
            (String::new(), BTreeSet::new(), false)
        };
    let exact = exact_by_format
        || (matches!(format, BenchFormat::Webp) && animated && has_checksum_consensus);
    Some(ReferenceImage {
        width: baseline.width?,
        height: baseline.height?,
        frames_decoded: baseline.frames_decoded.unwrap_or(1),
        total_duration_ms: baseline.total_duration_ms.unwrap_or(0),
        rgba_bytes: baseline.rgba_bytes?,
        checksum16,
        accepted_checksums,
        exact,
    })
}

fn preferred_reference_result<'a>(
    format: BenchFormat,
    results: &'a [DecoderCandidateResult],
) -> Option<&'a DecoderCandidateResult> {
    let has_animation_result = results
        .iter()
        .any(|result| result.error.is_none() && result.frames_decoded.unwrap_or(1) > 1);
    if has_animation_result {
        let preferred: &[&str] = match format {
            BenchFormat::Webp => &["image-webp-all-frames-rgba", "libwebp-all-frames-rgba"],
            BenchFormat::Gif => &["image-gif-animation-rgba", "gif-animation-rgba"],
            _ => &[],
        };
        for candidate in preferred {
            if let Some(result) = results
                .iter()
                .find(|result| result.candidate == *candidate && result.error.is_none())
            {
                return Some(result);
            }
        }
    }

    results
        .iter()
        .find(|result| result.candidate == "image-crate-rgba" && result.error.is_none())
        .or_else(|| results.iter().find(|result| result.error.is_none()))
}

fn accepted_exact_checksums(
    checksum_consensus_allowed: bool,
    results: &[DecoderCandidateResult],
    baseline: &DecoderCandidateResult,
    baseline_checksum: &str,
) -> (BTreeSet<String>, bool) {
    let mut accepted = BTreeSet::from([baseline_checksum.to_owned()]);
    if !checksum_consensus_allowed {
        return (accepted, false);
    }

    let (Some(width), Some(height), Some(frames_decoded), Some(rgba_bytes)) = (
        baseline.width,
        baseline.height,
        baseline.frames_decoded,
        baseline.rgba_bytes,
    ) else {
        return (accepted, false);
    };

    let mut counts = BTreeMap::<String, usize>::new();
    for result in results.iter().filter(|result| result.error.is_none()) {
        if result.width != Some(width)
            || result.height != Some(height)
            || result.frames_decoded != Some(frames_decoded)
            || result.rgba_bytes != Some(rgba_bytes)
        {
            continue;
        }
        if let Some(checksum) = result.checksum16.as_deref() {
            *counts.entry(checksum.to_owned()).or_default() += 1;
        }
    }

    let mut has_consensus = false;
    for (checksum, count) in counts {
        if count >= 2 {
            accepted.insert(checksum);
            has_consensus = true;
        }
    }
    (accepted, has_consensus)
}

fn apply_reference_validation(
    results: &mut [DecoderCandidateResult],
    reference: Option<&ReferenceImage>,
) {
    for result in results {
        let validation = validate_result_against_reference(result, reference);
        result.reference_match = validation.0;
        result.reference_note = validation.1;
    }
}

fn validate_result_against_reference(
    result: &DecoderCandidateResult,
    reference: Option<&ReferenceImage>,
) -> (Option<bool>, Option<String>) {
    if result.error.is_some() {
        return (None, None);
    }

    let Some(reference) = reference else {
        return (None, Some("image-crate reference result failed".to_owned()));
    };

    let (Some(width), Some(height), Some(rgba_bytes)) =
        (result.width, result.height, result.rgba_bytes)
    else {
        return (
            Some(false),
            Some("candidate did not report complete RGBA metadata".to_owned()),
        );
    };

    if width != reference.width || height != reference.height {
        return (
            Some(false),
            Some(format!(
                "dimension mismatch: candidate {}x{}, reference {}x{}",
                width, height, reference.width, reference.height
            )),
        );
    }

    if result.frames_decoded.unwrap_or(1) != reference.frames_decoded {
        return (
            Some(false),
            Some(format!(
                "frame count mismatch: candidate {}, reference {}",
                result.frames_decoded.unwrap_or(1),
                reference.frames_decoded
            )),
        );
    }

    if result.total_duration_ms.unwrap_or(0) != reference.total_duration_ms {
        return (
            Some(false),
            Some(format!(
                "animation duration mismatch: candidate {} ms, reference {} ms",
                result.total_duration_ms.unwrap_or(0),
                reference.total_duration_ms
            )),
        );
    }

    if rgba_bytes != reference.rgba_bytes {
        return (
            Some(false),
            Some(format!(
                "RGBA byte length mismatch: candidate {}, reference {}",
                rgba_bytes, reference.rgba_bytes
            )),
        );
    }

    if !reference.exact {
        return (
            Some(true),
            Some(
                "dimensions, frame metadata, and RGBA byte length match; exact pixels not required"
                    .to_owned(),
            ),
        );
    }

    let Some(checksum) = result.checksum16.as_deref() else {
        return (
            Some(false),
            Some("candidate did not report an RGBA checksum".to_owned()),
        );
    };
    if checksum == reference.checksum16 {
        (Some(true), Some("exact RGBA checksum match".to_owned()))
    } else if reference.accepted_checksums.contains(checksum) {
        (
            Some(true),
            Some(format!(
                "RGBA checksum matches another accepted decoder consensus: {checksum}"
            )),
        )
    } else {
        (
            Some(false),
            Some(format!(
                "RGBA checksum mismatch: candidate {checksum}, reference {}",
                reference.checksum16
            )),
        )
    }
}

fn rotated_candidates(
    candidates: &[candidates::CandidateDecoder],
    page_index: usize,
) -> impl Iterator<Item = &candidates::CandidateDecoder> {
    let start = page_index % candidates.len().max(1);
    (0..candidates.len()).map(move |offset| &candidates[(start + offset) % candidates.len()])
}

fn print_report(report: &DecoderBenchReport) {
    println!("SuiSuiView decoder bench");
    println!("Path: {}", report.path);
    println!("Book: {}", report.title);
    println!(
        "Pages: {} / {}, iterations: {}",
        report.pages_tested, report.source_page_count, report.iterations
    );
    println!(
        "Source bytes: {:.1} MB",
        report.total_source_bytes as f64 / (1024.0 * 1024.0)
    );
    println!(
        "Features: bench-native={}, jpeg-turbo={}, libwebp={}, wuffs={}, bench-avif-native={}, bench-libavif-native={}, bench-svg={}",
        report.features.bench_native,
        report.features.bench_native_jpeg_turbo,
        report.features.bench_native_webp,
        report.features.bench_native_wuffs,
        report.features.bench_avif_native,
        report.features.bench_libavif_native,
        report.features.bench_svg
    );
    println!();
    println!("Corpus summary:");
    for corpus in &report.corpus {
        println!(
            "  {:>7} pages {:>5} bytes {:>9.1} MB pixels {:>10.1} MP",
            corpus.format,
            corpus.pages,
            corpus.source_bytes as f64 / (1024.0 * 1024.0),
            corpus.decoded_pixels as f64 / 1_000_000.0
        );
    }
    println!();
    println!("Candidate summary:");
    for summary in &report.summaries {
        println!(
            "  {:>5} {:<24} warm mean {:>7.2} ms p95 {:>7.2} ms p99 {:>7.2} ms {:>8.1} MP/s ok {} fail {}",
            summary.format,
            summary.candidate,
            summary.warm_timing.mean_ms,
            summary.warm_timing.p95_ms,
            summary.warm_timing.p99_ms,
            summary.throughput_mpix_s,
            summary.pages_ok,
            summary.failures
        );
    }

    if !report.deferred_candidates.is_empty() {
        println!();
        println!("Deferred native/security-gated candidates:");
        for deferred in &report.deferred_candidates {
            println!(
                "  {:>5} {:<22} {}",
                deferred.format, deferred.candidate, deferred.reason
            );
        }
    }
}

fn write_report(path: &Path, report: &DecoderBenchReport) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string_pretty(report).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

fn record_corpus(
    corpus: &mut BTreeMap<String, CorpusAccumulator>,
    format: &str,
    source_bytes: u64,
    decoded_pixels: Option<u64>,
) {
    let summary = corpus.entry(format.to_owned()).or_default();
    summary.pages += 1;
    summary.source_bytes += source_bytes;
    if let Some(decoded_pixels) = decoded_pixels {
        summary.decoded_pixels += decoded_pixels;
    }
}

fn decoded_pixels_for_corpus(
    reference: Option<&ReferenceImage>,
    results: &[DecoderCandidateResult],
) -> Option<u64> {
    if let Some(reference) = reference {
        return Some(
            u64::from(reference.width)
                * u64::from(reference.height)
                * u64::from(reference.frames_decoded.max(1)),
        );
    }

    results.iter().find_map(|result| {
        if result.error.is_some() {
            return None;
        }
        let (Some(width), Some(height)) = (result.width, result.height) else {
            return None;
        };
        Some(u64::from(width) * u64::from(height) * u64::from(result.frames_decoded.unwrap_or(1)))
    })
}

fn checksum16(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().chars().take(16).collect()
}

fn throughput_mpix_s(pixels: u64, total_ms: f64) -> f64 {
    if pixels == 0 || total_ms <= 0.0 {
        0.0
    } else {
        pixels as f64 / (total_ms / 1000.0) / 1_000_000.0
    }
}

pub fn default_decoder_report_path() -> PathBuf {
    PathBuf::from("bench-output").join("decoder-bench.json")
}
