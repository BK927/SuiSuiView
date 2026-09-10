const FRAMEBUFFER_SRGB: bool = false;
@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var display_lut: texture_3d<f32>;
@group(0) @binding(2) var lut_sampler: sampler;

@vertex fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
}
fn linear_from_gamma(value: vec3<f32>) -> vec3<f32> {
    return select(pow((value + 0.055) / 1.055, vec3<f32>(2.4)),
                  value / 12.92, value <= vec3<f32>(0.04045));
}
@fragment fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let composed = textureLoad(scene, vec2<i32>(position.xy), 0);
    // The scene was rendered over an opaque background. Its UI, translucent
    // image pixels and fades have already been composited, before this LUT.
    // The FP16 scene always stores sRGB codes, independently of the surface.
    let srgb = composed.rgb;
    let edge = f32(textureDimensions(display_lut).x);
    let coordinate = (clamp(srgb, vec3<f32>(0.0), vec3<f32>(1.0)) * (edge - 1.0) + 0.5) / edge;
    var mapped = textureSampleLevel(display_lut, lut_sampler, coordinate, 0.0).rgb;
    // Stable output quantization dither, after color conversion. Never animate.
    let noise = fract(52.9829189 * fract(dot(position.xy, vec2<f32>(0.06711056, 0.00583715)))) - 0.5;
    mapped = clamp(mapped + noise / 255.0, vec3<f32>(0.0), vec3<f32>(1.0));
    // An sRGB attachment encodes once; a plain UNORM attachment stores codes.
    if FRAMEBUFFER_SRGB { mapped = linear_from_gamma(mapped); }
    return vec4<f32>(mapped, composed.a);
}
