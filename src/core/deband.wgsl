// Clean-room debanding pre-pass (Flanagan-style) for the WGPU display path.
// GPU mirror of src/core/deband.rs. LICENSE: implemented clean-room from a
// plain-language description; no libplacebo/mpv (LGPL) code was copied.
//
// Renders at SOURCE size (source texture -> debanded intermediate). The rest of
// the paint chain samples the debanded intermediate instead of the source. All
// iterations refine the in-register `cur` reading the SOURCE, so one pass is
// enough and CPU/GPU stay equivalent. The integer hash is bit-exact with the
// Rust reference (WGSL u32 arithmetic wraps by spec); trig and the final round
// make the overall result statistically equivalent, not bit-exact.
//
// Params are normalized: threshold/grain are uploaded as the 8-bit preset value
// divided by 255 (see gpu_paint/deband.rs).

struct DebandParams {
    dims: vec4<u32>,   // width, height, iterations, _pad
    config: vec4<f32>, // base_radius, threshold(0..1), grain(0..1), _pad
};

@group(0) @binding(0)
var source_texture: texture_2d<f32>;

// Unused by the pass (it uses textureLoad), present only so the deband pipeline
// can reuse the shared texture bind-group layout and the existing source bind group.
@group(0) @binding(1)
var source_sampler: sampler;

@group(1) @binding(0)
var<uniform> params: DebandParams;

// Must match GRAIN_SALT in src/core/deband.rs.
const GRAIN_SALT: u32 = 0x6D2B79F5u;
const TAU: f32 = 6.283185307179586;
const HALF_PI: f32 = 1.5707963267948966;

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

fn hash_u32(x: u32, y: u32, salt: u32) -> u32 {
    var h = x * 0x01000193u ^ y * 0x9E3779B1u ^ salt * 0x85EBCA77u;
    h = h ^ (h >> 15u);
    h = h * 0x2C1B3C6Du;
    h = h ^ (h >> 12u);
    h = h * 0x297A2D39u;
    h = h ^ (h >> 15u);
    return h;
}

fn hash_unit(x: u32, y: u32, salt: u32) -> f32 {
    return f32(hash_u32(x, y, salt)) / 4294967296.0;
}

fn load_clamped(x: i32, y: i32) -> vec3<f32> {
    let w = i32(params.dims.x);
    let h = i32(params.dims.y);
    let cx = clamp(x, 0, w - 1);
    let cy = clamp(y, 0, h - 1);
    return textureLoad(source_texture, vec2<i32>(cx, cy), 0).rgb;
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let x = u32(floor(position.x));
    let y = u32(floor(position.y));
    let center = textureLoad(source_texture, vec2<i32>(i32(x), i32(y)), 0);
    var cur = center.rgb;

    let iters = params.dims.z;
    let base_radius = params.config.x;
    let threshold = params.config.y;
    let grain = params.config.z;
    var replaced = false;

    for (var i: u32 = 1u; i <= iters; i = i + 1u) {
        let r = base_radius * f32(i);
        let a = hash_unit(x, y, i) * TAU;
        var avg = vec3<f32>(0.0);
        for (var k: u32 = 0u; k < 4u; k = k + 1u) {
            let ang = a + f32(k) * HALF_PI;
            let dx = i32(round(r * cos(ang)));
            let dy = i32(round(r * sin(ang)));
            avg = avg + load_clamped(i32(x) + dx, i32(y) + dy);
        }
        avg = avg * 0.25;
        let d = max(max(abs(avg.r - cur.r), abs(avg.g - cur.g)), abs(avg.b - cur.b));
        if d < threshold {
            cur = avg;
            replaced = true;
        }
    }

    if replaced {
        let g = (hash_unit(x, y, GRAIN_SALT) * 2.0 - 1.0) * grain;
        cur = cur + vec3<f32>(g, g, g);
    }

    cur = clamp(cur, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(cur, center.a);
}
