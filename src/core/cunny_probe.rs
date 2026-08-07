//! Structural probes for a CuNNy WGSL port: does it behave like a convolution
//! chain at all, and does it treat the two axes alike?
//!
//! `cunny_stage_stats` answers "did a pass die"; it found the broken 32-feature
//! ports have perfectly healthy-looking activations, so the fault produces
//! plausible-but-wrong values. This module goes after the next question, which is
//! whether the *wiring* is wrong — specifically whether the port's feature-tile
//! mapping is transposed or displaced.
//!
//! The porter numbers tiles row-major when it writes them
//! (`packed_slot = x + y * width_mul`) and two-per-source when it reads them
//! (`index(source) * 2 + mode`). Both schemes produce the same slot range at every
//! tile width, so reading the code cannot settle which is right — but a wrong
//! mapping has to show up in what the shader computes:
//!
//! - **Impulse** — a lone bright source pixel. A convolution chain answers with
//!   one compact lobe centred on that pixel's 2x position, no wider than its
//!   receptive field. A misrouted tile shows up as a displaced centroid, a
//!   scattered response, or more than one lobe.
//! - **Flat field** — a constant source. Any sane upscaler returns the same
//!   constant. Deviation means a bias or normalisation error, which is
//!   independent of wiring.
//! - **Step edges, one per axis** — a horizontal and a vertical edge of the same
//!   contrast. These models are not perfectly isotropic, but a port whose x and y
//!   tile axes are swapped answers them very differently, while a healthy sibling
//!   of the same family answers them alike. The comparison against that sibling is
//!   the measurement; the absolute numbers are not meaningful on their own.
//!
//! Everything is reported numerically. Nothing here requires looking at an image,
//! and the probes are synthesised in code so a run is reproducible anywhere.

use crate::core::state::WgpuUpscaleMethod;
use crate::core::upscale_bench::gpu::GpuUpscaleBench;
use egui::{Color32, ColorImage};
use serde::Serialize;
use std::path::PathBuf;

/// Source edge of each synthetic probe. Large enough that the receptive field
/// sits well inside the frame, small enough to stay instant.
pub const DEFAULT_PROBE_EDGE: u32 = 64;

#[derive(Debug, Serialize)]
pub struct CunnyProbeReport {
    pub method: String,
    pub probe_edge: u32,
    pub impulse: ImpulseResponse,
    pub flat_field: FlatField,
    pub horizontal_edge: EdgeResponse,
    pub vertical_edge: EdgeResponse,
    /// How differently the two axes were treated. Large here, small for a
    /// known-good sibling, is the signature of a transposed tile mapping.
    pub axis_asymmetry: AxisAsymmetry,
}

#[derive(Debug, Serialize)]
pub struct ImpulseResponse {
    /// Centroid of the response minus where the impulse should land (2x the
    /// source position). A correct chain is within a pixel or so of zero.
    pub centroid_offset_x: f64,
    pub centroid_offset_y: f64,
    /// Radius around the centroid holding 95% of the response energy, in output
    /// pixels. Bounded by the receptive field for a correct chain.
    pub radius95: f64,
    /// Separate blobs above 10% of peak. More than one means the response was
    /// split, which a single convolution chain cannot do.
    pub lobes: usize,
    pub peak: u8,
}

#[derive(Debug, Serialize)]
pub struct FlatField {
    pub input_level: u8,
    pub mean: f64,
    pub std_dev: f64,
    pub min: u8,
    pub max: u8,
}

#[derive(Debug, Serialize)]
pub struct EdgeResponse {
    /// Output pixels taken to go from 10% to 90% across the edge. Sharpening
    /// narrows it; a broken chain smears or ripples.
    pub transition_width: f64,
    /// How far the profile shoots past the plateaus, in levels. Some overshoot is
    /// the point of these models; a lot of it is ringing.
    pub overshoot: f64,
    pub undershoot: f64,
    pub low_plateau: f64,
    pub high_plateau: f64,
}

#[derive(Debug, Serialize)]
pub struct AxisAsymmetry {
    pub transition_width_delta: f64,
    pub overshoot_delta: f64,
}

pub fn run_cunny_probe(
    method: WgpuUpscaleMethod,
    probe_edge: u32,
) -> Result<CunnyProbeReport, String> {
    let edge = probe_edge.max(16) as usize;
    let gpu = GpuUpscaleBench::new_for_method(Some(method))?;
    let out = [edge * 2, edge * 2];

    let impulse = gpu.apply(&impulse_probe(edge), out, method)?.image;
    let flat = gpu.apply(&flat_probe(edge, 128), out, method)?.image;
    let horizontal = gpu.apply(&edge_probe(edge, true), out, method)?.image;
    let vertical = gpu.apply(&edge_probe(edge, false), out, method)?.image;

    let horizontal_edge = analyse_edge(&horizontal, true);
    let vertical_edge = analyse_edge(&vertical, false);
    Ok(CunnyProbeReport {
        method: method.token().to_owned(),
        probe_edge: edge as u32,
        impulse: analyse_impulse(&impulse, [edge, edge]),
        flat_field: analyse_flat(&flat, 128),
        axis_asymmetry: AxisAsymmetry {
            transition_width_delta: (horizontal_edge.transition_width
                - vertical_edge.transition_width)
                .abs(),
            overshoot_delta: (horizontal_edge.overshoot - vertical_edge.overshoot).abs(),
        },
        horizontal_edge,
        vertical_edge,
    })
}

fn luma(image: &ColorImage, x: usize, y: usize) -> f64 {
    let p = image.pixels[y * image.size[0] + x];
    0.2126 * p.r() as f64 + 0.7152 * p.g() as f64 + 0.0722 * p.b() as f64
}

/// A lone bright pixel at the centre of a black field.
fn impulse_probe(edge: usize) -> ColorImage {
    let mut pixels = vec![Color32::BLACK; edge * edge];
    pixels[(edge / 2) * edge + edge / 2] = Color32::WHITE;
    ColorImage::new([edge, edge], pixels)
}

fn flat_probe(edge: usize, level: u8) -> ColorImage {
    ColorImage::new([edge, edge], vec![Color32::from_gray(level); edge * edge])
}

/// Half dark, half light. `horizontal_edge` puts the boundary across the y axis
/// (so the transition runs along x) — i.e. a vertical line splitting left/right.
fn edge_probe(edge: usize, horizontal_edge: bool) -> ColorImage {
    let mut pixels = vec![Color32::BLACK; edge * edge];
    for y in 0..edge {
        for x in 0..edge {
            let past = if horizontal_edge { x } else { y } >= edge / 2;
            pixels[y * edge + x] = Color32::from_gray(if past { 220 } else { 35 });
        }
    }
    ColorImage::new([edge, edge], pixels)
}

fn analyse_impulse(image: &ColorImage, source_size: [usize; 2]) -> ImpulseResponse {
    let [w, h] = image.size;
    let mut peak = 0.0_f64;
    let mut sum = 0.0;
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    for y in 0..h {
        for x in 0..w {
            let v = luma(image, x, y);
            peak = peak.max(v);
            sum += v;
            sum_x += v * x as f64;
            sum_y += v * y as f64;
        }
    }
    let (cx, cy) = if sum > 0.0 {
        (sum_x / sum, sum_y / sum)
    } else {
        (0.0, 0.0)
    };
    // The impulse sat at the source centre, so a correct 2x chain answers around
    // twice that (the half-pixel offset is inside the tolerance we care about).
    let expected_x = (source_size[0] / 2) as f64 * 2.0;
    let expected_y = (source_size[1] / 2) as f64 * 2.0;

    let mut energies: Vec<(f64, f64)> = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let v = luma(image, x, y);
            if v <= 0.0 {
                continue;
            }
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            energies.push(((dx * dx + dy * dy).sqrt(), v));
        }
    }
    energies.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let target = sum * 0.95;
    let mut running = 0.0;
    let mut radius95 = 0.0;
    for (radius, energy) in &energies {
        running += energy;
        if running >= target {
            radius95 = *radius;
            break;
        }
    }

    ImpulseResponse {
        centroid_offset_x: cx - expected_x,
        centroid_offset_y: cy - expected_y,
        radius95,
        lobes: count_lobes(image, peak * 0.1),
        peak: peak.round().clamp(0.0, 255.0) as u8,
    }
}

/// Connected regions above `threshold`, 4-connected. One convolution chain
/// answering one impulse must produce exactly one.
fn count_lobes(image: &ColorImage, threshold: f64) -> usize {
    let [w, h] = image.size;
    let mut seen = vec![false; w * h];
    let mut lobes = 0;
    for start in 0..w * h {
        if seen[start] || luma(image, start % w, start / w) < threshold {
            continue;
        }
        lobes += 1;
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(index) = stack.pop() {
            let (x, y) = (index % w, index / w);
            let push = |nx: usize, ny: usize, stack: &mut Vec<usize>, seen: &mut Vec<bool>| {
                let next = ny * w + nx;
                if !seen[next] && luma(image, nx, ny) >= threshold {
                    seen[next] = true;
                    stack.push(next);
                }
            };
            if x > 0 {
                push(x - 1, y, &mut stack, &mut seen);
            }
            if x + 1 < w {
                push(x + 1, y, &mut stack, &mut seen);
            }
            if y > 0 {
                push(x, y - 1, &mut stack, &mut seen);
            }
            if y + 1 < h {
                push(x, y + 1, &mut stack, &mut seen);
            }
        }
    }
    lobes
}

fn analyse_flat(image: &ColorImage, input_level: u8) -> FlatField {
    let [w, h] = image.size;
    let count = (w * h) as f64;
    let mut sum = 0.0;
    let mut sum_sq = 0.0;
    let mut min = f64::MAX;
    let mut max = f64::MIN;
    for y in 0..h {
        for x in 0..w {
            let v = luma(image, x, y);
            sum += v;
            sum_sq += v * v;
            min = min.min(v);
            max = max.max(v);
        }
    }
    let mean = sum / count;
    FlatField {
        input_level,
        mean,
        std_dev: (sum_sq / count - mean * mean).max(0.0).sqrt(),
        min: min.round().clamp(0.0, 255.0) as u8,
        max: max.round().clamp(0.0, 255.0) as u8,
    }
}

/// Average the profile across the edge, then measure it. Averaging along the edge
/// keeps one noisy row from deciding the answer.
fn analyse_edge(image: &ColorImage, horizontal_edge: bool) -> EdgeResponse {
    let [w, h] = image.size;
    let (across, along) = if horizontal_edge { (w, h) } else { (h, w) };
    let profile: Vec<f64> = (0..across)
        .map(|i| {
            let total: f64 = (0..along)
                .map(|j| {
                    if horizontal_edge {
                        luma(image, i, j)
                    } else {
                        luma(image, j, i)
                    }
                })
                .sum();
            total / along as f64
        })
        .collect();
    analyse_profile(&profile)
}

fn analyse_profile(profile: &[f64]) -> EdgeResponse {
    if profile.len() < 8 {
        return EdgeResponse {
            transition_width: 0.0,
            overshoot: 0.0,
            undershoot: 0.0,
            low_plateau: 0.0,
            high_plateau: 0.0,
        };
    }
    // Plateaus are read well away from the edge so the transition cannot skew them.
    let quarter = profile.len() / 4;
    let eighth = profile.len() / 8;
    let low_plateau = mean(&profile[eighth..quarter]);
    let high_plateau = mean(&profile[profile.len() - quarter..profile.len() - eighth]);
    let span = high_plateau - low_plateau;
    if span.abs() < 1.0 {
        return EdgeResponse {
            transition_width: profile.len() as f64,
            overshoot: 0.0,
            undershoot: 0.0,
            low_plateau,
            high_plateau,
        };
    }

    let at = |fraction: f64| low_plateau + span * fraction;
    let first_at = |level: f64| {
        profile
            .iter()
            .position(|v| (*v - low_plateau) / span >= (level - low_plateau) / span)
            .unwrap_or(0) as f64
    };
    let transition_width = (first_at(at(0.9)) - first_at(at(0.1))).abs();

    let peak = profile.iter().copied().fold(f64::MIN, f64::max);
    let valley = profile.iter().copied().fold(f64::MAX, f64::min);
    EdgeResponse {
        transition_width,
        overshoot: (peak - high_plateau).max(0.0),
        undershoot: (low_plateau - valley).max(0.0),
        low_plateau,
        high_plateau,
    }
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

pub fn print_cunny_probe_report(report: &CunnyProbeReport) {
    println!("SuiSuiView CuNNy structural probe");
    println!(
        "Method: {}   probe {}x{} -> {}x{}",
        report.method,
        report.probe_edge,
        report.probe_edge,
        report.probe_edge * 2,
        report.probe_edge * 2
    );
    println!();
    let i = &report.impulse;
    println!(
        "impulse    centroid offset ({:+.2}, {:+.2}) px   radius95 {:.2} px   lobes {}   peak {}",
        i.centroid_offset_x, i.centroid_offset_y, i.radius95, i.lobes, i.peak
    );
    let f = &report.flat_field;
    println!(
        "flat {:>3}    mean {:.2}   stddev {:.3}   range {}..{}",
        f.input_level, f.mean, f.std_dev, f.min, f.max
    );
    for (name, e) in [
        ("h-edge", &report.horizontal_edge),
        ("v-edge", &report.vertical_edge),
    ] {
        println!(
            "{name}     width {:.2} px   overshoot {:.2}   undershoot {:.2}   plateaus {:.1}/{:.1}",
            e.transition_width, e.overshoot, e.undershoot, e.low_plateau, e.high_plateau
        );
    }
    println!(
        "asymmetry  width delta {:.2}   overshoot delta {:.2}",
        report.axis_asymmetry.transition_width_delta, report.axis_asymmetry.overshoot_delta
    );
    println!();
    println!("Read against a known-good sibling of the same family, not on its own:");
    println!("  centroid far from 0, lobes > 1, or a large radius95  -> tile mapping is wrong");
    println!("  those look sane but the asymmetry is much larger     -> x/y tile axes swapped");
    println!("  everything structural matches, values differ         -> weights, not wiring");
}

pub fn default_cunny_probe_report_path() -> PathBuf {
    PathBuf::from("bench-output").join("cunny-probe.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray_image(size: [usize; 2], f: impl Fn(usize, usize) -> u8) -> ColorImage {
        let mut pixels = Vec::with_capacity(size[0] * size[1]);
        for y in 0..size[1] {
            for x in 0..size[0] {
                pixels.push(Color32::from_gray(f(x, y)));
            }
        }
        ColorImage::new(size, pixels)
    }

    #[test]
    fn a_single_compact_blob_reads_as_one_centred_lobe() {
        // A 2x2 blob at the expected 2x position of a 32x32 source's centre.
        let image = gray_image([64, 64], |x, y| {
            if (32..34).contains(&x) && (32..34).contains(&y) {
                255
            } else {
                0
            }
        });
        let response = analyse_impulse(&image, [32, 32]);
        assert_eq!(response.lobes, 1);
        assert!(
            response.centroid_offset_x.abs() < 1.5 && response.centroid_offset_y.abs() < 1.5,
            "centroid {:?}",
            (response.centroid_offset_x, response.centroid_offset_y)
        );
        assert!(response.radius95 < 3.0, "radius95 {}", response.radius95);
    }

    #[test]
    fn a_split_response_is_counted_as_two_lobes() {
        // Exactly the shape a misrouted tile would produce: the same energy
        // delivered to two places.
        let image = gray_image([64, 64], |x, y| {
            let left = (10..12).contains(&x) && (32..34).contains(&y);
            let right = (50..52).contains(&x) && (32..34).contains(&y);
            if left || right {
                255
            } else {
                0
            }
        });
        let response = analyse_impulse(&image, [32, 32]);
        assert_eq!(response.lobes, 2);
        assert!(
            response.radius95 > 15.0,
            "a split response is not compact: {}",
            response.radius95
        );
    }

    #[test]
    fn a_displaced_response_shows_up_in_the_centroid() {
        let image = gray_image([64, 64], |x, y| {
            if (44..46).contains(&x) && (32..34).contains(&y) {
                255
            } else {
                0
            }
        });
        let response = analyse_impulse(&image, [32, 32]);
        assert_eq!(response.lobes, 1);
        assert!(
            response.centroid_offset_x > 10.0,
            "offset {}",
            response.centroid_offset_x
        );
        assert!(response.centroid_offset_y.abs() < 1.5);
    }

    #[test]
    fn a_constant_field_has_no_spread() {
        let flat = analyse_flat(&gray_image([32, 32], |_, _| 128), 128);
        assert!((flat.mean - 128.0).abs() < 0.5, "mean {}", flat.mean);
        assert!(flat.std_dev < 0.01, "std_dev {}", flat.std_dev);
    }

    #[test]
    fn a_sharp_step_measures_a_narrow_transition_and_no_ringing() {
        let profile: Vec<f64> = (0..64).map(|i| if i < 32 { 35.0 } else { 220.0 }).collect();
        let response = analyse_profile(&profile);
        assert!(
            response.transition_width <= 1.0,
            "width {}",
            response.transition_width
        );
        assert!(response.overshoot < 0.01 && response.undershoot < 0.01);
        assert!((response.low_plateau - 35.0).abs() < 0.5);
        assert!((response.high_plateau - 220.0).abs() < 0.5);
    }

    #[test]
    fn ringing_past_the_plateaus_is_reported() {
        let profile: Vec<f64> = (0..64)
            .map(|i| match i {
                30 => 20.0,  // undershoot before the rise
                33 => 245.0, // overshoot after it
                i if i < 32 => 35.0,
                _ => 220.0,
            })
            .collect();
        let response = analyse_profile(&profile);
        assert!(
            response.overshoot > 20.0,
            "overshoot {}",
            response.overshoot
        );
        assert!(
            response.undershoot > 10.0,
            "undershoot {}",
            response.undershoot
        );
    }

    #[test]
    fn probes_are_the_shape_the_chain_expects() {
        let impulse = impulse_probe(16);
        assert_eq!(impulse.size, [16, 16]);
        let lit: Vec<usize> = impulse
            .pixels
            .iter()
            .enumerate()
            .filter(|(_, p)| **p != Color32::BLACK)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(lit, vec![8 * 16 + 8], "exactly one lit source pixel");

        // The two edge probes must be transposes of each other, or the axis
        // comparison would be measuring the probes rather than the shader.
        let horizontal = edge_probe(16, true);
        let vertical = edge_probe(16, false);
        for y in 0..16 {
            for x in 0..16 {
                assert_eq!(
                    horizontal.pixels[y * 16 + x],
                    vertical.pixels[x * 16 + y],
                    "probe mismatch at ({x}, {y})"
                );
            }
        }
    }
}
