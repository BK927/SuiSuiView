use crate::core::state::{
    AppSettings, CacheMemoryMode, RendererMode, AMPLE_TOTAL_BUDGET_BYTES, MANUAL_CACHE_MB_MAX,
    MANUAL_CACHE_MB_MIN, SAVER_TOTAL_BUDGET_BYTES, STANDARD_TOTAL_BUDGET_BYTES,
};
use std::sync::OnceLock;

/// Decoded-page cache budget (bytes). Derived as a share of the single total-memory budget so
/// the total dominates; floored at [`MIN_DECODE_CACHE_BYTES`] so the current page and its
/// super-resolution round-trip always fit.
pub(in crate::app) fn cache_budget_bytes(settings: &AppSettings) -> usize {
    let total = total_memory_budget_bytes(settings);
    let share = renderer_pool_shares(settings.renderer_mode).decode_numerator;
    scale_share(total, share).max(MIN_DECODE_CACHE_BYTES)
}

/// System RAM in bytes, queried once via sysinfo and cached for the process lifetime. Shared by
/// every automatic budget so the total-memory heuristic reads physical RAM exactly once.
fn system_total_memory_bytes() -> usize {
    static SYSTEM_TOTAL_MEMORY: OnceLock<usize> = OnceLock::new();
    *SYSTEM_TOTAL_MEMORY.get_or_init(|| {
        sysinfo::System::new_with_specifics(
            sysinfo::RefreshKind::nothing()
                .with_memory(sysinfo::MemoryRefreshKind::nothing().with_ram()),
        )
        .total_memory() as usize
    })
}

// Automatic total-memory budget bounds, per renderer mode. The Glow (low-memory) path holds no
// GPU pools, so it aims lower; the WGPU path funds the GPU pools too, so it may reach higher.
const AUTO_TOTAL_GLOW_FRACTION_DIVISOR: usize = 50; // 2% of system RAM.
const AUTO_TOTAL_GLOW_MIN_BYTES: usize = 128 * 1024 * 1024;
const AUTO_TOTAL_GLOW_MAX_BYTES: usize = 256 * 1024 * 1024;
const AUTO_TOTAL_WGPU_FRACTION_DIVISOR: usize = 25; // 4% of system RAM.
const AUTO_TOTAL_WGPU_MIN_BYTES: usize = 256 * 1024 * 1024;
const AUTO_TOTAL_WGPU_MAX_BYTES: usize = 768 * 1024 * 1024;

/// Single total-memory budget (bytes) that dominates every cache pool. Manual and the preset
/// modes are fixed sizes; Auto scales with system RAM and the renderer mode.
pub(in crate::app) fn total_memory_budget_bytes(settings: &AppSettings) -> usize {
    match settings.cache_memory_mode {
        CacheMemoryMode::Auto => {
            automatic_total_budget_bytes_for(settings.renderer_mode, system_total_memory_bytes())
        }
        CacheMemoryMode::Saver => SAVER_TOTAL_BUDGET_BYTES,
        CacheMemoryMode::Standard => STANDARD_TOTAL_BUDGET_BYTES,
        CacheMemoryMode::Ample => AMPLE_TOTAL_BUDGET_BYTES,
        CacheMemoryMode::Manual => {
            (settings
                .manual_cache_mb
                .clamp(MANUAL_CACHE_MB_MIN, MANUAL_CACHE_MB_MAX) as usize)
                * 1024
                * 1024
        }
    }
}

/// Automatic total budget for a renderer mode given the system RAM, as a pure function so the
/// mode x RAM matrix is unit-testable without touching sysinfo.
pub(in crate::app) fn automatic_total_budget_bytes_for(
    renderer_mode: RendererMode,
    total_memory_bytes: usize,
) -> usize {
    match renderer_mode {
        RendererMode::LowMemoryGlow => (total_memory_bytes / AUTO_TOTAL_GLOW_FRACTION_DIVISOR)
            .clamp(AUTO_TOTAL_GLOW_MIN_BYTES, AUTO_TOTAL_GLOW_MAX_BYTES),
        RendererMode::Wgpu => (total_memory_bytes / AUTO_TOTAL_WGPU_FRACTION_DIVISOR)
            .clamp(AUTO_TOTAL_WGPU_MIN_BYTES, AUTO_TOTAL_WGPU_MAX_BYTES),
    }
}

/// Per-pool split of the total budget, expressed as numerators over [`POOL_SHARE_DENOMINATOR`].
/// In WGPU mode the four pools sum to the whole; in Glow mode the GPU pools are unused, so their
/// 40% is redistributed to the CPU-side decode and texture pools.
struct PoolShares {
    decode_numerator: usize,
    texture_numerator: usize,
    gpu_source_numerator: usize,
    gpu_intermediate_numerator: usize,
}

const POOL_SHARE_DENOMINATOR: usize = 100;

fn renderer_pool_shares(renderer_mode: RendererMode) -> PoolShares {
    match renderer_mode {
        RendererMode::Wgpu => PoolShares {
            decode_numerator: 35,
            texture_numerator: 25,
            gpu_source_numerator: 25,
            gpu_intermediate_numerator: 15,
        },
        RendererMode::LowMemoryGlow => PoolShares {
            decode_numerator: 55,
            texture_numerator: 45,
            gpu_source_numerator: 0,
            gpu_intermediate_numerator: 0,
        },
    }
}

fn scale_share(total_bytes: usize, numerator: usize) -> usize {
    total_bytes / POOL_SHARE_DENOMINATOR * numerator
}

/// Texture cache budget cap (bytes): the total's texture share, floored at
/// [`MIN_TEXTURE_CACHE_BYTES`] so visible spreads always fit.
pub(in crate::app) fn texture_cache_budget_cap_bytes(settings: &AppSettings) -> usize {
    let total = total_memory_budget_bytes(settings);
    let share = renderer_pool_shares(settings.renderer_mode).texture_numerator;
    scale_share(total, share).max(MIN_TEXTURE_CACHE_BYTES)
}

/// GPU source-texture pool budget (bytes): the total's GPU-source share, floored at
/// [`MIN_GPU_SOURCE_TEXTURE_BYTES`] so the current page can always be uploaded.
pub(in crate::app) fn gpu_source_texture_budget_bytes(settings: &AppSettings) -> usize {
    let total = total_memory_budget_bytes(settings);
    let share = renderer_pool_shares(settings.renderer_mode).gpu_source_numerator;
    scale_share(total, share).max(MIN_GPU_SOURCE_TEXTURE_BYTES)
}

/// GPU intermediate-texture pool budget (bytes): the total's GPU-intermediate share, floored at
/// [`MIN_GPU_INTERMEDIATE_TEXTURE_BYTES`] so an in-flight effect chain always fits.
pub(in crate::app) fn gpu_intermediate_texture_budget_bytes(settings: &AppSettings) -> usize {
    let total = total_memory_budget_bytes(settings);
    let share = renderer_pool_shares(settings.renderer_mode).gpu_intermediate_numerator;
    scale_share(total, share).max(MIN_GPU_INTERMEDIATE_TEXTURE_BYTES)
}

const MIN_DECODE_CACHE_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MIN_TEXTURE_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MIN_GPU_SOURCE_TEXTURE_BYTES: usize = 64 * 1024 * 1024;
const MIN_GPU_INTERMEDIATE_TEXTURE_BYTES: usize = 96 * 1024 * 1024;
