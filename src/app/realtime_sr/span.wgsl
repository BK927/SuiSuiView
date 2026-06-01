struct SpanBridgeParams {
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
};

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> input_values: array<f32>;
@group(0) @binding(2) var<uniform> input_params: SpanBridgeParams;

@group(1) @binding(0) var<storage, read> output_values: array<f32>;
@group(1) @binding(1) var output_tex: texture_storage_2d<rgba8unorm, write>;
@group(1) @binding(2) var<uniform> output_params: SpanBridgeParams;

fn chw_offset(channel: u32, y: u32, x: u32, width: u32, height: u32) -> u32 {
    return (channel * height + y) * width + x;
}

fn output_channel(channel: u32, y: u32, x: u32) -> f32 {
    return output_values[chw_offset(
        channel,
        y,
        x,
        output_params.output_width,
        output_params.output_height,
    )] / 255.0;
}

@compute @workgroup_size(8, 8, 1)
fn span_rgba_to_chw(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;
    if (x >= input_params.source_width || y >= input_params.source_height) {
        return;
    }

    let pixel = textureLoad(source_tex, vec2<i32>(i32(x), i32(y)), 0);
    input_values[chw_offset(0u, y, x, input_params.source_width, input_params.source_height)] =
        pixel.r;
    input_values[chw_offset(1u, y, x, input_params.source_width, input_params.source_height)] =
        pixel.g;
    input_values[chw_offset(2u, y, x, input_params.source_width, input_params.source_height)] =
        pixel.b;
}

@compute @workgroup_size(8, 8, 1)
fn span_chw_to_rgba(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;
    if (x >= output_params.output_width || y >= output_params.output_height) {
        return;
    }

    let r = output_channel(0u, y, x);
    let g = output_channel(1u, y, x);
    let b = output_channel(2u, y, x);
    textureStore(
        output_tex,
        vec2<i32>(i32(x), i32(y)),
        vec4<f32>(clamp(r, 0.0, 1.0), clamp(g, 0.0, 1.0), clamp(b, 0.0, 1.0), 1.0),
    );
}
