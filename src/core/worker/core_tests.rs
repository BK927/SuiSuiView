use super::{
    clamp_navigation_target_long_edge, clamp_target_long_edge, display_dimensions,
    display_dimensions_with_upscale, prepare_image, prepare_image_with_options,
    prepare_image_with_strategy, prepare_unavailable_or_image_fallback, run_worker, CachedPageKey,
    DecodeBackend, DecodeOptions, DecodeStrategy, NavigationDirection, WorkerCommand, WorkerEvent,
    WorkerOptions, MAX_ORIGINAL_TARGET_LONG_EDGE, MAX_TARGET_LONG_EDGE,
};
use crate::core::source::{BookSource, SharedSource, SourceError};
use crate::core::state::{CpuScaleFilter, DecoderPreference, DecoderPreferences};
use crossbeam_channel::unbounded;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

#[test]
fn damaged_image_returns_error_instead_of_panicking() {
    assert!(prepare_image(b"not-an-image", 2048).is_err());
}

#[test]
fn supported_formats_decode_to_prepared_pages() {
    for format in [
        ImageFormat::Jpeg,
        ImageFormat::Png,
        ImageFormat::WebP,
        ImageFormat::Bmp,
        ImageFormat::Gif,
    ] {
        let bytes = encoded_test_image(format);
        let page = prepare_image(&bytes, 1024).unwrap();
        assert_eq!(page.original_width, 48);
        assert_eq!(page.original_height, 32);
        assert_eq!(page.display_width, 48);
        assert_eq!(page.display_height, 32);
    }
}

#[test]
fn decode_options_select_scale_filter_by_direction() {
    let options = DecodeOptions {
        cpu_upscale_filter: CpuScaleFilter::Lanczos3,
        cpu_downscale_filter: CpuScaleFilter::Hamming,
        ..DecodeOptions::default()
    };

    assert_eq!(
        options.scale_filter_for(800, 600, 400, 300),
        CpuScaleFilter::Hamming
    );
    assert_eq!(
        options.scale_filter_for(800, 600, 1200, 900),
        CpuScaleFilter::Lanczos3
    );
}

#[test]
fn decode_cache_token_tracks_cpu_upscale_filter_only_when_allowed() {
    let normal = DecodeOptions::default().cache_token();
    let changed_upscaler = DecodeOptions {
        cpu_upscale_filter: CpuScaleFilter::Lanczos3,
        ..DecodeOptions::default()
    }
    .cache_token();
    let allowed_upscaler = DecodeOptions {
        cpu_upscale_filter: CpuScaleFilter::Lanczos3,
        allow_display_upscale: true,
        ..DecodeOptions::default()
    }
    .cache_token();
    let conservative_prepare = DecodeOptions {
        fast_sampled_scaled_decode: false,
        ..DecodeOptions::default()
    }
    .cache_token();

    assert_eq!(normal, changed_upscaler);
    assert_ne!(normal, allowed_upscaler);
    assert_ne!(normal, conservative_prepare);
}

#[test]
fn prepared_page_retains_single_rgba_buffer_budget() {
    let bytes = encoded_test_image(ImageFormat::Png);
    let page = prepare_image(&bytes, 1024).unwrap();

    assert_eq!(page.image_size(), [48, 32]);
    assert_eq!(page.pixels.byte_len(), 48 * 32 * 4);
    assert_eq!(page.byte_size, page.pixels.byte_len());
    assert_eq!(page.color_image().size, [48, 32]);
}

#[test]
fn auto_strategy_uses_scaled_decode_for_large_jpegs() {
    let bytes = encoded_sized_test_image(ImageFormat::Jpeg, 2304, 1536);
    let page = prepare_image_with_strategy(&bytes, 1024, DecodeStrategy::Auto).unwrap();

    assert_eq!(page.decode_backend, DecodeBackend::JpegScaled);
    assert_eq!(page.original_width, 2304);
    assert_eq!(page.original_height, 1536);
    assert_eq!(page.display_width, 1024);
    assert_eq!(page.display_height, 683);
}

#[test]
fn image_crate_strategy_keeps_baseline_decode_for_large_jpegs() {
    let bytes = encoded_sized_test_image(ImageFormat::Jpeg, 2304, 1536);
    let page = prepare_image_with_strategy(&bytes, 1024, DecodeStrategy::ImageCrate).unwrap();

    assert_eq!(page.decode_backend, DecodeBackend::ImageCrate);
    assert_eq!(page.display_width, 1024);
    assert_eq!(page.display_height, 683);
}

#[test]
fn fast_sampled_scaled_toggle_disables_large_jpeg_scaled_decode() {
    let bytes = encoded_sized_test_image(ImageFormat::Jpeg, 2304, 1536);
    let page = prepare_image_with_options(
        &bytes,
        1024,
        DecodeOptions {
            fast_sampled_scaled_decode: false,
            ..DecodeOptions::default()
        },
    )
    .unwrap();

    assert_ne!(page.decode_backend, DecodeBackend::JpegScaled);
    assert_eq!(page.display_width, 1024);
    assert_eq!(page.display_height, 683);
}

#[test]
fn auto_strategy_default_preferences_resolve_to_benchmark_winners() {
    let jpeg = encoded_test_image(ImageFormat::Jpeg);
    let jpeg_page = prepare_image_with_options(&jpeg, 1024, DecodeOptions::default()).unwrap();
    assert_eq!(jpeg_page.decode_backend, DecodeBackend::ZuneJpeg);

    let png = encoded_test_image(ImageFormat::Png);
    let png_page = prepare_image_with_options(&png, 1024, DecodeOptions::default()).unwrap();
    assert_eq!(png_page.decode_backend, DecodeBackend::PngCrate);

    let gif = encoded_test_image(ImageFormat::Gif);
    let gif_page = prepare_image_with_options(&gif, 1024, DecodeOptions::default()).unwrap();
    assert_eq!(gif_page.decode_backend, DecodeBackend::GifCrate);

    let bmp = encoded_test_image(ImageFormat::Bmp);
    let bmp_page = prepare_image_with_options(&bmp, 1024, DecodeOptions::default()).unwrap();
    assert_eq!(bmp_page.decode_backend, DecodeBackend::BmpFastPath);
}

#[test]
fn image_crate_strategy_ignores_format_preferences() {
    let bytes = encoded_test_image(ImageFormat::Jpeg);
    let page = prepare_image_with_options(
        &bytes,
        1024,
        DecodeOptions {
            strategy: DecodeStrategy::ImageCrate,
            decoder_preferences: DecoderPreferences {
                jpeg: DecoderPreference::ZuneJpeg,
                ..DecoderPreferences::default()
            },
            ..DecodeOptions::default()
        },
    )
    .unwrap();

    assert_eq!(page.decode_backend, DecodeBackend::ImageCrate);
}

#[test]
fn unavailable_selected_backend_falls_back_with_notice() {
    let bytes = encoded_test_image(ImageFormat::Png);
    let page = prepare_unavailable_or_image_fallback(
        &bytes,
        1024,
        DecodeOptions::default(),
        DecodeBackend::LibWebp,
        "backend not enabled",
    )
    .unwrap();

    assert_eq!(page.decode_backend, DecodeBackend::ImageCrate);
    let notice = page.notice.as_deref().unwrap_or_default();
    assert!(notice.contains("libwebp"));
    assert!(notice.contains("used image fallback"));
}

#[test]
fn auto_strategy_samples_large_uncompressed_bmps() {
    let bytes = encoded_sized_test_image(ImageFormat::Bmp, 2048, 16);
    let page = prepare_image_with_strategy(&bytes, 1024, DecodeStrategy::Auto).unwrap();

    assert_eq!(page.decode_backend, DecodeBackend::BmpSampled);
    assert_eq!(page.original_width, 2048);
    assert_eq!(page.original_height, 16);
    assert_eq!(page.display_width, 1024);
    assert_eq!(page.display_height, 8);
}

#[test]
fn image_crate_strategy_keeps_baseline_decode_for_large_bmps() {
    let bytes = encoded_sized_test_image(ImageFormat::Bmp, 2048, 16);
    let page = prepare_image_with_strategy(&bytes, 1024, DecodeStrategy::ImageCrate).unwrap();

    assert_eq!(page.decode_backend, DecodeBackend::ImageCrate);
    assert_eq!(page.display_width, 1024);
    assert_eq!(page.display_height, 8);
}

#[test]
fn fast_sampled_scaled_toggle_disables_large_bmp_sampling() {
    let bytes = encoded_sized_test_image(ImageFormat::Bmp, 2048, 16);
    let page = prepare_image_with_options(
        &bytes,
        1024,
        DecodeOptions {
            fast_sampled_scaled_decode: false,
            ..DecodeOptions::default()
        },
    )
    .unwrap();

    assert_ne!(page.decode_backend, DecodeBackend::BmpSampled);
    assert_eq!(page.display_width, 1024);
    assert_eq!(page.display_height, 8);
}

#[test]
fn auto_strategy_samples_large_static_gifs() {
    let bytes = encoded_sized_test_image(ImageFormat::Gif, 2048, 16);
    let page = prepare_image_with_strategy(&bytes, 1024, DecodeStrategy::Auto).unwrap();

    assert_eq!(page.decode_backend, DecodeBackend::GifSampled);
    assert_eq!(page.original_width, 2048);
    assert_eq!(page.original_height, 16);
    assert_eq!(page.display_width, 1024);
    assert_eq!(page.display_height, 8);
}

#[test]
fn image_crate_strategy_keeps_baseline_decode_for_large_gifs() {
    let bytes = encoded_sized_test_image(ImageFormat::Gif, 2048, 16);
    let page = prepare_image_with_strategy(&bytes, 1024, DecodeStrategy::ImageCrate).unwrap();

    assert_eq!(page.decode_backend, DecodeBackend::ImageCrate);
    assert_eq!(page.display_width, 1024);
    assert_eq!(page.display_height, 8);
}

#[test]
fn fast_sampled_scaled_toggle_disables_large_gif_sampling() {
    let bytes = encoded_sized_test_image(ImageFormat::Gif, 2048, 16);
    let page = prepare_image_with_options(
        &bytes,
        1024,
        DecodeOptions {
            fast_sampled_scaled_decode: false,
            ..DecodeOptions::default()
        },
    )
    .unwrap();

    assert_ne!(page.decode_backend, DecodeBackend::GifSampled);
    assert_eq!(page.display_width, 1024);
    assert_eq!(page.display_height, 8);
}

#[test]
fn auto_strategy_samples_large_pngs() {
    let bytes = encoded_sized_test_image(ImageFormat::Png, 2048, 16);
    let page = prepare_image_with_strategy(&bytes, 1024, DecodeStrategy::Auto).unwrap();

    assert_eq!(page.decode_backend, DecodeBackend::PngSampled);
    assert_eq!(page.original_width, 2048);
    assert_eq!(page.original_height, 16);
    assert_eq!(page.display_width, 1024);
    assert_eq!(page.display_height, 8);
}

#[test]
fn image_crate_strategy_keeps_baseline_decode_for_large_pngs() {
    let bytes = encoded_sized_test_image(ImageFormat::Png, 2048, 16);
    let page = prepare_image_with_strategy(&bytes, 1024, DecodeStrategy::ImageCrate).unwrap();

    assert_eq!(page.decode_backend, DecodeBackend::ImageCrate);
    assert_eq!(page.display_width, 1024);
    assert_eq!(page.display_height, 8);
}

#[test]
fn fast_sampled_scaled_toggle_disables_large_png_sampling() {
    let bytes = encoded_sized_test_image(ImageFormat::Png, 2048, 16);
    let page = prepare_image_with_options(
        &bytes,
        1024,
        DecodeOptions {
            fast_sampled_scaled_decode: false,
            ..DecodeOptions::default()
        },
    )
    .unwrap();

    assert_ne!(page.decode_backend, DecodeBackend::PngSampled);
    assert_eq!(page.display_width, 1024);
    assert_eq!(page.display_height, 8);
}

#[test]
fn display_dimensions_preserve_ratio_and_do_not_upscale() {
    assert_eq!(display_dimensions(800, 600, 2048).unwrap(), (800, 600));
    assert_eq!(display_dimensions(8000, 4000, 2000).unwrap(), (2000, 1000));
    assert_eq!(
        display_dimensions(3000, 9000, MAX_TARGET_LONG_EDGE + 500).unwrap(),
        (1532, 4596)
    );
    assert_eq!(
        clamp_target_long_edge(MAX_ORIGINAL_TARGET_LONG_EDGE + 500),
        MAX_ORIGINAL_TARGET_LONG_EDGE
    );
}

#[test]
fn navigation_target_clamp_keeps_display_path_capped() {
    assert_eq!(
        clamp_navigation_target_long_edge(MAX_TARGET_LONG_EDGE + 500),
        MAX_TARGET_LONG_EDGE
    );
}

#[test]
fn display_dimensions_can_upscale_for_fit_modes() {
    assert_eq!(
        display_dimensions_with_upscale(640, 320, 2048, true).unwrap(),
        (2048, 1024)
    );
    assert_eq!(
        display_dimensions_with_upscale(640, 320, 2048, false).unwrap(),
        (640, 320)
    );
}

#[test]
fn worker_publishes_completed_page_before_handling_queued_command() {
    let (command_tx, command_rx) = unbounded();
    let (event_tx, event_rx) = unbounded();
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let worker_shutdown = shutdown_requested.clone();
    let source: SharedSource = Arc::new(CommandingSource {
        command_tx: command_tx.clone(),
        sent_command: AtomicBool::new(false),
        page_bytes: encoded_test_image(ImageFormat::Png),
        path: PathBuf::from("commanding-source"),
    });
    let handle = thread::spawn(move || {
        run_worker(
            command_rx,
            event_tx,
            egui::Context::default(),
            worker_shutdown,
        );
    });
    command_tx
        .send(WorkerCommand::LoadBook {
            source,
            center: 0,
            direction: NavigationDirection::Forward,
            target_long_edge: 2048,
            visible_pages: 1,
            options: WorkerOptions::default(),
        })
        .unwrap();
    let first_event = event_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    match first_event {
        WorkerEvent::PageReady { index, .. } => assert_eq!(index, 0),
        WorkerEvent::PageFailed { message, .. } => panic!("page failed: {message}"),
    }

    shutdown_requested.store(true, Ordering::Release);
    let _ = command_tx.send(WorkerCommand::Shutdown);
    handle.join().unwrap();
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    assert!(crate::core::perf_trace::flush_timeout(Duration::from_secs(
        1
    )));
}

#[test]
fn cached_page_key_covers_same_page_decode_and_sufficient_size() {
    let decode = DecodeOptions::default();
    let key = CachedPageKey::new(3, 2048, decode);

    assert!(key.covers(3, 1024, decode));
    assert!(key.covers(3, 2048, decode));
    assert!(!key.covers(3, 4096, decode));
    assert!(!key.covers(4, 1024, decode));
    assert!(!key.covers(
        3,
        1024,
        DecodeOptions {
            apply_embedded_icc: true,
            ..decode
        }
    ));

    let original_key = CachedPageKey::new(3, MAX_TARGET_LONG_EDGE + 1, decode);
    assert!(original_key.covers(3, MAX_TARGET_LONG_EDGE + 1, decode));
    assert!(!original_key.covers(3, MAX_TARGET_LONG_EDGE, decode));
}

fn encoded_test_image(format: ImageFormat) -> Vec<u8> {
    encoded_sized_test_image(format, 48, 32)
}

fn encoded_sized_test_image(format: ImageFormat, width: u32, height: u32) -> Vec<u8> {
    let image = RgbImage::from_fn(width, height, |x, y| {
        Rgb([
            ((x * 3 + y) % 255) as u8,
            ((x + y * 5) % 255) as u8,
            ((x * 7 + y * 11) % 255) as u8,
        ])
    });
    let mut bytes = Vec::new();
    DynamicImage::ImageRgb8(image)
        .write_to(&mut Cursor::new(&mut bytes), format)
        .unwrap();
    bytes
}

struct CommandingSource {
    command_tx: crossbeam_channel::Sender<WorkerCommand>,
    sent_command: AtomicBool,
    page_bytes: Vec<u8>,
    path: PathBuf,
}

impl BookSource for CommandingSource {
    fn title(&self) -> &str {
        "commanding"
    }

    fn source_path(&self) -> &Path {
        &self.path
    }

    fn book_id(&self) -> &str {
        "commanding"
    }

    fn page_count(&self) -> usize {
        2
    }

    fn page_name(&self, index: usize) -> Option<&str> {
        match index {
            0 => Some("page-0000.png"),
            1 => Some("page-0001.png"),
            _ => None,
        }
    }

    fn read_page(&self, index: usize) -> Result<Vec<u8>, SourceError> {
        if index == 0 && !self.sent_command.swap(true, Ordering::AcqRel) {
            self.command_tx
                .send(WorkerCommand::SetPage {
                    center: 1,
                    direction: NavigationDirection::Forward,
                    target_long_edge: 2048,
                    visible_pages: 1,
                    options: WorkerOptions::default(),
                })
                .unwrap();
        }
        Ok(self.page_bytes.clone())
    }
}
