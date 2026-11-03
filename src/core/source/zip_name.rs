//! ZIP entry names decoded exactly as zip 2.x did. Page names feed
//! `zip_book_id` and saved bookmarks, so a decoder change gives existing
//! archives a new identity and drops their bookmarks. zip 9 reads unflagged
//! names as UTF-8 whenever they parse; zip 2.x used CP437 unless bit 11 was set.

const UTF8_NAME_FLAG: u16 = 1 << 11;

pub(super) fn decode_zip_entry_name(raw: &[u8], flags: u16) -> String {
    if flags & UTF8_NAME_FLAG != 0 {
        String::from_utf8_lossy(raw).into_owned()
    } else {
        raw.iter().map(|&byte| cp437_char(byte)).collect()
    }
}

fn cp437_char(byte: u8) -> char {
    match byte {
        0x00..=0x7f => char::from(byte),
        _ => CP437_HIGH[usize::from(byte - 0x80)],
    }
}

/// IBM code page 437, bytes 0x80..=0xFF.
const CP437_HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', 'É', 'æ', 'Æ',
    'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', 'á', 'í', 'ó', 'ú', 'ñ', 'Ñ',
    'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕',
    '╣', '║', '╗', '╝', '╜', '╛', '┐', '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦',
    '╠', '═', '╬', '╧', '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐',
    '▀', 'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', '≡', '±',
    '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}',
];

#[cfg(test)]
mod tests {
    use super::{decode_zip_entry_name, UTF8_NAME_FLAG};

    #[test]
    fn unflagged_names_stay_cp437_even_when_they_parse_as_utf8() {
        assert_eq!(decode_zip_entry_name("é.png".as_bytes(), 0), "├⌐.png");
        assert_eq!(decode_zip_entry_name(b"Cura\x87ao", 0), "Curaçao");
        assert_eq!(decode_zip_entry_name(b"\xcc\xcd\xcd\xb9", 0), "╠══╣");
    }

    #[test]
    fn flagged_names_are_lossy_utf8() {
        assert_eq!(
            decode_zip_entry_name("만화/001.png".as_bytes(), UTF8_NAME_FLAG),
            "만화/001.png"
        );
        assert_eq!(
            decode_zip_entry_name(b"a\xff.png", UTF8_NAME_FLAG),
            "a\u{fffd}.png"
        );
    }
}
