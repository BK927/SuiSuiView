use crate::core::state::NcnnRealEsrganSettings;
use directories::ProjectDirs;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use super::{normalized_output_format, normalized_tile_size};

const CACHE_VERSION: &str = "suisuiview:ncnn-realesrgan-cache-v1";
const MAX_CACHE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(super) struct AiUpscaleCacheEntry {
    key: String,
    extension: String,
    pub(super) path: PathBuf,
}

impl AiUpscaleCacheEntry {
    pub(super) fn new(source_hash: &str, settings: &NcnnRealEsrganSettings) -> Self {
        let extension = normalized_output_format(&settings.output_format);
        let key = ncnn_realesrgan_cache_key(source_hash, settings);
        let path = ai_upscale_cache_dir()
            .join(&key[..2])
            .join(format!("{key}.{extension}"));
        Self {
            key,
            extension,
            path,
        }
    }

    pub(super) fn extension_label(&self) -> &'static str {
        match self.extension.as_str() {
            "jpg" => "jpg",
            "webp" => "webp",
            _ => "png",
        }
    }
}

pub(super) fn ncnn_realesrgan_cache_key(
    source_hash: &str,
    settings: &NcnnRealEsrganSettings,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CACHE_VERSION.as_bytes());
    hasher.update(&[0]);
    hasher.update(source_hash.as_bytes());
    hasher.update(&[0]);
    hasher.update(settings.model_name.trim().as_bytes());
    hasher.update(&[0]);
    hasher.update(settings.executable_path.trim().as_bytes());
    hasher.update(&[0]);
    hasher.update(settings.model_path.trim().as_bytes());
    hasher.update(&[0]);
    hasher.update(&settings.scale.clamp(2, 4).to_le_bytes());
    hasher.update(&normalized_tile_size(settings.tile_size).to_le_bytes());
    hasher.update(normalized_output_format(&settings.output_format).as_bytes());
    hasher.finalize().to_hex().to_string()
}

pub(super) fn read_cached_output(entry: &AiUpscaleCacheEntry) -> Result<Option<Vec<u8>>, String> {
    match fs::read(&entry.path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "AI 업스케일 캐시를 읽을 수 없습니다 ({}): {error}",
            entry.path.display()
        )),
    }
}

pub(super) fn store_cached_output(entry: &AiUpscaleCacheEntry, bytes: &[u8]) -> Result<(), String> {
    let parent = entry
        .path
        .parent()
        .ok_or_else(|| "AI 업스케일 캐시 경로가 올바르지 않습니다.".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("AI 업스케일 캐시 폴더를 만들 수 없습니다: {error}"))?;
    let temp_path = parent.join(format!("{}.{}.tmp", entry.key, std::process::id()));
    fs::write(&temp_path, bytes)
        .map_err(|error| format!("AI 업스케일 캐시를 쓸 수 없습니다: {error}"))?;
    if entry.path.exists() {
        let _ = fs::remove_file(&entry.path);
    }
    fs::rename(&temp_path, &entry.path)
        .map_err(|error| format!("AI 업스케일 캐시를 저장할 수 없습니다: {error}"))?;
    prune_ai_upscale_cache()
}

fn ai_upscale_cache_dir() -> PathBuf {
    ProjectDirs::from("", "", "SuiSuiView")
        .map(|dirs| dirs.cache_dir().join("ai-upscale"))
        .unwrap_or_else(|| PathBuf::from("SuiSuiView-ai-upscale-cache"))
}

fn prune_ai_upscale_cache() -> Result<(), String> {
    let cache_dir = ai_upscale_cache_dir();
    let mut files = Vec::new();
    collect_cache_files(&cache_dir, &mut files)?;
    let mut total_bytes = files.iter().map(|file| file.bytes).sum::<u64>();
    if total_bytes <= MAX_CACHE_BYTES {
        return Ok(());
    }

    files.sort_by_key(|file| file.modified);
    for file in files {
        if total_bytes <= MAX_CACHE_BYTES {
            break;
        }
        if fs::remove_file(&file.path).is_ok() {
            total_bytes = total_bytes.saturating_sub(file.bytes);
        }
    }
    Ok(())
}

#[derive(Debug)]
struct CacheFile {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

fn collect_cache_files(dir: &PathBuf, files: &mut Vec<CacheFile>) -> Result<(), String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("AI 업스케일 캐시를 정리할 수 없습니다: {error}")),
    };

    for entry in entries {
        let entry =
            entry.map_err(|error| format!("AI 업스케일 캐시 항목을 읽을 수 없습니다: {error}"))?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .map_err(|error| format!("AI 업스케일 캐시 메타데이터를 읽을 수 없습니다: {error}"))?;
        if metadata.is_dir() {
            collect_cache_files(&path, files)?;
        } else if metadata.is_file() {
            files.push(CacheFile {
                path,
                bytes: metadata.len(),
                modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
    }
    Ok(())
}
