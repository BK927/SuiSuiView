use super::buffers::{GpuBuffer, SpanGpuModel};
use super::kernel::SpanGpuWorkspace;
use super::validation::validate_conv_shape;
use crate::core::sr_lab::SrLabManifest;

pub(super) fn validate_span_model(
    max_storage_buffer_binding_size: u64,
    manifest: &SrLabManifest,
    model: &SpanGpuModel,
    workspace: &SpanGpuWorkspace,
) -> Result<(), String> {
    model.validate_storage_buffer_limit(max_storage_buffer_binding_size)?;
    let span = manifest
        .span
        .as_ref()
        .ok_or_else(|| "SPAN GPU reference requires span metadata".to_owned())?;
    validate_conv(
        model,
        &workspace.shifted,
        &workspace.out_feature,
        "conv_1",
        3,
    )?;

    let mut current_is_a = true;
    for block in 1..=span.block_count {
        let current = if current_is_a {
            &workspace.current_a
        } else {
            &workspace.current_b
        };
        validate_conv(
            model,
            current,
            &workspace.out1,
            &format!("block_{block}.c1_r"),
            3,
        )?;
        validate_conv(
            model,
            &workspace.out1,
            &workspace.out2,
            &format!("block_{block}.c2_r"),
            3,
        )?;
        validate_conv(
            model,
            &workspace.out2,
            &workspace.out3,
            &format!("block_{block}.c3_r"),
            3,
        )?;
        current_is_a = !current_is_a;
    }

    let current = if current_is_a {
        &workspace.current_a
    } else {
        &workspace.current_b
    };
    validate_conv(model, current, &workspace.out_b6, "conv_2", 3)?;
    validate_conv(model, &workspace.joined, &workspace.cat, "conv_cat", 1)?;
    validate_conv(model, &workspace.cat, &workspace.up, "upsampler.0", 3)?;
    Ok(())
}

fn validate_conv(
    model: &SpanGpuModel,
    input: &GpuBuffer,
    output: &GpuBuffer,
    name: &str,
    kernel: u32,
) -> Result<(), String> {
    let weight = model.tensor(&format!("{name}.weight"))?;
    let bias = model.tensor(&format!("{name}.bias"))?;
    validate_conv_shape(input, output, weight, bias, kernel, name)
}
