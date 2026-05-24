struct Params {
    source_output: vec4<u32>,
    transform_filter: vec4<u32>,
    color_origin: vec4<u32>,
    upscale: vec4<u32>,
    opacity: vec4<f32>,
};

@group(0) @binding(0)
var source_texture: texture_2d<f32>;

@group(1) @binding(0)
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

fn transformed_pixel(dst_x: u32, dst_y: u32) -> vec4<f32> {
    let out_width = params.source_output.z;
    let out_height = params.source_output.w;
    let width = params.source_output.x;
    let height = params.source_output.y;

    var rotated_x = dst_x;
    var rotated_y = dst_y;
    if params.transform_filter.y != 0u {
        rotated_x = out_width - 1u - dst_x;
    }
    if params.transform_filter.z != 0u {
        rotated_y = out_height - 1u - dst_y;
    }

    var src_x = rotated_x;
    var src_y = rotated_y;
    if params.transform_filter.x == 1u {
        src_x = rotated_y;
        src_y = height - 1u - rotated_x;
    } else if params.transform_filter.x == 2u {
        src_x = width - 1u - rotated_x;
        src_y = height - 1u - rotated_y;
    } else if params.transform_filter.x == 3u {
        src_x = width - 1u - rotated_y;
        src_y = rotated_x;
    }

    return textureLoad(source_texture, vec2<i32>(i32(src_x), i32(src_y)), 0);
}

fn weighted_average(dst_x: u32, dst_y: u32) -> vec4<f32> {
    let out_width = params.source_output.z;
    let out_height = params.source_output.w;
    let min_x = select(dst_x - 1u, 0u, dst_x == 0u);
    let min_y = select(dst_y - 1u, 0u, dst_y == 0u);
    let max_x = min(dst_x + 1u, out_width - 1u);
    let max_y = min(dst_y + 1u, out_height - 1u);

    var total = 0.0;
    var color = vec4<f32>(0.0);
    for (var y = min_y; y <= max_y; y = y + 1u) {
        for (var x = min_x; x <= max_x; x = x + 1u) {
            var weight = 1.0;
            if x == dst_x && y == dst_y {
                weight = 4.0;
            } else if x == dst_x || y == dst_y {
                weight = 2.0;
            }
            color = color + transformed_pixel(x, y) * weight;
            total = total + weight;
        }
    }
    return color / total;
}

fn luma(color: vec4<f32>) -> f32 {
    return dot(color.rgb, vec3<f32>(0.299, 0.587, 0.114));
}

fn rcas_sharpen(dst_x: u32, dst_y: u32) -> vec4<f32> {
    let out_width = params.source_output.z;
    let out_height = params.source_output.w;
    let center = transformed_pixel(dst_x, dst_y);
    let left = transformed_pixel(select(dst_x - 1u, 0u, dst_x == 0u), dst_y);
    let right = transformed_pixel(min(dst_x + 1u, out_width - 1u), dst_y);
    let up = transformed_pixel(dst_x, select(dst_y - 1u, 0u, dst_y == 0u));
    let down = transformed_pixel(dst_x, min(dst_y + 1u, out_height - 1u));

    let min_luma = min(min(min(min(luma(left), luma(right)), luma(up)), luma(down)), luma(center));
    let max_luma = max(max(max(max(luma(left), luma(right)), luma(up)), luma(down)), luma(center));
    let contrast = clamp(max_luma - min_luma, 0.0, 1.0);
    let amount = 0.18 + (1.0 - contrast) * 0.22;
    let average = (left.rgb + right.rgb + up.rgb + down.rgb) * 0.25;
    let rgb = clamp(center.rgb + (center.rgb - average) * amount, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rgb, center.a);
}

fn effect_pixel(dst_x: u32, dst_y: u32) -> vec4<f32> {
    var color = transformed_pixel(dst_x, dst_y);

    if params.transform_filter.w == 1u {
        color = weighted_average(dst_x, dst_y);
    } else if params.transform_filter.w == 2u {
        let blurred = weighted_average(dst_x, dst_y);
        color = clamp(color * 1.55 - blurred * 0.55, vec4<f32>(0.0), vec4<f32>(1.0));
        color.a = transformed_pixel(dst_x, dst_y).a;
    } else if params.transform_filter.w == 3u {
        color = rcas_sharpen(dst_x, dst_y);
    }

    if params.color_origin.x != 0u {
        color = vec4<f32>(pow(color.rgb, vec3<f32>(1.0 / 1.2)), color.a);
    }
    if params.color_origin.y != 0u {
        color = vec4<f32>(vec3<f32>(1.0) - color.rgb, color.a);
    }
    return color;
}

fn effect_pixel_clamped(ix: i32, iy: i32) -> vec4<f32> {
    let max_x = i32(params.source_output.z) - 1;
    let max_y = i32(params.source_output.w) - 1;
    return effect_pixel(u32(clamp(ix, 0, max_x)), u32(clamp(iy, 0, max_y)));
}

fn sample_effect(sample_x: f32, sample_y: f32) -> vec4<f32> {
    let max_x = params.source_output.z - 1u;
    let max_y = params.source_output.w - 1u;
    let x = clamp(sample_x, 0.0, f32(max_x));
    let y = clamp(sample_y, 0.0, f32(max_y));
    let x0 = u32(floor(x));
    let y0 = u32(floor(y));
    let x1 = min(x0 + 1u, max_x);
    let y1 = min(y0 + 1u, max_y);
    let tx = x - f32(x0);
    let ty = y - f32(y0);

    let top = mix(effect_pixel(x0, y0), effect_pixel(x1, y0), tx);
    let bottom = mix(effect_pixel(x0, y1), effect_pixel(x1, y1), tx);
    return mix(top, bottom, ty);
}

fn cubic_weight(x: f32) -> f32 {
    let ax = abs(x);
    if ax <= 1.0 {
        return 1.5 * ax * ax * ax - 2.5 * ax * ax + 1.0;
    }
    if ax < 2.0 {
        return -0.5 * ax * ax * ax + 2.5 * ax * ax - 4.0 * ax + 2.0;
    }
    return 0.0;
}

fn cubic_effect_sample(coord: vec2<f32>) -> vec4<f32> {
    let base = vec2<i32>(floor(coord));
    var color = vec4<f32>(0.0);
    var total = 0.0;
    for (var yy = -1; yy <= 2; yy = yy + 1) {
        let wy = cubic_weight(coord.y - f32(base.y + yy));
        for (var xx = -1; xx <= 2; xx = xx + 1) {
            let wx = cubic_weight(coord.x - f32(base.x + xx));
            let weight = wx * wy;
            color = color + effect_pixel_clamped(base.x + xx, base.y + yy) * weight;
            total = total + weight;
        }
    }
    return clamp(color / max(total, 0.0001), vec4<f32>(0.0), vec4<f32>(1.0));
}

fn effect_luma3(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn local_effect_contrast(coord: vec2<f32>) -> f32 {
    let p = vec2<i32>(floor(coord));
    let c = effect_luma3(effect_pixel_clamped(p.x, p.y).rgb);
    let l = effect_luma3(effect_pixel_clamped(p.x - 1, p.y).rgb);
    let r = effect_luma3(effect_pixel_clamped(p.x + 1, p.y).rgb);
    let u = effect_luma3(effect_pixel_clamped(p.x, p.y - 1).rgb);
    let d = effect_luma3(effect_pixel_clamped(p.x, p.y + 1).rgb);
    return clamp(max(max(abs(c - l), abs(c - r)), max(abs(c - u), abs(c - d))), 0.0, 1.0);
}

fn contrast_effect_sharpen(color: vec4<f32>, coord: vec2<f32>, amount: f32) -> vec4<f32> {
    let p = vec2<i32>(floor(coord));
    let average = (
        effect_pixel_clamped(p.x - 1, p.y).rgb +
        effect_pixel_clamped(p.x + 1, p.y).rgb +
        effect_pixel_clamped(p.x, p.y - 1).rgb +
        effect_pixel_clamped(p.x, p.y + 1).rgb
    ) * 0.25;
    let contrast = local_effect_contrast(coord);
    let adaptive = amount * (0.65 + (1.0 - contrast) * 0.35);
    return vec4<f32>(clamp(color.rgb + (color.rgb - average) * adaptive, vec3<f32>(0.0), vec3<f32>(1.0)), color.a);
}

fn contrast_effect_sharpen_from_stats(color: vec4<f32>, average: vec3<f32>, contrast: f32, amount: f32) -> vec4<f32> {
    let adaptive = amount * (0.65 + (1.0 - contrast) * 0.35);
    return vec4<f32>(clamp(color.rgb + (color.rgb - average) * adaptive, vec3<f32>(0.0), vec3<f32>(1.0)), color.a);
}

fn fsr1_style_effect_sample(coord: vec2<f32>) -> vec4<f32> {
    let base = cubic_effect_sample(coord);
    return contrast_effect_sharpen(base, coord, 0.20);
}

fn nis_style_effect_sample(coord: vec2<f32>) -> vec4<f32> {
    let base = sample_effect(coord.x, coord.y);
    let cubic = cubic_effect_sample(coord);
    let mixed = mix(base, cubic, 0.65);
    return contrast_effect_sharpen(mixed, coord, 0.34);
}

fn effect_cross_average(coord: vec2<f32>) -> vec3<f32> {
    let p = vec2<i32>(floor(coord));
    return (
        effect_pixel_clamped(p.x - 1, p.y).rgb +
        effect_pixel_clamped(p.x + 1, p.y).rgb +
        effect_pixel_clamped(p.x, p.y - 1).rgb +
        effect_pixel_clamped(p.x, p.y + 1).rgb
    ) * 0.25;
}

fn set_effect_luma_rgb(rgb: vec3<f32>, target_luma: f32) -> vec3<f32> {
    let current = effect_luma3(rgb);
    return clamp(rgb + vec3<f32>(target_luma - current), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn artcnn_style_effect_sample(coord: vec2<f32>, detail_amount: f32, cleanup: f32) -> vec4<f32> {
    let base = cubic_effect_sample(coord);
    let average = effect_cross_average(coord);
    let edge = local_effect_contrast(coord);
    let base_luma = effect_luma3(base.rgb);
    let average_luma = effect_luma3(average);
    let detail = base_luma - average_luma;
    let boosted_luma = base_luma + detail * detail_amount;
    let boosted = set_effect_luma_rgb(base.rgb, boosted_luma);
    let cleaned = mix(boosted, average, cleanup * (1.0 - edge));
    return contrast_effect_sharpen_from_stats(vec4<f32>(cleaned, base.a), average, edge, detail_amount * 0.20);
}

fn anime4k_style_effect_sample(coord: vec2<f32>) -> vec4<f32> {
    let base = cubic_effect_sample(coord);
    let soft_sample = sample_effect(coord.x, coord.y);
    let average = effect_cross_average(coord);
    let edge = local_effect_contrast(coord);
    let line_gain = clamp(edge * 2.6, 0.0, 1.0);
    let restored = vec4<f32>(
        clamp(base.rgb + (base.rgb - soft_sample.rgb) * (0.30 + line_gain * 0.30), vec3<f32>(0.0), vec3<f32>(1.0)),
        base.a,
    );
    return contrast_effect_sharpen_from_stats(mix(base, restored, 0.75), average, edge, 0.38);
}

fn acnet_style_effect_sample(coord: vec2<f32>) -> vec4<f32> {
    let base = cubic_effect_sample(coord);
    let average = effect_cross_average(coord);
    let edge = local_effect_contrast(coord);
    let clean = mix(base.rgb, average, 0.12 * (1.0 - edge));
    let detail = base.rgb - average;
    let restored = vec4<f32>(
        clamp(clean + detail * (0.24 + edge * 0.46), vec3<f32>(0.0), vec3<f32>(1.0)),
        base.a,
    );
    return contrast_effect_sharpen_from_stats(restored, average, edge, 0.26);
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

fn fsr_easu_effect_sample(coord: vec2<f32>) -> vec4<f32> {
    let fp = vec2<i32>(floor(coord));
    let pp = coord - vec2<f32>(fp);

    let b = effect_pixel_clamped(fp.x, fp.y - 1);
    let c = effect_pixel_clamped(fp.x + 1, fp.y - 1);
    let e = effect_pixel_clamped(fp.x - 1, fp.y);
    let f = effect_pixel_clamped(fp.x, fp.y);
    let g = effect_pixel_clamped(fp.x + 1, fp.y);
    let h = effect_pixel_clamped(fp.x + 2, fp.y);
    let i = effect_pixel_clamped(fp.x - 1, fp.y + 1);
    let j = effect_pixel_clamped(fp.x, fp.y + 1);
    let k = effect_pixel_clamped(fp.x + 1, fp.y + 1);
    let l = effect_pixel_clamped(fp.x + 2, fp.y + 1);
    let n = effect_pixel_clamped(fp.x, fp.y + 2);
    let o = effect_pixel_clamped(fp.x + 1, fp.y + 2);

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

fn fsr_rcas_effect_sample(coord: vec2<f32>) -> vec4<f32> {
    let p = vec2<i32>(floor(coord));
    let b = effect_pixel_clamped(p.x, p.y - 1);
    let d = effect_pixel_clamped(p.x - 1, p.y);
    let e = effect_pixel_clamped(p.x, p.y);
    let f = effect_pixel_clamped(p.x + 1, p.y);
    let h = effect_pixel_clamped(p.x, p.y + 1);

    let b_l = fsr_luma(b.rgb);
    let d_l = fsr_luma(d.rgb);
    let e_l = fsr_luma(e.rgb);
    let f_l = fsr_luma(f.rgb);
    let h_l = fsr_luma(h.rgb);
    let nz_range = max(max(max(max(b_l, d_l), e_l), f_l), h_l) - min(min(min(min(b_l, d_l), e_l), f_l), h_l);
    let amount = 0.18 + (1.0 - clamp(nz_range, 0.0, 1.0)) * 0.16;
    let average = (b.rgb + d.rgb + f.rgb + h.rgb) * 0.25;
    return vec4<f32>(clamp(e.rgb + (e.rgb - average) * amount, vec3<f32>(0.0), vec3<f32>(1.0)), e.a);
}

fn sample_display(sample_x: f32, sample_y: f32) -> vec4<f32> {
    let coord = vec2<f32>(sample_x, sample_y);
    if params.upscale.x == 2u {
        return fsr1_style_effect_sample(coord);
    }
    if params.upscale.x == 3u {
        return nis_style_effect_sample(coord);
    }
    if params.upscale.x == 4u {
        return fsr_easu_effect_sample(coord);
    }
    if params.upscale.x == 5u {
        return fsr_rcas_effect_sample(coord);
    }
    if params.upscale.x == 6u {
        return artcnn_style_effect_sample(coord, 0.48, 0.05);
    }
    if params.upscale.x == 7u {
        return artcnn_style_effect_sample(coord, 0.74, 0.03);
    }
    if params.upscale.x == 8u {
        return anime4k_style_effect_sample(coord);
    }
    if params.upscale.x == 9u {
        return acnet_style_effect_sample(coord);
    }
    return sample_effect(sample_x, sample_y);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let local_x = floor(max(position.x - f32(params.color_origin.z), 0.0));
    let local_y = floor(max(position.y - f32(params.color_origin.w), 0.0));
    let sample_x = (local_x + 0.5) * f32(params.source_output.z) / params.opacity.y - 0.5;
    let sample_y = (local_y + 0.5) * f32(params.source_output.w) / params.opacity.z - 0.5;
    let color = sample_display(sample_x, sample_y);
    return vec4<f32>(color.rgb, color.a * params.opacity.x);
}
