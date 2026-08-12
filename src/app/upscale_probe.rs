use super::{PageCacheKey, SuiSuiViewApp, UpscaleDecisionOrigin};
use crate::core::source::PageId;
use crate::core::state::{UpscaleProbeRecord, WgpuUpscaleMethod, UPSCALE_PROBE_VERSION};
use crate::core::upscale_bench::gpu::GpuUpscaleBench;
use crate::core::upscale_quality::compare_images;
use crate::core::worker::PreparedPage;
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use egui::ColorImage;
use image::{imageops::FilterType, RgbaImage};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

const PROBE_QUEUE_LIMIT: usize = 8;
/// Preferred square tile side; smaller books fall back to the largest even square that fits.
const TILE_SIZE: usize = 256;
const MIN_TILE_SIZE: usize = 64;
/// Luma standard deviation (0..1) below which a tile is treated as blank and skipped.
const BLANK_LUMA_STD: f64 = 0.02;
/// Successful page probes needed before the book decision is finalized.
pub(in crate::app) const PROBE_PAGES: usize = 3;
/// SSIM gap below which the two candidates are a tie; FSR (cheaper, arbitrary-ratio) wins ties.
const PROBE_TIE_MARGIN: f64 = 0.002;
/// Consecutive `None` results (typically a missing GPU bench) after which the book is abandoned.
const PROBE_FAILURE_LIMIT: usize = 3;

/// The two candidates AUTO routes between: Anime4K M (drawn/line art) vs FSR EASU+RCAS (photo).
const PROBE_METHODS: [WgpuUpscaleMethod; 2] = [
    WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
    WgpuUpscaleMethod::WgslFsr1EasuRcas,
];

#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct PageProbeResult {
    pub(in crate::app) ssim_anime4k: f64,
    pub(in crate::app) ssim_fsr: f64,
}

struct UpscaleProbeRequest {
    generation: u64,
    page_id: PageId,
    page: Arc<PreparedPage>,
}

pub(in crate::app) struct UpscaleProbeEvent {
    generation: u64,
    page_id: PageId,
    result: Option<PageProbeResult>,
}

pub(in crate::app) struct UpscaleProbeWorker {
    tx: Sender<UpscaleProbeRequest>,
    rx: Receiver<UpscaleProbeEvent>,
    generation: Arc<AtomicU64>,
}

impl UpscaleProbeWorker {
    pub(in crate::app) fn new(ctx: egui::Context) -> Self {
        let (request_tx, request_rx) = bounded::<UpscaleProbeRequest>(PROBE_QUEUE_LIMIT);
        let (event_tx, event_rx) = bounded::<UpscaleProbeEvent>(PROBE_QUEUE_LIMIT);
        let generation = Arc::new(AtomicU64::new(0));
        let worker_generation = generation.clone();

        let _ = thread::Builder::new()
            .name("suisuiview-upscale-probe".to_owned())
            .spawn(move || {
                // Device/pipeline creation is tens-hundreds of ms, so build the bench lazily on
                // the FIRST request rather than at app startup. `Err(())` means the bench is
                // unavailable; every probe then returns `None` and the app abandons the book.
                let mut bench: Option<Result<GpuUpscaleBench, ()>> = None;
                while let Ok(request) = request_rx.recv() {
                    if request.generation != worker_generation.load(Ordering::Acquire) {
                        continue;
                    }
                    let bench_slot = bench.get_or_insert_with(|| {
                        GpuUpscaleBench::new_for_methods(&PROBE_METHODS).map_err(|error| {
                            eprintln!("upscale probe bench unavailable: {error}");
                        })
                    });
                    let result = match bench_slot {
                        Ok(bench) => probe_page(bench, &request.page),
                        Err(()) => None,
                    };
                    if request.generation != worker_generation.load(Ordering::Acquire) {
                        continue;
                    }
                    if event_tx
                        .send(UpscaleProbeEvent {
                            generation: request.generation,
                            page_id: request.page_id,
                            result,
                        })
                        .is_ok()
                    {
                        ctx.request_repaint();
                    }
                }
            });

        Self {
            tx: request_tx,
            rx: event_rx,
            generation,
        }
    }

    pub(in crate::app) fn set_generation(&self, generation: u64) {
        self.generation.store(generation, Ordering::Release);
    }

    pub(in crate::app) fn enqueue(
        &self,
        generation: u64,
        page_id: PageId,
        page: Arc<PreparedPage>,
    ) -> bool {
        match self.tx.try_send(UpscaleProbeRequest {
            generation,
            page_id,
            page,
        }) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }

    pub(in crate::app) fn try_recv(&self) -> Option<UpscaleProbeEvent> {
        self.rx.try_recv().ok()
    }
}

/// Round-trip each usable tile of `page` (0.5x downscale then GPU upscale back) with both
/// candidates and average their SSIM against the untouched tile. `None` when no tile is usable.
fn probe_page(bench: &GpuUpscaleBench, page: &PreparedPage) -> Option<PageProbeResult> {
    let width = page.display_width;
    let height = page.display_height;
    let plan = plan_tiles(width, height)?;
    let rgba = page.pixels.to_rgba_vec(width, height);
    if rgba.len() < width.saturating_mul(height).saturating_mul(4) {
        return None;
    }

    let tiles: Vec<RgbaImage> = plan
        .origins
        .iter()
        .map(|&(x, y)| extract_tile(&rgba, width, x, y, plan.size))
        .collect();
    let stds: Vec<f64> = tiles.iter().map(luma_std).collect();

    let mut anime4k_sum = 0.0;
    let mut fsr_sum = 0.0;
    let mut count = 0usize;
    for index in usable_tile_indices(&stds) {
        let Some((anime4k, fsr)) = probe_tile(bench, &tiles[index], plan.size) else {
            continue;
        };
        anime4k_sum += anime4k;
        fsr_sum += fsr;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    Some(PageProbeResult {
        ssim_anime4k: anime4k_sum / count as f64,
        ssim_fsr: fsr_sum / count as f64,
    })
}

/// Downscale one tile 0.5x, upscale it back with each candidate, and return their SSIM.
fn probe_tile(bench: &GpuUpscaleBench, tile: &RgbaImage, size: usize) -> Option<(f64, f64)> {
    let half = (size / 2).max(1);
    let downscaled = image::imageops::resize(tile, half as u32, half as u32, FilterType::Lanczos3);
    let half_image = ColorImage::from_rgba_unmultiplied([half, half], downscaled.as_raw());
    let original = ColorImage::from_rgba_unmultiplied([size, size], tile.as_raw());
    let anime4k = upscaled_ssim(
        bench,
        &half_image,
        &original,
        [size, size],
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
    )?;
    let fsr = upscaled_ssim(
        bench,
        &half_image,
        &original,
        [size, size],
        WgpuUpscaleMethod::WgslFsr1EasuRcas,
    )?;
    Some((anime4k, fsr))
}

fn upscaled_ssim(
    bench: &GpuUpscaleBench,
    half: &ColorImage,
    original: &ColorImage,
    output_size: [usize; 2],
    method: WgpuUpscaleMethod,
) -> Option<f64> {
    let output = bench.apply(half, output_size, method).ok()?;
    compare_images(original, &output.image)
        .ok()
        .map(|metrics| metrics.ssim)
}

fn extract_tile(rgba: &[u8], image_width: usize, x: usize, y: usize, size: usize) -> RgbaImage {
    let mut buffer = Vec::with_capacity(size * size * 4);
    let row_bytes = size * 4;
    for row in 0..size {
        let start = ((y + row) * image_width + x) * 4;
        buffer.extend_from_slice(&rgba[start..start + row_bytes]);
    }
    RgbaImage::from_raw(size as u32, size as u32, buffer)
        .expect("tile buffer matches its dimensions")
}

fn luma_std(tile: &RgbaImage) -> f64 {
    let pixels = tile.as_raw();
    let count = (pixels.len() / 4) as f64;
    if count == 0.0 {
        return 0.0;
    }
    let mut sum = 0.0;
    let mut sum_sq = 0.0;
    for chunk in pixels.chunks_exact(4) {
        let luma = (0.299 * f64::from(chunk[0])
            + 0.587 * f64::from(chunk[1])
            + 0.114 * f64::from(chunk[2]))
            / 255.0;
        sum += luma;
        sum_sq += luma * luma;
    }
    let mean = sum / count;
    (sum_sq / count - mean * mean).max(0.0).sqrt()
}

struct TilePlan {
    size: usize,
    origins: Vec<(usize, usize)>,
}

/// Up to three non-overlapping square tiles centered on the top/middle/bottom thirds, clamped
/// inside the image. Origins that clamp to the same row are deduplicated. `None` when even the
/// smallest acceptable square does not fit.
fn plan_tiles(width: usize, height: usize) -> Option<TilePlan> {
    let size = tile_size(width, height)?;
    let x = (width - size) / 2;
    let max_y = height - size;
    let mut origins: Vec<(usize, usize)> = Vec::with_capacity(3);
    for band in 0..3usize {
        let center_y = height * (2 * band + 1) / 6;
        let top = center_y.saturating_sub(size / 2).min(max_y);
        if !origins.iter().any(|&(_, origin_y)| origin_y == top) {
            origins.push((x, top));
        }
    }
    Some(TilePlan { size, origins })
}

/// The square tile side to use: `TILE_SIZE` when both axes allow it, otherwise the largest even
/// square that fits, or `None` when that would be below `MIN_TILE_SIZE`.
fn tile_size(width: usize, height: usize) -> Option<usize> {
    let min_axis = width.min(height);
    if min_axis >= TILE_SIZE {
        return Some(TILE_SIZE);
    }
    let even = min_axis & !1;
    (even >= MIN_TILE_SIZE).then_some(even)
}

/// Indices of tiles whose luma standard deviation clears the blank threshold. If every tile is
/// blank, fall back to the single middle tile so a blank-ish book still yields a decision.
fn usable_tile_indices(stds: &[f64]) -> Vec<usize> {
    if stds.is_empty() {
        return Vec::new();
    }
    let non_blank: Vec<usize> = stds
        .iter()
        .enumerate()
        .filter(|&(_, &std)| std >= BLANK_LUMA_STD)
        .map(|(index, _)| index)
        .collect();
    if non_blank.is_empty() {
        vec![stds.len() / 2]
    } else {
        non_blank
    }
}

/// Higher mean SSIM wins; within `PROBE_TIE_MARGIN` the two are a tie and FSR wins.
pub(in crate::app) fn decide(mean_anime4k: f64, mean_fsr: f64) -> WgpuUpscaleMethod {
    if (mean_anime4k - mean_fsr).abs() < PROBE_TIE_MARGIN {
        return WgpuUpscaleMethod::WgslFsr1EasuRcas;
    }
    if mean_anime4k > mean_fsr {
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2M
    } else {
        WgpuUpscaleMethod::WgslFsr1EasuRcas
    }
}

/// Parse a persisted `winner` token back into a known candidate; unknown tokens are ignored.
fn method_for_probe_token(token: &str) -> Option<WgpuUpscaleMethod> {
    match token {
        "anime4k_v32_cnn_x2_m" => Some(WgpuUpscaleMethod::WgslAnime4kV32CnnX2M),
        "wgsl_fsr1_easu_rcas" => Some(WgpuUpscaleMethod::WgslFsr1EasuRcas),
        _ => None,
    }
}

fn route_book_upscale(
    fallback: WgpuUpscaleMethod,
    decision: Option<WgpuUpscaleMethod>,
) -> (WgpuUpscaleMethod, UpscaleDecisionOrigin) {
    if fallback != WgpuUpscaleMethod::Auto {
        return (fallback, UpscaleDecisionOrigin::User);
    }
    match decision {
        Some(method) => (method, UpscaleDecisionOrigin::ProbeAuto),
        None => (WgpuUpscaleMethod::Auto, UpscaleDecisionOrigin::AutoDefault),
    }
}

impl SuiSuiViewApp {
    pub(in crate::app) fn clear_upscale_probe_state(&mut self) {
        self.upscale_probe_generation = self.upscale_probe_generation.wrapping_add(1);
        if let Some(worker) = self.upscale_probe_worker.as_ref() {
            worker.set_generation(self.upscale_probe_generation);
        }
        self.probe_page_results.clear();
        self.probed_page_ids.clear();
        self.upscale_probe_failures = 0;
        self.book_upscale_decision = None;
    }

    /// Seed the decision from a persisted probe record so an already-decided book skips probing.
    pub(in crate::app) fn seed_upscale_decision_from_record(&mut self, book_id: &str) {
        let Some(record) = self.store.book_record(book_id) else {
            return;
        };
        let Some(probe) = record.upscale_probe else {
            return;
        };
        if probe.version != UPSCALE_PROBE_VERSION {
            return;
        }
        if let Some(method) = method_for_probe_token(&probe.winner) {
            self.book_upscale_decision = Some(method);
        }
    }

    pub(in crate::app) fn maybe_enqueue_upscale_probe(
        &mut self,
        key: PageCacheKey,
        page: Arc<PreparedPage>,
    ) {
        if self.settings.wgpu_upscale_method != WgpuUpscaleMethod::Auto
            || self.active_wgpu_upscale_method() != WgpuUpscaleMethod::Auto
            || self.book_upscale_decision.is_some()
            || self.upscale_probe_failures >= PROBE_FAILURE_LIMIT
            || self.probed_page_ids.len() >= PROBE_PAGES
            || self.probed_page_ids.contains(&key.page_id)
        {
            return;
        }

        let page_id = key.page_id;
        self.probed_page_ids.insert(page_id);
        let generation = self.upscale_probe_generation;
        if !self
            .ensure_upscale_probe_worker()
            .enqueue(generation, page_id, page)
        {
            self.probed_page_ids.remove(&page_id);
        }
    }

    pub(in crate::app) fn drain_upscale_probe_events(&mut self) {
        let events = self
            .upscale_probe_worker
            .as_ref()
            .map(|worker| {
                let mut events = Vec::new();
                while let Some(event) = worker.try_recv() {
                    events.push(event);
                }
                events
            })
            .unwrap_or_default();
        if events.is_empty() {
            return;
        }

        let mut changed = false;
        for event in events {
            if event.generation != self.upscale_probe_generation {
                continue;
            }
            // page_id stays in `probed_page_ids` so the page is never re-probed.
            let _ = event.page_id;
            match event.result {
                Some(result) => {
                    self.upscale_probe_failures = 0;
                    self.probe_page_results.push(result);
                }
                None => {
                    self.upscale_probe_failures = self.upscale_probe_failures.saturating_add(1);
                }
            }
            changed = true;
        }

        if changed && self.book_upscale_decision.is_none() {
            self.maybe_finalize_upscale_decision();
        }
    }

    fn maybe_finalize_upscale_decision(&mut self) {
        let page_count = self.source.as_ref().map_or(0, |source| source.page_count());
        let ready = self.probe_page_results.len() >= PROBE_PAGES
            || (!self.probe_page_results.is_empty() && page_count < PROBE_PAGES);
        if !ready {
            return;
        }

        let count = self.probe_page_results.len() as f64;
        let mean_anime4k = self
            .probe_page_results
            .iter()
            .map(|result| result.ssim_anime4k)
            .sum::<f64>()
            / count;
        let mean_fsr = self
            .probe_page_results
            .iter()
            .map(|result| result.ssim_fsr)
            .sum::<f64>()
            / count;
        let winner = decide(mean_anime4k, mean_fsr);
        self.book_upscale_decision = Some(winner);
        self.egui_ctx.request_repaint();
        self.persist_upscale_decision(winner, mean_anime4k, mean_fsr);
    }

    fn persist_upscale_decision(
        &mut self,
        winner: WgpuUpscaleMethod,
        mean_anime4k: f64,
        mean_fsr: f64,
    ) {
        let Some(book_id) = self
            .source
            .as_ref()
            .map(|source| source.book_id().to_owned())
        else {
            return;
        };
        let record = UpscaleProbeRecord {
            winner: winner.token().to_owned(),
            ssim_anime4k: mean_anime4k as f32,
            ssim_fsr: mean_fsr as f32,
            pages: self.probe_page_results.len().min(u8::MAX as usize) as u8,
            version: UPSCALE_PROBE_VERSION,
        };
        // Ensure the record exists (mirrors the bookmark write path), then attach the probe.
        if let Err(error) = self.write_current_book_record() {
            self.notify_state_save_failed(&error);
            return;
        }
        if let Err(error) = self.store.set_book_upscale_probe(&book_id, record) {
            self.notify_state_save_failed(&error);
        }
    }

    pub(in crate::app) fn book_aware_wgpu_upscale_method(
        &self,
        fallback: WgpuUpscaleMethod,
    ) -> (WgpuUpscaleMethod, UpscaleDecisionOrigin) {
        route_book_upscale(fallback, self.book_upscale_decision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANIME4K: WgpuUpscaleMethod = WgpuUpscaleMethod::WgslAnime4kV32CnnX2M;
    const FSR: WgpuUpscaleMethod = WgpuUpscaleMethod::WgslFsr1EasuRcas;

    #[test]
    fn decide_prefers_higher_mean() {
        assert_eq!(decide(0.6, 0.5), ANIME4K);
        assert_eq!(decide(0.5, 0.6), FSR);
    }

    #[test]
    fn decide_breaks_ties_toward_fsr() {
        assert_eq!(decide(0.5, 0.5), FSR);
        // Gap of 0.0005 is under the tie margin, so FSR still wins.
        assert_eq!(decide(0.5005, 0.5), FSR);
    }

    #[test]
    fn decide_at_exact_margin_is_not_a_tie() {
        // Subtracting from 0.0 is exact, so the gap is exactly PROBE_TIE_MARGIN, which is not
        // strictly less than the margin: the higher mean wins.
        assert_eq!(decide(PROBE_TIE_MARGIN, 0.0), ANIME4K);
        assert_eq!(decide(0.0, PROBE_TIE_MARGIN), FSR);
    }

    #[test]
    fn tile_size_uses_full_tile_for_large_images() {
        assert_eq!(tile_size(2000, 3000), Some(256));
        assert_eq!(tile_size(256, 256), Some(256));
    }

    #[test]
    fn tile_size_shrinks_to_largest_even_square() {
        assert_eq!(tile_size(255, 4000), Some(254));
        assert_eq!(tile_size(64, 100), Some(64));
    }

    #[test]
    fn tile_size_rejects_squares_below_minimum() {
        assert_eq!(tile_size(63, 100), None);
        assert_eq!(tile_size(50, 4000), None);
    }

    #[test]
    fn plan_tiles_places_three_non_overlapping_tiles() {
        let plan = plan_tiles(2000, 3000).expect("large image plans tiles");
        assert_eq!(plan.size, 256);
        assert_eq!(plan.origins, vec![(872, 372), (872, 1372), (872, 2372)]);
    }

    #[test]
    fn plan_tiles_clamps_and_dedupes_on_small_images() {
        // A single-tile image collapses all three band centers onto the same origin.
        let plan = plan_tiles(256, 256).expect("exact-size image plans one tile");
        assert_eq!(plan.origins, vec![(0, 0)]);

        // A short image clamps every origin inside [0, height - size].
        let plan = plan_tiles(300, 300).expect("small image plans tiles");
        assert_eq!(plan.size, 256);
        assert!(plan.origins.iter().all(|&(x, y)| x == 22 && y <= 44));
    }

    #[test]
    fn plan_tiles_rejects_tiny_images() {
        assert!(plan_tiles(50, 4000).is_none());
    }

    #[test]
    fn usable_tiles_skip_blank_tiles() {
        assert_eq!(usable_tile_indices(&[0.5, 0.0, 0.3]), vec![0, 2]);
    }

    #[test]
    fn usable_tiles_fall_back_to_middle_when_all_blank() {
        assert_eq!(usable_tile_indices(&[0.001, 0.0, 0.01]), vec![1]);
        assert_eq!(usable_tile_indices(&[0.0]), vec![0]);
        assert!(usable_tile_indices(&[]).is_empty());
    }

    #[test]
    fn probe_tokens_round_trip_to_methods() {
        assert_eq!(method_for_probe_token(ANIME4K.token()), Some(ANIME4K));
        assert_eq!(method_for_probe_token(FSR.token()), Some(FSR));
        assert_eq!(method_for_probe_token("something_else"), None);
    }

    #[test]
    fn routing_passes_through_non_auto_fallbacks() {
        assert_eq!(
            route_book_upscale(FSR, Some(ANIME4K)),
            (FSR, UpscaleDecisionOrigin::User)
        );
        assert_eq!(
            route_book_upscale(WgpuUpscaleMethod::None, Some(ANIME4K)),
            (WgpuUpscaleMethod::None, UpscaleDecisionOrigin::User)
        );
    }

    #[test]
    fn routing_uses_decision_only_for_auto() {
        assert_eq!(
            route_book_upscale(WgpuUpscaleMethod::Auto, Some(ANIME4K)),
            (ANIME4K, UpscaleDecisionOrigin::ProbeAuto)
        );
        assert_eq!(
            route_book_upscale(WgpuUpscaleMethod::Auto, None),
            (WgpuUpscaleMethod::Auto, UpscaleDecisionOrigin::AutoDefault)
        );
    }
}
