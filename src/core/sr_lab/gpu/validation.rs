use super::buffers::{GpuBuffer, GpuTensor};
use crate::core::sr_lab::cpu::FeatureMap;
use crate::core::sr_lab::{SrLabFamily, SrLabManifest};

const MAX_TRANSIENT_BYTES: u64 = 768 * 1024 * 1024;

pub(super) fn validate_span_manifest(
    manifest: &SrLabManifest,
    input: &FeatureMap,
) -> Result<(), String> {
    if !matches!(manifest.family, SrLabFamily::Span | SrLabFamily::SpanS) {
        return Err("SPAN GPU reference requires a SPAN-family manifest".to_owned());
    }
    if manifest.scale != 2 {
        return Err(format!(
            "SPAN GPU reference currently supports x2 pixel shuffle only, got x{}",
            manifest.scale
        ));
    }
    if manifest.input_channels as usize != input.channels {
        return Err(format!(
            "input channel mismatch: manifest expects {}, image has {}",
            manifest.input_channels, input.channels
        ));
    }
    if manifest.output_channels != 3 {
        return Err("SPAN GPU reference currently supports RGB output only".to_owned());
    }
    let span = manifest
        .span
        .as_ref()
        .ok_or_else(|| "SPAN GPU reference requires span metadata".to_owned())?;
    if span.block_count == 0 || span.feature_channels == 0 {
        return Err("SPAN GPU reference requires positive span metadata".to_owned());
    }
    Ok(())
}

pub(super) fn validate_transient_size(
    input: &FeatureMap,
    feature_channels: usize,
    output_channels: usize,
    scale: usize,
    include_readback: bool,
) -> Result<(), String> {
    let pixel_count = (input.width as u64)
        .checked_mul(input.height as u64)
        .ok_or_else(|| "SPAN GPU transient size overflowed".to_owned())?;
    let input_values = pixel_count
        .checked_mul(input.channels as u64)
        .ok_or_else(|| "SPAN GPU input size overflowed".to_owned())?;
    let feature_values = pixel_count
        .checked_mul(feature_channels as u64)
        .ok_or_else(|| "SPAN GPU feature size overflowed".to_owned())?;
    let joined_values = feature_values
        .checked_mul(4)
        .ok_or_else(|| "SPAN GPU joined size overflowed".to_owned())?;
    let up_values = pixel_count
        .checked_mul(output_channels as u64)
        .and_then(|values| values.checked_mul((scale * scale) as u64))
        .ok_or_else(|| "SPAN GPU upsample size overflowed".to_owned())?;
    let output_values = pixel_count
        .checked_mul(output_channels as u64)
        .and_then(|values| values.checked_mul((scale * scale) as u64))
        .ok_or_else(|| "SPAN GPU output size overflowed".to_owned())?;
    let readback_values = if include_readback { output_values } else { 0 };
    let transient_values = input_values
        .checked_mul(2)
        .and_then(|values| values.checked_add(feature_values.checked_mul(10)?))
        .and_then(|values| values.checked_add(joined_values))
        .and_then(|values| values.checked_add(up_values))
        .and_then(|values| values.checked_add(output_values))
        .and_then(|values| values.checked_add(readback_values))
        .ok_or_else(|| "SPAN GPU transient size overflowed".to_owned())?;
    let transient_bytes = transient_values
        .checked_mul(std::mem::size_of::<f32>() as u64)
        .ok_or_else(|| "SPAN GPU transient byte size overflowed".to_owned())?;
    if transient_bytes > MAX_TRANSIENT_BYTES {
        return Err(format!(
            "SPAN GPU reference would allocate about {} MiB of transient buffers, above the {} MiB safety limit",
            bytes_to_mib(transient_bytes),
            bytes_to_mib(MAX_TRANSIENT_BYTES)
        ));
    }
    Ok(())
}

pub(super) fn validate_conv_shape(
    input: &GpuBuffer,
    output: &GpuBuffer,
    weight: &GpuTensor,
    bias: &GpuTensor,
    kernel: u32,
    name: &str,
) -> Result<(), String> {
    let expected_weight = vec![
        output.channels as u32,
        input.channels as u32,
        kernel,
        kernel,
    ];
    if weight.shape != expected_weight {
        return Err(format!(
            "{name}.weight shape {:?} does not match expected {:?}",
            weight.shape, expected_weight
        ));
    }
    let expected_bias = vec![output.channels as u32];
    if bias.shape != expected_bias {
        return Err(format!(
            "{name}.bias shape {:?} does not match expected {:?}",
            bias.shape, expected_bias
        ));
    }
    Ok(())
}

fn bytes_to_mib(bytes: u64) -> u64 {
    bytes.div_ceil(1024 * 1024)
}
