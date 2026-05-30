use crate::core::formats::{
    descriptor_for_extension, is_image_page_name, unsupported_message_for_extension, FormatPolicy,
};
use crate::core::natural::cmp_natural;
use blake3::Hasher;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use zip::ZipArchive;

pub type SharedSource = Arc<dyn BookSource>;
const MAX_SOURCE_PAGE_BYTES: u64 = 256 * 1024 * 1024;

pub trait BookSource: Send + Sync {
    fn title(&self) -> &str;
    fn source_path(&self) -> &Path;
    fn book_id(&self) -> &str;
    fn page_count(&self) -> usize;
    fn page_name(&self, index: usize) -> Option<&str>;
    fn page_file_path(&self, _index: usize) -> Option<PathBuf> {
        None
    }
    fn page_byte_size(&self, _index: usize) -> Option<u64> {
        None
    }
    fn page_display_path(&self, index: usize) -> Option<String> {
        if let Some(path) = self.page_file_path(index) {
            return Some(path.display().to_string());
        }
        self.page_name(index)
            .map(|name| format!("{}::{name}", self.source_path().display()))
    }
    fn read_page(&self, index: usize) -> Result<Vec<u8>, SourceError>;
}

#[derive(Debug)]
pub enum SourceError {
    Io(io::Error),
    Zip(zip::result::ZipError),
    Unsupported(String),
    NoPages(PathBuf),
    InvalidPage { index: usize, page_count: usize },
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Zip(error) => write!(f, "ZIP error: {error}"),
            Self::Unsupported(message) => write!(f, "{message}"),
            Self::NoPages(path) => {
                write!(f, "No supported image pages found in {}", path.display())
            }
            Self::InvalidPage { index, page_count } => {
                write!(f, "Invalid page {index}; source has {page_count} pages")
            }
        }
    }
}

impl std::error::Error for SourceError {}

impl From<io::Error> for SourceError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<zip::result::ZipError> for SourceError {
    fn from(value: zip::result::ZipError) -> Self {
        Self::Zip(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Folder,
    ZipCbz,
    SingleImage,
    UnsupportedRar,
    Unsupported,
}

pub fn classify_path(path: &Path) -> SourceKind {
    if path.is_dir() {
        return SourceKind::Folder;
    }

    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();

    if extension.eq_ignore_ascii_case("zip") || extension.eq_ignore_ascii_case("cbz") {
        return SourceKind::ZipCbz;
    }

    match descriptor_for_extension(extension) {
        Some(descriptor) if descriptor.is_image_page() => SourceKind::SingleImage,
        Some(descriptor) if descriptor.policy == FormatPolicy::RestrictedReadOnly => {
            SourceKind::UnsupportedRar
        }
        Some(_) | None => SourceKind::Unsupported,
    }
}

pub fn open_source_from_path(path: &Path) -> Result<(SharedSource, Option<usize>), SourceError> {
    match classify_path(path) {
        SourceKind::Folder => {
            FolderSource::open(path).map(|source| (Arc::new(source) as SharedSource, None))
        }
        SourceKind::ZipCbz => {
            ZipCbzSource::open(path).map(|source| (Arc::new(source) as SharedSource, None))
        }
        SourceKind::SingleImage => {
            let Some(parent) = path.parent() else {
                return Err(SourceError::Unsupported(
                    "Image file has no parent folder".to_owned(),
                ));
            };
            let source = FolderSource::open_direct(parent)?;
            let page = source.page_index_for_path(path);
            Ok((Arc::new(source), page))
        }
        SourceKind::UnsupportedRar => {
            let extension = path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default();
            Err(SourceError::Unsupported(
                unsupported_message_for_extension(extension).unwrap_or_else(|| {
                    "CBR/RAR support needs a restricted read-only backend.".to_owned()
                }),
            ))
        }
        SourceKind::Unsupported => {
            let extension = path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default();
            Err(SourceError::Unsupported(
                unsupported_message_for_extension(extension)
                    .unwrap_or_else(|| format!("Unsupported file type: {}", path.display())),
            ))
        }
    }
}

pub fn is_supported_image_name(name: &str) -> bool {
    is_image_page_name(name)
}

pub struct FolderSource {
    root: PathBuf,
    title: String,
    book_id: String,
    pages: Vec<FolderPage>,
}

struct FolderPage {
    relative_name: String,
    path: PathBuf,
    byte_size: u64,
}

impl FolderSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SourceError> {
        let root = path.as_ref().to_path_buf();
        let mut pages = Vec::new();
        collect_folder_pages(&root, &root, &mut pages)?;
        Self::from_pages(root, pages)
    }

    pub fn open_direct(path: impl AsRef<Path>) -> Result<Self, SourceError> {
        let root = path.as_ref().to_path_buf();
        let mut pages = Vec::new();
        collect_direct_folder_pages(&root, &mut pages)?;
        Self::from_pages(root, pages)
    }

    fn from_pages(root: PathBuf, mut pages: Vec<FolderPage>) -> Result<Self, SourceError> {
        pages.sort_by(|a, b| cmp_natural(&a.relative_name, &b.relative_name));
        if pages.is_empty() {
            return Err(SourceError::NoPages(root));
        }

        let title = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Folder")
            .to_owned();
        let book_id = folder_book_id(&pages);

        Ok(Self {
            root,
            title,
            book_id,
            pages,
        })
    }

    pub fn page_index_for_path(&self, path: &Path) -> Option<usize> {
        self.pages
            .iter()
            .position(|page| page.path == path)
            .or_else(|| {
                let wanted = path.file_name()?.to_string_lossy();
                self.pages.iter().position(|page| {
                    Path::new(&page.relative_name)
                        .file_name()
                        .map(|name| name.to_string_lossy() == wanted)
                        .unwrap_or(false)
                })
            })
    }
}

impl BookSource for FolderSource {
    fn title(&self) -> &str {
        &self.title
    }

    fn source_path(&self) -> &Path {
        &self.root
    }

    fn book_id(&self) -> &str {
        &self.book_id
    }

    fn page_count(&self) -> usize {
        self.pages.len()
    }

    fn page_name(&self, index: usize) -> Option<&str> {
        self.pages
            .get(index)
            .map(|page| page.relative_name.as_str())
    }

    fn page_file_path(&self, index: usize) -> Option<PathBuf> {
        self.pages.get(index).map(|page| page.path.clone())
    }

    fn page_byte_size(&self, index: usize) -> Option<u64> {
        self.pages.get(index).map(|page| page.byte_size)
    }

    fn read_page(&self, index: usize) -> Result<Vec<u8>, SourceError> {
        let page = self.pages.get(index).ok_or(SourceError::InvalidPage {
            index,
            page_count: self.pages.len(),
        })?;
        if page.byte_size > MAX_SOURCE_PAGE_BYTES {
            return Err(SourceError::Unsupported(format!(
                "Page {} is too large to read safely: {:.1} MB",
                index + 1,
                page.byte_size as f32 / (1024.0 * 1024.0)
            )));
        }
        fs::read(&page.path).map_err(SourceError::Io)
    }
}

pub struct ZipCbzSource {
    path: PathBuf,
    title: String,
    book_id: String,
    pages: Vec<ZipPage>,
    archive: Mutex<ZipArchive<File>>,
}

#[derive(Debug, Clone)]
struct ZipPage {
    name: String,
    zip_index: usize,
    crc32: u32,
    uncompressed_size: u64,
    compressed_size: u64,
}

impl ZipCbzSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SourceError> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path)?;
        let mut archive = ZipArchive::new(file)?;
        let mut pages = Vec::new();

        for index in 0..archive.len() {
            let file = archive.by_index(index)?;
            if file.is_dir() {
                continue;
            }
            let Some(name) = normalize_zip_name(file.name()) else {
                continue;
            };
            if !is_supported_image_name(&name) {
                continue;
            }

            pages.push(ZipPage {
                name,
                zip_index: index,
                crc32: file.crc32(),
                uncompressed_size: file.size(),
                compressed_size: file.compressed_size(),
            });
        }

        pages.sort_by(|a, b| cmp_natural(&a.name, &b.name));

        if pages.is_empty() {
            return Err(SourceError::NoPages(path));
        }

        let title = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Archive")
            .to_owned();
        let book_id = zip_book_id(&pages);

        Ok(Self {
            path,
            title,
            book_id,
            pages,
            archive: Mutex::new(archive),
        })
    }
}

impl BookSource for ZipCbzSource {
    fn title(&self) -> &str {
        &self.title
    }

    fn source_path(&self) -> &Path {
        &self.path
    }

    fn book_id(&self) -> &str {
        &self.book_id
    }

    fn page_count(&self) -> usize {
        self.pages.len()
    }

    fn page_name(&self, index: usize) -> Option<&str> {
        self.pages.get(index).map(|page| page.name.as_str())
    }

    fn page_byte_size(&self, index: usize) -> Option<u64> {
        self.pages.get(index).map(|page| page.uncompressed_size)
    }

    fn read_page(&self, index: usize) -> Result<Vec<u8>, SourceError> {
        let page = self.pages.get(index).ok_or(SourceError::InvalidPage {
            index,
            page_count: self.pages.len(),
        })?;

        if page.uncompressed_size > MAX_SOURCE_PAGE_BYTES {
            return Err(SourceError::Unsupported(format!(
                "Page {} is too large to read safely: {:.1} MB",
                index + 1,
                page.uncompressed_size as f32 / (1024.0 * 1024.0)
            )));
        }

        let mut archive = self.archive.lock().map_err(|_| {
            SourceError::Unsupported("ZIP archive reader is unavailable".to_owned())
        })?;
        let mut zip_file = archive.by_index(page.zip_index)?;
        let mut bytes = Vec::with_capacity(page.uncompressed_size.min(128 * 1024 * 1024) as usize);
        let mut limited = zip_file.by_ref().take(MAX_SOURCE_PAGE_BYTES + 1);
        limited.read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_SOURCE_PAGE_BYTES {
            return Err(SourceError::Unsupported(format!(
                "Page {} exceeded the safe read limit: {:.1} MB",
                index + 1,
                MAX_SOURCE_PAGE_BYTES as f32 / (1024.0 * 1024.0)
            )));
        }
        Ok(bytes)
    }
}

fn collect_folder_pages(
    root: &Path,
    current: &Path,
    pages: &mut Vec<FolderPage>,
) -> Result<(), SourceError> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();

        if should_skip_component(&file_name) {
            continue;
        }

        if path.is_dir() {
            collect_folder_pages(root, &path, pages)?;
        } else if is_supported_image_name(&file_name) {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let relative_name = normalize_path_text(relative);
            let byte_size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            pages.push(FolderPage {
                relative_name,
                path,
                byte_size,
            });
        }
    }

    Ok(())
}

fn collect_direct_folder_pages(
    root: &Path,
    pages: &mut Vec<FolderPage>,
) -> Result<(), SourceError> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();

        if path.is_dir()
            || should_skip_component(&file_name)
            || !is_supported_image_name(&file_name)
        {
            continue;
        }

        let byte_size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        pages.push(FolderPage {
            relative_name: file_name.into_owned(),
            path,
            byte_size,
        });
    }

    Ok(())
}

fn folder_book_id(pages: &[FolderPage]) -> String {
    let mut hasher = Hasher::new();
    hasher.update(b"suisuiview:folder-v1\0");
    hasher.update(&(pages.len() as u64).to_le_bytes());
    for page in pages {
        hasher.update(page.relative_name.as_bytes());
        hasher.update(&[0]);
        hasher.update(&page.byte_size.to_le_bytes());
        hasher.update(&[0]);
    }
    format!("folder:{}", hasher.finalize().to_hex())
}

fn zip_book_id(pages: &[ZipPage]) -> String {
    let mut hasher = Hasher::new();
    hasher.update(b"suisuiview:zip-v1\0");
    hasher.update(&(pages.len() as u64).to_le_bytes());
    for page in pages {
        hasher.update(page.name.as_bytes());
        hasher.update(&[0]);
        hasher.update(&page.crc32.to_le_bytes());
        hasher.update(&page.uncompressed_size.to_le_bytes());
        hasher.update(&page.compressed_size.to_le_bytes());
        hasher.update(&[0]);
    }
    format!("zip:{}", hasher.finalize().to_hex())
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

fn normalize_path_text(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn should_skip_component(component: &str) -> bool {
    component == "__MACOSX" || component.starts_with('.') || component.starts_with("._")
}

#[cfg(test)]
mod tests;
