#[cfg(not(test))]
use directories::ProjectDirs;
use eframe::egui::ColorImage;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const THUMBNAIL_DISK_CACHE_LIMIT_BYTES: u64 = 128 * 1024 * 1024;

pub(super) struct BookmarkThumbnailDiskEntry {
    key: String,
    path: PathBuf,
}

impl BookmarkThumbnailDiskEntry {
    pub(super) fn new(key: String) -> Self {
        let path = bookmark_thumbnail_cache_dir()
            .join(&key[..2])
            .join(format!("{key}.png"));
        Self { key, path }
    }

    #[cfg(test)]
    pub(super) fn path_for_test(&self) -> &Path {
        &self.path
    }
}

pub(super) fn read_cached_thumbnail(
    entry: &BookmarkThumbnailDiskEntry,
) -> Result<Option<ColorImage>, String> {
    match fs::read(&entry.path) {
        Ok(bytes) => decode_thumbnail_png(&bytes).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("북마크 썸네일 캐시를 읽을 수 없습니다: {error}")),
    }
}

pub(super) fn store_cached_thumbnail(
    entry: &BookmarkThumbnailDiskEntry,
    image: &ColorImage,
    prune_after_store: bool,
) -> Result<(), String> {
    let parent = entry
        .path
        .parent()
        .ok_or_else(|| "북마크 썸네일 캐시 경로가 올바르지 않습니다.".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("북마크 썸네일 캐시 폴더를 만들 수 없습니다: {error}"))?;
    let bytes = encode_thumbnail_png(image)?;
    let temp_path = parent.join(format!("{}.{}.tmp", entry.key, std::process::id()));
    fs::write(&temp_path, bytes)
        .map_err(|error| format!("북마크 썸네일 캐시를 쓸 수 없습니다: {error}"))?;
    if entry.path.exists() {
        let _ = fs::remove_file(&entry.path);
    }
    fs::rename(&temp_path, &entry.path)
        .map_err(|error| format!("북마크 썸네일 캐시를 저장할 수 없습니다: {error}"))?;
    if prune_after_store {
        prune_bookmark_thumbnail_cache()
    } else {
        Ok(())
    }
}

fn encode_thumbnail_png(image: &ColorImage) -> Result<Vec<u8>, String> {
    let [width, height] = image.size;
    let mut rgba = Vec::with_capacity(image.pixels.len() * 4);
    for pixel in &image.pixels {
        rgba.extend_from_slice(&[pixel.r(), pixel.g(), pixel.b(), pixel.a()]);
    }
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| format!("PNG 헤더를 쓸 수 없습니다: {error}"))?;
        writer
            .write_image_data(&rgba)
            .map_err(|error| format!("PNG 데이터를 쓸 수 없습니다: {error}"))?;
        writer
            .finish()
            .map_err(|error| format!("PNG 저장을 마무리할 수 없습니다: {error}"))?;
    }
    Ok(output)
}

fn decode_thumbnail_png(bytes: &[u8]) -> Result<ColorImage, String> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|error| format!("PNG 썸네일을 읽을 수 없습니다: {error}"))?
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, &image.into_raw()))
}

#[cfg(not(test))]
fn bookmark_thumbnail_cache_dir() -> PathBuf {
    ProjectDirs::from("", "", "SuiSuiView")
        .map(|dirs| dirs.cache_dir().join("bookmark-thumbnails"))
        .unwrap_or_else(|| PathBuf::from("SuiSuiView-bookmark-thumbnails-cache"))
}

#[cfg(test)]
fn bookmark_thumbnail_cache_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "suisuiview-bookmark-thumbnails-test-{}",
        std::process::id()
    ))
}

fn prune_bookmark_thumbnail_cache() -> Result<(), String> {
    let cache_dir = bookmark_thumbnail_cache_dir();
    let mut files = Vec::new();
    collect_thumbnail_cache_files(&cache_dir, &mut files)?;
    let mut total_bytes = files.iter().map(|file| file.bytes).sum::<u64>();
    if total_bytes <= THUMBNAIL_DISK_CACHE_LIMIT_BYTES {
        return Ok(());
    }

    files.sort_by_key(|file| file.modified);
    for file in files {
        if total_bytes <= THUMBNAIL_DISK_CACHE_LIMIT_BYTES {
            break;
        }
        if fs::remove_file(&file.path).is_ok() {
            total_bytes = total_bytes.saturating_sub(file.bytes);
        }
    }
    Ok(())
}

struct CacheFile {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

fn collect_thumbnail_cache_files(dir: &Path, files: &mut Vec<CacheFile>) -> Result<(), String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("북마크 썸네일 캐시를 정리할 수 없습니다: {error}")),
    };

    for entry in entries {
        let entry = entry
            .map_err(|error| format!("북마크 썸네일 캐시 항목을 읽을 수 없습니다: {error}"))?;
        let path = entry.path();
        let metadata = entry.metadata().map_err(|error| {
            format!("북마크 썸네일 캐시 메타데이터를 읽을 수 없습니다: {error}")
        })?;
        if metadata.is_dir() {
            collect_thumbnail_cache_files(&path, files)?;
        } else if metadata.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        {
            files.push(CacheFile {
                path,
                bytes: metadata.len(),
                modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::BookmarkThumbnailDiskEntry;
    use super::{decode_thumbnail_png, encode_thumbnail_png, read_cached_thumbnail};
    use eframe::egui::{Color32, ColorImage};
    use std::fs;

    #[test]
    fn thumbnail_png_round_trips() {
        let source = ColorImage::new(
            [2, 2],
            vec![
                Color32::RED,
                Color32::GREEN,
                Color32::BLUE,
                Color32::from_rgba_unmultiplied(1, 2, 3, 4),
            ],
        );

        let bytes = encode_thumbnail_png(&source).unwrap();
        let decoded = decode_thumbnail_png(&bytes).unwrap();

        assert_eq!(decoded.size, source.size);
        assert_eq!(decoded.pixels, source.pixels);
    }

    #[test]
    fn cached_thumbnail_read_returns_image_without_decode_job() {
        let dir = std::env::temp_dir().join(format!(
            "suisuiview-bookmark-thumbnail-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let entry = BookmarkThumbnailDiskEntry {
            key: "test".to_owned(),
            path: dir.join("thumb.png"),
        };
        let source = ColorImage::new([1, 1], vec![Color32::WHITE]);
        fs::write(&entry.path, encode_thumbnail_png(&source).unwrap()).unwrap();

        let cached = read_cached_thumbnail(&entry).unwrap().unwrap();

        assert_eq!(cached.size, [1, 1]);
        assert_eq!(cached.pixels, vec![Color32::WHITE]);
        let _ = fs::remove_dir_all(dir);
    }
}
