fn write_test_png(path: &Path, width: u32, height: u32) {
    use image::{codecs::png::PngEncoder, ColorType, ImageEncoder};
    let pixels = vec![0u8; width as usize * height as usize * 3];
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(&pixels, width, height, ColorType::Rgb8.into())
        .expect("encode test png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn write_test_zip(path: &Path, page_names: &[&str]) {
    use image::{codecs::png::PngEncoder, ColorType, ImageEncoder};
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    for (index, name) in page_names.iter().enumerate() {
        let width = 24 + index as u32 * 4;
        let pixels = vec![0u8; width as usize * 32 * 3];
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(&pixels, width, 32, ColorType::Rgb8.into())
            .expect("encode test png");
        zip.start_file(*name, options).expect("zip start_file");
        zip.write_all(&bytes).expect("zip write");
    }
    zip.finish().expect("zip finish");
}

/// A books directory no other test can be looking at. `cargo test` runs the lib
/// and bin targets as separate processes at the same time, and both compile
/// `core`, so the same test name runs twice concurrently. The Windows system
/// clock advances in ~15 ms steps, so a timestamp alone can collide — and tests
/// that scan the whole directory (`load_all_book_records`) then see each other's
/// records. Process id plus a per-process counter makes the name unique.
fn unique_base(name: &str) -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir()
        .join("suisuiview-tests")
        .join(format!("{name}-{stamp}-{}-{seq}", std::process::id()))
}

fn store_at(base: &Path) -> StateStore {
    StateStore {
        path: base.join("state.json"),
        books_dir: base.join("books"),
        state: PersistedState::default(),
        pending_books: Default::default(),
        state_dirty: false,
        books: Default::default(),
    }
}

fn test_store(name: &str) -> StateStore {
    store_at(&unique_base(name))
}
