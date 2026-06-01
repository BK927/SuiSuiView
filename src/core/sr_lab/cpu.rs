use super::blob::{SrLabTensor, SrLabWeights};
use super::{SrLabFamily, SrLabManifest};
use image::{imageops::FilterType, DynamicImage, GenericImageView, RgbaImage};
use serde::Serialize;
use std::fs;
use std::path::Path;
use std::time::Instant;

const DEFAULT_LONG_EDGE: u32 = 64;
const MAX_LONG_EDGE: u32 = 128;

#[derive(Debug, Serialize)]
pub struct SpanCpuReferenceReport {
    pub manifest: String,
    pub input: String,
    pub model: String,
    pub variant: Option<String>,
    pub requested_long_edge: u32,
    pub effective_long_edge: u32,
    pub input_width: usize,
    pub input_height: usize,
    pub output_width: usize,
    pub output_height: usize,
    pub elapsed_ms: f64,
}

#[derive(Clone)]
pub(crate) struct FeatureMap {
    pub(crate) channels: usize,
    pub(crate) height: usize,
    pub(crate) width: usize,
    pub(crate) values: Vec<f32>,
}

impl FeatureMap {
    pub(crate) fn zeros(channels: usize, height: usize, width: usize) -> Self {
        Self {
            channels,
            height,
            width,
            values: vec![0.0; channels * height * width],
        }
    }

    pub(crate) fn get(&self, channel: usize, y: isize, x: isize) -> f32 {
        if y < 0 || x < 0 || y >= self.height as isize || x >= self.width as isize {
            return 0.0;
        }
        self.values[self.offset(channel, y as usize, x as usize)]
    }

    pub(crate) fn set(&mut self, channel: usize, y: usize, x: usize, value: f32) {
        let offset = self.offset(channel, y, x);
        self.values[offset] = value;
    }

    fn offset(&self, channel: usize, y: usize, x: usize) -> usize {
        (channel * self.height + y) * self.width + x
    }
}

pub fn run_span_cpu_reference(
    manifest_path: &Path,
    input_path: &Path,
    long_edge: Option<u32>,
    output_path: Option<&Path>,
    report_path: Option<&Path>,
) -> Result<(), String> {
    let manifest = super::read_manifest(manifest_path).map_err(|error| error.to_string())?;
    let weights =
        super::blob::read_checked_weights(manifest_path, &manifest, "SPAN CPU reference")?;
    let (requested_long_edge, effective_long_edge) = span_reference_long_edge(long_edge);
    let input = load_input_image(input_path, effective_long_edge)?;

    let started = Instant::now();
    let output = span_forward(&manifest, &weights, &input)?;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    if output.channels != manifest.output_channels as usize {
        return Err(format!(
            "output channel mismatch: manifest expects {}, graph produced {}",
            manifest.output_channels, output.channels
        ));
    }

    if let Some(output_path) = output_path {
        write_output_image(output_path, &output)?;
    }
    let report = SpanCpuReferenceReport {
        manifest: manifest_path.display().to_string(),
        input: input_path.display().to_string(),
        model: manifest.name.clone(),
        variant: manifest.variant.clone(),
        requested_long_edge,
        effective_long_edge,
        input_width: input.width,
        input_height: input.height,
        output_width: output.width,
        output_height: output.height,
        elapsed_ms,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
    );
    if let Some(report_path) = report_path {
        if let Some(parent) = report_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(
            report_path,
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(crate) fn span_reference_long_edge(long_edge: Option<u32>) -> (u32, u32) {
    let requested_long_edge = long_edge.unwrap_or(DEFAULT_LONG_EDGE);
    (
        requested_long_edge,
        requested_long_edge.clamp(1, MAX_LONG_EDGE),
    )
}

pub(crate) fn load_input_image(path: &Path, long_edge: u32) -> Result<FeatureMap, String> {
    let mut image = image::ImageReader::open(path)
        .map_err(|error| error.to_string())?
        .decode()
        .map_err(|error| error.to_string())?;
    let (width, height) = image.dimensions();
    let current_long = width.max(height);
    if current_long > long_edge {
        let scale = long_edge as f32 / current_long as f32;
        let target_width = ((width as f32 * scale).round() as u32).max(1);
        let target_height = ((height as f32 * scale).round() as u32).max(1);
        image = DynamicImage::ImageRgba8(image::imageops::resize(
            &image.to_rgba8(),
            target_width,
            target_height,
            FilterType::CatmullRom,
        ));
    }

    let rgb = image.to_rgb8();
    let width = rgb.width() as usize;
    let height = rgb.height() as usize;
    let mut input = FeatureMap::zeros(3, height, width);
    for y in 0..height {
        for x in 0..width {
            let pixel = rgb.get_pixel(x as u32, y as u32);
            for channel in 0..3 {
                input.set(channel, y, x, pixel[channel] as f32 / 255.0);
            }
        }
    }
    Ok(input)
}

pub(crate) fn span_forward(
    manifest: &SrLabManifest,
    weights: &SrLabWeights,
    input: &FeatureMap,
) -> Result<FeatureMap, String> {
    if !matches!(manifest.family, SrLabFamily::Span | SrLabFamily::SpanS) {
        return Err("SPAN CPU reference requires a SPAN-family manifest".to_owned());
    }
    let span = manifest
        .span
        .as_ref()
        .ok_or_else(|| "SPAN CPU reference requires span metadata".to_owned())?;
    if input.channels != manifest.input_channels as usize {
        return Err(format!(
            "input channel mismatch: manifest expects {}, image has {}",
            manifest.input_channels, input.channels
        ));
    }

    let shifted = mean_shift(input, span.rgb_mean, span.img_range);
    let out_feature = conv2d3x3(&shifted, weights, "conv_1")?;
    let mut current = out_feature.clone();
    let mut out_b1 = None;
    let mut out_b5_2 = None;

    for block in 1..=span.block_count {
        let out1 = conv2d3x3(&current, weights, &format!("block_{block}.c1_r"))?;
        let out1_act = silu(&out1);
        let out2 = conv2d3x3(&out1_act, weights, &format!("block_{block}.c2_r"))?;
        let out2_act = silu(&out2);
        let out3 = conv2d3x3(&out2_act, weights, &format!("block_{block}.c3_r"))?;
        current = span_gate(&out3, &current)?;
        if block == 1 {
            out_b1 = Some(current.clone());
        }
        if block == span.block_count {
            out_b5_2 = Some(out1);
        }
    }

    let out_b1 = out_b1.ok_or_else(|| "SPAN graph did not produce block_1 output".to_owned())?;
    let out_b5_2 =
        out_b5_2.ok_or_else(|| "SPAN graph did not produce final block skip output".to_owned())?;
    let out_b6 = conv2d3x3(&current, weights, "conv_2")?;
    let joined = concat4(&out_feature, &out_b6, &out_b1, &out_b5_2)?;
    let out = conv2d1x1(&joined, weights, "conv_cat")?;
    let out = conv2d3x3(&out, weights, "upsampler.0")?;
    pixel_shuffle(&out, manifest.scale as usize)
}

fn mean_shift(input: &FeatureMap, rgb_mean: [f32; 3], img_range: f32) -> FeatureMap {
    let mut output = input.clone();
    for channel in 0..input.channels {
        let mean = rgb_mean[channel.min(2)];
        for y in 0..input.height {
            for x in 0..input.width {
                let value = (input.get(channel, y as isize, x as isize) - mean) * img_range;
                output.set(channel, y, x, value);
            }
        }
    }
    output
}

fn conv2d3x3(input: &FeatureMap, weights: &SrLabWeights, name: &str) -> Result<FeatureMap, String> {
    conv2d(input, weights, name, 3, 1)
}

fn conv2d1x1(input: &FeatureMap, weights: &SrLabWeights, name: &str) -> Result<FeatureMap, String> {
    conv2d(input, weights, name, 1, 0)
}

fn conv2d(
    input: &FeatureMap,
    weights: &SrLabWeights,
    name: &str,
    kernel: usize,
    padding: isize,
) -> Result<FeatureMap, String> {
    let weight = tensor(weights, &format!("{name}.weight"))?;
    let bias = tensor(weights, &format!("{name}.bias"))?;
    if weight.shape.len() != 4 {
        return Err(format!("{name}.weight must be rank 4"));
    }
    if bias.shape.len() != 1 {
        return Err(format!("{name}.bias must be rank 1"));
    }
    let output_channels = weight.shape[0] as usize;
    let input_channels = weight.shape[1] as usize;
    let kernel_y = weight.shape[2] as usize;
    let kernel_x = weight.shape[3] as usize;
    if input_channels != input.channels || kernel_y != kernel || kernel_x != kernel {
        return Err(format!("{name}.weight shape does not match input/kernel"));
    }
    if bias.values.len() != output_channels {
        return Err(format!("{name}.bias length does not match output channels"));
    }

    let mut output = FeatureMap::zeros(output_channels, input.height, input.width);
    for oc in 0..output_channels {
        for y in 0..input.height {
            for x in 0..input.width {
                let mut sum = bias.values[oc];
                for ic in 0..input_channels {
                    for ky in 0..kernel {
                        for kx in 0..kernel {
                            let input_y = y as isize + ky as isize - padding;
                            let input_x = x as isize + kx as isize - padding;
                            let weight_offset =
                                ((oc * input_channels + ic) * kernel + ky) * kernel + kx;
                            sum += input.get(ic, input_y, input_x) * weight.values[weight_offset];
                        }
                    }
                }
                output.set(oc, y, x, sum);
            }
        }
    }
    Ok(output)
}

fn silu(input: &FeatureMap) -> FeatureMap {
    let mut output = input.clone();
    for value in &mut output.values {
        *value *= 1.0 / (1.0 + (-*value).exp());
    }
    output
}

fn span_gate(out3: &FeatureMap, current: &FeatureMap) -> Result<FeatureMap, String> {
    ensure_same_shape(out3, current, "span_gate")?;
    let mut output = out3.clone();
    for ((output, out3), current) in output
        .values
        .iter_mut()
        .zip(&out3.values)
        .zip(&current.values)
    {
        let sim_att = 1.0 / (1.0 + (-*out3).exp()) - 0.5;
        *output = (*out3 + *current) * sim_att;
    }
    Ok(output)
}

fn concat4(
    a: &FeatureMap,
    b: &FeatureMap,
    c: &FeatureMap,
    d: &FeatureMap,
) -> Result<FeatureMap, String> {
    for map in [b, c, d] {
        if map.height != a.height || map.width != a.width {
            return Err("concat4 input dimensions do not match".to_owned());
        }
    }
    let mut output = FeatureMap::zeros(
        a.channels + b.channels + c.channels + d.channels,
        a.height,
        a.width,
    );
    let mut channel_offset = 0usize;
    for map in [a, b, c, d] {
        for channel in 0..map.channels {
            for y in 0..map.height {
                for x in 0..map.width {
                    output.set(
                        channel_offset + channel,
                        y,
                        x,
                        map.get(channel, y as isize, x as isize),
                    );
                }
            }
        }
        channel_offset += map.channels;
    }
    Ok(output)
}

fn pixel_shuffle(input: &FeatureMap, scale: usize) -> Result<FeatureMap, String> {
    if scale == 0 {
        return Err("pixel shuffle scale must be positive".to_owned());
    }
    let scale_sq = scale * scale;
    if input.channels % scale_sq != 0 {
        return Err("pixel shuffle input channels are not divisible by scale^2".to_owned());
    }
    let output_channels = input.channels / scale_sq;
    let mut output = FeatureMap::zeros(output_channels, input.height * scale, input.width * scale);
    for oc in 0..output_channels {
        for y in 0..input.height {
            for x in 0..input.width {
                for sy in 0..scale {
                    for sx in 0..scale {
                        let input_channel = oc * scale_sq + sy * scale + sx;
                        output.set(
                            oc,
                            y * scale + sy,
                            x * scale + sx,
                            input.get(input_channel, y as isize, x as isize),
                        );
                    }
                }
            }
        }
    }
    Ok(output)
}

fn tensor<'a>(weights: &'a SrLabWeights, name: &str) -> Result<&'a SrLabTensor, String> {
    weights
        .tensor(name)
        .ok_or_else(|| format!("missing SR Lab tensor: {name}"))
}

fn ensure_same_shape(left: &FeatureMap, right: &FeatureMap, label: &str) -> Result<(), String> {
    if left.channels != right.channels || left.height != right.height || left.width != right.width {
        return Err(format!("{label} input shapes do not match"));
    }
    Ok(())
}

pub(crate) fn write_output_image(path: &Path, output: &FeatureMap) -> Result<(), String> {
    let rgba = RgbaImage::from_raw(
        output.width as u32,
        output.height as u32,
        output_to_rgba_bytes(output)?,
    )
    .ok_or_else(|| "failed to build output image".to_owned())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    rgba.save(path).map_err(|error| error.to_string())
}

fn output_to_rgba_bytes(output: &FeatureMap) -> Result<Vec<u8>, String> {
    if output.channels != 3 {
        return Err(format!(
            "SPAN CPU reference expected 3 output channels, got {}",
            output.channels
        ));
    }
    let mut bytes = Vec::with_capacity(output.width * output.height * 4);
    for y in 0..output.height {
        for x in 0..output.width {
            for channel in 0..3 {
                let value = output.get(channel, y as isize, x as isize);
                bytes.push(value.round().clamp(0.0, 255.0) as u8);
            }
            bytes.push(255);
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{span_forward, FeatureMap};
    use crate::core::sr_lab::blob::{SrLabTensor, SrLabWeights};
    use crate::core::sr_lab::{SrLabFamily, SrLabManifest, SrLabSpanMetadata};

    fn zero_tensor(name: &str, shape: Vec<u32>) -> SrLabTensor {
        let value_count = shape.iter().product::<u32>() as usize;
        SrLabTensor {
            name: name.to_owned(),
            shape,
            values: vec![0.0; value_count],
        }
    }

    fn zero_weights() -> SrLabWeights {
        SrLabWeights {
            tensors: vec![
                zero_tensor("conv_1.weight", vec![1, 3, 3, 3]),
                zero_tensor("conv_1.bias", vec![1]),
                zero_tensor("block_1.c1_r.weight", vec![1, 1, 3, 3]),
                zero_tensor("block_1.c1_r.bias", vec![1]),
                zero_tensor("block_1.c2_r.weight", vec![1, 1, 3, 3]),
                zero_tensor("block_1.c2_r.bias", vec![1]),
                zero_tensor("block_1.c3_r.weight", vec![1, 1, 3, 3]),
                zero_tensor("block_1.c3_r.bias", vec![1]),
                zero_tensor("conv_2.weight", vec![1, 1, 3, 3]),
                zero_tensor("conv_2.bias", vec![1]),
                zero_tensor("conv_cat.weight", vec![1, 4, 1, 1]),
                zero_tensor("conv_cat.bias", vec![1]),
                zero_tensor("upsampler.0.weight", vec![12, 1, 3, 3]),
                zero_tensor("upsampler.0.bias", vec![12]),
            ],
        }
    }

    fn tiny_manifest() -> SrLabManifest {
        SrLabManifest {
            name: "tiny SPAN-S".to_owned(),
            family: SrLabFamily::SpanS,
            variant: Some("SPAN-S".to_owned()),
            scale: 2,
            input_channels: 3,
            output_channels: 3,
            weights_format: "suisui-srlab-v1".to_owned(),
            weights_file: Some("weights.srlab".to_owned()),
            weights_sha256: "0".repeat(64),
            source: "local-test".to_owned(),
            source_commit: None,
            source_checkpoint_url: None,
            source_checkpoint_archive_sha256: None,
            source_checkpoint_file: None,
            source_checkpoint_sha256: None,
            license: "MIT".to_owned(),
            notes: Vec::new(),
            span: Some(SrLabSpanMetadata {
                feature_channels: 1,
                block_count: 1,
                reparameterized_conv3xc: true,
                img_range: 255.0,
                rgb_mean: [0.4488, 0.4371, 0.4040],
            }),
            layers: Vec::new(),
        }
    }

    #[test]
    fn zero_weight_span_graph_outputs_scaled_black_image() {
        let input = FeatureMap {
            channels: 3,
            height: 1,
            width: 1,
            values: vec![1.0, 1.0, 1.0],
        };

        let output = span_forward(&tiny_manifest(), &zero_weights(), &input).unwrap();

        assert_eq!(output.channels, 3);
        assert_eq!(output.width, 2);
        assert_eq!(output.height, 2);
        assert!(output.values.iter().all(|value| *value == 0.0));
    }
}
