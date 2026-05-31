pub(in crate::app) fn dimensions_from_header(bytes: &[u8]) -> Option<(u32, u32)> {
    png_dimensions(bytes)
        .or_else(|| jpeg_dimensions(bytes))
        .or_else(|| webp_dimensions(bytes))
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || bytes.get(..8)? != b"\x89PNG\r\n\x1a\n" || bytes.get(12..16)? != b"IHDR"
    {
        return None;
    }
    let width = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
    let height = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
    Some((width, height))
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut index = 2usize;
    while index + 3 < bytes.len() {
        while index < bytes.len() && bytes[index] != 0xFF {
            index += 1;
        }
        while index < bytes.len() && bytes[index] == 0xFF {
            index += 1;
        }
        if index >= bytes.len() {
            return None;
        }
        let marker = bytes[index];
        index += 1;
        if matches!(marker, 0x01 | 0xD0..=0xD9) {
            continue;
        }
        if index + 2 > bytes.len() {
            return None;
        }
        let segment_len = u16::from_be_bytes(bytes.get(index..index + 2)?.try_into().ok()?);
        let segment_len = usize::from(segment_len);
        if segment_len < 2 {
            return None;
        }
        let payload = index + 2;
        let next = index.checked_add(segment_len)?;
        if jpeg_sof_marker(marker) {
            if payload + 5 > bytes.len() {
                return None;
            }
            let height = u16::from_be_bytes(bytes.get(payload + 1..payload + 3)?.try_into().ok()?);
            let width = u16::from_be_bytes(bytes.get(payload + 3..payload + 5)?.try_into().ok()?);
            return Some((u32::from(width), u32::from(height)));
        }
        index = next;
    }
    None
}

fn jpeg_sof_marker(marker: u8) -> bool {
    matches!(
        marker,
        0xC0 | 0xC1 | 0xC2 | 0xC3 | 0xC5 | 0xC6 | 0xC7 | 0xC9 | 0xCA | 0xCB | 0xCD | 0xCE | 0xCF
    )
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 30 || bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WEBP" {
        return None;
    }
    match bytes.get(12..16)? {
        b"VP8X" => {
            let width = 1 + read_u24_le(bytes.get(24..27)?)?;
            let height = 1 + read_u24_le(bytes.get(27..30)?)?;
            Some((width, height))
        }
        b"VP8L" => {
            if *bytes.get(20)? != 0x2F {
                return None;
            }
            let bits = u32::from_le_bytes(bytes.get(21..25)?.try_into().ok()?);
            let width = (bits & 0x3FFF) + 1;
            let height = ((bits >> 14) & 0x3FFF) + 1;
            Some((width, height))
        }
        b"VP8 " => {
            if bytes.get(23..26)? != [0x9D, 0x01, 0x2A] {
                return None;
            }
            let width = u16::from_le_bytes(bytes.get(26..28)?.try_into().ok()?) & 0x3FFF;
            let height = u16::from_le_bytes(bytes.get(28..30)?.try_into().ok()?) & 0x3FFF;
            Some((u32::from(width), u32::from(height)))
        }
        _ => None,
    }
}

fn read_u24_le(bytes: &[u8]) -> Option<u32> {
    Some(
        u32::from(*bytes.first()?)
            | (u32::from(*bytes.get(1)?) << 8)
            | (u32::from(*bytes.get(2)?) << 16),
    )
}

#[cfg(test)]
mod tests {
    use super::dimensions_from_header;

    #[test]
    fn reads_jpeg_sof_dimensions() {
        let bytes = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x03,
            0x20, 0x04, 0x00, 0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x00, 0x03, 0x11, 0x00,
        ];

        assert_eq!(dimensions_from_header(&bytes), Some((1024, 800)));
    }

    #[test]
    fn reads_webp_vp8x_dimensions() {
        let mut bytes = [0; 30];
        bytes[0..4].copy_from_slice(b"RIFF");
        bytes[8..12].copy_from_slice(b"WEBP");
        bytes[12..16].copy_from_slice(b"VP8X");
        bytes[24..27].copy_from_slice(&(1024u32 - 1).to_le_bytes()[0..3]);
        bytes[27..30].copy_from_slice(&(800u32 - 1).to_le_bytes()[0..3]);

        assert_eq!(dimensions_from_header(&bytes), Some((1024, 800)));
    }
}
