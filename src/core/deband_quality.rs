//! Deband quality metrics and the in-memory synthetic fixture. Test-only: this
//! is the Rust port of the supervisor's validated Python metric suite, used by
//! the `#[ignore]`d `deband_quality_scan` to prove the presets on real banding.
//!
//! All metrics run on a single luma plane (BT.601). The fixture is generated in
//! memory (no files) and pushed through a JPEG quality-80 round-trip so the
//! banding is real 8-bit/JPEG banding rather than a synthetic step function.

use crate::core::deband::{deband_rgba, DebandStrength};
use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, ImageFormat};

const FIXTURE_WIDTH: u32 = 690;
const FIXTURE_HEIGHT: u32 = 1600;

/// Flat-mask cutoff: a pixel is "flat" when the largest gradient in its radius-3
/// neighborhood is below this (8-bit units).
const FLAT_MAX_GRADIENT: f32 = 2.5;
/// Edge-mask cutoff: a pixel is an "edge" when its own gradient exceeds this.
const EDGE_MIN_GRADIENT: f32 = 24.0;
/// A qualifying band-edge pixel has a vertical neighbor delta in this closed band.
const BAND_DELTA_LO: f32 = 0.5;
const BAND_DELTA_HI: f32 = 2.5;
/// Minimum horizontal run length (consecutive qualifying pixels) that counts as a band edge.
const BAND_RUN_MIN: usize = 24;

/// A luma plane plus its dimensions.
pub(crate) struct LumaPlane {
    pub(crate) data: Vec<f32>,
    pub(crate) width: usize,
    pub(crate) height: usize,
}

impl LumaPlane {
    fn at(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }
}

/// BT.601 luma plane from tightly packed RGBA.
pub(crate) fn luma_plane(rgba: &[u8], width: u32, height: u32) -> LumaPlane {
    let width = width as usize;
    let height = height as usize;
    let mut data = Vec::with_capacity(width * height);
    for chunk in rgba.chunks_exact(4).take(width * height) {
        let y = 0.299 * chunk[0] as f32 + 0.587 * chunk[1] as f32 + 0.114 * chunk[2] as f32;
        data.push(y);
    }
    LumaPlane {
        data,
        width,
        height,
    }
}

/// Forward-difference gradient magnitude `max(|dx|, |dy|)` at `(x, y)`, edges clamped.
fn gradient_at(plane: &LumaPlane, x: usize, y: usize) -> f32 {
    let right = plane.at((x + 1).min(plane.width - 1), y);
    let down = plane.at(x, (y + 1).min(plane.height - 1));
    let here = plane.at(x, y);
    (right - here).abs().max((down - here).abs())
}

/// Flat mask: local max gradient over a radius-3 neighborhood below the cutoff.
pub(crate) fn flat_mask(plane: &LumaPlane) -> Vec<bool> {
    let (w, h) = (plane.width, plane.height);
    let mut mask = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut local_max = 0.0f32;
            let y0 = y.saturating_sub(3);
            let y1 = (y + 3).min(h - 1);
            let x0 = x.saturating_sub(3);
            let x1 = (x + 3).min(w - 1);
            for ny in y0..=y1 {
                for nx in x0..=x1 {
                    local_max = local_max.max(gradient_at(plane, nx, ny));
                }
            }
            mask[y * w + x] = local_max < FLAT_MAX_GRADIENT;
        }
    }
    mask
}

/// Edge mask: the pixel's own gradient exceeds the edge cutoff.
pub(crate) fn edge_mask(plane: &LumaPlane) -> Vec<bool> {
    let (w, h) = (plane.width, plane.height);
    let mut mask = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            mask[y * w + x] = gradient_at(plane, x, y) > EDGE_MIN_GRADIENT;
        }
    }
    mask
}

/// Band-edge density per megapixel: pixels lying in horizontal runs of at least
/// [`BAND_RUN_MIN`] consecutive pixels that are flat AND whose vertical neighbor
/// delta falls in `[BAND_DELTA_LO, BAND_DELTA_HI]`.
pub(crate) fn band_edge_density(plane: &LumaPlane, flat: &[bool]) -> f64 {
    let (w, h) = (plane.width, plane.height);
    let mut count = 0usize;
    for y in 0..h {
        let mut run = 0usize;
        for x in 0..w {
            let vdelta = if y + 1 < h {
                (plane.at(x, y + 1) - plane.at(x, y)).abs()
            } else {
                0.0
            };
            let qualifies = flat[y * w + x] && (BAND_DELTA_LO..=BAND_DELTA_HI).contains(&vdelta);
            if qualifies {
                run += 1;
            } else {
                if run >= BAND_RUN_MIN {
                    count += run;
                }
                run = 0;
            }
        }
        if run >= BAND_RUN_MIN {
            count += run;
        }
    }
    count as f64 / (w * h) as f64 * 1_000_000.0
}

/// Mean absolute luma difference over edge-mask pixels.
pub(crate) fn edge_dmean(out: &LumaPlane, src: &LumaPlane, edge: &[bool]) -> f64 {
    let mut total = 0.0f64;
    let mut n = 0usize;
    for (i, &is_edge) in edge.iter().enumerate() {
        if is_edge {
            total += (out.data[i] - src.data[i]).abs() as f64;
            n += 1;
        }
    }
    if n == 0 {
        0.0
    } else {
        total / n as f64
    }
}

/// High-pass noise standard deviation over flat-mask pixels: `out` minus its own
/// 3x3 box blur, sampled where the mask is set.
pub(crate) fn flat_noise_stddev(out: &LumaPlane, flat: &[bool]) -> f64 {
    let (w, h) = (out.width, out.height);
    let mut values = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if !flat[y * w + x] {
                continue;
            }
            let mut sum = 0.0f32;
            let mut n = 0.0f32;
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(h - 1);
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(w - 1);
            for ny in y0..=y1 {
                for nx in x0..=x1 {
                    sum += out.at(nx, ny);
                    n += 1.0;
                }
            }
            values.push((out.at(x, y) - sum / n) as f64);
        }
    }
    if values.is_empty() {
        return 0.0;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
    var.sqrt()
}

/// The synthetic banding fixture as tightly packed RGBA (gray, `a = 255`):
/// top 2/3 a vertical luma gradient 8..48, bottom 1/3 line art (235 background,
/// 12-luma strokes), pushed through a JPEG quality-80 round-trip to bake in real
/// banding. Panics on encode/decode failure (test-only).
pub(crate) fn synthetic_banding_fixture() -> (Vec<u8>, u32, u32) {
    let w = FIXTURE_WIDTH as usize;
    let h = FIXTURE_HEIGHT as usize;
    let gradient_rows = h * 2 / 3;
    // A short steep ramp bridges the gradient (luma 48) to the 235 line-art
    // background so the two regions don't meet at a hard seam. A hard 48->235
    // edge produces one row of JPEG ringing that deband correctly keeps (it
    // borders a strong edge), which would otherwise register as a spurious band.
    // The ramp is far too steep to read as flat, so it never counts as a band.
    let transition = 24usize;
    let art_top = gradient_rows + transition;
    let mut rgb = vec![0u8; w * h * 3];
    for y in 0..h {
        let luma = if y < gradient_rows {
            // Smooth 8..48 ramp; quantized to 8-bit it forms the bands JPEG keeps.
            let t = y as f32 / (gradient_rows.max(1) - 1).max(1) as f32;
            (8.0 + (48.0 - 8.0) * t).round() as u8
        } else if y < art_top {
            let t = (y - gradient_rows) as f32 / transition as f32;
            (48.0 + (235.0 - 48.0) * t).round() as u8
        } else {
            235
        };
        for x in 0..w {
            let idx = (y * w + x) * 3;
            rgb[idx] = luma;
            rgb[idx + 1] = luma;
            rgb[idx + 2] = luma;
        }
    }
    // Line art below the transition: 12 horizontal 3px bars, 8 vertical 2px bars.
    let art_h = h - art_top;
    for bar in 0..12 {
        let y = art_top + (bar + 1) * art_h / 13;
        for dy in 0..3 {
            let row = (y + dy).min(h - 1);
            for x in 0..w {
                paint_stroke(&mut rgb, w, x, row);
            }
        }
    }
    for bar in 0..8 {
        let x = (bar + 1) * w / 9;
        for dx in 0..2 {
            let col = (x + dx).min(w - 1);
            for y in art_top..h {
                paint_stroke(&mut rgb, w, col, y);
            }
        }
    }

    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 80)
        .encode(&rgb, FIXTURE_WIDTH, FIXTURE_HEIGHT, ExtendedColorType::Rgb8)
        .expect("jpeg encode of the deband fixture should succeed");
    let decoded = image::load_from_memory_with_format(&jpeg, ImageFormat::Jpeg)
        .expect("jpeg decode of the deband fixture should succeed")
        .to_rgb8();

    let mut rgba = vec![0u8; w * h * 4];
    for (i, px) in decoded.pixels().enumerate() {
        rgba[i * 4] = px[0];
        rgba[i * 4 + 1] = px[1];
        rgba[i * 4 + 2] = px[2];
        rgba[i * 4 + 3] = 255;
    }
    (rgba, FIXTURE_WIDTH, FIXTURE_HEIGHT)
}

fn paint_stroke(rgb: &mut [u8], width: usize, x: usize, y: usize) {
    let idx = (y * width + x) * 3;
    rgb[idx] = 12;
    rgb[idx + 1] = 12;
    rgb[idx + 2] = 12;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ground-truth quality scan on the synthetic fixture. Ignored by default
    /// (it JPEG-round-trips a 1.1MP page and runs the CPU reference at every
    /// strength); run with:
    ///   cargo test --release deband_quality_scan -- --ignored
    #[test]
    #[ignore = "quality scan: run explicitly with --ignored"]
    fn deband_quality_scan() {
        let (src_rgba, w, h) = synthetic_banding_fixture();
        let src_luma = luma_plane(&src_rgba, w, h);
        let src_flat = flat_mask(&src_luma);
        let src_edge = edge_mask(&src_luma);
        let src_density = band_edge_density(&src_luma, &src_flat);

        println!(
            "{:<8} {:>14} {:>12} {:>14}",
            "strength", "band_density", "edge_dmean", "flat_noise_sd"
        );
        println!(
            "{:<8} {:>14.1} {:>12} {:>14}",
            "source", src_density, "-", "-"
        );

        let mut medium: Option<(f64, f64, f64)> = None;
        for strength in [
            DebandStrength::Weak,
            DebandStrength::Medium,
            DebandStrength::Strong,
        ] {
            let params = strength
                .params()
                .expect("non-Off strengths resolve to params");
            let mut out_rgba = src_rgba.clone();
            deband_rgba(&mut out_rgba, w, h, params);
            let out_luma = luma_plane(&out_rgba, w, h);
            let out_flat = flat_mask(&out_luma);
            let density = band_edge_density(&out_luma, &out_flat);
            let dmean = edge_dmean(&out_luma, &src_luma, &src_edge);
            let noise = flat_noise_stddev(&out_luma, &out_flat);
            println!(
                "{:<8} {:>14.1} {:>12.4} {:>14.4}",
                strength.token(),
                density,
                dmean,
                noise
            );
            if strength == DebandStrength::Medium {
                medium = Some((density, dmean, noise));
            }
        }

        assert!(
            src_density > 10_000.0,
            "source band density {src_density:.1} should exceed 10000/MP"
        );
        let (density, dmean, noise) = medium.expect("medium strength was scanned");
        assert!(
            density < 200.0,
            "medium band density {density:.1} should drop below 200/MP"
        );
        assert!(
            dmean < 1.0,
            "medium edge contamination {dmean:.4} should stay below 1.0/255"
        );
        assert!(
            noise < 1.2,
            "medium flat noise stddev {noise:.4} should stay below 1.2"
        );
    }
}
