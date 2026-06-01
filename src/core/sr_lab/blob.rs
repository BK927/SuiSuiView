use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use super::sha256::sha256_hex;
use super::SrLabManifest;

const MAGIC: &[u8; 8] = b"SSRLAB01";
const MAX_WEIGHT_BLOB_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct SrLabTensor {
    pub name: String,
    pub shape: Vec<u32>,
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SrLabWeights {
    pub tensors: Vec<SrLabTensor>,
}

impl SrLabWeights {
    pub fn tensor(&self, name: &str) -> Option<&SrLabTensor> {
        self.tensors.iter().find(|tensor| tensor.name == name)
    }
}

pub fn read_weights(path: &Path) -> Result<SrLabWeights, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_WEIGHT_BLOB_BYTES {
        return Err(format!(
            "SR Lab weight blob is too large: {} bytes",
            metadata.len()
        ));
    }
    parse_weights(&fs::read(path).map_err(|error| error.to_string())?)
}

pub(crate) fn read_checked_weights(
    manifest_path: &Path,
    manifest: &SrLabManifest,
    context: &str,
) -> Result<SrLabWeights, String> {
    let weights_file = manifest
        .weights_file
        .as_deref()
        .ok_or_else(|| format!("{context} requires manifest weights_file"))?;
    let manifest_dir = manifest_parent_dir(manifest_path);
    let weights_path = checked_weights_path(manifest_dir, weights_file, context)?;
    let metadata = fs::metadata(&weights_path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_WEIGHT_BLOB_BYTES {
        return Err(format!(
            "{context} weight blob is too large: {} bytes",
            metadata.len()
        ));
    }
    let bytes = fs::read(&weights_path).map_err(|error| error.to_string())?;
    let actual_sha256 = sha256_hex(&bytes);
    if !actual_sha256.eq_ignore_ascii_case(&manifest.weights_sha256) {
        return Err(format!(
            "{context} weight SHA-256 mismatch for {}",
            weights_path.display()
        ));
    }
    parse_weights(&bytes)
}

fn manifest_parent_dir(manifest_path: &Path) -> &Path {
    manifest_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn checked_weights_path(
    manifest_dir: &Path,
    weights_file: &str,
    context: &str,
) -> Result<PathBuf, String> {
    let relative_path = safe_relative_weights_path(weights_file, context)?;
    let weights_path = manifest_dir.join(relative_path);
    let canonical_manifest_dir = fs::canonicalize(manifest_dir)
        .map_err(|error| format!("{context} manifest directory cannot be resolved: {}", error))?;
    let canonical_weights_path = fs::canonicalize(&weights_path)
        .map_err(|error| format!("{context} weight path cannot be resolved: {}", error))?;
    if !canonical_weights_path.starts_with(&canonical_manifest_dir) {
        return Err(format!(
            "{context} weight path must stay under the manifest directory"
        ));
    }
    Ok(canonical_weights_path)
}

fn safe_relative_weights_path(weights_file: &str, context: &str) -> Result<PathBuf, String> {
    let weights_file = weights_file.trim();
    if weights_file.is_empty() {
        return Err(format!("{context} requires a non-empty weights_file"));
    }
    let path = Path::new(weights_file);
    if path.is_absolute() {
        return Err(format!("{context} weight path must be relative"));
    }
    let mut saw_normal_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => saw_normal_component = true,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "{context} weight path must not leave the manifest directory"
                ));
            }
        }
    }
    if !saw_normal_component {
        return Err(format!("{context} weight path must name a file"));
    }
    Ok(path.to_path_buf())
}

pub fn parse_weights(bytes: &[u8]) -> Result<SrLabWeights, String> {
    let mut cursor = Cursor::new(bytes);
    if cursor.take(MAGIC.len())? != MAGIC {
        return Err("invalid SR Lab weight blob header".to_owned());
    }
    let tensor_count = cursor.u32()? as usize;
    let mut tensors = Vec::with_capacity(tensor_count);
    let mut names = BTreeSet::new();

    for _ in 0..tensor_count {
        let name_len = cursor.u16()? as usize;
        let name_bytes = cursor.take(name_len)?;
        let name = std::str::from_utf8(name_bytes)
            .map_err(|_| "SR Lab tensor name is not UTF-8".to_owned())?
            .to_owned();
        if name.is_empty() {
            return Err("SR Lab tensor name is empty".to_owned());
        }
        if !names.insert(name.clone()) {
            return Err(format!("duplicate SR Lab tensor name: {name}"));
        }

        let rank = cursor.u8()? as usize;
        if rank > 4 {
            return Err(format!("SR Lab tensor {name} has unsupported rank {rank}"));
        }
        let shape4 = [cursor.u32()?, cursor.u32()?, cursor.u32()?, cursor.u32()?];
        let shape = shape4[..rank].to_vec();
        if shape.iter().any(|dimension| *dimension == 0) {
            return Err(format!("SR Lab tensor {name} has a zero dimension"));
        }

        let byte_len_u64 = cursor.u64()?;
        let byte_len = usize::try_from(byte_len_u64)
            .map_err(|_| format!("SR Lab tensor {name} byte length does not fit usize"))?;
        if byte_len % std::mem::size_of::<f32>() != 0 {
            return Err(format!(
                "SR Lab tensor {name} byte length is not f32-aligned"
            ));
        }
        let expected_values = shape
            .iter()
            .try_fold(1usize, |total, dimension| {
                total.checked_mul(*dimension as usize)
            })
            .ok_or_else(|| format!("SR Lab tensor {name} shape is too large"))?;
        let actual_values = byte_len / std::mem::size_of::<f32>();
        if expected_values != actual_values {
            return Err(format!(
                "SR Lab tensor {name} shape expects {expected_values} values, blob has {actual_values}"
            ));
        }

        let raw = cursor.take(byte_len)?;
        let mut values = Vec::with_capacity(actual_values);
        for chunk in raw.chunks_exact(4) {
            values.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        tensors.push(SrLabTensor {
            name,
            shape,
            values,
        });
    }

    if !cursor.is_finished() {
        return Err("SR Lab weight blob has trailing bytes".to_owned());
    }

    Ok(SrLabWeights { tensors })
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, size: usize) -> Result<&'a [u8], String> {
        let end = self
            .offset
            .checked_add(size)
            .ok_or_else(|| "SR Lab weight blob offset overflowed".to_owned())?;
        let chunk = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| "truncated SR Lab weight blob".to_owned())?;
        self.offset = end;
        Ok(chunk)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64, String> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn is_finished(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        manifest_parent_dir, parse_weights, read_checked_weights, safe_relative_weights_path,
    };
    use crate::core::sr_lab::{SrLabFamily, SrLabManifest};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tiny_blob() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"SSRLAB01");
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&6u16.to_le_bytes());
        bytes.extend_from_slice(b"weight");
        bytes.push(2);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&8u64.to_le_bytes());
        bytes.extend_from_slice(&1.5f32.to_le_bytes());
        bytes.extend_from_slice(&(-2.0f32).to_le_bytes());
        bytes
    }

    fn tiny_manifest(weights_file: Option<String>, weights_sha256: String) -> SrLabManifest {
        SrLabManifest {
            name: "tiny SPAN-S".to_owned(),
            family: SrLabFamily::SpanS,
            variant: Some("SPAN-S".to_owned()),
            scale: 2,
            input_channels: 3,
            output_channels: 3,
            weights_format: "suisui-srlab-v1".to_owned(),
            weights_file,
            weights_sha256,
            source: "local-test".to_owned(),
            source_commit: None,
            source_checkpoint_url: None,
            source_checkpoint_archive_sha256: None,
            source_checkpoint_file: None,
            source_checkpoint_sha256: None,
            license: "Apache-2.0".to_owned(),
            notes: Vec::new(),
            span: None,
            layers: Vec::new(),
        }
    }

    fn temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "suisuiview-srlab-weights-test-{}-{unique}",
            std::process::id()
        ))
    }

    #[test]
    fn parses_srlab01_tensor_blob() {
        let weights = parse_weights(&tiny_blob()).unwrap();

        let tensor = weights.tensor("weight").unwrap();
        assert_eq!(tensor.shape, vec![1, 2]);
        assert_eq!(tensor.values, vec![1.5, -2.0]);
    }

    #[test]
    fn rejects_duplicate_tensor_names() {
        let mut bytes = tiny_blob();
        bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
        let second = tiny_blob().split_off(12);
        bytes.extend_from_slice(&second);

        let error = parse_weights(&bytes).unwrap_err();

        assert!(error.contains("duplicate"));
    }

    #[test]
    fn rejects_shape_byte_length_mismatch() {
        let mut bytes = tiny_blob();
        let byte_len_offset = 8 + 4 + 2 + 6 + 1 + 16;
        bytes[byte_len_offset..byte_len_offset + 8].copy_from_slice(&4u64.to_le_bytes());

        let error = parse_weights(&bytes).unwrap_err();

        assert!(error.contains("shape expects"));
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut bytes = tiny_blob();
        bytes.push(0);

        let error = parse_weights(&bytes).unwrap_err();

        assert!(error.contains("trailing"));
    }

    #[test]
    fn checked_weight_paths_must_stay_relative() {
        assert_eq!(
            safe_relative_weights_path("weights.srlab", "SR Lab").unwrap(),
            PathBuf::from("weights.srlab")
        );
        assert!(safe_relative_weights_path("", "SR Lab").is_err());
        assert!(safe_relative_weights_path(".", "SR Lab").is_err());
        assert!(safe_relative_weights_path("../weights.srlab", "SR Lab").is_err());
        assert!(safe_relative_weights_path("nested/../weights.srlab", "SR Lab").is_err());
        assert!(safe_relative_weights_path("/models/weights.srlab", "SR Lab").is_err());
        #[cfg(windows)]
        assert!(safe_relative_weights_path("C:\\models\\weights.srlab", "SR Lab").is_err());
    }

    #[test]
    fn read_checked_weights_verifies_manifest_sha256() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let manifest_path = dir.join("manifest.json");
        let weights_path = dir.join("weights.srlab");
        let bytes = tiny_blob();
        fs::write(&weights_path, &bytes).unwrap();
        let manifest = tiny_manifest(
            Some("weights.srlab".to_owned()),
            crate::core::sr_lab::sha256::sha256_hex(&bytes),
        );

        let weights = read_checked_weights(&manifest_path, &manifest, "SPAN test").unwrap();

        assert_eq!(weights.tensor("weight").unwrap().values, vec![1.5, -2.0]);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn read_checked_weights_accepts_single_component_manifest_path() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = PathBuf::from("target").join(format!(
            "suisuiview-srlab-weights-test-{}-{unique}",
            std::process::id(),
        ));
        fs::create_dir_all(&dir).unwrap();
        let weights_path = dir.join("single-component-weights.srlab");
        let bytes = tiny_blob();
        fs::write(&weights_path, &bytes).unwrap();
        let manifest = tiny_manifest(
            Some(weights_path.to_string_lossy().into_owned()),
            crate::core::sr_lab::sha256::sha256_hex(&bytes),
        );

        let weights =
            read_checked_weights(Path::new("manifest.json"), &manifest, "SPAN test").unwrap();

        assert_eq!(weights.tensor("weight").unwrap().values, vec![1.5, -2.0]);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn read_checked_weights_rejects_sha256_mismatch() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let manifest_path = dir.join("manifest.json");
        fs::write(dir.join("weights.srlab"), tiny_blob()).unwrap();
        let manifest = tiny_manifest(Some("weights.srlab".to_owned()), "0".repeat(64));

        let error = read_checked_weights(&manifest_path, &manifest, "SPAN test").unwrap_err();

        assert!(error.contains("SHA-256 mismatch"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn read_checked_weights_requires_weights_file() {
        let manifest = tiny_manifest(None, "0".repeat(64));

        let error =
            read_checked_weights(Path::new("manifest.json"), &manifest, "SPAN test").unwrap_err();

        assert!(error.contains("requires manifest weights_file"));
    }

    #[test]
    fn manifest_parent_dir_uses_current_dir_for_single_component_paths() {
        assert_eq!(
            manifest_parent_dir(Path::new("manifest.json")),
            Path::new(".")
        );
    }
}
