use super::*;

/// Capture renderer capabilities on the UI thread, then resolve the destination
/// book's view after its reading position is loaded on the source thread.
pub(in crate::app) struct OpenSeedPlan {
    fit_decode: DecodeOptions,
    viewport: Option<Vec2>,
    session_view_mode: ViewMode,
    pixels_per_point: f32,
}

impl SuiSuiViewApp {
    pub(in crate::app) fn open_seed_plan(&self) -> OpenSeedPlan {
        OpenSeedPlan {
            fit_decode: self.decode_options_for_fit(FitMode::FitPage),
            viewport: self.last_viewer_size_points,
            session_view_mode: self.view_mode,
            pixels_per_point: self.egui_ctx.pixels_per_point(),
        }
    }
}

impl OpenSeedPlan {
    pub(in crate::app) fn resolve(
        &self,
        settings: &AppSettings,
        reading_position: Option<&ReadingPosition>,
        view_fallback: Option<OpenViewFallback>,
    ) -> (DecodeOptions, Option<SeedTargetView>) {
        let view = resolve_open_view(
            reading_position,
            view_fallback,
            settings.remember_zoom_per_book,
        );
        let mut decode = self.fit_decode;
        if matches!(view.fit_mode, FitMode::Manual | FitMode::Original) {
            decode.allow_display_upscale = false;
        }
        let target_view = self.viewport.map(|viewport| {
            let mode = view.view_mode.unwrap_or(self.session_view_mode);
            let page_viewport = if mode.step() <= 1 {
                viewport
            } else {
                Vec2::new((viewport.x - SPREAD_GAP_POINTS).max(1.0) * 0.5, viewport.y)
            };
            SeedTargetView {
                fit_mode: view.fit_mode,
                manual_zoom: view.manual_zoom,
                page_viewport,
                pixels_per_point: self.pixels_per_point,
            }
        });
        (decode, target_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::worker_events::decode_options_from_settings;
    use crate::core::source::PageId;
    use crate::core::worker::CachedPageKey;

    fn plan(settings: &AppSettings, allow_display_upscale: bool) -> OpenSeedPlan {
        OpenSeedPlan {
            fit_decode: decode_options_from_settings(settings, allow_display_upscale),
            viewport: Some(Vec2::new(1600.0, 1000.0)),
            session_view_mode: ViewMode::Single,
            pixels_per_point: 1.5,
        }
    }

    #[test]
    fn fit_seed_matches_live_cache_policy_for_both_enlargement_owners() {
        let settings = AppSettings::default();
        // WGPU without a display upscaler uses CPU enlargement; a healthy Glow
        // kernel (or WGPU display upscaler) owns enlargement at draw time.
        for allow in [true, false] {
            let (decode, _) = plan(&settings, allow).resolve(&settings, None, None);
            let live_decode = decode_options_from_settings(&settings, allow);
            let cached = CachedPageKey::new(PageId(0), 1458, decode);
            assert_eq!(cached, CachedPageKey::new(PageId(0), 1458, live_decode));
            // Keep rejecting incompatible policy changes; do not loosen keys.
            assert_ne!(
                cached,
                CachedPageKey::new(
                    PageId(0),
                    1458,
                    decode_options_from_settings(&settings, !allow)
                )
            );
        }
    }

    #[test]
    fn destination_saved_view_overrides_the_previous_books_fit_and_spread() {
        let settings = AppSettings {
            remember_zoom_per_book: true,
            ..AppSettings::default()
        };
        let position = ReadingPosition {
            last_page: 0,
            last_page_name: None,
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::Manual,
            manual_zoom: Some(2.0),
            view_mode: Some(ViewMode::DoubleLeftToRight.token().to_owned()),
            strip_offset_frac: None,
            smart_spread_phase: 0,
            updated_at: 0,
        };
        let fallback = OpenViewFallback {
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitPage,
            manual_zoom: 1.0,
            view_mode: ViewMode::Single,
        };
        let (decode, view) =
            plan(&settings, true).resolve(&settings, Some(&position), Some(fallback));
        assert!(!decode.allow_display_upscale);
        let view = view.unwrap();
        assert_eq!(view.fit_mode, FitMode::Manual);
        assert_eq!(view.manual_zoom, 2.0);
        assert_eq!(view.page_viewport.x, (1600.0 - SPREAD_GAP_POINTS) * 0.5);
        assert_eq!(view.pixels_per_point, 1.5);
    }

    #[test]
    fn inspection_fallback_keeps_native_pixels_without_a_known_viewport() {
        let settings = AppSettings::default();
        for fit_mode in [FitMode::Original, FitMode::Manual] {
            let mut plan = plan(&settings, true);
            plan.viewport = None;
            let (decode, view) = plan.resolve(
                &settings,
                None,
                Some(OpenViewFallback {
                    reading_direction: ReadingDirection::LeftToRight,
                    fit_mode,
                    manual_zoom: 3.0,
                    view_mode: ViewMode::Single,
                }),
            );
            assert!(!decode.allow_display_upscale);
            assert!(view.is_none());
        }
    }
}
