// Benchmark-only display upscalers.
//
// Method 3/4 are a WGSL port of AMD FidelityFX Super Resolution 1.0 EASU
// followed by RCAS for evaluation. AMD FSR1 is MIT licensed; see
// THIRD_PARTY_NOTICES.txt before promoting this path into product UI.

struct Params {
    source_output: vec4<u32>,
    method: vec4<u32>,
};

@group(0) @binding(0)
var source_texture: texture_2d<f32>;
@group(0) @binding(1)
var<uniform> params: Params;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0),
    );
    var out: VertexOut;
    out.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    return out;
}

fn pixel(ix: i32, iy: i32) -> vec4<f32> {
    let max_x = i32(params.source_output.x) - 1;
    let max_y = i32(params.source_output.y) - 1;
    return textureLoad(source_texture, vec2<i32>(clamp(ix, 0, max_x), clamp(iy, 0, max_y)), 0);
}

fn luma(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn bilinear_sample(coord: vec2<f32>) -> vec4<f32> {
    let p0 = vec2<i32>(floor(coord));
    let f = fract(coord);
    let c00 = pixel(p0.x, p0.y);
    let c10 = pixel(p0.x + 1, p0.y);
    let c01 = pixel(p0.x, p0.y + 1);
    let c11 = pixel(p0.x + 1, p0.y + 1);
    return mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y);
}

fn catmull_rom_weight(x: f32) -> f32 {
    let ax = abs(x);
    if ax <= 1.0 {
        return 1.5 * ax * ax * ax - 2.5 * ax * ax + 1.0;
    }
    if ax < 2.0 {
        return -0.5 * ax * ax * ax + 2.5 * ax * ax - 4.0 * ax + 2.0;
    }
    return 0.0;
}

fn catmull_rom_sample(coord: vec2<f32>) -> vec4<f32> {
    let base = vec2<i32>(floor(coord));
    var color = vec4<f32>(0.0);
    var total = 0.0;
    for (var yy = -1; yy <= 2; yy = yy + 1) {
        let wy = catmull_rom_weight(coord.y - f32(base.y + yy));
        for (var xx = -1; xx <= 2; xx = xx + 1) {
            let wx = catmull_rom_weight(coord.x - f32(base.x + xx));
            let weight = wx * wy;
            color = color + pixel(base.x + xx, base.y + yy) * weight;
            total = total + weight;
        }
    }
    return clamp(color / max(total, 0.0001), vec4<f32>(0.0), vec4<f32>(1.0));
}

fn local_contrast(coord: vec2<f32>) -> f32 {
    let p = vec2<i32>(floor(coord));
    let c = luma(pixel(p.x, p.y).rgb);
    let l = luma(pixel(p.x - 1, p.y).rgb);
    let r = luma(pixel(p.x + 1, p.y).rgb);
    let u = luma(pixel(p.x, p.y - 1).rgb);
    let d = luma(pixel(p.x, p.y + 1).rgb);
    return clamp(max(max(abs(c - l), abs(c - r)), max(abs(c - u), abs(c - d))), 0.0, 1.0);
}

fn contrast_sharpen(color: vec4<f32>, coord: vec2<f32>, amount: f32) -> vec4<f32> {
    let p = vec2<i32>(floor(coord));
    let average = (
        pixel(p.x - 1, p.y).rgb +
        pixel(p.x + 1, p.y).rgb +
        pixel(p.x, p.y - 1).rgb +
        pixel(p.x, p.y + 1).rgb
    ) * 0.25;
    let contrast = local_contrast(coord);
    let adaptive = amount * (0.65 + (1.0 - contrast) * 0.35);
    return vec4<f32>(clamp(color.rgb + (color.rgb - average) * adaptive, vec3<f32>(0.0), vec3<f32>(1.0)), color.a);
}

fn fsr1_style_sample(coord: vec2<f32>) -> vec4<f32> {
    let base = catmull_rom_sample(coord);
    return contrast_sharpen(base, coord, 0.20);
}

fn nis_style_sample(coord: vec2<f32>) -> vec4<f32> {
    let base = bilinear_sample(coord);
    let cubic = catmull_rom_sample(coord);
    let mixed = mix(base, cubic, 0.65);
    return contrast_sharpen(mixed, coord, 0.34);
}

fn safe_rcp(value: f32) -> f32 {
    if abs(value) < 0.000001 {
        return select(-1000000.0, 1000000.0, value >= 0.0);
    }
    return 1.0 / value;
}

fn fsr_luma(color: vec3<f32>) -> f32 {
    return color.b * 0.5 + (color.r * 0.5 + color.g);
}

fn fsr_easu_set(state: vec3<f32>, weight: f32, l_a: f32, l_b: f32, l_c: f32, l_d: f32, l_e: f32) -> vec3<f32> {
    var next = state;
    let dc = l_d - l_c;
    let cb = l_c - l_b;
    let dir_x = l_d - l_b;
    let len_x = clamp(abs(dir_x) * safe_rcp(max(abs(dc), abs(cb))), 0.0, 1.0);
    next.x = next.x + dir_x * weight;
    next.z = next.z + len_x * len_x * weight;

    let ec = l_e - l_c;
    let ca = l_c - l_a;
    let dir_y = l_e - l_a;
    let len_y = clamp(abs(dir_y) * safe_rcp(max(abs(ec), abs(ca))), 0.0, 1.0);
    next.y = next.y + dir_y * weight;
    next.z = next.z + len_y * len_y * weight;
    return next;
}

fn fsr_easu_tap_weight(off: vec2<f32>, dir: vec2<f32>, len: vec2<f32>, lob: f32, clp: f32) -> f32 {
    var v = vec2<f32>(
        off.x * dir.x + off.y * dir.y,
        off.x * -dir.y + off.y * dir.x,
    );
    v = v * len;
    var d2 = v.x * v.x + v.y * v.y;
    d2 = min(d2, clp);
    var wb = (2.0 / 5.0) * d2 - 1.0;
    var wa = lob * d2 - 1.0;
    wb = wb * wb;
    wa = wa * wa;
    wb = (25.0 / 16.0) * wb - (25.0 / 16.0 - 1.0);
    return wb * wa;
}

fn fsr_easu_sample(coord: vec2<f32>) -> vec4<f32> {
    let fp = vec2<i32>(floor(coord));
    let pp = coord - vec2<f32>(fp);

    let b = pixel(fp.x, fp.y - 1);
    let c = pixel(fp.x + 1, fp.y - 1);
    let e = pixel(fp.x - 1, fp.y);
    let f = pixel(fp.x, fp.y);
    let g = pixel(fp.x + 1, fp.y);
    let h = pixel(fp.x + 2, fp.y);
    let i = pixel(fp.x - 1, fp.y + 1);
    let j = pixel(fp.x, fp.y + 1);
    let k = pixel(fp.x + 1, fp.y + 1);
    let l = pixel(fp.x + 2, fp.y + 1);
    let n = pixel(fp.x, fp.y + 2);
    let o = pixel(fp.x + 1, fp.y + 2);

    let b_l = fsr_luma(b.rgb);
    let c_l = fsr_luma(c.rgb);
    let e_l = fsr_luma(e.rgb);
    let f_l = fsr_luma(f.rgb);
    let g_l = fsr_luma(g.rgb);
    let h_l = fsr_luma(h.rgb);
    let i_l = fsr_luma(i.rgb);
    let j_l = fsr_luma(j.rgb);
    let k_l = fsr_luma(k.rgb);
    let l_l = fsr_luma(l.rgb);
    let n_l = fsr_luma(n.rgb);
    let o_l = fsr_luma(o.rgb);

    var dir_len = vec3<f32>(0.0);
    dir_len = fsr_easu_set(dir_len, (1.0 - pp.x) * (1.0 - pp.y), b_l, e_l, f_l, g_l, j_l);
    dir_len = fsr_easu_set(dir_len, pp.x * (1.0 - pp.y), c_l, f_l, g_l, h_l, k_l);
    dir_len = fsr_easu_set(dir_len, (1.0 - pp.x) * pp.y, f_l, i_l, j_l, k_l, n_l);
    dir_len = fsr_easu_set(dir_len, pp.x * pp.y, g_l, j_l, k_l, l_l, o_l);

    var dir = dir_len.xy;
    var dir_r = dot(dir, dir);
    if dir_r < (1.0 / 32768.0) {
        dir = vec2<f32>(1.0, 0.0);
    } else {
        dir = dir * inverseSqrt(dir_r);
    }

    var len = dir_len.z * 0.5;
    len = len * len;
    let stretch = dot(dir, dir) * safe_rcp(max(abs(dir.x), abs(dir.y)));
    let len2 = vec2<f32>(1.0 + (stretch - 1.0) * len, 1.0 - 0.5 * len);
    let lob = 0.5 + ((1.0 / 4.0 - 0.04) - 0.5) * len;
    let clp = safe_rcp(lob);

    let min4 = min(min(f.rgb, g.rgb), min(j.rgb, k.rgb));
    let max4 = max(max(f.rgb, g.rgb), max(j.rgb, k.rgb));

    var accum = vec3<f32>(0.0);
    var weight_sum = 0.0;
    var weight = 0.0;

    weight = fsr_easu_tap_weight(vec2<f32>(0.0, -1.0) - pp, dir, len2, lob, clp);
    accum = accum + b.rgb * weight; weight_sum = weight_sum + weight;
    weight = fsr_easu_tap_weight(vec2<f32>(1.0, -1.0) - pp, dir, len2, lob, clp);
    accum = accum + c.rgb * weight; weight_sum = weight_sum + weight;
    weight = fsr_easu_tap_weight(vec2<f32>(-1.0, 1.0) - pp, dir, len2, lob, clp);
    accum = accum + i.rgb * weight; weight_sum = weight_sum + weight;
    weight = fsr_easu_tap_weight(vec2<f32>(0.0, 1.0) - pp, dir, len2, lob, clp);
    accum = accum + j.rgb * weight; weight_sum = weight_sum + weight;
    weight = fsr_easu_tap_weight(vec2<f32>(0.0, 0.0) - pp, dir, len2, lob, clp);
    accum = accum + f.rgb * weight; weight_sum = weight_sum + weight;
    weight = fsr_easu_tap_weight(vec2<f32>(-1.0, 0.0) - pp, dir, len2, lob, clp);
    accum = accum + e.rgb * weight; weight_sum = weight_sum + weight;
    weight = fsr_easu_tap_weight(vec2<f32>(1.0, 1.0) - pp, dir, len2, lob, clp);
    accum = accum + k.rgb * weight; weight_sum = weight_sum + weight;
    weight = fsr_easu_tap_weight(vec2<f32>(2.0, 1.0) - pp, dir, len2, lob, clp);
    accum = accum + l.rgb * weight; weight_sum = weight_sum + weight;
    weight = fsr_easu_tap_weight(vec2<f32>(2.0, 0.0) - pp, dir, len2, lob, clp);
    accum = accum + h.rgb * weight; weight_sum = weight_sum + weight;
    weight = fsr_easu_tap_weight(vec2<f32>(1.0, 0.0) - pp, dir, len2, lob, clp);
    accum = accum + g.rgb * weight; weight_sum = weight_sum + weight;
    weight = fsr_easu_tap_weight(vec2<f32>(1.0, 2.0) - pp, dir, len2, lob, clp);
    accum = accum + o.rgb * weight; weight_sum = weight_sum + weight;
    weight = fsr_easu_tap_weight(vec2<f32>(0.0, 2.0) - pp, dir, len2, lob, clp);
    accum = accum + n.rgb * weight; weight_sum = weight_sum + weight;

    let rgb = clamp(accum * safe_rcp(weight_sum), min4, max4);
    return vec4<f32>(rgb, f.a);
}

fn fsr_rcas_sample(coord: vec2<f32>) -> vec4<f32> {
    let p = vec2<i32>(floor(coord));
    let b = pixel(p.x, p.y - 1);
    let d = pixel(p.x - 1, p.y);
    let e = pixel(p.x, p.y);
    let f = pixel(p.x + 1, p.y);
    let h = pixel(p.x, p.y + 1);

    let b_l = fsr_luma(b.rgb);
    let d_l = fsr_luma(d.rgb);
    let e_l = fsr_luma(e.rgb);
    let f_l = fsr_luma(f.rgb);
    let h_l = fsr_luma(h.rgb);
    let nz_range = max(max(max(max(b_l, d_l), e_l), f_l), h_l) - min(min(min(min(b_l, d_l), e_l), f_l), h_l);
    var nz = abs(0.25 * b_l + 0.25 * d_l + 0.25 * f_l + 0.25 * h_l - e_l) * safe_rcp(nz_range);
    nz = -0.5 * clamp(nz, 0.0, 1.0) + 1.0;

    let mn4 = min(min(b.rgb, d.rgb), min(f.rgb, h.rgb));
    let mx4 = max(max(b.rgb, d.rgb), max(f.rgb, h.rgb));
    let peak = vec2<f32>(1.0, -4.0);
    let hit_min = min(mn4, e.rgb) / max(vec3<f32>(0.000001), 4.0 * mx4);
    let hit_max = (peak.x - max(mx4, e.rgb)) / min(vec3<f32>(-0.000001), 4.0 * mn4 + vec3<f32>(peak.y));
    let lobe_rgb = max(-hit_min, hit_max);
    let fsr_rcas_limit = 0.25 - (1.0 / 16.0);
    let sharpness = exp2(-0.2);
    var lobe = max(-fsr_rcas_limit, min(max(max(lobe_rgb.r, lobe_rgb.g), lobe_rgb.b), 0.0)) * sharpness;
    lobe = lobe * nz;
    let rcp_l = safe_rcp(4.0 * lobe + 1.0);
    let rgb = (lobe * b.rgb + lobe * d.rgb + lobe * h.rgb + lobe * f.rgb + e.rgb) * rcp_l;
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), e.a);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let out_size = vec2<f32>(f32(params.source_output.z), f32(params.source_output.w));
    let src_size = vec2<f32>(f32(params.source_output.x), f32(params.source_output.y));
    let coord = (floor(position.xy) + vec2<f32>(0.5)) * src_size / out_size - vec2<f32>(0.5);

    if params.method.x == 2u {
        return fsr1_style_sample(coord);
    }
    if params.method.x == 3u {
        return nis_style_sample(coord);
    }
    if params.method.x == 4u {
        return fsr_easu_sample(coord);
    }
    if params.method.x == 5u {
        return fsr_rcas_sample(coord);
    }
    return bilinear_sample(coord);
}
