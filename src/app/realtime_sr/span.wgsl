struct SpanBridgeParams {
    source_width: u32,
    source_height: u32,
    input_width: u32,
    input_height: u32,
    output_width: u32,
    output_height: u32,
    dest_width: u32,
    dest_height: u32,
    source_x: u32,
    source_y: u32,
    read_x: u32,
    read_y: u32,
    dest_x: u32,
    dest_y: u32,
    copy_width: u32,
    copy_height: u32,
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
    if (x >= input_params.input_width || y >= input_params.input_height) {
        return;
    }
    let source_x = input_params.source_x + x;
    let source_y = input_params.source_y + y;
    if (source_x >= input_params.source_width || source_y >= input_params.source_height) {
        return;
    }

    let pixel = textureLoad(source_tex, vec2<i32>(i32(source_x), i32(source_y)), 0);
    input_values[chw_offset(0u, y, x, input_params.input_width, input_params.input_height)] =
        pixel.r;
    input_values[chw_offset(1u, y, x, input_params.input_width, input_params.input_height)] =
        pixel.g;
    input_values[chw_offset(2u, y, x, input_params.input_width, input_params.input_height)] =
        pixel.b;
}

@compute @workgroup_size(8, 8, 1)
fn span_chw_to_rgba(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;
    if (x >= output_params.copy_width || y >= output_params.copy_height) {
        return;
    }

    let read_x = output_params.read_x + x;
    let read_y = output_params.read_y + y;
    let dest_x = output_params.dest_x + x;
    let dest_y = output_params.dest_y + y;
    if (read_x >= output_params.output_width || read_y >= output_params.output_height ||
        dest_x >= output_params.dest_width || dest_y >= output_params.dest_height) {
        return;
    }

    let r = output_channel(0u, read_y, read_x);
    let g = output_channel(1u, read_y, read_x);
    let b = output_channel(2u, read_y, read_x);
    textureStore(
        output_tex,
        vec2<i32>(i32(dest_x), i32(dest_y)),
        vec4<f32>(clamp(r, 0.0, 1.0), clamp(g, 0.0, 1.0), clamp(b, 0.0, 1.0), 1.0),
    );
}
