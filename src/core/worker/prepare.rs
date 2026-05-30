use super::scheduler::PageJob;
use super::{prepare_image_with_options, DecodeOptions, PreparedPage};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::{perf_trace, perf_trace::PerfField};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::{Duration, Instant};

pub(in crate::core::worker) fn prepare_page_with_perf(
    bytes: &[u8],
    job: PageJob,
    book_epoch: usize,
    decode: DecodeOptions,
    decode_ahead: bool,
) -> Result<PreparedPage, String> {
    #[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
    let _ = (book_epoch, decode_ahead);
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    let prepare_started = Instant::now();
    let prepared = prepare_image_with_options(bytes, job.target_long_edge, decode);
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    perf_trace::record_duration_if_at_least(
        "page_prepare",
        prepare_started.elapsed(),
        Duration::from_millis(40),
        &[
            PerfField::Usize("page", job.index),
            PerfField::Usize("book_epoch", book_epoch),
            PerfField::U32("target_long_edge", job.target_long_edge),
            PerfField::Str("decode_strategy", decode.strategy.as_str()),
            PerfField::Str("resize_filter", decode.resize_filter.token()),
            PerfField::Bool("allow_display_upscale", decode.allow_display_upscale),
            PerfField::Bool("apply_exif_orientation", decode.apply_exif_orientation),
            PerfField::Bool("apply_embedded_icc", decode.apply_embedded_icc),
            PerfField::Bool("decode_ahead", decode_ahead),
            PerfField::Bool("success", prepared.is_ok()),
        ],
    );
    prepared
}
