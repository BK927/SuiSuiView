use super::{
    gpu_paint::{GpuPaintRequest, GpuPaintSourceKey},
    page_visual_size, PageCacheKey, PageVisual, SuiSuiViewApp, TextureCacheKey, TextureEntry,
};
use crate::core::effects::ViewEffects;
use crate::core::source::SharedSource;
use crate::core::state::{DisplayUpscaler, ResizeFilter};
use crate::core::worker::{prepare_image_with_options, DecodeOptions, PreparedPage};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use eframe::egui::{self, Align2, Color32, ImageData, Pos2, Rect, TextureOptions, Vec2};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const COMPARE_GAP_POINTS: f32 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct DebugCompareState {
    pub(in crate::app) enabled: bool,
    pub(in crate::app) left: DebugCompareTarget,
    pub(in crate::app) right: DebugCompareTarget,
}

impl Default for DebugCompareState {
    fn default() -> Self {
        Self {
            enabled: false,
            left: DebugCompareTarget::Current,
            right: DebugCompareTarget::Lanczos3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::app) enum DebugCompareTarget {
    Current,
    Bicubic,
    Lanczos3,
    FastTriangle,
    Nearest,
    WgslBilinear,
    WgslFsr1Style,
    WgslFsr1EasuRcas,
    WgslNisStyle,
    AiResult,
}

impl DebugCompareTarget {
    pub(in crate::app) const ALL: [Self; 10] = [
        Self::Current,
        Self::Bicubic,
        Self::Lanczos3,
        Self::FastTriangle,
        Self::Nearest,
        Self::WgslBilinear,
        Self::WgslFsr1Style,
        Self::WgslFsr1EasuRcas,
        Self::WgslNisStyle,
        Self::AiResult,
    ];

    pub(in crate::app) fn label(self) -> &'static str {
        match self {
            Self::Current => "앱 기본",
            Self::Bicubic => "Bicubic",
            Self::Lanczos3 => "Lanczos3",
            Self::FastTriangle => "Fast/Triangle",
            Self::Nearest => "Nearest",
            Self::WgslBilinear => "WGSL Bilinear",
            Self::WgslFsr1Style => "WGSL FSR-style",
            Self::WgslFsr1EasuRcas => "WGSL FSR1 EASU+RCAS",
            Self::WgslNisStyle => "WGSL NIS-style",
            Self::AiResult => "AI 결과",
        }
    }

    fn decode_options(self, current: DecodeOptions) -> Option<DecodeOptions> {
        let resize_filter = match self {
            Self::Current => return Some(current),
            Self::Bicubic => ResizeFilter::Bicubic,
            Self::Lanczos3 => ResizeFilter::Lanczos3,
            Self::FastTriangle => ResizeFilter::FastTriangle,
            Self::Nearest => ResizeFilter::Nearest,
            Self::WgslBilinear
            | Self::WgslFsr1Style
            | Self::WgslFsr1EasuRcas
            | Self::WgslNisStyle => return Some(current),
            Self::AiResult => return None,
        };
        Some(DecodeOptions {
            resize_filter,
            ..current
        })
    }

    fn display_upscaler(self) -> Option<DisplayUpscaler> {
        match self {
            Self::WgslBilinear => Some(DisplayUpscaler::WgslBilinear),
            Self::WgslFsr1Style => Some(DisplayUpscaler::WgslFsr1Style),
            Self::WgslFsr1EasuRcas => Some(DisplayUpscaler::WgslFsr1EasuRcas),
            Self::WgslNisStyle => Some(DisplayUpscaler::WgslNisStyle),
            _ => None,
        }
    }
}

pub(in crate::app) struct DebugCompareWorker {
    command_tx: Sender<DebugCompareCommand>,
    event_rx: Receiver<DebugCompareEvent>,
    shutdown_requested: Arc<AtomicBool>,
    stopped_rx: Receiver<()>,
    join: Option<JoinHandle<()>>,
}

impl DebugCompareWorker {
    pub(in crate::app) fn new(ctx: egui::Context) -> Self {
        let (command_tx, command_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        let (stopped_tx, stopped_rx) = bounded(1);
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let worker_shutdown_requested = shutdown_requested.clone();
        let join = thread::Builder::new()
            .name("suisuiview-debug-compare-worker".to_owned())
            .spawn(move || {
                run_debug_compare_worker(command_rx, event_tx, ctx, worker_shutdown_requested);
                let _ = stopped_tx.send(());
            })
            .expect("debug compare worker thread should start");

        Self {
            command_tx,
            event_rx,
            shutdown_requested,
            stopped_rx,
            join: Some(join),
        }
    }

    fn prepare(&self, request: DebugCompareRequest) {
        let _ = self.command_tx.send(DebugCompareCommand::Prepare(request));
    }

    pub(in crate::app) fn try_recv(&self) -> Option<DebugCompareEvent> {
        self.event_rx.try_recv().ok()
    }

    pub(in crate::app) fn request_shutdown(&mut self) -> bool {
        if self.shutdown_requested.swap(true, Ordering::AcqRel) {
            return self.join.is_none();
        }
        let sent = self.command_tx.send(DebugCompareCommand::Shutdown).is_ok();
        let had_thread = self.join.take().is_some();
        let stopped = self
            .stopped_rx
            .recv_timeout(Duration::from_millis(30))
            .is_ok();
        sent && (!had_thread || stopped)
    }
}

impl Drop for DebugCompareWorker {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

struct DebugCompareRequest {
    book_id: String,
    source: SharedSource,
    page_index: usize,
    target_long_edge: u32,
    decode: DecodeOptions,
}

pub(in crate::app) struct DebugCompareEvent {
    book_id: String,
    page_index: usize,
    target_long_edge: u32,
    decode: DecodeOptions,
    result: Result<Arc<PreparedPage>, String>,
}

enum DebugCompareCommand {
    Prepare(DebugCompareRequest),
    Shutdown,
}

impl SuiSuiViewApp {
    pub(in crate::app) fn drain_debug_compare_events(&mut self) {
        while let Some(event) = self.debug_compare_worker.try_recv() {
            let key = PageCacheKey {
                index: event.page_index,
                target_long_edge: event.target_long_edge,
                decode: event.decode,
            };
            self.debug_compare_inflight.remove(&key);

            if self.book_id.as_deref() != Some(event.book_id.as_str())
                || !self.is_relevant_debug_compare_key(key)
            {
                continue;
            }

            match event.result {
                Ok(page) => {
                    if let Some(notice) = page.notice.as_ref() {
                        self.set_status(notice.clone());
                    }
                    self.page_errors.remove(&event.page_index);
                    self.insert_prepared_page(key, page);
                    self.prune_decoded_cache();
                }
                Err(message) if self.debug_compare.enabled => {
                    self.set_status(format!(
                        "비교 이미지 준비 실패 p.{}: {message}",
                        event.page_index + 1
                    ));
                }
                Err(_) => {}
            }
        }
    }

    pub(in crate::app) fn set_debug_compare_enabled(&mut self, enabled: bool) {
        if self.debug_compare.enabled == enabled {
            return;
        }
        self.debug_compare.enabled = enabled;
        self.transition = None;
        self.pan = Vec2::ZERO;
        if enabled {
            self.set_status("디버그 좌우 비교 모드가 켜졌습니다.");
        } else {
            self.debug_compare_inflight.clear();
            self.set_status("디버그 좌우 비교 모드가 꺼졌습니다.");
        }
    }

    pub(in crate::app) fn paint_debug_compare(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        viewport: Rect,
    ) {
        let divider_x = viewport.center().x;
        let left = Rect::from_min_max(
            viewport.min,
            Pos2::new(divider_x - COMPARE_GAP_POINTS * 0.5, viewport.max.y),
        );
        let right = Rect::from_min_max(
            Pos2::new(divider_x + COMPARE_GAP_POINTS * 0.5, viewport.min.y),
            viewport.max,
        );

        painter.line_segment(
            [
                Pos2::new(divider_x, viewport.top()),
                Pos2::new(divider_x, viewport.bottom()),
            ],
            egui::Stroke::new(1.0, Color32::from_gray(70)),
        );
        self.paint_compare_target(ctx, painter, left, self.debug_compare.left, "A");
        self.paint_compare_target(ctx, painter, right, self.debug_compare.right, "B");
    }

    fn paint_compare_target(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        viewport: Rect,
        target: DebugCompareTarget,
        side_label: &str,
    ) {
        let visual = self.compare_visual(ctx, target);
        let natural = page_visual_size(&visual);
        let scale = self.scale_for(viewport.size(), natural);
        let page_size = natural * scale;
        let page_rect =
            Rect::from_min_size(self.spread_origin(viewport, page_size, self.pan), page_size);
        let tint = Color32::WHITE;

        match visual {
            PageVisual::Ready { texture, .. } => {
                painter.image(
                    texture.id(),
                    page_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    tint,
                );
            }
            PageVisual::ReadyGpu {
                source_key,
                image_size,
                rgba,
                effects,
                display_upscaler,
                ..
            } => {
                if !self.paint_wgsl_effects(
                    painter,
                    GpuPaintRequest {
                        rect: page_rect,
                        source_key,
                        image_size,
                        rgba,
                        effects,
                        display_upscaler,
                        opacity: 1.0,
                    },
                ) {
                    self.paint_placeholder(
                        painter,
                        page_rect,
                        "WGSL 비교 경로를 사용할 수 없습니다.",
                        Color32::from_gray(120),
                        tint,
                    );
                }
            }
            PageVisual::Loading { index } => {
                self.paint_placeholder(
                    painter,
                    page_rect,
                    &format!("Preparing page {}", index + 1),
                    Color32::from_gray(120),
                    tint,
                );
            }
            PageVisual::Failed { index, message } => {
                self.paint_placeholder(
                    painter,
                    page_rect,
                    &format!("Page {} unavailable\n{}", index + 1, message),
                    Color32::from_rgb(180, 80, 80),
                    tint,
                );
            }
        }

        self.paint_compare_label(painter, viewport, side_label, target);
    }

    fn paint_compare_label(
        &self,
        painter: &egui::Painter,
        viewport: Rect,
        side_label: &str,
        target: DebugCompareTarget,
    ) {
        let label = format!("{side_label}: {}", target.label());
        let pos = viewport.left_top() + egui::vec2(12.0, 10.0);
        let rect = Rect::from_min_size(pos - egui::vec2(7.0, 5.0), egui::vec2(238.0, 30.0));
        painter.rect_filled(rect, 4.0, Color32::from_black_alpha(150));
        painter.text(
            pos,
            Align2::LEFT_TOP,
            label,
            egui::FontId::proportional(14.0),
            Color32::WHITE,
        );
    }

    fn compare_visual(&mut self, ctx: &egui::Context, target: DebugCompareTarget) -> PageVisual {
        if let Some(display_upscaler) = target.display_upscaler() {
            return self.compare_wgsl_visual(display_upscaler);
        }
        match target {
            DebugCompareTarget::AiResult => self.compare_ai_visual(ctx),
            _ => self.compare_decoded_visual(ctx, target),
        }
    }

    fn compare_wgsl_visual(&mut self, display_upscaler: DisplayUpscaler) -> PageVisual {
        if !self.gpu_effects_available || self.gpu_target_format.is_none() {
            return PageVisual::Failed {
                index: self.current_page,
                message: "WGSL 업스케일러를 사용할 수 없습니다.".to_owned(),
            };
        }
        let requested = PageCacheKey {
            index: self.current_page,
            target_long_edge: self.target_long_edge,
            decode: self.decode_options(),
        };
        let Some(best_key) = self.best_page_key(requested) else {
            self.request_debug_compare_page(requested);
            return PageVisual::Loading {
                index: self.current_page,
            };
        };
        let page = self
            .decoded_pages
            .get(&best_key)
            .cloned()
            .expect("compare page key should exist in decoded cache");
        PageVisual::ReadyGpu {
            source_key: GpuPaintSourceKey {
                book: self.gpu_paint_book_key(),
                page: best_key,
                upscaled: false,
                generation: 0,
            },
            image_size: page.image_size(),
            rgba: page.rgba.clone(),
            size: page_natural_size(&page),
            effects: ViewEffects::default(),
            display_upscaler,
        }
    }

    fn compare_ai_visual(&mut self, ctx: &egui::Context) -> PageVisual {
        let requested = PageCacheKey {
            index: self.current_page,
            target_long_edge: self.target_long_edge,
            decode: self.decode_options(),
        };
        let Some(best_key) = self.best_upscaled_page_key(requested) else {
            return PageVisual::Failed {
                index: self.current_page,
                message: "AI 업스케일 결과가 없습니다. 보정 > AI x4로 먼저 생성하세요.".to_owned(),
            };
        };
        self.compare_ready_visual(ctx, best_key, true)
    }

    fn compare_decoded_visual(
        &mut self,
        ctx: &egui::Context,
        target: DebugCompareTarget,
    ) -> PageVisual {
        let Some(decode) = target.decode_options(self.decode_options()) else {
            return PageVisual::Loading {
                index: self.current_page,
            };
        };
        let requested = PageCacheKey {
            index: self.current_page,
            target_long_edge: self.target_long_edge,
            decode,
        };
        let Some(best_key) = self.best_page_key(requested) else {
            if target != DebugCompareTarget::Current {
                self.request_debug_compare_page(requested);
            }
            return PageVisual::Loading {
                index: self.current_page,
            };
        };
        self.compare_ready_visual(ctx, best_key, false)
    }

    fn compare_ready_visual(
        &mut self,
        ctx: &egui::Context,
        best_key: PageCacheKey,
        upscaled: bool,
    ) -> PageVisual {
        let texture_key = TextureCacheKey {
            page: best_key,
            effects: ViewEffects::default(),
            upscaled,
        };
        let page = if upscaled {
            self.upscaled_pages.get(&best_key)
        } else {
            self.decoded_pages.get(&best_key)
        }
        .cloned()
        .expect("compare page key should exist in cache");

        if let Some(texture) = self
            .textures
            .get(&texture_key)
            .map(|entry| entry.texture.clone())
        {
            return PageVisual::Ready {
                texture,
                size: page_natural_size(&page),
            };
        }

        let texture = ctx.load_texture(
            format!(
                "compare-page-{}-{}-{}",
                best_key.index,
                best_key.target_long_edge,
                if upscaled { "ai" } else { "base" }
            ),
            ImageData::Color(Arc::new(page.color_image())),
            TextureOptions::LINEAR,
        );
        self.textures.put(
            texture_key,
            TextureEntry {
                texture: texture.clone(),
            },
        );

        PageVisual::Ready {
            texture,
            size: page_natural_size(&page),
        }
    }

    fn request_debug_compare_page(&mut self, key: PageCacheKey) {
        if self.debug_compare_inflight.contains(&key) {
            return;
        }
        let Some(source) = self.source.as_ref().cloned() else {
            return;
        };
        let Some(book_id) = self.book_id.clone() else {
            return;
        };
        self.debug_compare_inflight.insert(key);
        self.debug_compare_worker.prepare(DebugCompareRequest {
            book_id,
            source,
            page_index: key.index,
            target_long_edge: key.target_long_edge,
            decode: key.decode,
        });
    }

    pub(in crate::app) fn debug_compare_pin_keys(&self) -> Vec<PageCacheKey> {
        if !self.debug_compare.enabled {
            return Vec::new();
        }
        [self.debug_compare.left, self.debug_compare.right]
            .into_iter()
            .filter_map(|target| target.decode_options(self.decode_options()))
            .map(|decode| PageCacheKey {
                index: self.current_page,
                target_long_edge: self.target_long_edge,
                decode,
            })
            .collect()
    }

    fn is_relevant_debug_compare_key(&self, key: PageCacheKey) -> bool {
        self.debug_compare.enabled
            && key.index == self.current_page
            && self.target_is_relevant(key.target_long_edge)
            && self.debug_compare_pin_keys().contains(&key)
    }
}

fn page_natural_size(page: &PreparedPage) -> Vec2 {
    Vec2::new(page.original_width as f32, page.original_height as f32)
}

fn run_debug_compare_worker(
    command_rx: Receiver<DebugCompareCommand>,
    event_tx: Sender<DebugCompareEvent>,
    ctx: egui::Context,
    shutdown_requested: Arc<AtomicBool>,
) {
    while !shutdown_requested.load(Ordering::Acquire) {
        let Ok(command) = command_rx.recv() else {
            break;
        };
        let request = match command {
            DebugCompareCommand::Prepare(request) => request,
            DebugCompareCommand::Shutdown => break,
        };
        let result = request
            .source
            .read_page(request.page_index)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                prepare_image_with_options(&bytes, request.target_long_edge, request.decode)
            })
            .map(Arc::new);
        let _ = event_tx.send(DebugCompareEvent {
            book_id: request.book_id,
            page_index: request.page_index,
            target_long_edge: request.target_long_edge,
            decode: request.decode,
            result,
        });
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::DebugCompareTarget;
    use crate::core::state::ResizeFilter;
    use crate::core::worker::{DecodeOptions, DecodeStrategy};

    #[test]
    fn compare_targets_override_only_resize_filter() {
        let current = DecodeOptions {
            strategy: DecodeStrategy::ImageCrate,
            resize_filter: ResizeFilter::Nearest,
            allow_display_upscale: true,
            apply_exif_orientation: true,
            apply_embedded_icc: true,
        };

        let lanczos = DebugCompareTarget::Lanczos3
            .decode_options(current)
            .unwrap();

        assert_eq!(lanczos.resize_filter, ResizeFilter::Lanczos3);
        assert_eq!(lanczos.strategy, DecodeStrategy::ImageCrate);
        assert!(lanczos.allow_display_upscale);
        assert!(lanczos.apply_exif_orientation);
        assert!(lanczos.apply_embedded_icc);
    }

    #[test]
    fn ai_compare_target_uses_upscaled_cache_not_decode_options() {
        assert!(DebugCompareTarget::AiResult
            .decode_options(DecodeOptions::default())
            .is_none());
    }
}
