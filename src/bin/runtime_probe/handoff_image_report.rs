use super::image_first_page::PreparedImageReport;

#[derive(Default)]
pub(super) struct ImageHandoffMetrics {
    pub(super) first_glow_visible_ms: Option<f64>,
    pub(super) image_worker_started_ms: Option<f64>,
    pub(super) open_source_ms: Option<f64>,
    pub(super) read_page_ms: Option<f64>,
    pub(super) prepare_ms: Option<f64>,
    pub(super) glow_image_register_ms: Option<f64>,
    pub(super) glow_image_visible_ms: Option<f64>,
    pub(super) last_glow_image_present_ms: Option<f64>,
    pub(super) handoff_started_ms: Option<f64>,
    pub(super) glow_destroy_ms: Option<f64>,
    pub(super) context_destroy_ms: Option<f64>,
    pub(super) painter_new_ms: Option<f64>,
    pub(super) set_window_ms: Option<f64>,
    pub(super) wgpu_image_register_ms: Option<f64>,
    pub(super) first_wgpu_image_present_ms: Option<f64>,
    pub(super) first_wgpu_frame_ms: Option<f64>,
    pub(super) handoff_gap_ms: Option<f64>,
    pub(super) prewarm_started_ms: Option<f64>,
    pub(super) prewarm_ready_ms: Option<f64>,
    pub(super) prewarm_init_ms: Option<f64>,
    pub(super) prewarm_backend: Option<String>,
    pub(super) prewarm_device_type: Option<String>,
    pub(super) used_prewarmed_wgpu: bool,
    pub(super) error: Option<String>,
}

pub(super) fn print_image_handoff_summary(
    metrics: &ImageHandoffMetrics,
    image: Option<&PreparedImageReport>,
) {
    println!(
        "runtime_probe_handoff_image glow_first_visible_ms={:.3} image_worker_started_ms={:.3} open_source_ms={:.3} read_page_ms={:.3} prepare_ms={:.3} glow_image_register_ms={:.3} glow_image_visible_ms={:.3} last_glow_image_present_ms={:.3} handoff_started_ms={:.3} glow_destroy_ms={:.3} gl_context_destroy_ms={:.3} wgpu_painter_new_ms={:.3} wgpu_set_window_ms={:.3} wgpu_image_register_ms={:.3} first_wgpu_image_present_ms={:.3} first_wgpu_frame_ms={:.3} handoff_gap_ms={:.3} original={}x{} display={}x{} page_index={} page_count={} decode_backend={} used_prewarmed_wgpu={} prewarm_started_ms={:.3} prewarm_ready_ms={:.3} prewarm_init_ms={:.3} prewarm_backend={} prewarm_device_type={} error={}",
        metrics.first_glow_visible_ms.unwrap_or(-1.0),
        metrics.image_worker_started_ms.unwrap_or(-1.0),
        metrics.open_source_ms.unwrap_or(-1.0),
        metrics.read_page_ms.unwrap_or(-1.0),
        metrics.prepare_ms.unwrap_or(-1.0),
        metrics.glow_image_register_ms.unwrap_or(-1.0),
        metrics.glow_image_visible_ms.unwrap_or(-1.0),
        metrics.last_glow_image_present_ms.unwrap_or(-1.0),
        metrics.handoff_started_ms.unwrap_or(-1.0),
        metrics.glow_destroy_ms.unwrap_or(-1.0),
        metrics.context_destroy_ms.unwrap_or(-1.0),
        metrics.painter_new_ms.unwrap_or(-1.0),
        metrics.set_window_ms.unwrap_or(-1.0),
        metrics.wgpu_image_register_ms.unwrap_or(-1.0),
        metrics.first_wgpu_image_present_ms.unwrap_or(-1.0),
        metrics.first_wgpu_frame_ms.unwrap_or(-1.0),
        metrics.handoff_gap_ms.unwrap_or(-1.0),
        image
            .and_then(|report| report.original_size)
            .map_or(0, |size| size[0]),
        image
            .and_then(|report| report.original_size)
            .map_or(0, |size| size[1]),
        image
            .and_then(|report| report.display_size)
            .map_or(0, |size| size[0]),
        image
            .and_then(|report| report.display_size)
            .map_or(0, |size| size[1]),
        image.and_then(|report| report.page_index).unwrap_or_default(),
        image.and_then(|report| report.page_count).unwrap_or_default(),
        image
            .and_then(|report| report.decode_backend)
            .unwrap_or("unknown"),
        metrics.used_prewarmed_wgpu,
        metrics.prewarm_started_ms.unwrap_or(-1.0),
        metrics.prewarm_ready_ms.unwrap_or(-1.0),
        metrics.prewarm_init_ms.unwrap_or(-1.0),
        metrics.prewarm_backend.as_deref().unwrap_or("unknown"),
        metrics
            .prewarm_device_type
            .as_deref()
            .unwrap_or("unknown"),
        metrics.error.as_deref().unwrap_or("none")
    );
}
