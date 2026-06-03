use serde::Serialize;

const THUMB_MAX_SIDE: usize = 128;
const FEATURE_COUNT: usize = 29;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum AutoKind {
    Anime,
    MangaBw,
    Photo,
    Webtoon,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AutoKindPrediction {
    pub kind: AutoKind,
    pub confidence: f32,
    pub probabilities: [f32; 4],
}

impl AutoKindPrediction {
    pub fn probability(self, kind: AutoKind) -> f32 {
        self.probabilities[kind_index(kind)]
    }
}

#[derive(Debug, Clone)]
struct FeatureSet {
    values: [f64; FEATURE_COUNT],
}

pub fn classify_rgba(rgba: &[u8], width: usize, height: usize) -> Option<AutoKindPrediction> {
    let features = extract_features_rgba(rgba, width, height)?;
    Some(predict(&features))
}

fn extract_features_rgba(rgba: &[u8], width: usize, height: usize) -> Option<FeatureSet> {
    if width == 0 || height == 0 {
        return None;
    }
    let expected_len = width.checked_mul(height)?.checked_mul(4)?;
    if rgba.len() < expected_len {
        return None;
    }

    let long_edge = width.max(height);
    let scale = if long_edge > THUMB_MAX_SIDE {
        THUMB_MAX_SIDE as f64 / long_edge as f64
    } else {
        1.0
    };
    let thumb_width = ((width as f64 * scale).round() as usize).max(1);
    let thumb_height = ((height as f64 * scale).round() as usize).max(1);
    let pixels = thumb_width.saturating_mul(thumb_height).max(1);

    let mut luma = Vec::with_capacity(pixels);
    let mut hist4 = vec![0usize; 4096];
    let mut hist5 = vec![0usize; 32768];

    let mut sat_sum = 0.0;
    let mut sat_values = Vec::with_capacity(pixels);
    let mut chroma_sum = 0.0;
    let mut grayish_count = 0usize;
    let mut near_white_count = 0usize;
    let mut near_black_count = 0usize;
    let mut paper_or_ink_count = 0usize;
    let mut luma_sum = 0.0;
    let mut luma_sq_sum = 0.0;

    for y in 0..thumb_height {
        let src_y = sample_coord(y, height, thumb_height);
        for x in 0..thumb_width {
            let src_x = sample_coord(x, width, thumb_width);
            let offset = (src_y * width + src_x) * 4;
            let r_u8 = rgba[offset];
            let g_u8 = rgba[offset + 1];
            let b_u8 = rgba[offset + 2];
            let r = f64::from(r_u8) / 255.0;
            let g = f64::from(g_u8) / 255.0;
            let b = f64::from(b_u8) / 255.0;
            let maxc = r.max(g).max(b);
            let minc = r.min(g).min(b);
            let chroma = maxc - minc;
            let sat = chroma / maxc.max(1.0 / 255.0);
            let y_luma = 0.299 * r + 0.587 * g + 0.114 * b;

            sat_sum += sat;
            sat_values.push(sat);
            chroma_sum += chroma;
            luma_sum += y_luma;
            luma_sq_sum += y_luma * y_luma;
            luma.push(y_luma);

            let grayish = chroma < 0.035;
            let near_white = y_luma > 0.90 && sat < 0.18;
            let near_black = y_luma < 0.15;
            if grayish {
                grayish_count += 1;
            }
            if near_white {
                near_white_count += 1;
            }
            if near_black {
                near_black_count += 1;
            }
            if near_white || near_black {
                paper_or_ink_count += 1;
            }

            let q4r = usize::from(r_u8) * 15 / 255;
            let q4g = usize::from(g_u8) * 15 / 255;
            let q4b = usize::from(b_u8) * 15 / 255;
            hist4[q4r * 256 + q4g * 16 + q4b] += 1;

            let q5r = usize::from(r_u8) * 31 / 255;
            let q5g = usize::from(g_u8) * 31 / 255;
            let q5b = usize::from(b_u8) * 31 / 255;
            hist5[q5r * 1024 + q5g * 32 + q5b] += 1;
        }
    }

    let mut grad = Vec::with_capacity(pixels);
    let mut edge_count = 0usize;
    let mut strong_edge_count = 0usize;
    let mut tiny_grad_count = 0usize;
    let mut small_grad_count = 0usize;
    let mut medium_grad_count = 0usize;
    let mut grad_sum = 0.0;
    let mut flat_count = 0usize;
    let mut lap_sum = 0.0;

    for y in 0..thumb_height {
        for x in 0..thumb_width {
            let index = y * thumb_width + x;
            let gx = if x > 0 && x + 1 < thumb_width {
                (luma[index + 1] - luma[index - 1]).abs() * 0.5
            } else {
                0.0
            };
            let gy = if y > 0 && y + 1 < thumb_height {
                (luma[index + thumb_width] - luma[index - thumb_width]).abs() * 0.5
            } else {
                0.0
            };
            let value = (gx * gx + gy * gy).sqrt();
            if value > 0.060 {
                edge_count += 1;
            }
            if value > 0.140 {
                strong_edge_count += 1;
            }
            if value > 0.002 && value < 0.012 {
                tiny_grad_count += 1;
            }
            if (0.012..0.045).contains(&value) {
                small_grad_count += 1;
            }
            if (0.045..0.120).contains(&value) {
                medium_grad_count += 1;
            }
            if value < 0.010 {
                flat_count += 1;
            }
            grad_sum += value;
            grad.push(value);

            if x > 0 && x + 1 < thumb_width && y > 0 && y + 1 < thumb_height {
                let lap = (4.0 * luma[index]
                    - luma[index - thumb_width]
                    - luma[index + thumb_width]
                    - luma[index - 1]
                    - luma[index + 1])
                    .abs();
                lap_sum += lap;
            }
        }
    }

    let inv_pixels = 1.0 / pixels as f64;
    let color_bin_count = hist4.iter().filter(|&&count| count > 0).count();
    let color_bin5_count = hist5.iter().filter(|&&count| count > 0).count();
    let mut nonzero_hist5 = hist5
        .iter()
        .copied()
        .filter(|&count| count > 0)
        .collect::<Vec<_>>();
    nonzero_hist5.sort_unstable_by(|left, right| right.cmp(left));

    let luma_mean = luma_sum * inv_pixels;
    let luma_variance = (luma_sq_sum * inv_pixels - luma_mean * luma_mean).max(0.0);

    sat_values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    grad.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));

    let values = [
        height as f64 / width.max(1) as f64,
        sat_sum * inv_pixels,
        percentile_sorted(&sat_values, 0.90),
        chroma_sum * inv_pixels,
        grayish_count as f64 * inv_pixels,
        near_white_count as f64 * inv_pixels,
        near_black_count as f64 * inv_pixels,
        paper_or_ink_count as f64 * inv_pixels,
        edge_count as f64 * inv_pixels,
        strong_edge_count as f64 * inv_pixels,
        tiny_grad_count as f64 * inv_pixels,
        small_grad_count as f64 * inv_pixels,
        medium_grad_count as f64 * inv_pixels,
        percentile_sorted(&grad, 0.50),
        percentile_sorted(&grad, 0.75),
        percentile_sorted(&grad, 0.90),
        percentile_sorted(&grad, 0.95),
        grad_sum * inv_pixels,
        flat_count as f64 * inv_pixels,
        lap_sum * inv_pixels,
        color_bin_count as f64,
        color_bin_count as f64 * inv_pixels,
        entropy(&hist4, pixels),
        color_bin5_count as f64,
        color_bin5_count as f64 * inv_pixels,
        entropy(&hist5, pixels),
        top_count_fraction(&nonzero_hist5, 8, pixels),
        top_count_fraction(&nonzero_hist5, 32, pixels),
        luma_variance.sqrt(),
    ];

    Some(FeatureSet { values })
}

fn sample_coord(coord: usize, source_len: usize, target_len: usize) -> usize {
    (((coord as f64 + 0.5) * source_len as f64 / target_len.max(1) as f64).floor() as usize)
        .min(source_len.saturating_sub(1))
}

fn entropy(hist: &[usize], total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    hist.iter()
        .copied()
        .filter(|&count| count > 0)
        .map(|count| {
            let p = count as f64 / total as f64;
            -(p * p.log2())
        })
        .sum()
}

fn top_count_fraction(sorted_counts_desc: &[usize], count: usize, total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    sorted_counts_desc.iter().take(count).sum::<usize>() as f64 / total as f64
}

fn percentile_sorted(values: &[f64], quantile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    if values.len() == 1 {
        return values[0];
    }
    let position = (values.len() - 1) as f64 * quantile.clamp(0.0, 1.0);
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        values[lower]
    } else {
        values[lower] * (upper as f64 - position) + values[upper] * (position - lower as f64)
    }
}

fn predict(features: &FeatureSet) -> AutoKindPrediction {
    let mut logits = [0.0f64; 4];
    for class_index in 0..4 {
        let mut value = INTERCEPT[class_index];
        for feature_index in 0..FEATURE_COUNT {
            let normalized =
                (features.values[feature_index] - MEANS[feature_index]) / SCALES[feature_index];
            value += COEFFICIENTS[class_index][feature_index] * normalized;
        }
        logits[class_index] = value;
    }

    let max_logit = logits.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mut probabilities = [0.0f32; 4];
    let mut sum = 0.0;
    for (index, logit) in logits.iter().copied().enumerate() {
        let exp = (logit - max_logit).exp();
        sum += exp;
        probabilities[index] = exp as f32;
    }
    if sum > 0.0 {
        for probability in &mut probabilities {
            *probability /= sum as f32;
        }
    }

    let (best_index, confidence) = probabilities
        .iter()
        .copied()
        .enumerate()
        .max_by(|left, right| {
            left.1
                .partial_cmp(&right.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or((kind_index(AutoKind::Photo), 0.0));

    AutoKindPrediction {
        kind: CLASSES[best_index],
        confidence,
        probabilities,
    }
}

const CLASSES: [AutoKind; 4] = [
    AutoKind::Anime,
    AutoKind::MangaBw,
    AutoKind::Photo,
    AutoKind::Webtoon,
];

const fn kind_index(kind: AutoKind) -> usize {
    match kind {
        AutoKind::Anime => 0,
        AutoKind::MangaBw => 1,
        AutoKind::Photo => 2,
        AutoKind::Webtoon => 3,
    }
}

const MEANS: [f64; FEATURE_COUNT] = [
    1.707674864189795,
    0.22717514909371467,
    0.46750844709042993,
    0.11989657851596089,
    0.47426563517013115,
    0.2992567169395781,
    0.1642169273197169,
    0.46347364425929494,
    0.37058539016731895,
    0.22556643866495135,
    0.12335807063506296,
    0.19803001122461,
    0.168360036031425,
    0.03882042154353777,
    0.1422368973913503,
    0.26373696360470994,
    0.32567950560977416,
    0.0908706091244572,
    0.36213743768602263,
    0.29459113108792473,
    241.23214285714286,
    0.024355583450028143,
    4.560512967532362,
    814.6071428571429,
    0.07613000423520064,
    5.758213851967615,
    0.5079916497277486,
    0.6713400004652436,
    0.2693093762001289,
];

const SCALES: [f64; FEATURE_COUNT] = [
    2.5860383302084093,
    0.1788691834172333,
    0.3491504486469663,
    0.10910555229502286,
    0.3627434425617692,
    0.2799408431197497,
    0.19087170632767914,
    0.2543584704010433,
    0.16994877599769043,
    0.14110823727378896,
    0.10806207728180965,
    0.15370789318018446,
    0.08621424077753827,
    0.042582327861225795,
    0.1058273615693978,
    0.13345178321016435,
    0.13560805626673475,
    0.04826968637676489,
    0.19056791768043754,
    0.18703390463518396,
    208.3064784562322,
    0.025670576560341277,
    1.7809817992636592,
    761.6896789978425,
    0.07233171875733606,
    2.425097651824816,
    0.2715149562660915,
    0.2632028250212879,
    0.07551896049068478,
];

const INTERCEPT: [f64; 4] = [
    1.903409676555715,
    -2.759223939019229,
    -1.2526870508130148,
    2.1085013132765478,
];

#[rustfmt::skip]
const COEFFICIENTS: [[f64; FEATURE_COUNT]; 4] = [
    [
        0.10130393615257445, -0.4981718484549818, -0.3424412246232461,
        0.7207464521089433, 0.5124031866563219, -0.33365584412384297,
        0.6139546244350695, 0.09350059551912174, 0.2863616114088823,
        0.6138170061183561, -1.9008781575428495, -0.5808657656341247,
        -0.33713043281419763, -0.4378910342491637, -0.2171819786510926,
        -0.3957593075328638, 2.311676265491913, -0.224040730234224,
        0.6909403299815662, -2.1304615220823337, 2.3354255493468776,
        0.44918273301229966, 2.0331889590589163, -2.2613184996145037,
        -1.524538941406709, 2.3115538176827317, 1.2968094652391589,
        -1.1324950902659647, -1.7390382706537513,
    ],
    [
        0.30714819336133214, 0.44446152922218335, -0.9911347888744939,
        1.7054842791669196, 0.010611485079127108, 0.7479628487911657,
        -0.22841304944349383, 0.6517878557638199, 0.853866438996782,
        0.6588044262469731, 2.036288745251433, 1.2543630515128055,
        -0.600475555366519, 0.3647955109026983, -1.152602246141247,
        0.8402709574177298, 0.726361352075602, -0.4252289427195013,
        -1.8193658909169859, -0.13908559891027936, -1.7321255637301123,
        -1.2392609998110053, -1.3860213018483007, -0.8661321191899275,
        -0.8379879238921341, -1.0262884230332459, 0.8961228786095659,
        0.7948675274185364, 0.52957964744241,
    ],
    [
        -1.702007755419734, -1.135818178135778, 2.546542900167638,
        -0.01132793315013082, 0.31409781932118813, -0.9879381349770783,
        0.4609375118646902, -0.7414115392274883, 0.1362425945046204,
        -0.4058543329119341, -0.6248206829920565, -0.31949952655008035,
        0.7633278883703064, 0.5836337789926028, -0.8304700429539684,
        -1.4423919635813836, -2.0228092721781157, -0.8514276732449253,
        0.06524181050532152, 1.6650955260392282, -0.7021692350981313,
        -1.8481575993066508, 0.23749568183119307, 1.876766587008189,
        -0.07275020081369428, 0.416483480286647, 0.23322912253248662,
        -0.8066977373091697, 2.4780271084263905,
    ],
    [
        1.2935556259058276, 1.1895284973685778, -1.2129668866699028,
        -2.414902798125738, -0.8371124910566368, 0.5736311303097461,
        -0.8464790868562697, -0.003876912055458108, -1.2764706449102812,
        -0.8667670994533941, 0.48941009528348495, -0.3539977593286018,
        0.17427809981041545, -0.5105382556461333, 2.2002542677463155,
        0.9978803136965204, -1.0152283453893847, 1.5006973461986548,
        1.0631837504300814, 0.6044515949533915, 0.09886924948135968,
        2.6382358661053553, -0.8846633390418038, 1.250684031796243,
        2.4352770661125405, -1.7017488749361318, -2.426161466381212,
        1.1443253001565985, -1.2685684852150458,
    ],
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    #[test]
    fn classify_rgba_rejects_invalid_dimensions() {
        assert!(classify_rgba(&[], 0, 12).is_none());
        assert!(classify_rgba(&[0; 12], 2, 2).is_none());
    }

    #[test]
    fn probabilities_sum_to_one_for_solid_image() {
        let rgba = vec![255u8; 64 * 64 * 4];
        let prediction = classify_rgba(&rgba, 64, 64).expect("solid image should classify");
        let sum = prediction.probabilities.iter().sum::<f32>();
        assert!((sum - 1.0).abs() < 0.0001);
        assert!(prediction.confidence >= 0.0 && prediction.confidence <= 1.0);
    }

    #[test]
    fn synthetic_bw_line_art_is_not_confident_anime() {
        let width = 96usize;
        let height = 128usize;
        let mut rgba = vec![255u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let border = x % 24 == 0 || y % 32 == 0 || (x + y) % 37 == 0;
                let offset = (y * width + x) * 4;
                let value = if border { 16 } else { 245 };
                rgba[offset] = value;
                rgba[offset + 1] = value;
                rgba[offset + 2] = value;
                rgba[offset + 3] = 255;
            }
        }
        let prediction = classify_rgba(&rgba, width, height).expect("line art should classify");
        assert!(prediction.probability(AutoKind::Anime) < 0.90);
    }

    #[test]
    #[ignore = "fixture microbench; set SUISUIVIEW_AUTO_KIND_BENCH_IMAGE to a local image"]
    fn auto_kind_fixture_microbench() {
        let path = std::env::var("SUISUIVIEW_AUTO_KIND_BENCH_IMAGE")
            .expect("SUISUIVIEW_AUTO_KIND_BENCH_IMAGE must point to an image");
        let image = image::open(&path).expect("image should open").to_rgba8();
        let width = image.width() as usize;
        let height = image.height() as usize;
        let rgba = image.into_raw();
        let iterations = std::env::var("SUISUIVIEW_AUTO_KIND_BENCH_ITERATIONS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1000)
            .max(1);

        let start = Instant::now();
        let mut last = None;
        for _ in 0..iterations {
            last = classify_rgba(&rgba, width, height);
        }
        let elapsed = start.elapsed();
        let mean_ms = elapsed.as_secs_f64() * 1000.0 / iterations as f64;
        println!(
            "auto_kind_microbench path={path} size={}x{} iterations={} mean_ms={:.4} last={:?}",
            width, height, iterations, mean_ms, last
        );
    }

    #[derive(Debug, Serialize)]
    struct AutoKindScanReport {
        input: String,
        file_count: usize,
        valid_count: usize,
        error_count: usize,
        kind_counts: BTreeMap<&'static str, usize>,
        routes_to_anime4k_m: usize,
        route_rate: f32,
        mean_confidence: f32,
        mean_webtoon_probability: f32,
        mean_photo_probability: f32,
        by_episode: Vec<AutoKindEpisodeSummary>,
        rows: Vec<AutoKindScanRow>,
    }

    #[derive(Debug, Serialize)]
    struct AutoKindEpisodeSummary {
        episode_key: String,
        pages: usize,
        kind_counts: BTreeMap<&'static str, usize>,
        routes_to_anime4k_m: usize,
        mean_confidence: f32,
        mean_webtoon_probability: f32,
        mean_photo_probability: f32,
    }

    #[derive(Debug, Serialize)]
    struct AutoKindScanRow {
        file: String,
        name: String,
        episode_key: String,
        page_index: Option<usize>,
        width: Option<usize>,
        height: Option<usize>,
        kind: Option<&'static str>,
        confidence: Option<f32>,
        probabilities: Option<[f32; 4]>,
        routes_to_anime4k_m: bool,
        error: Option<String>,
    }

    #[derive(Default)]
    struct EpisodeAccumulator {
        pages: usize,
        kind_counts: BTreeMap<&'static str, usize>,
        routes_to_anime4k_m: usize,
        confidence_sum: f32,
        webtoon_probability_sum: f32,
        photo_probability_sum: f32,
    }

    #[test]
    #[ignore = "dataset scan; set SUISUIVIEW_AUTO_KIND_SCAN_DIR to a local image folder"]
    fn auto_kind_dataset_scan() {
        let input = PathBuf::from(
            std::env::var("SUISUIVIEW_AUTO_KIND_SCAN_DIR")
                .expect("SUISUIVIEW_AUTO_KIND_SCAN_DIR must point to an image folder"),
        );
        let mut files = Vec::new();
        collect_image_files(&input, &mut files).expect("scan input should be readable");
        files.sort_by(|left, right| {
            scan_episode_key(left)
                .cmp(&scan_episode_key(right))
                .then(scan_page_index(left).cmp(&scan_page_index(right)))
                .then(left.file_name().cmp(&right.file_name()))
        });

        let mut rows = Vec::with_capacity(files.len());
        let mut kind_counts = BTreeMap::<&'static str, usize>::new();
        let mut by_episode = BTreeMap::<String, EpisodeAccumulator>::new();
        let mut valid_count = 0usize;
        let mut routes_to_anime4k_m = 0usize;
        let mut confidence_sum = 0.0f32;
        let mut webtoon_probability_sum = 0.0f32;
        let mut photo_probability_sum = 0.0f32;

        for path in files {
            let file = path
                .strip_prefix(&input)
                .unwrap_or(path.as_path())
                .display()
                .to_string();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
                .to_owned();
            let episode_key = scan_episode_key(&path);
            let page_index = scan_page_index(&path);

            match image::open(&path) {
                Ok(image) => {
                    let image = image.to_rgba8();
                    let width = image.width() as usize;
                    let height = image.height() as usize;
                    match classify_rgba(&image.into_raw(), width, height) {
                        Some(prediction) => {
                            valid_count += 1;
                            let kind = auto_kind_scan_token(prediction.kind);
                            *kind_counts.entry(kind).or_default() += 1;
                            let routed = routes_auto_kind_to_anime4k_m(prediction);
                            if routed {
                                routes_to_anime4k_m += 1;
                            }
                            confidence_sum += prediction.confidence;
                            webtoon_probability_sum += prediction.probability(AutoKind::Webtoon);
                            photo_probability_sum += prediction.probability(AutoKind::Photo);

                            let episode = by_episode.entry(episode_key.clone()).or_default();
                            episode.pages += 1;
                            *episode.kind_counts.entry(kind).or_default() += 1;
                            if routed {
                                episode.routes_to_anime4k_m += 1;
                            }
                            episode.confidence_sum += prediction.confidence;
                            episode.webtoon_probability_sum +=
                                prediction.probability(AutoKind::Webtoon);
                            episode.photo_probability_sum +=
                                prediction.probability(AutoKind::Photo);

                            rows.push(AutoKindScanRow {
                                file,
                                name,
                                episode_key,
                                page_index,
                                width: Some(width),
                                height: Some(height),
                                kind: Some(kind),
                                confidence: Some(prediction.confidence),
                                probabilities: Some(prediction.probabilities),
                                routes_to_anime4k_m: routed,
                                error: None,
                            });
                        }
                        None => rows.push(AutoKindScanRow {
                            file,
                            name,
                            episode_key,
                            page_index,
                            width: Some(width),
                            height: Some(height),
                            kind: None,
                            confidence: None,
                            probabilities: None,
                            routes_to_anime4k_m: false,
                            error: Some("classifier rejected image".to_owned()),
                        }),
                    }
                }
                Err(error) => rows.push(AutoKindScanRow {
                    file,
                    name,
                    episode_key,
                    page_index,
                    width: None,
                    height: None,
                    kind: None,
                    confidence: None,
                    probabilities: None,
                    routes_to_anime4k_m: false,
                    error: Some(error.to_string()),
                }),
            }
        }

        let by_episode = by_episode
            .into_iter()
            .map(|(episode_key, episode)| AutoKindEpisodeSummary {
                episode_key,
                pages: episode.pages,
                kind_counts: episode.kind_counts,
                routes_to_anime4k_m: episode.routes_to_anime4k_m,
                mean_confidence: mean_f32(episode.confidence_sum, episode.pages),
                mean_webtoon_probability: mean_f32(episode.webtoon_probability_sum, episode.pages),
                mean_photo_probability: mean_f32(episode.photo_probability_sum, episode.pages),
            })
            .collect::<Vec<_>>();

        let report = AutoKindScanReport {
            input: input.display().to_string(),
            file_count: rows.len(),
            valid_count,
            error_count: rows.len().saturating_sub(valid_count),
            kind_counts,
            routes_to_anime4k_m,
            route_rate: mean_f32(routes_to_anime4k_m as f32, valid_count),
            mean_confidence: mean_f32(confidence_sum, valid_count),
            mean_webtoon_probability: mean_f32(webtoon_probability_sum, valid_count),
            mean_photo_probability: mean_f32(photo_probability_sum, valid_count),
            by_episode,
            rows,
        };

        println!(
            "auto_kind_scan input={} valid={}/{} kind_counts={:?} routes_to_anime4k_m={} route_rate={:.3} mean_confidence={:.4}",
            report.input,
            report.valid_count,
            report.file_count,
            report.kind_counts,
            report.routes_to_anime4k_m,
            report.route_rate,
            report.mean_confidence,
        );

        if let Ok(path) = std::env::var("SUISUIVIEW_AUTO_KIND_SCAN_REPORT") {
            let path = PathBuf::from(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("report parent should be created");
            }
            fs::write(
                &path,
                serde_json::to_string_pretty(&report).expect("report should serialize"),
            )
            .expect("report should be written");
            println!("auto_kind_scan_report={}", path.display());
        }
    }

    fn mean_f32(total: f32, count: usize) -> f32 {
        if count == 0 {
            0.0
        } else {
            total / count as f32
        }
    }

    fn routes_auto_kind_to_anime4k_m(prediction: AutoKindPrediction) -> bool {
        match prediction.kind {
            AutoKind::MangaBw => prediction.confidence >= 0.55,
            AutoKind::Anime | AutoKind::Webtoon => prediction.confidence >= 0.65,
            AutoKind::Photo => false,
        }
    }

    fn auto_kind_scan_token(kind: AutoKind) -> &'static str {
        match kind {
            AutoKind::Anime => "anime",
            AutoKind::MangaBw => "manga_bw",
            AutoKind::Photo => "photo",
            AutoKind::Webtoon => "webtoon",
        }
    }

    fn collect_image_files(root: &Path, output: &mut Vec<PathBuf>) -> std::io::Result<()> {
        if root.is_file() {
            if is_image_path(root) {
                output.push(root.to_owned());
            }
            return Ok(());
        }
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            if path.is_dir() {
                collect_image_files(&path, output)?;
            } else if is_image_path(&path) {
                output.push(path);
            }
        }
        Ok(())
    }

    fn is_image_path(path: &Path) -> bool {
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "jpg" | "jpeg" | "png" | "webp" | "bmp"
                )
            })
            .unwrap_or(false)
    }

    fn scan_episode_key(path: &Path) -> String {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        name.split_once("_IMAG01_").map_or_else(
            || "aux".to_owned(),
            |(episode_key, _)| episode_key.to_owned(),
        )
    }

    fn scan_page_index(path: &Path) -> Option<usize> {
        let name = path.file_name().and_then(|name| name.to_str())?;
        let (_, suffix) = name.split_once("_IMAG01_")?;
        let (page, _) = suffix.rsplit_once('.')?;
        page.parse().ok()
    }
}
