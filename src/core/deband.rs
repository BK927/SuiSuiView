//! Clean-room debanding pass (Flanagan-style, as popularized by libplacebo/mpv)
//! for the WGPU display path. This module is the CPU reference implementation and
//! the ground truth the WGSL pass mirrors and the quality tests measure against.
//!
//! LICENSE: the algorithm is implemented clean-room from a plain-language
//! description — no libplacebo/mpv code (LGPL) was copied or ported. See
//! `deband.wgsl` for the GPU mirror and `deband_quality.rs` for the metrics.
//!
//! Algorithm, per pixel, per iteration `i = 1..=iterations`:
//! - radius `r = base_radius * i`
//! - a coordinate-stable pseudorandom angle `a` from an integer hash of
//!   `(x, y, i)` — NOT time/frame, so a static page never shimmers on scroll
//! - sample the SOURCE (never the previous iteration's output — iterations refine
//!   the in-register `current`, which keeps the GPU pass single-pass and makes
//!   CPU/GPU equivalence exact) at four points `a + k*90deg`, distance `r`,
//!   clamped to the edges
//! - `avg` = mean of the four samples, per RGB channel
//! - keep/replace: if `max_c |avg_c - current_c| < threshold` then
//!   `current = avg` (one decision for all three channels — a per-channel
//!   decision would speckle color). Alpha is never touched.
//!
//! After the iterations, grain is injected ONLY into pixels replaced at least
//! once: a coordinate-hashed uniform offset in `[-grain, +grain]`, added equally
//! to R, G and B (luma grain, no chroma noise).
//!
//! Units are 8-bit (0..=255) throughout, matching the presets. The WGSL mirror
//! works in normalized 0..1 and divides `threshold`/`grain` by 255 on upload.

use crate::core::i18n::I18n;
use serde::{Deserialize, Serialize};
use std::f32::consts::{FRAC_PI_2, TAU};

/// Salt for the grain hash so grain noise decorrelates from the per-iteration
/// angle hashes. The WGSL mirror (`deband.wgsl`) hardcodes the SAME value.
///
/// The CPU reference below (`deband_rgba` and its hash/rounding helpers) is the
/// testing ground truth and the future authority for a baked clipboard/export
/// path; this display-only stage runs the WGSL mirror, so the CPU path has no
/// production caller yet — hence the `dead_code` allowances.
#[allow(dead_code)]
const GRAIN_SALT: u32 = 0x6D2B_79F5;

/// Debanding strength preset. `Off` is the default; the other three map to the
/// validated `(iterations, base_radius, threshold, grain)` tuples via [`params`].
///
/// [`params`]: DebandStrength::params
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DebandStrength {
    #[default]
    Off,
    Weak,
    Medium,
    Strong,
}

/// Resolved debanding parameters in 8-bit units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DebandParams {
    pub(crate) iterations: u32,
    pub(crate) base_radius: f32,
    /// Keep/replace threshold in 8-bit units.
    pub(crate) threshold: f32,
    /// Grain amplitude in 8-bit units.
    pub(crate) grain: f32,
}

impl DebandStrength {
    /// Menu order: Off first, then increasing strength.
    pub const ALL: [Self; 4] = [Self::Off, Self::Weak, Self::Medium, Self::Strong];

    /// The validated preset for this strength, or `None` for `Off` (no pass).
    ///
    /// Consumed by the WGPU deband pass in the binary crate; the library
    /// compilation of `core` has no non-test caller, hence `dead_code` there.
    #[allow(dead_code)]
    pub(crate) fn params(self) -> Option<DebandParams> {
        match self {
            Self::Off => None,
            Self::Weak => Some(DebandParams {
                iterations: 2,
                base_radius: 8.0,
                threshold: 2.0,
                grain: 0.5,
            }),
            Self::Medium => Some(DebandParams {
                iterations: 3,
                base_radius: 12.0,
                threshold: 3.0,
                grain: 0.8,
            }),
            Self::Strong => Some(DebandParams {
                iterations: 4,
                base_radius: 16.0,
                threshold: 4.5,
                grain: 1.2,
            }),
        }
    }

    pub fn is_active(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Stable token for hashing (cache keys / draw-id) and diagnostics logging.
    pub fn token(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Weak => "weak",
            Self::Medium => "medium",
            Self::Strong => "strong",
        }
    }

    /// Localized level name (`끄기 / 약 / 중 / 강`) for the settings combo and the
    /// scaler-chip tooltip.
    pub fn label_i18n(self, i18n: I18n) -> String {
        let key = match self {
            Self::Off => "deband.level.off",
            Self::Weak => "deband.level.weak",
            Self::Medium => "deband.level.medium",
            Self::Strong => "deband.level.strong",
        };
        i18n.text(key)
    }
}

/// Deterministic integer hash of `(x, y, salt)` -> `u32`. A wrapping
/// multiply/xor-shift finalizer (Murmur/xxhash-style constants). WGSL u32
/// arithmetic wraps by spec, so `deband.wgsl` reproduces this bit-exactly.
#[allow(dead_code)]
fn hash_u32(x: u32, y: u32, salt: u32) -> u32 {
    let mut h =
        x.wrapping_mul(0x0100_0193) ^ y.wrapping_mul(0x9E37_79B1) ^ salt.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h
}

/// Hash mapped to `[0, 1)`. Matches the WGSL `f32(hash) / 4294967296.0`.
#[allow(dead_code)]
fn hash_unit(x: u32, y: u32, salt: u32) -> f32 {
    hash_u32(x, y, salt) as f32 / 4_294_967_296.0
}

/// Deband `pixels` (tightly packed RGBA, `width * height` texels) in place.
/// `Off`-equivalent params (`iterations == 0`) and degenerate sizes are no-ops.
/// Alpha bytes are preserved untouched.
#[allow(dead_code)]
pub(crate) fn deband_rgba(pixels: &mut [u8], width: u32, height: u32, params: DebandParams) {
    let w = width as usize;
    let h = height as usize;
    if params.iterations == 0 || w == 0 || h == 0 {
        return;
    }
    let needed = w * h * 4;
    if pixels.len() < needed {
        return;
    }
    // Iterations always read the ORIGINAL source, so snapshot it once.
    let src = pixels[..needed].to_vec();
    let sample = |sx: i32, sy: i32| -> [f32; 3] {
        let cx = sx.clamp(0, w as i32 - 1) as usize;
        let cy = sy.clamp(0, h as i32 - 1) as usize;
        let idx = (cy * w + cx) * 4;
        [src[idx] as f32, src[idx + 1] as f32, src[idx + 2] as f32]
    };

    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) * 4;
            let mut cur = [src[idx] as f32, src[idx + 1] as f32, src[idx + 2] as f32];
            let mut replaced = false;
            for i in 1..=params.iterations {
                let r = params.base_radius * i as f32;
                let a = hash_unit(x as u32, y as u32, i) * TAU;
                let mut avg = [0.0f32; 3];
                for k in 0..4u32 {
                    let ang = a + k as f32 * FRAC_PI_2;
                    let dx = (r * ang.cos()).round() as i32;
                    let dy = (r * ang.sin()).round() as i32;
                    let s = sample(x as i32 + dx, y as i32 + dy);
                    avg[0] += s[0];
                    avg[1] += s[1];
                    avg[2] += s[2];
                }
                avg[0] *= 0.25;
                avg[1] *= 0.25;
                avg[2] *= 0.25;
                let d = (avg[0] - cur[0])
                    .abs()
                    .max((avg[1] - cur[1]).abs())
                    .max((avg[2] - cur[2]).abs());
                if d < params.threshold {
                    cur = avg;
                    replaced = true;
                }
            }
            if replaced {
                let g = (hash_unit(x as u32, y as u32, GRAIN_SALT) * 2.0 - 1.0) * params.grain;
                cur[0] += g;
                cur[1] += g;
                cur[2] += g;
            }
            pixels[idx] = to_u8(cur[0]);
            pixels[idx + 1] = to_u8(cur[1]);
            pixels[idx + 2] = to_u8(cur[2]);
            // pixels[idx + 3] (alpha) intentionally left untouched.
        }
    }
}

#[allow(dead_code)]
fn to_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic_and_varies() {
        assert_eq!(hash_u32(3, 7, 1), hash_u32(3, 7, 1));
        assert_ne!(hash_u32(3, 7, 1), hash_u32(3, 7, 2));
        assert_ne!(hash_u32(3, 7, 1), hash_u32(4, 7, 1));
        assert_ne!(hash_u32(3, 7, 1), hash_u32(3, 8, 1));
        // Mapped unit is in range.
        for salt in 0..8u32 {
            let u = hash_unit(11, 23, salt);
            assert!((0.0..1.0).contains(&u), "unit {u} out of range");
        }
    }

    #[test]
    fn presets_match_the_validated_tuples() {
        assert_eq!(DebandStrength::Off.params(), None);
        assert_eq!(
            DebandStrength::Weak.params(),
            Some(DebandParams {
                iterations: 2,
                base_radius: 8.0,
                threshold: 2.0,
                grain: 0.5
            })
        );
        assert_eq!(
            DebandStrength::Medium.params(),
            Some(DebandParams {
                iterations: 3,
                base_radius: 12.0,
                threshold: 3.0,
                grain: 0.8
            })
        );
        assert_eq!(
            DebandStrength::Strong.params(),
            Some(DebandParams {
                iterations: 4,
                base_radius: 16.0,
                threshold: 4.5,
                grain: 1.2
            })
        );
    }

    #[test]
    fn zero_threshold_is_a_no_op_and_preserves_alpha() {
        // d = |avg - cur| >= 0 is never < 0, so nothing is replaced, so no grain
        // is injected: RGB and alpha come out byte-identical to the input.
        let w = 40;
        let h = 40;
        let mut pixels = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                pixels.extend_from_slice(&[(x * 3) as u8, (y * 5) as u8, 90, 200]);
            }
        }
        let original = pixels.clone();
        deband_rgba(
            &mut pixels,
            w as u32,
            h as u32,
            DebandParams {
                iterations: 3,
                base_radius: 12.0,
                threshold: 0.0,
                grain: 0.8,
            },
        );
        assert_eq!(pixels, original);
    }

    #[test]
    fn flat_region_is_replaced_then_only_grain_shifts_it() {
        // A uniform field: every sample equals the center, so d == 0 < threshold
        // for every iteration -> replaced. The only change is the grain offset,
        // bounded by ±grain (rounds to at most ±1 here for grain 0.8).
        let w = 48;
        let h = 48;
        let gray = 120u8;
        let mut pixels = vec![0u8; w * h * 4];
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.copy_from_slice(&[gray, gray, gray, 255]);
        }
        deband_rgba(
            &mut pixels,
            w as u32,
            h as u32,
            DebandParams {
                iterations: 3,
                base_radius: 12.0,
                threshold: 3.0,
                grain: 0.8,
            },
        );
        for chunk in pixels.chunks_exact(4) {
            for &c in &chunk[..3] {
                assert!(
                    (c as i32 - gray as i32).abs() <= 1,
                    "channel {c} drifted from {gray} by more than grain"
                );
            }
            assert_eq!(chunk[3], 255, "alpha must be preserved");
        }
    }

    #[test]
    fn strong_edge_is_kept_not_blended() {
        // Left half black, right half white with a hard seam. On the light plateau
        // far from the seam the neighbor average is white, so the pixel is kept
        // (delta 255 >> threshold): a true edge is never smeared into a band.
        let w = 64;
        let h = 8;
        let mut pixels = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let v = if x < w / 2 { 0 } else { 255 };
                let idx = (y * w + x) * 4;
                pixels[idx] = v;
                pixels[idx + 1] = v;
                pixels[idx + 2] = v;
                pixels[idx + 3] = 255;
            }
        }
        let before = pixels.clone();
        deband_rgba(
            &mut pixels,
            w as u32,
            h as u32,
            DebandParams {
                iterations: 2,
                base_radius: 8.0,
                threshold: 3.0,
                grain: 0.0,
            },
        );
        // Column near the right edge (far from the seam) stays pure white.
        let idx = (4 * w + (w - 2)) * 4;
        assert_eq!(&pixels[idx..idx + 3], &before[idx..idx + 3]);
    }
}
