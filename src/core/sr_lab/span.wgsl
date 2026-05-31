struct SpanParams {
    width: u32,
    height: u32,
    input_channels: u32,
    output_channels: u32,
    kernel: u32,
    padding: u32,
    scale: u32,
    activation: u32,
    rgb_mean: vec4<f32>,
    img_range: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<storage, read> input0: array<f32>;
@group(0) @binding(1) var<storage, read> input1: array<f32>;
@group(0) @binding(2) var<storage, read> input2: array<f32>;
@group(0) @binding(3) var<storage, read> input3: array<f32>;
@group(0) @binding(4) var<storage, read> weights: array<f32>;
@group(0) @binding(5) var<storage, read> bias: array<f32>;
@group(0) @binding(6) var<storage, read_write> output0: array<f32>;
@group(0) @binding(7) var<uniform> params: SpanParams;

fn chw_index(channel: u32, y: u32, x: u32, width: u32, height: u32) -> u32 {
    return (channel * height + y) * width + x;
}

fn silu(value: f32) -> f32 {
    return value / (1.0 + exp(-value));
}

@compute @workgroup_size(8, 8, 1)
fn span_mean_shift(@builtin(global_invocation_id) global_id: vec3<u32>) {
    if (global_id.x >= params.width || global_id.y >= params.height || global_id.z >= 3u) {
        return;
    }
    let offset = chw_index(global_id.z, global_id.y, global_id.x, params.width, params.height);
    output0[offset] = (input0[offset] - params.rgb_mean[global_id.z]) * params.img_range;
}

@compute @workgroup_size(8, 8, 1)
fn span_conv2d(@builtin(global_invocation_id) global_id: vec3<u32>) {
    if (global_id.x >= params.width || global_id.y >= params.height || global_id.z >= params.output_channels) {
        return;
    }

    let oc = global_id.z;
    var sum = bias[oc];
    var ic = 0u;
    loop {
        if (ic >= params.input_channels) {
            break;
        }
        var ky = 0u;
        loop {
            if (ky >= params.kernel) {
                break;
            }
            var kx = 0u;
            loop {
                if (kx >= params.kernel) {
                    break;
                }
                let input_y = i32(global_id.y) + i32(ky) - i32(params.padding);
                let input_x = i32(global_id.x) + i32(kx) - i32(params.padding);
                if (input_y >= 0 && input_x >= 0 && input_y < i32(params.height) && input_x < i32(params.width)) {
                    let input_offset = chw_index(
                        ic,
                        u32(input_y),
                        u32(input_x),
                        params.width,
                        params.height,
                    );
                    var input_value = input0[input_offset];
                    if (params.activation == 1u) {
                        input_value = silu(input_value);
                    }
                    let weight_offset = ((oc * params.input_channels + ic) * params.kernel + ky) * params.kernel + kx;
                    sum = sum + input_value * weights[weight_offset];
                }
                kx = kx + 1u;
            }
            ky = ky + 1u;
        }
        ic = ic + 1u;
    }

    let output_offset = chw_index(oc, global_id.y, global_id.x, params.width, params.height);
    output0[output_offset] = sum;
}

@compute @workgroup_size(8, 8, 1)
fn span_gate(@builtin(global_invocation_id) global_id: vec3<u32>) {
    if (global_id.x >= params.width || global_id.y >= params.height || global_id.z >= params.output_channels) {
        return;
    }
    let offset = chw_index(global_id.z, global_id.y, global_id.x, params.width, params.height);
    let out3 = input0[offset];
    let current = input1[offset];
    let sim_att = 1.0 / (1.0 + exp(-out3)) - 0.5;
    output0[offset] = (out3 + current) * sim_att;
}

@compute @workgroup_size(8, 8, 1)
fn span_concat4(@builtin(global_invocation_id) global_id: vec3<u32>) {
    if (global_id.x >= params.width || global_id.y >= params.height || global_id.z >= params.output_channels) {
        return;
    }
    let feature_channels = params.input_channels;
    let channel = global_id.z;
    let local_channel = channel % feature_channels;
    let offset = chw_index(local_channel, global_id.y, global_id.x, params.width, params.height);
    let value = select(
        select(input0[offset], input1[offset], channel >= feature_channels),
        select(input2[offset], input3[offset], channel >= feature_channels * 3u),
        channel >= feature_channels * 2u,
    );
    let output_offset = chw_index(channel, global_id.y, global_id.x, params.width, params.height);
    output0[output_offset] = value;
}

@compute @workgroup_size(8, 8, 1)
fn span_pixel_shuffle2x(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let output_width = params.width * params.scale;
    let output_height = params.height * params.scale;
    if (global_id.x >= output_width || global_id.y >= output_height || global_id.z >= params.output_channels) {
        return;
    }

    let sx = global_id.x % params.scale;
    let sy = global_id.y % params.scale;
    let input_x = global_id.x / params.scale;
    let input_y = global_id.y / params.scale;
    let input_channel = global_id.z * params.scale * params.scale + sy * params.scale + sx;
    let input_offset = chw_index(input_channel, input_y, input_x, params.width, params.height);
    let output_offset = chw_index(global_id.z, global_id.y, global_id.x, output_width, output_height);
    output0[output_offset] = input0[input_offset];
}
