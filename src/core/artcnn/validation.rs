use super::{exact_output_size, ArtcnnRenderOptions, ArtcnnVariant, FEATURE_BYTES_PER_PIXEL};

pub(crate) fn validate_render_options(
    device: &wgpu::Device,
    variant: ArtcnnVariant,
    source_size: [usize; 2],
    options: &ArtcnnRenderOptions,
) -> Result<[usize; 2], String> {
    let exact_output_size = exact_output_size(variant, source_size)?;
    let feature_size = variant.feature_size(source_size)?;
    validate_output_crop(variant, source_size, options.output_size, exact_output_size)?;
    validate_resource_size(
        device,
        variant,
        source_size,
        feature_size,
        exact_output_size,
        options,
    )?;
    Ok(exact_output_size)
}

fn validate_output_crop(
    variant: ArtcnnVariant,
    source_size: [usize; 2],
    output_size: [usize; 2],
    exact_output_size: [usize; 2],
) -> Result<(), String> {
    if output_size[0] == 0
        || output_size[1] == 0
        || output_size[0] > exact_output_size[0]
        || output_size[1] > exact_output_size[1]
        || exact_output_size[0] - output_size[0] > 1
        || exact_output_size[1] - output_size[1] > 1
    {
        return Err(format!(
            "{} requires 2x output or a one-pixel crop, got {}x{} -> {}x{}",
            variant.label(),
            source_size[0],
            source_size[1],
            output_size[0],
            output_size[1]
        ));
    }
    Ok(())
}

fn validate_resource_size(
    device: &wgpu::Device,
    variant: ArtcnnVariant,
    source_size: [usize; 2],
    feature_size: [usize; 2],
    exact_output_size: [usize; 2],
    options: &ArtcnnRenderOptions,
) -> Result<(), String> {
    let max_texture_dimension = device.limits().max_texture_dimension_2d as usize;
    validate_texture_size(max_texture_dimension, variant, source_size, "source")?;
    validate_texture_size(max_texture_dimension, variant, feature_size, "feature")?;
    validate_texture_size(
        max_texture_dimension,
        variant,
        exact_output_size,
        "exact output",
    )?;
    validate_texture_size(
        max_texture_dimension,
        variant,
        options.output_size,
        "output",
    )?;

    let feature_bytes = texture_bytes(variant, feature_size, FEATURE_BYTES_PER_PIXEL)?;
    let conv6_bytes = texture_bytes(variant, source_size, FEATURE_BYTES_PER_PIXEL)?;
    let output_bytes = texture_bytes(variant, options.output_size, 4)?;
    let readback_bytes = options
        .readback_padded_bytes_per_row
        .map(|bytes_per_row| {
            (bytes_per_row as u64)
                .checked_mul(options.output_size[1] as u64)
                .ok_or_else(|| format!("{} readback buffer size overflowed", variant.label()))
        })
        .transpose()?
        .unwrap_or(0);
    let transient_bytes = feature_bytes
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(conv6_bytes))
        .and_then(|bytes| bytes.checked_add(output_bytes))
        .and_then(|bytes| bytes.checked_add(readback_bytes))
        .ok_or_else(|| format!("{} transient resource size overflowed", variant.label()))?;
    if transient_bytes > options.transient_limit {
        return Err(format!(
            "{} transient resources would use about {} MiB, above the {} MiB safety limit",
            variant.label(),
            bytes_to_mib(transient_bytes),
            bytes_to_mib(options.transient_limit)
        ));
    }

    Ok(())
}

fn validate_texture_size(
    max_texture_dimension: usize,
    variant: ArtcnnVariant,
    size: [usize; 2],
    label: &str,
) -> Result<(), String> {
    if size[0] > max_texture_dimension || size[1] > max_texture_dimension {
        return Err(format!(
            "{} {label} texture {}x{} exceeds adapter 2D texture limit {max_texture_dimension}",
            variant.label(),
            size[0],
            size[1]
        ));
    }
    Ok(())
}

fn texture_bytes(
    variant: ArtcnnVariant,
    size: [usize; 2],
    bytes_per_pixel: u64,
) -> Result<u64, String> {
    let pixels = size[0]
        .checked_mul(size[1])
        .ok_or_else(|| format!("{} texture pixel count overflowed", variant.label()))?;
    (pixels as u64)
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| format!("{} texture byte size overflowed", variant.label()))
}

pub(super) fn workspace_texture_bytes(
    variant: ArtcnnVariant,
    source_size: [usize; 2],
    feature_size: [usize; 2],
) -> Result<u64, String> {
    let feature_bytes = texture_bytes(variant, feature_size, FEATURE_BYTES_PER_PIXEL)?;
    let conv6_bytes = texture_bytes(variant, source_size, FEATURE_BYTES_PER_PIXEL)?;
    feature_bytes
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(conv6_bytes))
        .ok_or_else(|| "ArtCNN workspace texture size overflowed".to_owned())
}

fn bytes_to_mib(bytes: u64) -> u64 {
    bytes.div_ceil(1024 * 1024)
}

pub(crate) fn extent_for_size(size: [usize; 2]) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: size[0] as u32,
        height: size[1] as u32,
        depth_or_array_layers: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::{exact_output_size, ArtcnnVariant};

    #[test]
    fn exact_output_size_rejects_empty_source() {
        assert_eq!(
            exact_output_size(ArtcnnVariant::C4F16, [0, 8]),
            Err("ArtCNN C4F16 requires a non-empty source image".to_owned())
        );
        assert_eq!(
            exact_output_size(ArtcnnVariant::C4F16, [8, 0]),
            Err("ArtCNN C4F16 requires a non-empty source image".to_owned())
        );
        assert_eq!(
            exact_output_size(ArtcnnVariant::C4F32Ds, [8, 6]),
            Ok([16, 12])
        );
    }

    #[test]
    fn c4f32_feature_size_uses_wide_packing() {
        assert_eq!(ArtcnnVariant::C4F32.feature_size([8, 6]), Ok([32, 12]));
    }
}
