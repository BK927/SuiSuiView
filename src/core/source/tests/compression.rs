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
