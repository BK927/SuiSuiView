use super::{
    classify_path, open_source_from_path, BookSource, FolderSource, PageReadCompression,
    PageReadSourceKind, SourceError, SourceKind, ZipCbzSource,
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
fn folder_book_id_uses_bounded_content_identity_not_only_name_and_size() {
    let first = temp_test_dir("folder-content-id-first");
    let second = temp_test_dir("folder-content-id-second");
    fs::write(first.join("page.jpg"), b"same-length-A").unwrap();
    fs::write(second.join("page.jpg"), b"same-length-B").unwrap();

    let first_source = FolderSource::open(&first).unwrap();
    let second_source = FolderSource::open(&second).unwrap();

    assert_ne!(first_source.book_id(), second_source.book_id());
}

#[test]
fn folder_book_id_survives_copying_the_same_contents() {
    let first = temp_test_dir("folder-copy-id-first");
    let second = temp_test_dir("folder-copy-id-second");
    fs::write(first.join("page.jpg"), b"synthetic image contents").unwrap();
    fs::copy(first.join("page.jpg"), second.join("page.jpg")).unwrap();

    let first_source = FolderSource::open(&first).unwrap();
    let second_source = FolderSource::open(&second).unwrap();

    assert_eq!(first_source.book_id(), second_source.book_id());
}

#[test]
fn folder_book_id_samples_the_middle_of_large_same_size_pages() {
    let first = temp_test_dir("folder-large-id-first");
    let second = temp_test_dir("folder-large-id-second");
    let mut first_bytes = vec![b'x'; 20 * 1024];
    let mut second_bytes = first_bytes.clone();
    first_bytes[10 * 1024] = b'A';
    second_bytes[10 * 1024] = b'B';
    fs::write(first.join("page.jpg"), first_bytes).unwrap();
    fs::write(second.join("page.jpg"), second_bytes).unwrap();

    let first_source = FolderSource::open(&first).unwrap();
    let second_source = FolderSource::open(&second).unwrap();

    assert_ne!(first_source.book_id(), second_source.book_id());
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
fn psd_is_indexed_and_ai_depends_on_native_feature() {
    assert_eq!(
        classify_path(Path::new("preview.psd")),
        SourceKind::SingleImage
    );
    assert_eq!(
        classify_path(Path::new("document.pdf")),
        SourceKind::Unsupported
    );
    assert_eq!(
        classify_path(Path::new("art.ai")),
        if cfg!(feature = "native-ai") {
            SourceKind::SingleImage
        } else {
            SourceKind::Unsupported
        }
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
fn opening_missing_single_image_does_not_fall_back_to_a_sibling() {
    let dir = temp_test_dir("missing-single-image");
    fs::write(dir.join("page-001.jpg"), b"image-placeholder").unwrap();
    let missing = dir.join("page-002.jpg");

    let result = open_source_from_path(&missing);

    assert!(matches!(
        result,
        Err(SourceError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound
    ));
}

#[test]
fn opening_an_image_excluded_from_the_folder_index_returns_an_error() {
    let dir = temp_test_dir("unindexed-single-image");
    fs::write(dir.join("page-001.jpg"), b"image-placeholder").unwrap();
    let excluded = dir.join(".hidden-page.jpg");
    fs::write(&excluded, b"image-placeholder").unwrap();

    let result = open_source_from_path(&excluded);

    assert!(matches!(
        result,
        Err(SourceError::Unsupported(message))
            if message.contains("not an indexed page") && message.contains(".hidden-page.jpg")
    ));
}

#[test]
fn page_path_lookup_does_not_match_the_same_name_in_another_folder() {
    let first = temp_test_dir("same-name-first");
    let second = temp_test_dir("same-name-second");
    fs::write(first.join("page.jpg"), b"first").unwrap();
    fs::write(second.join("page.jpg"), b"second").unwrap();
    let source = FolderSource::open_direct(&first).unwrap();

    assert_eq!(source.page_index_for_path(&second.join("page.jpg")), None);
}

#[cfg(windows)]
#[test]
fn opening_single_image_accepts_windows_path_case_differences() {
    let dir = temp_test_dir("single-image-path-case");
    fs::write(dir.join("Page-001.PNG"), b"image-placeholder").unwrap();

    let (source, forced_page) = open_source_from_path(&dir.join("page-001.png")).unwrap();

    assert_eq!(source.page_count(), 1);
    assert_eq!(source.page_name(0), Some("Page-001.PNG"));
    assert_eq!(forced_page, Some(0));
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

#[test]
fn folder_refresh_keeps_its_cache_session_but_a_new_open_does_not() {
    let dir = temp_test_dir("folder-cache-session");
    fs::write(dir.join("page-001.jpg"), b"image-placeholder").unwrap();

    let source = FolderSource::open(&dir).unwrap();
    let initial_instance = source.source_instance_id();
    let initial_cache = source.source_cache_id();
    let refreshed = source.refresh_snapshot().unwrap().unwrap();
    let reopened = FolderSource::open(&dir).unwrap();

    assert_ne!(refreshed.source_instance_id(), initial_instance);
    assert_eq!(refreshed.source_cache_id(), initial_cache);
    assert_ne!(reopened.source_cache_id(), initial_cache);
}

#[test]
fn zip_page_read_hint_reports_compression_and_sizes() {
    let dir = temp_test_dir("zip-read-hint");
    let archive = dir.join("book.cbz");
    let file = File::create(&archive).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("stored/page-001.jpg", stored).unwrap();
    zip.write_all(b"stored").unwrap();
    zip.start_file("deflated/page-002.jpg", deflated).unwrap();
    zip.write_all(b"deflated deflated deflated").unwrap();
    zip.finish().unwrap();

    let source = ZipCbzSource::open(&archive).unwrap();
    let stored_hint = source.page_read_hint(1).unwrap();
    let deflated_hint = source.page_read_hint(0).unwrap();

    assert_eq!(stored_hint.source_kind, PageReadSourceKind::ZipCbz);
    assert_eq!(stored_hint.compression_method, PageReadCompression::Stored);
    assert_eq!(stored_hint.compression_state(), "stored");
    assert_eq!(stored_hint.uncompressed_size, Some(6));
    assert_eq!(stored_hint.compressed_size, Some(6));

    assert_eq!(deflated_hint.source_kind, PageReadSourceKind::ZipCbz);
    assert_eq!(
        deflated_hint.compression_method,
        PageReadCompression::Deflated
    );
    assert_eq!(deflated_hint.compression_state(), "compressed");
    assert_eq!(deflated_hint.uncompressed_size, Some(26));
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
