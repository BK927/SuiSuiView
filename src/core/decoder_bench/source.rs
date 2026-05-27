use crate::core::natural::cmp_natural;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use zip::ZipArchive;

const MAX_DECODER_BENCH_PAGE_BYTES: u64 = 256 * 1024 * 1024;
const DECODER_BENCH_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "apng", "webp", "gif", "bmp", "dib", "ico", "avif", "svg",
];

pub(super) enum DecoderBenchSource {
    Zip {
        title: String,
        book_id: String,
        pages: Vec<DecoderBenchZipPage>,
        archive: Mutex<ZipArchive<File>>,
    },
    Files {
        title: String,
        book_id: String,
        pages: Vec<DecoderBenchFilePage>,
    },
}

pub(super) struct DecoderBenchFilePage {
    name: String,
    path: PathBuf,
    byte_size: u64,
}

pub(super) struct DecoderBenchZipPage {
    name: String,
    zip_index: usize,
    crc32: u32,
    uncompressed_size: u64,
    compressed_size: u64,
}

impl DecoderBenchSource {
    pub(super) fn open(path: &Path) -> Result<Self, String> {
        if path.is_dir() {
            return Self::open_folder(path);
        }

        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        if extension.eq_ignore_ascii_case("zip") || extension.eq_ignore_ascii_case("cbz") {
            return Self::open_zip(path);
        }

        if !is_decoder_bench_file_name(path) {
            return Err(format!(
                "Unsupported decoder-bench input file: {}",
                path.display()
            ));
        }

        let byte_size = fs::metadata(path).map_err(|error| error.to_string())?.len();
        let title = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Image")
            .to_owned();
        let page = DecoderBenchFilePage {
            name: title.clone(),
            path: path.to_path_buf(),
            byte_size,
        };
        let book_id = file_list_book_id(std::slice::from_ref(&page));
        Ok(Self::Files {
            title,
            book_id,
            pages: vec![page],
        })
    }

    fn open_folder(root: &Path) -> Result<Self, String> {
        let mut pages = Vec::new();
        collect_decoder_bench_files(root, root, &mut pages)?;
        pages.sort_by(|left, right| cmp_natural(&left.name, &right.name));
        if pages.is_empty() {
            return Err(format!(
                "No decoder-bench image candidates found in {}",
                root.display()
            ));
        }
        let title = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Folder")
            .to_owned();
        let book_id = file_list_book_id(&pages);
        Ok(Self::Files {
            title,
            book_id,
            pages,
        })
    }

    fn open_zip(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|error| error.to_string())?;
        let mut archive = ZipArchive::new(file).map_err(|error| error.to_string())?;
        let mut pages = Vec::new();

        for index in 0..archive.len() {
            let file = archive.by_index(index).map_err(|error| error.to_string())?;
            if file.is_dir() {
                continue;
            }
            let Some(name) = normalize_zip_name(file.name()) else {
                continue;
            };
            if !is_decoder_bench_file_name(Path::new(&name)) {
                continue;
            }
            pages.push(DecoderBenchZipPage {
                name,
                zip_index: index,
                crc32: file.crc32(),
                uncompressed_size: file.size(),
                compressed_size: file.compressed_size(),
            });
        }

        pages.sort_by(|left, right| cmp_natural(&left.name, &right.name));
        if pages.is_empty() {
            return Err(format!(
                "No decoder-bench image candidates found in {}",
                path.display()
            ));
        }

        let title = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Archive")
            .to_owned();
        let book_id = zip_list_book_id(&pages);

        Ok(Self::Zip {
            title,
            book_id,
            pages,
            archive: Mutex::new(archive),
        })
    }

    pub(super) fn title(&self) -> &str {
        match self {
            Self::Zip { title, .. } => title,
            Self::Files { title, .. } => title,
        }
    }

    pub(super) fn book_id(&self) -> &str {
        match self {
            Self::Zip { book_id, .. } => book_id,
            Self::Files { book_id, .. } => book_id,
        }
    }

    pub(super) fn page_count(&self) -> usize {
        match self {
            Self::Zip { pages, .. } => pages.len(),
            Self::Files { pages, .. } => pages.len(),
        }
    }

    pub(super) fn page_name(&self, index: usize) -> Option<&str> {
        match self {
            Self::Zip { pages, .. } => pages.get(index).map(|page| page.name.as_str()),
            Self::Files { pages, .. } => pages.get(index).map(|page| page.name.as_str()),
        }
    }

    pub(super) fn read_page(&self, index: usize) -> Result<Vec<u8>, String> {
        match self {
            Self::Zip { pages, archive, .. } => {
                let page = pages.get(index).ok_or_else(|| {
                    format!("Invalid page {index}; source has {} pages", pages.len())
                })?;
                if page.uncompressed_size > MAX_DECODER_BENCH_PAGE_BYTES {
                    return Err(format!(
                        "Page {} is too large to read safely: {:.1} MB",
                        index + 1,
                        page.uncompressed_size as f32 / (1024.0 * 1024.0)
                    ));
                }

                let mut archive = archive
                    .lock()
                    .map_err(|_| "ZIP archive reader is unavailable".to_owned())?;
                let mut zip_file = archive
                    .by_index(page.zip_index)
                    .map_err(|error| error.to_string())?;
                let mut bytes = Vec::with_capacity(
                    page.uncompressed_size.min(MAX_DECODER_BENCH_PAGE_BYTES) as usize,
                );
                let mut limited = zip_file.by_ref().take(MAX_DECODER_BENCH_PAGE_BYTES + 1);
                limited
                    .read_to_end(&mut bytes)
                    .map_err(|error| error.to_string())?;
                if bytes.len() as u64 > MAX_DECODER_BENCH_PAGE_BYTES {
                    return Err(format!(
                        "Page {} exceeded the safe read limit: {:.1} MB",
                        index + 1,
                        MAX_DECODER_BENCH_PAGE_BYTES as f32 / (1024.0 * 1024.0)
                    ));
                }
                Ok(bytes)
            }
            Self::Files { pages, .. } => {
                let page = pages.get(index).ok_or_else(|| {
                    format!("Invalid page {index}; source has {} pages", pages.len())
                })?;
                if page.byte_size > MAX_DECODER_BENCH_PAGE_BYTES {
                    return Err(format!(
                        "Page {} is too large to read safely: {:.1} MB",
                        index + 1,
                        page.byte_size as f32 / (1024.0 * 1024.0)
                    ));
                }

                let file = File::open(&page.path).map_err(|error| error.to_string())?;
                let mut bytes =
                    Vec::with_capacity(page.byte_size.min(MAX_DECODER_BENCH_PAGE_BYTES) as usize);
                let mut limited = file.take(MAX_DECODER_BENCH_PAGE_BYTES + 1);
                limited
                    .read_to_end(&mut bytes)
                    .map_err(|error| error.to_string())?;
                if bytes.len() as u64 > MAX_DECODER_BENCH_PAGE_BYTES {
                    return Err(format!(
                        "Page {} exceeded the safe read limit: {:.1} MB",
                        index + 1,
                        MAX_DECODER_BENCH_PAGE_BYTES as f32 / (1024.0 * 1024.0)
                    ));
                }
                Ok(bytes)
            }
        }
    }
}

fn collect_decoder_bench_files(
    root: &Path,
    current: &Path,
    pages: &mut Vec<DecoderBenchFilePage>,
) -> Result<(), String> {
    for entry in fs::read_dir(current).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();

        if should_skip_component(&file_name) {
            continue;
        }

        if path.is_dir() {
            collect_decoder_bench_files(root, &path, pages)?;
            continue;
        }

        if !is_decoder_bench_file_name(&path) {
            continue;
        }

        let relative = path.strip_prefix(root).unwrap_or(&path);
        pages.push(DecoderBenchFilePage {
            name: normalize_path_text(relative),
            byte_size: entry.metadata().map(|metadata| metadata.len()).unwrap_or(0),
            path,
        });
    }

    Ok(())
}

fn is_decoder_bench_file_name(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            DECODER_BENCH_EXTENSIONS
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(extension))
        })
}

fn should_skip_component(component: &str) -> bool {
    component == "__MACOSX" || component.starts_with('.') || component.starts_with("._")
}

fn normalize_path_text(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize_zip_name(name: &str) -> Option<String> {
    let normalized = name.replace('\\', "/").trim_start_matches('/').to_owned();
    if normalized.is_empty() || normalized.ends_with('/') {
        return None;
    }

    for component in normalized.split('/') {
        if component.is_empty() || should_skip_component(component) {
            return None;
        }
    }

    Some(normalized)
}

fn file_list_book_id(pages: &[DecoderBenchFilePage]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"suisuiview:decoder-bench-files-v1\0");
    hasher.update(&(pages.len() as u64).to_le_bytes());
    for page in pages {
        hasher.update(page.name.as_bytes());
        hasher.update(&[0]);
        hasher.update(&page.byte_size.to_le_bytes());
        hasher.update(&[0]);
    }
    format!("decoder-bench-files:{}", hasher.finalize().to_hex())
}

fn zip_list_book_id(pages: &[DecoderBenchZipPage]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"suisuiview:decoder-bench-zip-v1\0");
    hasher.update(&(pages.len() as u64).to_le_bytes());
    for page in pages {
        hasher.update(page.name.as_bytes());
        hasher.update(&[0]);
        hasher.update(&page.crc32.to_le_bytes());
        hasher.update(&page.uncompressed_size.to_le_bytes());
        hasher.update(&page.compressed_size.to_le_bytes());
        hasher.update(&[0]);
    }
    format!("decoder-bench-zip:{}", hasher.finalize().to_hex())
}
