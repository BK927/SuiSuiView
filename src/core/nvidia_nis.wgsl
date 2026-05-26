// Benchmark-only scalar WGSL port of NVIDIA Image Scaling NVScaler.
// Derived from NVIDIAImageScaling NIS_Scaler.h and NIS_Config.h (MIT).
// This keeps the SDK coefficient tables and adaptive luma/chroma correction,
// but uses direct per-pixel compute sampling instead of the SDK shared tile
// optimization. See THIRD_PARTY_NOTICES.txt before product exposure.

struct NisParams {
    source_output: vec4<u32>,
    config0: vec4<f32>,
    config1: vec4<f32>,
    config2: vec4<f32>,
    config3: vec4<f32>,
};

@group(0) @binding(0)
var input_texture: texture_2d<f32>;
@group(0) @binding(1)
var output_texture: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2)
var<uniform> params: NisParams;

const K_PHASE_COUNT: u32 = 64u;

const COEF_SCALE: array<array<f32, 6>, 64> = array<array<f32, 6>, 64>(
    array<f32, 6>(0.0, 0.0, 1.0000, 0.0, 0.0, 0.0),
    array<f32, 6>(0.0029, -0.0127, 1.0000, 0.0132, -0.0034, 0.0),
    array<f32, 6>(0.0063, -0.0249, 0.9985, 0.0269, -0.0068, 0.0),
    array<f32, 6>(0.0088, -0.0361, 0.9956, 0.0415, -0.0103, 0.0005),
    array<f32, 6>(0.0117, -0.0474, 0.9932, 0.0562, -0.0142, 0.0005),
    array<f32, 6>(0.0142, -0.0576, 0.9897, 0.0713, -0.0181, 0.0005),
    array<f32, 6>(0.0166, -0.0674, 0.9844, 0.0874, -0.0220, 0.0010),
    array<f32, 6>(0.0186, -0.0762, 0.9785, 0.1040, -0.0264, 0.0015),
    array<f32, 6>(0.0205, -0.0850, 0.9727, 0.1206, -0.0308, 0.0020),
    array<f32, 6>(0.0225, -0.0928, 0.9648, 0.1382, -0.0352, 0.0024),
    array<f32, 6>(0.0239, -0.1006, 0.9575, 0.1558, -0.0396, 0.0029),
    array<f32, 6>(0.0254, -0.1074, 0.9487, 0.1738, -0.0439, 0.0034),
    array<f32, 6>(0.0264, -0.1138, 0.9390, 0.1929, -0.0488, 0.0044),
    array<f32, 6>(0.0278, -0.1191, 0.9282, 0.2119, -0.0537, 0.0049),
    array<f32, 6>(0.0288, -0.1245, 0.9170, 0.2310, -0.0581, 0.0059),
    array<f32, 6>(0.0293, -0.1294, 0.9058, 0.2510, -0.0630, 0.0063),
    array<f32, 6>(0.0303, -0.1333, 0.8926, 0.2710, -0.0679, 0.0073),
    array<f32, 6>(0.0308, -0.1367, 0.8789, 0.2915, -0.0728, 0.0083),
    array<f32, 6>(0.0308, -0.1401, 0.8657, 0.3120, -0.0776, 0.0093),
    array<f32, 6>(0.0313, -0.1426, 0.8506, 0.3330, -0.0825, 0.0103),
    array<f32, 6>(0.0313, -0.1445, 0.8354, 0.3540, -0.0874, 0.0112),
    array<f32, 6>(0.0313, -0.1460, 0.8193, 0.3755, -0.0923, 0.0122),
    array<f32, 6>(0.0313, -0.1470, 0.8022, 0.3965, -0.0967, 0.0137),
    array<f32, 6>(0.0308, -0.1479, 0.7856, 0.4185, -0.1016, 0.0146),
    array<f32, 6>(0.0303, -0.1479, 0.7681, 0.4399, -0.1060, 0.0156),
    array<f32, 6>(0.0298, -0.1479, 0.7505, 0.4614, -0.1104, 0.0166),
    array<f32, 6>(0.0293, -0.1470, 0.7314, 0.4829, -0.1147, 0.0181),
    array<f32, 6>(0.0288, -0.1460, 0.7119, 0.5049, -0.1187, 0.0190),
    array<f32, 6>(0.0278, -0.1445, 0.6929, 0.5264, -0.1226, 0.0200),
    array<f32, 6>(0.0273, -0.1431, 0.6724, 0.5479, -0.1260, 0.0215),
    array<f32, 6>(0.0264, -0.1411, 0.6528, 0.5693, -0.1299, 0.0225),
    array<f32, 6>(0.0254, -0.1387, 0.6323, 0.5903, -0.1328, 0.0234),
    array<f32, 6>(0.0244, -0.1357, 0.6113, 0.6113, -0.1357, 0.0244),
    array<f32, 6>(0.0234, -0.1328, 0.5903, 0.6323, -0.1387, 0.0254),
    array<f32, 6>(0.0225, -0.1299, 0.5693, 0.6528, -0.1411, 0.0264),
    array<f32, 6>(0.0215, -0.1260, 0.5479, 0.6724, -0.1431, 0.0273),
    array<f32, 6>(0.0200, -0.1226, 0.5264, 0.6929, -0.1445, 0.0278),
    array<f32, 6>(0.0190, -0.1187, 0.5049, 0.7119, -0.1460, 0.0288),
    array<f32, 6>(0.0181, -0.1147, 0.4829, 0.7314, -0.1470, 0.0293),
    array<f32, 6>(0.0166, -0.1104, 0.4614, 0.7505, -0.1479, 0.0298),
    array<f32, 6>(0.0156, -0.1060, 0.4399, 0.7681, -0.1479, 0.0303),
    array<f32, 6>(0.0146, -0.1016, 0.4185, 0.7856, -0.1479, 0.0308),
    array<f32, 6>(0.0137, -0.0967, 0.3965, 0.8022, -0.1470, 0.0313),
    array<f32, 6>(0.0122, -0.0923, 0.3755, 0.8193, -0.1460, 0.0313),
    array<f32, 6>(0.0112, -0.0874, 0.3540, 0.8354, -0.1445, 0.0313),
    array<f32, 6>(0.0103, -0.0825, 0.3330, 0.8506, -0.1426, 0.0313),
    array<f32, 6>(0.0093, -0.0776, 0.3120, 0.8657, -0.1401, 0.0308),
    array<f32, 6>(0.0083, -0.0728, 0.2915, 0.8789, -0.1367, 0.0308),
    array<f32, 6>(0.0073, -0.0679, 0.2710, 0.8926, -0.1333, 0.0303),
    array<f32, 6>(0.0063, -0.0630, 0.2510, 0.9058, -0.1294, 0.0293),
    array<f32, 6>(0.0059, -0.0581, 0.2310, 0.9170, -0.1245, 0.0288),
    array<f32, 6>(0.0049, -0.0537, 0.2119, 0.9282, -0.1191, 0.0278),
    array<f32, 6>(0.0044, -0.0488, 0.1929, 0.9390, -0.1138, 0.0264),
    array<f32, 6>(0.0034, -0.0439, 0.1738, 0.9487, -0.1074, 0.0254),
    array<f32, 6>(0.0029, -0.0396, 0.1558, 0.9575, -0.1006, 0.0239),
    array<f32, 6>(0.0024, -0.0352, 0.1382, 0.9648, -0.0928, 0.0225),
    array<f32, 6>(0.0020, -0.0308, 0.1206, 0.9727, -0.0850, 0.0205),
    array<f32, 6>(0.0015, -0.0264, 0.1040, 0.9785, -0.0762, 0.0186),
    array<f32, 6>(0.0010, -0.0220, 0.0874, 0.9844, -0.0674, 0.0166),
    array<f32, 6>(0.0005, -0.0181, 0.0713, 0.9897, -0.0576, 0.0142),
    array<f32, 6>(0.0005, -0.0142, 0.0562, 0.9932, -0.0474, 0.0117),
    array<f32, 6>(0.0005, -0.0103, 0.0415, 0.9956, -0.0361, 0.0088),
    array<f32, 6>(0.0, -0.0068, 0.0269, 0.9985, -0.0249, 0.0063),
    array<f32, 6>(0.0, -0.0034, 0.0132, 1.0000, -0.0127, 0.0029),
);

const COEF_USM: array<array<f32, 6>, 64> = array<array<f32, 6>, 64>(
    array<f32, 6>(0.0, -0.6001, 1.2002, -0.6001, 0.0, 0.0),
    array<f32, 6>(0.0029, -0.6084, 1.1987, -0.5903, -0.0029, 0.0),
    array<f32, 6>(0.0049, -0.6147, 1.1958, -0.5791, -0.0068, 0.0005),
    array<f32, 6>(0.0073, -0.6196, 1.1890, -0.5659, -0.0103, 0.0),
    array<f32, 6>(0.0093, -0.6235, 1.1802, -0.5513, -0.0151, 0.0),
    array<f32, 6>(0.0112, -0.6265, 1.1699, -0.5352, -0.0195, 0.0005),
    array<f32, 6>(0.0122, -0.6270, 1.1582, -0.5181, -0.0259, 0.0005),
    array<f32, 6>(0.0142, -0.6284, 1.1455, -0.5005, -0.0317, 0.0005),
    array<f32, 6>(0.0156, -0.6265, 1.1274, -0.4790, -0.0386, 0.0005),
    array<f32, 6>(0.0166, -0.6235, 1.1089, -0.4570, -0.0454, 0.0010),
    array<f32, 6>(0.0176, -0.6187, 1.0879, -0.4346, -0.0532, 0.0010),
    array<f32, 6>(0.0181, -0.6138, 1.0659, -0.4102, -0.0615, 0.0015),
    array<f32, 6>(0.0190, -0.6069, 1.0405, -0.3843, -0.0698, 0.0015),
    array<f32, 6>(0.0195, -0.6006, 1.0161, -0.3574, -0.0796, 0.0020),
    array<f32, 6>(0.0200, -0.5928, 0.9893, -0.3286, -0.0898, 0.0024),
    array<f32, 6>(0.0200, -0.5820, 0.9580, -0.2988, -0.1001, 0.0029),
    array<f32, 6>(0.0200, -0.5728, 0.9292, -0.2690, -0.1104, 0.0034),
    array<f32, 6>(0.0200, -0.5620, 0.8975, -0.2368, -0.1226, 0.0039),
    array<f32, 6>(0.0205, -0.5498, 0.8643, -0.2046, -0.1343, 0.0044),
    array<f32, 6>(0.0200, -0.5371, 0.8301, -0.1709, -0.1465, 0.0049),
    array<f32, 6>(0.0195, -0.5239, 0.7944, -0.1367, -0.1587, 0.0054),
    array<f32, 6>(0.0195, -0.5107, 0.7598, -0.1021, -0.1724, 0.0059),
    array<f32, 6>(0.0190, -0.4966, 0.7231, -0.0649, -0.1865, 0.0063),
    array<f32, 6>(0.0186, -0.4819, 0.6846, -0.0288, -0.1997, 0.0068),
    array<f32, 6>(0.0186, -0.4668, 0.6460, 0.0093, -0.2144, 0.0073),
    array<f32, 6>(0.0176, -0.4507, 0.6055, 0.0479, -0.2290, 0.0083),
    array<f32, 6>(0.0171, -0.4370, 0.5693, 0.0859, -0.2446, 0.0088),
    array<f32, 6>(0.0161, -0.4199, 0.5283, 0.1255, -0.2598, 0.0098),
    array<f32, 6>(0.0161, -0.4048, 0.4883, 0.1655, -0.2754, 0.0103),
    array<f32, 6>(0.0151, -0.3887, 0.4497, 0.2041, -0.2910, 0.0107),
    array<f32, 6>(0.0142, -0.3711, 0.4072, 0.2446, -0.3066, 0.0117),
    array<f32, 6>(0.0137, -0.3555, 0.3672, 0.2852, -0.3228, 0.0122),
    array<f32, 6>(0.0132, -0.3394, 0.3262, 0.3262, -0.3394, 0.0132),
    array<f32, 6>(0.0122, -0.3228, 0.2852, 0.3672, -0.3555, 0.0137),
    array<f32, 6>(0.0117, -0.3066, 0.2446, 0.4072, -0.3711, 0.0142),
    array<f32, 6>(0.0107, -0.2910, 0.2041, 0.4497, -0.3887, 0.0151),
    array<f32, 6>(0.0103, -0.2754, 0.1655, 0.4883, -0.4048, 0.0161),
    array<f32, 6>(0.0098, -0.2598, 0.1255, 0.5283, -0.4199, 0.0161),
    array<f32, 6>(0.0088, -0.2446, 0.0859, 0.5693, -0.4370, 0.0171),
    array<f32, 6>(0.0083, -0.2290, 0.0479, 0.6055, -0.4507, 0.0176),
    array<f32, 6>(0.0073, -0.2144, 0.0093, 0.6460, -0.4668, 0.0186),
    array<f32, 6>(0.0068, -0.1997, -0.0288, 0.6846, -0.4819, 0.0186),
    array<f32, 6>(0.0063, -0.1865, -0.0649, 0.7231, -0.4966, 0.0190),
    array<f32, 6>(0.0059, -0.1724, -0.1021, 0.7598, -0.5107, 0.0195),
    array<f32, 6>(0.0054, -0.1587, -0.1367, 0.7944, -0.5239, 0.0195),
    array<f32, 6>(0.0049, -0.1465, -0.1709, 0.8301, -0.5371, 0.0200),
    array<f32, 6>(0.0044, -0.1343, -0.2046, 0.8643, -0.5498, 0.0205),
    array<f32, 6>(0.0039, -0.1226, -0.2368, 0.8975, -0.5620, 0.0200),
    array<f32, 6>(0.0034, -0.1104, -0.2690, 0.9292, -0.5728, 0.0200),
    array<f32, 6>(0.0029, -0.1001, -0.2988, 0.9580, -0.5820, 0.0200),
    array<f32, 6>(0.0024, -0.0898, -0.3286, 0.9893, -0.5928, 0.0200),
    array<f32, 6>(0.0020, -0.0796, -0.3574, 1.0161, -0.6006, 0.0195),
    array<f32, 6>(0.0015, -0.0698, -0.3843, 1.0405, -0.6069, 0.0190),
    array<f32, 6>(0.0015, -0.0615, -0.4102, 1.0659, -0.6138, 0.0181),
    array<f32, 6>(0.0010, -0.0532, -0.4346, 1.0879, -0.6187, 0.0176),
    array<f32, 6>(0.0010, -0.0454, -0.4570, 1.1089, -0.6235, 0.0166),
    array<f32, 6>(0.0005, -0.0386, -0.4790, 1.1274, -0.6265, 0.0156),
    array<f32, 6>(0.0005, -0.0317, -0.5005, 1.1455, -0.6284, 0.0142),
    array<f32, 6>(0.0005, -0.0259, -0.5181, 1.1582, -0.6270, 0.0122),
    array<f32, 6>(0.0005, -0.0195, -0.5352, 1.1699, -0.6265, 0.0112),
    array<f32, 6>(0.0, -0.0151, -0.5513, 1.1802, -0.6235, 0.0093),
    array<f32, 6>(0.0, -0.0103, -0.5659, 1.1890, -0.6196, 0.0073),
    array<f32, 6>(0.0005, -0.0068, -0.5791, 1.1958, -0.6147, 0.0049),
    array<f32, 6>(0.0, -0.0029, -0.5903, 1.1987, -0.6084, 0.0029),
);

fn saturate(value: f32) -> f32 {
    return clamp(value, 0.0, 1.0);
}

fn source_size() -> vec2<i32> {
    return vec2<i32>(i32(params.source_output.x), i32(params.source_output.y));
}

fn output_size() -> vec2<u32> {
    return vec2<u32>(params.source_output.z, params.source_output.w);
}

fn pixel(ix: i32, iy: i32) -> vec4<f32> {
    let size = source_size();
    return textureLoad(input_texture, vec2<i32>(clamp(ix, 0, size.x - 1), clamp(iy, 0, size.y - 1)), 0);
}

fn get_y(rgb: vec3<f32>) -> f32 {
    return dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn y_at(ix: i32, iy: i32) -> f32 {
    return get_y(pixel(ix, iy).rgb);
}

fn sample_color(coord: vec2<f32>) -> vec4<f32> {
    let base = vec2<i32>(floor(coord));
    let f = fract(coord);
    let c00 = pixel(base.x, base.y);
    let c10 = pixel(base.x + 1, base.y);
    let c01 = pixel(base.x, base.y + 1);
    let c11 = pixel(base.x + 1, base.y + 1);
    return mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y);
}

fn cfg_scale_x() -> f32 { return params.config0.x; }
fn cfg_scale_y() -> f32 { return params.config0.y; }
fn cfg_detect_ratio() -> f32 { return params.config0.z; }
fn cfg_detect_thres() -> f32 { return params.config0.w; }
fn cfg_min_contrast_ratio() -> f32 { return params.config1.x; }
fn cfg_ratio_norm() -> f32 { return params.config1.y; }
fn cfg_contrast_boost() -> f32 { return params.config1.z; }
fn cfg_eps() -> f32 { return params.config1.w; }
fn cfg_sharp_start_y() -> f32 { return params.config2.x; }
fn cfg_sharp_scale_y() -> f32 { return params.config2.y; }
fn cfg_sharp_strength_min() -> f32 { return params.config2.z; }
fn cfg_sharp_strength_scale() -> f32 { return params.config2.w; }
fn cfg_sharp_limit_min() -> f32 { return params.config3.x; }
fn cfg_sharp_limit_scale() -> f32 { return params.config3.y; }

fn get_edge_map(base: vec2<i32>) -> vec4<f32> {
    let p00 = y_at(base.x + 0, base.y + 0);
    let p01 = y_at(base.x + 1, base.y + 0);
    let p02 = y_at(base.x + 2, base.y + 0);
    let p10 = y_at(base.x + 0, base.y + 1);
    let p11 = y_at(base.x + 1, base.y + 1);
    let p12 = y_at(base.x + 2, base.y + 1);
    let p20 = y_at(base.x + 0, base.y + 2);
    let p21 = y_at(base.x + 1, base.y + 2);
    let p22 = y_at(base.x + 2, base.y + 2);

    let g_0 = abs(p00 + p01 + p02 - p20 - p21 - p22);
    let g_45 = abs(p10 + p00 + p01 - p21 - p22 - p12);
    let g_90 = abs(p00 + p10 + p20 - p02 - p12 - p22);
    let g_135 = abs(p10 + p20 + p21 - p01 - p02 - p12);

    let g_0_90_max = max(g_0, g_90);
    let g_0_90_min = min(g_0, g_90);
    let g_45_135_max = max(g_45, g_135);
    let g_45_135_min = min(g_45, g_135);

    if g_0_90_max + g_45_135_max == 0.0 {
        return vec4<f32>(0.0);
    }

    let e_0_90 = min(g_0_90_max / (g_0_90_max + g_45_135_max), 1.0);
    let e_45_135 = 1.0 - e_0_90;
    let c_0_90 = (g_0_90_max > (g_0_90_min * cfg_detect_ratio())) && (g_0_90_max > cfg_detect_thres()) && (g_0_90_max > g_45_135_min);
    let c_45_135 = (g_45_135_max > (g_45_135_min * cfg_detect_ratio())) && (g_45_135_max > cfg_detect_thres()) && (g_45_135_max > g_0_90_min);
    let c_g_0_90 = g_0_90_max == g_0;
    let c_g_45_135 = g_45_135_max == g_45;
    let f_e_0_90 = select(1.0, e_0_90, c_0_90 && c_45_135);
    let f_e_45_135 = select(1.0, e_45_135, c_0_90 && c_45_135);

    return vec4<f32>(
        select(0.0, f_e_0_90, c_0_90 && c_g_0_90),
        select(0.0, f_e_0_90, c_0_90 && !c_g_0_90),
        select(0.0, f_e_45_135, c_45_135 && c_g_45_135),
        select(0.0, f_e_45_135, c_45_135 && !c_g_45_135)
    );
}

fn interp_edge_map(base: vec2<i32>, phase: vec2<f32>) -> vec4<f32> {
    let e00 = get_edge_map(base + vec2<i32>(-1, -1));
    let e10 = get_edge_map(base + vec2<i32>(0, -1));
    let e01 = get_edge_map(base + vec2<i32>(-1, 0));
    let e11 = get_edge_map(base + vec2<i32>(0, 0));
    return mix(mix(e00, e10, phase.x), mix(e01, e11, phase.x), phase.y);
}

fn calc_lti(p0: f32, p1: f32, p2: f32, p3: f32, p4: f32, p5: f32, phase_index: u32) -> f32 {
    let selector = phase_index <= K_PHASE_COUNT / 2u;
    let sel_a = select(p3, p0, selector);
    let a_min = min(min(p1, p2), sel_a);
    let a_max = max(max(p1, p2), sel_a);
    let sel_b = select(p5, p2, selector);
    let b_min = min(min(p3, p4), sel_b);
    let b_max = max(max(p3, p4), sel_b);
    let a_cont = a_max - a_min;
    let b_cont = b_max - b_min;
    let cont_ratio = max(a_cont, b_cont) / (min(a_cont, b_cont) + cfg_eps());
    return (1.0 - saturate((cont_ratio - cfg_min_contrast_ratio()) * cfg_ratio_norm())) * cfg_contrast_boost();
}

fn eval_poly6(values: array<f32, 6>, phase: u32) -> f32 {
    var y = 0.0;
    var y_usm = 0.0;
    for (var i = 0u; i < 6u; i = i + 1u) {
        y = y + COEF_SCALE[phase][i] * values[i];
        y_usm = y_usm + COEF_USM[phase][i] * values[i];
    }
    let y_scale = 1.0 - saturate((y - cfg_sharp_start_y()) * cfg_sharp_scale_y());
    let y_sharpness = y_scale * cfg_sharp_strength_scale() + cfg_sharp_strength_min();
    y_usm = y_usm * y_sharpness;
    let y_sharpness_limit = (y_scale * cfg_sharp_limit_scale() + cfg_sharp_limit_min()) * y;
    y_usm = clamp(y_usm, -y_sharpness_limit, y_sharpness_limit);
    y_usm = y_usm * calc_lti(values[0], values[1], values[2], values[3], values[4], values[5], phase);
    return y + y_usm;
}

fn support_y(base: vec2<i32>, row: i32, col: i32) -> f32 {
    return y_at(base.x + col - 2, base.y + row - 2);
}

fn filter_normal(base: vec2<i32>, phase_x: u32, phase_y: u32) -> f32 {
    var h_acc = 0.0;
    for (var j = 0i; j < 6i; j = j + 1i) {
        var v_acc = 0.0;
        for (var i = 0i; i < 6i; i = i + 1i) {
            v_acc = v_acc + support_y(base, i, j) * COEF_SCALE[phase_y][u32(i)];
        }
        h_acc = h_acc + v_acc * COEF_SCALE[phase_x][u32(j)];
    }
    return h_acc;
}

fn add_dir_filters(base: vec2<i32>, phase: vec2<f32>, phase_x: u32, phase_y: u32, w: vec4<f32>) -> f32 {
    var f = 0.0;
    if w.x > 0.0 {
        var interp: array<f32, 6>;
        for (var i = 0i; i < 6i; i = i + 1i) {
            interp[u32(i)] = mix(support_y(base, i, 2), support_y(base, i, 3), phase.x);
        }
        f = f + eval_poly6(interp, phase_y) * w.x;
    }
    if w.y > 0.0 {
        var interp: array<f32, 6>;
        for (var i = 0i; i < 6i; i = i + 1i) {
            interp[u32(i)] = mix(support_y(base, 2, i), support_y(base, 3, i), phase.y);
        }
        f = f + eval_poly6(interp, phase_x) * w.y;
    }
    if w.z > 0.0 {
        var tmp: array<f32, 7>;
        var pphase_b45 = 0.5 + 0.5 * (phase.x - phase.y);
        tmp[1] = mix(support_y(base, 2, 1), support_y(base, 1, 2), pphase_b45);
        tmp[3] = mix(support_y(base, 3, 2), support_y(base, 2, 3), pphase_b45);
        tmp[5] = mix(support_y(base, 4, 3), support_y(base, 3, 4), pphase_b45);
        pphase_b45 = pphase_b45 - 0.5;
        let t = abs(pphase_b45);
        tmp[0] = mix(support_y(base, 1, 1), select(support_y(base, 2, 0), support_y(base, 0, 2), pphase_b45 >= 0.0), t);
        tmp[2] = mix(support_y(base, 2, 2), select(support_y(base, 3, 1), support_y(base, 1, 3), pphase_b45 >= 0.0), t);
        tmp[4] = mix(support_y(base, 3, 3), select(support_y(base, 4, 2), support_y(base, 2, 4), pphase_b45 >= 0.0), t);
        tmp[6] = mix(support_y(base, 4, 4), select(support_y(base, 5, 3), support_y(base, 3, 5), pphase_b45 >= 0.0), t);
        var interp: array<f32, 6>;
        var pphase_p45 = phase.x + phase.y;
        let offset = select(0u, 1u, pphase_p45 >= 1.0);
        pphase_p45 = select(pphase_p45, pphase_p45 - 1.0, pphase_p45 >= 1.0);
        for (var i = 0u; i < 6u; i = i + 1u) {
            interp[i] = tmp[i + offset];
        }
        f = f + eval_poly6(interp, min(u32(pphase_p45 * f32(K_PHASE_COUNT)), K_PHASE_COUNT - 1u)) * w.z;
    }
    if w.w > 0.0 {
        var tmp: array<f32, 7>;
        var pphase_b135 = 0.5 * (phase.x + phase.y);
        tmp[1] = mix(support_y(base, 3, 1), support_y(base, 4, 2), pphase_b135);
        tmp[3] = mix(support_y(base, 2, 2), support_y(base, 3, 3), pphase_b135);
        tmp[5] = mix(support_y(base, 1, 3), support_y(base, 2, 4), pphase_b135);
        pphase_b135 = pphase_b135 - 0.5;
        let t = abs(pphase_b135);
        tmp[0] = mix(support_y(base, 4, 1), select(support_y(base, 3, 0), support_y(base, 5, 2), pphase_b135 >= 0.0), t);
        tmp[2] = mix(support_y(base, 3, 2), select(support_y(base, 2, 1), support_y(base, 4, 3), pphase_b135 >= 0.0), t);
        tmp[4] = mix(support_y(base, 2, 3), select(support_y(base, 1, 2), support_y(base, 3, 4), pphase_b135 >= 0.0), t);
        tmp[6] = mix(support_y(base, 1, 4), select(support_y(base, 0, 3), support_y(base, 2, 5), pphase_b135 >= 0.0), t);
        var interp: array<f32, 6>;
        var pphase_p135 = 1.0 + (phase.x - phase.y);
        let offset = select(0u, 1u, pphase_p135 >= 1.0);
        pphase_p135 = select(pphase_p135, pphase_p135 - 1.0, pphase_p135 >= 1.0);
        for (var i = 0u; i < 6u; i = i + 1u) {
            interp[i] = tmp[i + offset];
        }
        f = f + eval_poly6(interp, min(u32(pphase_p135 * f32(K_PHASE_COUNT)), K_PHASE_COUNT - 1u)) * w.w;
    }
    return f;
}

@compute @workgroup_size(16, 16, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let out_size = output_size();
    if id.x >= out_size.x || id.y >= out_size.y {
        return;
    }

    let src_coord = vec2<f32>(
        (f32(id.x) + 0.5) * cfg_scale_x() - 0.5,
        (f32(id.y) + 0.5) * cfg_scale_y() - 0.5
    );
    let base = vec2<i32>(floor(src_coord));
    let phase = fract(src_coord);
    let phase_x = min(u32(phase.x * f32(K_PHASE_COUNT)), K_PHASE_COUNT - 1u);
    let phase_y = min(u32(phase.y * f32(K_PHASE_COUNT)), K_PHASE_COUNT - 1u);

    let edge = interp_edge_map(base, phase);
    let base_weight = 1.0 - edge.x - edge.y - edge.z - edge.w;
    var op_y = filter_normal(base, phase_x, phase_y) * base_weight;
    op_y = op_y + add_dir_filters(base, phase, phase_x, phase_y, edge);

    var op = sample_color(src_coord);
    let y = get_y(op.rgb);
    let corr = op_y - y;
    op = vec4<f32>(clamp(op.rgb + vec3<f32>(corr), vec3<f32>(0.0), vec3<f32>(1.0)), op.a);
    textureStore(output_texture, vec2<i32>(i32(id.x), i32(id.y)), op);
}
