use super::bmp;
use super::candidates::DecodedImage;
use image::ImageReader;
use std::io::Cursor;

pub fn decode_ico(bytes: &[u8]) -> Result<DecodedImage, String> {
    let entries = parse_directory(bytes)?;
    let entry = entries
        .into_iter()
        .max_by_key(|entry| {
            (
                u64::from(entry.width) * u64::from(entry.height),
                entry.bit_count,
            )
        })
        .ok_or_else(|| "ICO did not contain an image entry".to_owned())?;
    let payload_end = entry
        .offset
        .checked_add(entry.size)
        .ok_or_else(|| "ICO payload offset overflows memory limits".to_owned())?;
    let payload = bytes
        .get(entry.offset..payload_end)
        .ok_or_else(|| "ICO payload points outside the file".to_owned())?;

    if payload.starts_with(b"\x89PNG\r\n\x1a\n") {
        decode_png_payload(payload)
    } else {
        bmp::decode_dib(payload, entry.width, entry.height)
    }
}

#[derive(Debug, Clone, Copy)]
struct IcoEntry {
    width: u32,
    height: u32,
    bit_count: u16,
    size: usize,
    offset: usize,
}

fn parse_directory(bytes: &[u8]) -> Result<Vec<IcoEntry>, String> {
    if bytes.len() < 6 || &bytes[0..4] != b"\0\0\x01\0" {
        return Err("not an ICO file".to_owned());
    }

    let count = usize::from(le_u16(bytes, 4)?);
    let table_bytes = count
        .checked_mul(16)
        .and_then(|value| value.checked_add(6))
        .ok_or_else(|| "ICO directory size overflows memory limits".to_owned())?;
    if count == 0 || table_bytes > bytes.len() {
        return Err("ICO directory is truncated".to_owned());
    }

    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let offset = 6 + index * 16;
        let width = ico_dimension(bytes[offset]);
        let height = ico_dimension(bytes[offset + 1]);
        let bit_count = le_u16(bytes, offset + 6)?;
        let size = le_u32(bytes, offset + 8)? as usize;
        let image_offset = le_u32(bytes, offset + 12)? as usize;
        if width == 0 || height == 0 || size == 0 {
            continue;
        }
        entries.push(IcoEntry {
            width,
            height,
            bit_count,
            size,
            offset: image_offset,
        });
    }

    Ok(entries)
}

fn decode_png_payload(bytes: &[u8]) -> Result<DecodedImage, String> {
    let image = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?
        .decode()
        .map_err(|error| error.to_string())?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Ok(DecodedImage::still(width, height, image.into_raw()))
}

fn ico_dimension(value: u8) -> u32 {
    if value == 0 {
        256
    } else {
        u32::from(value)
    }
}

fn le_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| "Unexpected end of ICO header".to_owned())?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn le_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "Unexpected end of ICO header".to_owned())?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

#[cfg(test)]
mod tests {
    use super::decode_ico;

    #[test]
    fn decodes_bmp_payload_ico() {
        let decoded = decode_ico(&bmp_payload_ico()).unwrap();

        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        assert_eq!(
            decoded.pixels,
            vec![0, 0, 255, 255, 255, 255, 255, 255, 255, 0, 0, 255, 0, 255, 0, 255,]
        );
    }

    fn bmp_payload_ico() -> Vec<u8> {
        let dib = two_by_two_icon_dib();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&[2, 2, 0, 0]);
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&24u16.to_le_bytes());
        bytes.extend_from_slice(&(dib.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&22u32.to_le_bytes());
        bytes.extend_from_slice(&dib);
        bytes
    }

    fn two_by_two_icon_dib() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&2i32.to_le_bytes());
        bytes.extend_from_slice(&4i32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&24u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 16]);
        bytes.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
        bytes.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]);
        bytes
    }
}
