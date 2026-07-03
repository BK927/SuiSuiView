use std::path::PathBuf;
use std::time::Instant;

use suisuiview::core::source::open_source_from_path;
use suisuiview::core::state::{DecodeMode, DecoderPreferences, StateStore};
use suisuiview::core::worker::{prepare_image_with_options, DecodeOptions, DecodeStrategy};

use super::wgpu_worker::elapsed_ms;

pub(crate) const DEFAULT_TARGET_LONG_EDGE: u32 = 2048;

#[derive(Clone, Debug)]
pub(crate) struct PreparedImageReport {
    pub(crate) worker_started_ms: f64,
    pub(crate) open_source_ms: Option<f64>,
    pub(crate) read_page_ms: Option<f64>,
    pub(crate) prepare_ms: Option<f64>,
    pub(crate) page_index: Option<usize>,
    pub(crate) page_count: Option<usize>,
    pub(crate) page_name: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) original_size: Option<[usize; 2]>,
    pub(crate) display_size: Option<[usize; 2]>,
    pub(crate) target_long_edge: u32,
    pub(crate) decode_backend: Option<&'static str>,
    pub(crate) rgba: Option<Vec<u8>>,
    pub(crate) error: Option<String>,
}

pub(crate) fn spawn_first_page_prepare(
    started_at: Instant,
    path: PathBuf,
    target_long_edge: u32,
    on_report: impl FnOnce(PreparedImageReport) + Send + 'static,
) {
    std::thread::Builder::new()
        .name("suisuiview-runtime-probe-image".to_owned())
        .spawn(move || {
            on_report(prepare_first_page(started_at, path, target_long_edge));
        })
        .expect("failed to spawn first-page prepare thread");
}

fn prepare_first_page(
    started_at: Instant,
    path: PathBuf,
    target_long_edge: u32,
) -> PreparedImageReport {
    let worker_started_ms = elapsed_ms(started_at.elapsed());
    let open_started = Instant::now();
    let (source, forced_page) = match open_source_from_path(&path) {
        Ok(source) => source,
        Err(error) => {
            return failed_report(
                worker_started_ms,
                target_long_edge,
                format!("open_source failed: {error}"),
            );
        }
    };
    let open_source_ms = elapsed_ms(open_started.elapsed());
    let page_count = source.page_count();
    if page_count == 0 {
        return failed_report(
            worker_started_ms,
            target_long_edge,
            "source has no pages".to_owned(),
        );
    }
    let page_index = forced_page
        .unwrap_or_default()
        .min(page_count.saturating_sub(1));
    let read_started = Instant::now();
    let bytes = match source.read_page(page_index) {
        Ok(bytes) => bytes,
        Err(error) => {
            return failed_report(
                worker_started_ms,
                target_long_edge,
                format!("read_page failed: {error}"),
            );
        }
    };
    let read_page_ms = elapsed_ms(read_started.elapsed());
    let decode = decode_options_from_current_settings();
    let prepare_started = Instant::now();
    let prepared = match prepare_image_with_options(&bytes, target_long_edge, decode) {
        Ok(page) => page,
        Err(error) => {
            return failed_report(
                worker_started_ms,
                target_long_edge,
                format!("prepare_image failed: {error}"),
            );
        }
    };
    let prepare_ms = elapsed_ms(prepare_started.elapsed());
    PreparedImageReport {
        worker_started_ms,
        open_source_ms: Some(open_source_ms),
        read_page_ms: Some(read_page_ms),
        prepare_ms: Some(prepare_ms),
        page_index: Some(page_index),
        page_count: Some(page_count),
        page_name: source.page_name(page_index).map(ToOwned::to_owned),
        title: Some(source.title().to_owned()),
        original_size: Some([prepared.original_width, prepared.original_height]),
        display_size: Some([prepared.display_width, prepared.display_height]),
        target_long_edge,
        decode_backend: Some(prepared.decode_backend.as_str()),
        rgba: Some(
            prepared
                .pixels
                .to_rgba_vec(prepared.display_width, prepared.display_height),
        ),
        error: None,
    }
}

fn decode_options_from_current_settings() -> DecodeOptions {
    let store = StateStore::load();
    let settings = store.settings();
    let strategy = match settings.decode_mode {
        DecodeMode::AutoFast => DecodeStrategy::Auto,
        DecodeMode::Compatibility => DecodeStrategy::ImageCrate,
        DecodeMode::Custom => DecodeStrategy::Auto,
    };
    let decoder_preferences = if matches!(settings.decode_mode, DecodeMode::Custom) {
        settings.decoder_preferences
    } else {
        DecoderPreferences::default()
    };
    DecodeOptions {
        strategy,
        decoder_preferences,
        cpu_upscale_filter: settings.cpu_upscale_filter,
        cpu_downscale_filter: settings.cpu_downscale_filter,
        allow_display_upscale: false,
        apply_exif_orientation: settings.apply_exif_orientation,
        apply_embedded_icc: settings.apply_embedded_icc,
    }
}

fn failed_report(
    worker_started_ms: f64,
    target_long_edge: u32,
    error: String,
) -> PreparedImageReport {
    PreparedImageReport {
        worker_started_ms,
        open_source_ms: None,
        read_page_ms: None,
        prepare_ms: None,
        page_index: None,
        page_count: None,
        page_name: None,
        title: None,
        original_size: None,
        display_size: None,
        target_long_edge,
        decode_backend: None,
        rgba: None,
        error: Some(error),
    }
}
