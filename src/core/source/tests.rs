use super::{
    classify_path, open_source_from_path, BookSource, FolderSource, SourceKind, ZipCbzSource,
};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use zip::write::SimpleFileOptions;

#[test]
fn zip_book_id_survives_path_and_filename_changes() {
    let dir = temp_test_dir("same-cbz-id");
    let first = dir.join("first.cbz");
    let second = dir.join("renamed.cbz");

    write_test_zip(&first);
    fs::copy(&first, &second).unwrap();

    let first_source = ZipCbzSource::open(&first).unwrap();
    let second_source = ZipCbzSource::open(&second).unwrap();

    assert_eq!(first_source.book_id(), second_source.book_id());
}

#[test]
fn zip_source_filters_metadata_and_non_images() {
    let dir = temp_test_dir("zip-filter");
    let archive = dir.join("book.cbz");
    write_test_zip(&archive);

    let source = ZipCbzSource::open(&archive).unwrap();

    assert_eq!(source.page_count(), 3);
    assert_eq!(source.page_name(0), Some("chapter/page-001.jpg"));
    assert_eq!(source.page_name(1), Some("chapter/page-2.jpg"));
    assert_eq!(source.page_name(2), Some("chapter/page-10.jpg"));
}

#[test]
fn system_codec_formats_are_not_indexed_without_backend() {
    assert_eq!(
        classify_path(Path::new("photo.heic")),
        SourceKind::Unsupported
    );
    assert_eq!(
        classify_path(Path::new("candidate.avif")),
        SourceKind::Unsupported
    );
}

#[test]
fn folder_zip_and_cbz_report_same_pages_for_same_set() {
    let dir = temp_test_dir("source-kind-counts");
    let folder = dir.join("pages");
    fs::create_dir_all(&folder).unwrap();
    for name in ["page-10.jpg", "page-2.png", "page-001.webp"] {
        fs::write(folder.join(name), b"image-placeholder").unwrap();
    }

    let zip_path = dir.join("pages.zip");
    let cbz_path = dir.join("pages.cbz");
    write_three_page_zip(&zip_path);
    fs::copy(&zip_path, &cbz_path).unwrap();

    let folder_source = FolderSource::open(&folder).unwrap();
    let zip_source = ZipCbzSource::open(&zip_path).unwrap();
    let cbz_source = ZipCbzSource::open(&cbz_path).unwrap();

    assert_eq!(folder_source.page_count(), 3);
    assert_eq!(zip_source.page_count(), 3);
    assert_eq!(cbz_source.page_count(), 3);
    assert_eq!(folder_source.page_name(0), Some("page-001.webp"));
    assert_eq!(zip_source.page_name(0), Some("chapter/page-001.jpg"));
}

#[test]
fn opening_single_image_indexes_only_direct_siblings() {
    let dir = temp_test_dir("single-image-direct-siblings");
    fs::write(dir.join("page-002.png"), b"image-placeholder").unwrap();
    fs::write(dir.join("page-001.jpg"), b"image-placeholder").unwrap();
    let nested = dir.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("page-003.png"), b"image-placeholder").unwrap();
    fs::create_dir_all(dir.join("cover.jpg")).unwrap();

    let (source, forced_page) = open_source_from_path(&dir.join("page-002.png")).unwrap();

    assert_eq!(source.page_count(), 2);
    assert_eq!(source.page_name(0), Some("page-001.jpg"));
    assert_eq!(source.page_name(1), Some("page-002.png"));
    assert_eq!(forced_page, Some(1));
}

#[test]
fn page_display_path_uses_real_files_for_folders_and_virtual_paths_for_archives() {
    let dir = temp_test_dir("page-paths");
    let folder = dir.join("pages");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("page-001.jpg"), b"image-placeholder").unwrap();
    let archive = dir.join("book.cbz");
    write_test_zip(&archive);

    let folder_source = FolderSource::open(&folder).unwrap();
    let zip_source = ZipCbzSource::open(&archive).unwrap();

    assert_eq!(
        folder_source.page_file_path(0),
        Some(folder.join("page-001.jpg"))
    );
    assert_eq!(
        zip_source.page_display_path(0),
        Some(format!("{}::chapter/page-001.jpg", archive.display()))
    );
}

fn write_test_zip(path: &Path) {
    let file = File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for (name, bytes) in [
        ("chapter/page-10.jpg", b"ten".as_slice()),
        ("chapter/page-2.jpg", b"two".as_slice()),
        ("chapter/page-001.jpg", b"one".as_slice()),
        ("chapter/readme.txt", b"skip".as_slice()),
        ("__MACOSX/chapter/._page-3.jpg", b"skip".as_slice()),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(bytes).unwrap();
    }

    zip.finish().unwrap();
}

fn write_three_page_zip(path: &Path) {
    let file = File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for name in [
        "chapter/page-10.jpg",
        "chapter/page-2.png",
        "chapter/page-001.jpg",
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(b"image-placeholder").unwrap();
    }

    zip.finish().unwrap();
}

fn temp_test_dir(name: &str) -> std::path::PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("suisuiview-{name}-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}
