use crate::core::perf_trace::{self, PerfField};
use crate::core::sr_lab::{gpu::tiled::DEFAULT_SPAN_TILE_EDGE, SrLabManifest};
use std::env;
use std::sync::OnceLock;
use std::time::Duration;

const EXPERIMENT_SPAN_TILE_EDGE_ENV: &str = "SUISUIVIEW_EXPERIMENT_SPAN_TILE_EDGE";
const EXPERIMENT_SPAN_TILES_PER_FRAME_ENV: &str = "SUISUIVIEW_EXPERIMENT_SPAN_TILES_PER_FRAME";
const EXPERIMENT_SPAN_WORKSPACE_CACHE_MB_ENV: &str =
    "SUISUIVIEW_EXPERIMENT_SPAN_WORKSPACE_CACHE_MB";
const MIB_BYTES: u64 = 1024 * 1024;
const DEFAULT_DISPLAY_WORKSPACE_CACHE_MB: u64 = 192;
const MIN_DISPLAY_WORKSPACE_CACHE_MB: u64 = 64;
const MAX_DISPLAY_WORKSPACE_CACHE_MB: u64 = 512;
const MIN_DISPLAY_TILE_EDGE: usize = 32;
const MAX_DISPLAY_TILE_EDGE: usize = 256;
const DEFAULT_DISPLAY_TILES_PER_FRAME: usize = 8;
const MIN_DISPLAY_TILES_PER_FRAME: usize = 1;
const MAX_DISPLAY_TILES_PER_FRAME: usize = 64;

#[derive(Clone, Copy)]
pub(super) enum SpanDisplaySkipStats {
    None,
    FrameWorkspaceLimit {
        required_bytes: u64,
        limit_bytes: u64,
    },
    WorkspaceCacheLimit {
        required_bytes: u64,
        limit_bytes: u64,
    },
    TileWorkspaceLimit {
        required_bytes: u64,
        limit_bytes: u64,
    },
}

impl SpanDisplaySkipStats {
    pub(super) fn frame_workspace_limit(required_bytes: u64, limit_bytes: u64) -> Self {
        Self::FrameWorkspaceLimit {
            required_bytes,
            limit_bytes,
        }
    }

    pub(super) fn workspace_cache_limit(required_bytes: u64, limit_bytes: u64) -> Self {
        Self::WorkspaceCacheLimit {
            required_bytes,
            limit_bytes,
        }
    }

    pub(super) fn tile_workspace_limit(required_bytes: u64, limit_bytes: u64) -> Self {
        Self::TileWorkspaceLimit {
            required_bytes,
            limit_bytes,
        }
    }
}

// established metrics call surface; a params struct would be pure boilerplate
#[allow(clippy::too_many_arguments)]
pub(super) fn record_span_display_encode(
    duration: Duration,
    source_size: [usize; 2],
    output_size: [usize; 2],
    tile_count: usize,
    workspace_shapes: usize,
    workspace_slots: usize,
    workspace_bytes: u64,
    workspace_cache_limit_bytes: u64,
    tile_edge: usize,
    estimated_dispatches: usize,
    tiles_per_frame: usize,
) {
    perf_trace::record_duration(
        "span_display_encode",
        duration,
        &[
            PerfField::Str("method", "srlab_span_x2"),
            PerfField::Usize("source_width", source_size[0]),
            PerfField::Usize("source_height", source_size[1]),
            PerfField::Usize("output_width", output_size[0]),
            PerfField::Usize("output_height", output_size[1]),
            PerfField::Usize("tile_count", tile_count),
            PerfField::Usize("workspace_shapes", workspace_shapes),
            PerfField::Usize("workspace_slots", workspace_slots),
            PerfField::Usize("tile_edge", tile_edge),
            PerfField::Usize("tiles_per_frame", tiles_per_frame),
            PerfField::Usize("estimated_dispatches", estimated_dispatches),
            PerfField::Usize(
                "workspace_cache_bytes",
                usize_from_u64_saturating(workspace_bytes),
            ),
            PerfField::Usize(
                "workspace_cache_limit_bytes",
                usize_from_u64_saturating(workspace_cache_limit_bytes),
            ),
        ],
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn record_span_display_prepare(
    duration: Duration,
    source_size: [usize; 2],
    output_size: [usize; 2],
    tile_count: usize,
    workspace_shapes: usize,
    workspace_slots: usize,
    workspace_bytes: u64,
    workspace_cache_limit_bytes: u64,
    tile_edge: usize,
    estimated_dispatches: usize,
) {
    perf_trace::record_duration(
        "span_display_prepare",
        duration,
        &[
            PerfField::Str("method", "srlab_span_x2"),
            PerfField::Usize("source_width", source_size[0]),
            PerfField::Usize("source_height", source_size[1]),
            PerfField::Usize("output_width", output_size[0]),
            PerfField::Usize("output_height", output_size[1]),
            PerfField::Usize("tile_count", tile_count),
            PerfField::Usize("workspace_shapes", workspace_shapes),
            PerfField::Usize("workspace_slots", workspace_slots),
            PerfField::Usize("tile_edge", tile_edge),
            PerfField::Usize("estimated_dispatches", estimated_dispatches),
            PerfField::Usize(
                "workspace_cache_bytes",
                usize_from_u64_saturating(workspace_bytes),
            ),
            PerfField::Usize(
                "workspace_cache_limit_bytes",
                usize_from_u64_saturating(workspace_cache_limit_bytes),
            ),
        ],
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn record_span_display_tile_batch(
    duration: Duration,
    source_size: [usize; 2],
    output_size: [usize; 2],
    tile_count: usize,
    encoded_tiles: usize,
    next_tile: usize,
    tiles_per_frame: usize,
    completed: bool,
    tile_edge: usize,
    estimated_dispatches: usize,
) {
    perf_trace::record_duration(
        "span_display_tile_batch",
        duration,
        &[
            PerfField::Str("method", "srlab_span_x2"),
            PerfField::Usize("source_width", source_size[0]),
            PerfField::Usize("source_height", source_size[1]),
            PerfField::Usize("output_width", output_size[0]),
            PerfField::Usize("output_height", output_size[1]),
            PerfField::Usize("tile_count", tile_count),
            PerfField::Usize("encoded_tiles", encoded_tiles),
            PerfField::Usize("next_tile", next_tile),
            PerfField::Usize("tiles_per_frame", tiles_per_frame),
            PerfField::Bool("completed", completed),
            PerfField::Usize("tile_edge", tile_edge),
            PerfField::Usize("estimated_dispatches", estimated_dispatches),
        ],
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn record_span_display_cancel(
    reason: &'static str,
    duration: Duration,
    source_size: [usize; 2],
    output_size: [usize; 2],
    tile_count: usize,
    next_tile: usize,
    tiles_per_frame: usize,
    tile_edge: usize,
    workspace_shapes: usize,
    workspace_cache_limit_bytes: u64,
) {
    perf_trace::record_duration(
        "span_display_cancel",
        duration,
        &[
            PerfField::Str("method", "srlab_span_x2"),
            PerfField::Str("reason", reason),
            PerfField::Usize("source_width", source_size[0]),
            PerfField::Usize("source_height", source_size[1]),
            PerfField::Usize("output_width", output_size[0]),
            PerfField::Usize("output_height", output_size[1]),
            PerfField::Usize("tile_count", tile_count),
            PerfField::Usize("next_tile", next_tile),
            PerfField::Usize("remaining_tiles", tile_count.saturating_sub(next_tile)),
            PerfField::Usize("tiles_per_frame", tiles_per_frame),
            PerfField::Usize("tile_edge", tile_edge),
            PerfField::Usize("workspace_shapes", workspace_shapes),
            PerfField::Usize(
                "workspace_cache_limit_bytes",
                usize_from_u64_saturating(workspace_cache_limit_bytes),
            ),
        ],
    );
}

pub(super) fn record_span_display_loader_failure(reason: &'static str, error: &str) {
    if perf_trace::is_active() {
        eprintln!("SPAN display experiment disabled ({reason}): {error}");
    }
    perf_trace::record_duration(
        "span_display_loader_failure",
        Duration::ZERO,
        &[
            PerfField::Str("method", "srlab_span_x2"),
            PerfField::Str("reason", reason),
        ],
    );
}

pub(super) fn record_span_display_skip(
    reason: &'static str,
    source_size: [usize; 2],
    output_size: [usize; 2],
    tile_edge: usize,
    tile_count: usize,
    workspace_shapes: usize,
) {
    record_span_display_skip_with_stats(
        reason,
        source_size,
        output_size,
        tile_edge,
        tile_count,
        workspace_shapes,
        SpanDisplaySkipStats::None,
    );
}

pub(super) fn record_span_display_skip_with_stats(
    reason: &'static str,
    source_size: [usize; 2],
    output_size: [usize; 2],
    tile_edge: usize,
    tile_count: usize,
    workspace_shapes: usize,
    stats: SpanDisplaySkipStats,
) {
    macro_rules! record_skip {
        ($($extra:expr),* $(,)?) => {{
            perf_trace::record_duration(
                "span_display_skip",
                Duration::ZERO,
                &[
                    PerfField::Str("method", "srlab_span_x2"),
                    PerfField::Str("reason", reason),
                    PerfField::Usize("source_width", source_size[0]),
                    PerfField::Usize("source_height", source_size[1]),
                    PerfField::Usize("output_width", output_size[0]),
                    PerfField::Usize("output_height", output_size[1]),
                    PerfField::Usize("tile_edge", tile_edge),
                    PerfField::Usize("tile_count", tile_count),
                    PerfField::Usize("workspace_shapes", workspace_shapes),
                    $($extra,)*
                ],
            );
        }};
    }

    match stats {
        SpanDisplaySkipStats::None => record_skip!(),
        SpanDisplaySkipStats::FrameWorkspaceLimit {
            required_bytes,
            limit_bytes,
        } => record_skip!(
            PerfField::Str("workspace_limit_stage", "frame_distinct_workspaces"),
            PerfField::Usize(
                "required_workspace_cache_bytes",
                usize_from_u64_saturating(required_bytes),
            ),
            PerfField::Usize(
                "workspace_cache_limit_bytes",
                usize_from_u64_saturating(limit_bytes),
            ),
        ),
        SpanDisplaySkipStats::WorkspaceCacheLimit {
            required_bytes,
            limit_bytes,
        } => record_skip!(
            PerfField::Str("workspace_limit_stage", "workspace_cache_insert"),
            PerfField::Usize(
                "required_workspace_cache_bytes",
                usize_from_u64_saturating(required_bytes),
            ),
            PerfField::Usize(
                "workspace_cache_limit_bytes",
                usize_from_u64_saturating(limit_bytes),
            ),
        ),
        SpanDisplaySkipStats::TileWorkspaceLimit {
            required_bytes,
            limit_bytes,
        } => record_skip!(
            PerfField::Str("workspace_limit_stage", "tile_transient"),
            PerfField::Usize(
                "tile_workspace_bytes",
                usize_from_u64_saturating(required_bytes),
            ),
            PerfField::Usize(
                "tile_workspace_limit_bytes",
                usize_from_u64_saturating(limit_bytes),
            ),
        ),
    }
}

fn usize_from_u64_saturating(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

pub(super) fn estimated_dispatch_count(manifest: &SrLabManifest, tile_count: usize) -> usize {
    let span_graph_dispatches = manifest
        .span
        .as_ref()
        .map(|span| 7usize.saturating_add(4usize.saturating_mul(span.block_count as usize)))
        .unwrap_or_default();
    let bridge_dispatches = 2usize;
    tile_count.saturating_mul(span_graph_dispatches.saturating_add(bridge_dispatches))
}

pub(super) fn span_display_tile_edge() -> usize {
    static TILE_EDGE: OnceLock<usize> = OnceLock::new();
    *TILE_EDGE.get_or_init(|| {
        parse_span_display_tile_edge(env::var(EXPERIMENT_SPAN_TILE_EDGE_ENV).ok().as_deref())
            .unwrap_or(DEFAULT_SPAN_TILE_EDGE)
    })
}

fn parse_span_display_tile_edge(value: Option<&str>) -> Option<usize> {
    let edge = value?.trim().parse::<usize>().ok()?;
    (MIN_DISPLAY_TILE_EDGE..=MAX_DISPLAY_TILE_EDGE)
        .contains(&edge)
        .then_some(edge)
}

pub(super) fn span_display_tiles_per_frame() -> usize {
    static TILES_PER_FRAME: OnceLock<usize> = OnceLock::new();
    *TILES_PER_FRAME.get_or_init(|| {
        parse_span_display_tiles_per_frame(
            env::var(EXPERIMENT_SPAN_TILES_PER_FRAME_ENV)
                .ok()
                .as_deref(),
        )
        .unwrap_or(DEFAULT_DISPLAY_TILES_PER_FRAME)
    })
}

fn parse_span_display_tiles_per_frame(value: Option<&str>) -> Option<usize> {
    let count = value?.trim().parse::<usize>().ok()?;
    (MIN_DISPLAY_TILES_PER_FRAME..=MAX_DISPLAY_TILES_PER_FRAME)
        .contains(&count)
        .then_some(count)
}

pub(super) fn span_display_workspace_cache_limit_bytes() -> u64 {
    static CACHE_LIMIT_BYTES: OnceLock<u64> = OnceLock::new();
    *CACHE_LIMIT_BYTES.get_or_init(|| {
        parse_span_display_workspace_cache_mb(
            env::var(EXPERIMENT_SPAN_WORKSPACE_CACHE_MB_ENV)
                .ok()
                .as_deref(),
        )
        .unwrap_or(DEFAULT_DISPLAY_WORKSPACE_CACHE_MB * MIB_BYTES)
    })
}

fn parse_span_display_workspace_cache_mb(value: Option<&str>) -> Option<u64> {
    let mb = value?.trim().parse::<u64>().ok()?;
    (MIN_DISPLAY_WORKSPACE_CACHE_MB..=MAX_DISPLAY_WORKSPACE_CACHE_MB)
        .contains(&mb)
        .then_some(mb.saturating_mul(MIB_BYTES))
}

#[cfg(test)]
mod tests {
    use super::{
        estimated_dispatch_count, parse_span_display_tile_edge, parse_span_display_tiles_per_frame,
        parse_span_display_workspace_cache_mb, MIB_BYTES,
    };
    use crate::core::sr_lab::{SrLabFamily, SrLabManifest, SrLabSpanMetadata};

    #[test]
    fn span_display_dispatch_estimate_includes_bridge_and_graph_passes() {
        let manifest = SrLabManifest {
            name: "SPAN-S x2".to_owned(),
            family: SrLabFamily::SpanS,
            variant: Some("SPAN-S".to_owned()),
            scale: 2,
            input_channels: 3,
            output_channels: 3,
            weights_format: "srlab01".to_owned(),
            weights_file: Some("weights.srlab".to_owned()),
            weights_sha256: "0".repeat(64),
            source: "test".to_owned(),
            source_commit: None,
            source_checkpoint_url: None,
            source_checkpoint_archive_sha256: None,
            source_checkpoint_file: None,
            source_checkpoint_sha256: None,
            license: "Apache-2.0".to_owned(),
            notes: Vec::new(),
            span: Some(SrLabSpanMetadata {
                feature_channels: 48,
                block_count: 6,
                reparameterized_conv3xc: true,
                img_range: 255.0,
                rgb_mean: [0.4488, 0.4371, 0.4040],
            }),
            layers: Vec::new(),
        };

        assert_eq!(estimated_dispatch_count(&manifest, 96), 3168);
    }

    #[test]
    fn span_display_tile_edge_override_accepts_bounded_values() {
        assert_eq!(parse_span_display_tile_edge(Some("128")), Some(128));
        assert_eq!(parse_span_display_tile_edge(Some(" 72 ")), Some(72));
        assert_eq!(parse_span_display_tile_edge(Some("31")), None);
        assert_eq!(parse_span_display_tile_edge(Some("257")), None);
        assert_eq!(parse_span_display_tile_edge(Some("wide")), None);
        assert_eq!(parse_span_display_tile_edge(None), None);
    }

    #[test]
    fn span_display_tiles_per_frame_override_accepts_bounded_values() {
        assert_eq!(parse_span_display_tiles_per_frame(Some("8")), Some(8));
        assert_eq!(parse_span_display_tiles_per_frame(Some(" 64 ")), Some(64));
        assert_eq!(parse_span_display_tiles_per_frame(Some("0")), None);
        assert_eq!(parse_span_display_tiles_per_frame(Some("65")), None);
        assert_eq!(parse_span_display_tiles_per_frame(Some("many")), None);
        assert_eq!(parse_span_display_tiles_per_frame(None), None);
    }

    #[test]
    fn span_display_workspace_cache_override_accepts_bounded_mib() {
        assert_eq!(
            parse_span_display_workspace_cache_mb(Some("256")),
            Some(256 * MIB_BYTES)
        );
        assert_eq!(
            parse_span_display_workspace_cache_mb(Some(" 512 ")),
            Some(512 * MIB_BYTES)
        );
        assert_eq!(parse_span_display_workspace_cache_mb(Some("63")), None);
        assert_eq!(parse_span_display_workspace_cache_mb(Some("513")), None);
        assert_eq!(parse_span_display_workspace_cache_mb(Some("large")), None);
        assert_eq!(parse_span_display_workspace_cache_mb(None), None);
    }
}
