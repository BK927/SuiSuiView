//! Source byte identity and natural order are independent of the image decoder.
use super::*;

#[test]
fn folder_and_zip_methods_preserve_order_original_bytes_and_prefixes() {
    let dir = temp_test_dir("한글 archive with spaces");
    let folder = dir.join("한글 pages");
    fs::create_dir(&folder).unwrap();
    let names = [
        "page-1.png",
        "page-2.jpg",
        "page-3.webp",
        "page-4.bmp",
        "page-5.gif",
        "page-10.tiff",
    ];
    let pages: Vec<_> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            format!("original page {index}: {name}")
                .repeat(30)
                .into_bytes()
        })
        .collect();
    for (name, bytes) in names.iter().zip(&pages) {
        fs::write(folder.join(name), bytes).unwrap();
    }
    let check = |source: &dyn BookSource| {
        assert_eq!(source.page_count(), names.len());
        for (index, (name, bytes)) in names.iter().zip(&pages).enumerate() {
            assert_eq!(source.page_name(index), Some(*name));
            assert_eq!(source.read_page(index).unwrap(), *bytes);
            assert_eq!(source.read_page_prefix(index, 17).unwrap(), bytes[..17]);
        }
    };
    check(&FolderSource::open(&folder).unwrap());
    for method in [
        zip::CompressionMethod::Stored,
        zip::CompressionMethod::Deflated,
        zip::CompressionMethod::Bzip2,
    ] {
        for extension in ["zip", "cbz"] {
            let path = dir.join(format!("만화 {method:?}.{extension}"));
            let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
            for (name, bytes) in names.iter().zip(&pages).rev() {
                writer
                    .start_file(
                        *name,
                        SimpleFileOptions::default().compression_method(method),
                    )
                    .unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
            check(&ZipCbzSource::open(&path).unwrap());
        }
    }
}

/// Some archivers (macOS Finder among them) store UTF-8 names without the
/// UTF-8 flag. Earlier builds keyed those books and bookmarks by the CP437
/// reading, so it must not change under a newer zip crate.
#[test]
fn unflagged_utf8_entry_names_keep_their_cp437_identity() {
    let dir = temp_test_dir("unflagged-utf8-names");
    let name = "페이지-01.png";
    let mut bytes = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        writer
            .start_file(
                name,
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(b"page").unwrap();
        writer.finish().unwrap();
    }
    let flagged = dir.join("flagged.cbz");
    fs::write(&flagged, &bytes).unwrap();
    clear_utf8_name_flags(&mut bytes);
    let unflagged = dir.join("unflagged.cbz");
    fs::write(&unflagged, &bytes).unwrap();

    let flagged = ZipCbzSource::open(&flagged).unwrap();
    let unflagged = ZipCbzSource::open(&unflagged).unwrap();

    assert_eq!(flagged.page_name(0), Some(name));
    assert_eq!(unflagged.page_name(0), Some("φÄÿ∞¥┤∞ºÇ-01.png"));
    assert_ne!(flagged.book_id(), unflagged.book_id());
}

/// Clears general-purpose bit 11 in every local and central header.
fn clear_utf8_name_flags(zip: &mut [u8]) {
    for at in 0..zip.len().saturating_sub(4) {
        let flags_high_byte = match &zip[at..at + 4] {
            b"PK\x03\x04" => at + 7,
            b"PK\x01\x02" => at + 9,
            _ => continue,
        };
        zip[flags_high_byte] &= !0x08;
    }
}
