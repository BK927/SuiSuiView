use super::{exact_output_size, ArtcnnC4F16RenderOptions, FEATURE_BYTES_PER_PIXEL};

pub(crate) fn validate_render_options(
    device: &wgpu::Device,
    source_size: [usize; 2],
    options: &ArtcnnC4F16RenderOptions,
) -> Result<[usize; 2], String> {
    let exact_output_size = exact_output_size(source_size)?;
    validate_output_crop(source_size, options.output_size, exact_output_size)?;
    validate_resource_size(device, source_size, exact_output_size, options)?;
    Ok(exact_output_size)
}

fn validate_output_crop(
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
            "ArtCNN C4F16 requires 2x output or a one-pixel crop, got {}x{} -> {}x{}",
            source_size[0], source_size[1], output_size[0], output_size[1]
        ));
    }
    Ok(())
}

fn validate_resource_size(
    device: &wgpu::Device,
    source_size: [usize; 2],
    exact_output_size: [usize; 2],
    options: &ArtcnnC4F16RenderOptions,
) -> Result<(), String> {
    let max_texture_dimension = device.limits().max_texture_dimension_2d as usize;
    validate_texture_size(max_texture_dimension, source_size, "source")?;
    validate_texture_size(max_texture_dimension, exact_output_size, "feature")?;
    validate_texture_size(max_texture_dimension, options.output_size, "output")?;

    let feature_bytes = texture_bytes(exact_output_size, FEATURE_BYTES_PER_PIXEL)?;
    let conv6_bytes = texture_bytes(source_size, FEATURE_BYTES_PER_PIXEL)?;
    let output_bytes = texture_bytes(options.output_size, 4)?;
    let readback_bytes = options
        .readback_padded_bytes_per_row
        .map(|bytes_per_row| {
            (bytes_per_row as u64)
                .checked_mul(options.output_size[1] as u64)
                .ok_or_else(|| "ArtCNN C4F16 readback buffer size overflowed".to_owned())
        })
        .transpose()?
        .unwrap_or(0);
    let transient_bytes = feature_bytes
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(conv6_bytes))
        .and_then(|bytes| bytes.checked_add(output_bytes))
        .and_then(|bytes| bytes.checked_add(readback_bytes))
        .ok_or_else(|| "ArtCNN C4F16 transient resource size overflowed".to_owned())?;
    if transient_bytes > options.transient_limit {
        return Err(format!(
            "ArtCNN C4F16 transient resources would use about {} MiB, above the {} MiB safety limit",
            bytes_to_mib(transient_bytes),
            bytes_to_mib(options.transient_limit)
        ));
    }

    Ok(())
}

fn validate_texture_size(
    max_texture_dimension: usize,
    size: [usize; 2],
    label: &str,
) -> Result<(), String> {
    if size[0] > max_texture_dimension || size[1] > max_texture_dimension {
        return Err(format!(
            "ArtCNN C4F16 {label} texture {}x{} exceeds adapter 2D texture limit {max_texture_dimension}",
            size[0], size[1]
        ));
    }
    Ok(())
}

fn texture_bytes(size: [usize; 2], bytes_per_pixel: u64) -> Result<u64, String> {
    let pixels = size[0]
        .checked_mul(size[1])
        .ok_or_else(|| "ArtCNN C4F16 texture pixel count overflowed".to_owned())?;
    (pixels as u64)
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| "ArtCNN C4F16 texture byte size overflowed".to_owned())
}

pub(super) fn workspace_texture_bytes(
    source_size: [usize; 2],
    exact_output_size: [usize; 2],
) -> Result<u64, String> {
    let feature_bytes = texture_bytes(exact_output_size, FEATURE_BYTES_PER_PIXEL)?;
    let conv6_bytes = texture_bytes(source_size, FEATURE_BYTES_PER_PIXEL)?;
    feature_bytes
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(conv6_bytes))
        .ok_or_else(|| "ArtCNN C4F16 workspace texture size overflowed".to_owned())
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
    use super::exact_output_size;

    #[test]
    fn exact_output_size_rejects_empty_source() {
        assert_eq!(
            exact_output_size([0, 8]),
            Err("ArtCNN C4F16 requires a non-empty source image".to_owned())
        );
        assert_eq!(
            exact_output_size([8, 0]),
            Err("ArtCNN C4F16 requires a non-empty source image".to_owned())
        );
        assert_eq!(exact_output_size([8, 6]), Ok([16, 12]));
    }
}
