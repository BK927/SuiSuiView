use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SrLabFamily {
    Rfdn,
    RepRfn,
    Span,
}

impl SrLabFamily {
    fn label(self) -> &'static str {
        match self {
            Self::Rfdn => "RFDN",
            Self::RepRfn => "RepRFN",
            Self::Span => "SPAN",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrLabLayerKind {
    Conv2d3x3,
    Conv2d1x1,
    Relu,
    LeakyRelu,
    ResidualAdd,
    PixelShuffle2x,
    SpanAttention,
}

impl SrLabLayerKind {
    fn is_tiny_wgsl_supported(&self) -> bool {
        matches!(
            self,
            Self::Conv2d3x3
                | Self::Conv2d1x1
                | Self::Relu
                | Self::ResidualAdd
                | Self::PixelShuffle2x
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SrLabLayer {
    pub name: String,
    pub kind: SrLabLayerKind,
    #[serde(default)]
    pub input_channels: Option<u32>,
    #[serde(default)]
    pub output_channels: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SrLabManifest {
    pub name: String,
    pub family: SrLabFamily,
    pub scale: u32,
    pub input_channels: u32,
    pub output_channels: u32,
    pub weights_format: String,
    pub weights_sha256: String,
    pub source: String,
    pub license: String,
    pub layers: Vec<SrLabLayer>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SrLabInspectReport {
    pub name: String,
    pub family: String,
    pub scale: u32,
    pub layer_count: usize,
    pub weights_format: String,
    pub weights_sha256: String,
    pub source: String,
    pub license: String,
    pub tiny_wgsl_supported: bool,
    pub unsupported_ops: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn run_sr_lab_inspect(
    manifest_path: &Path,
    report_path: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = read_manifest(manifest_path)?;
    let report = inspect_manifest(&manifest)?;
    println!("{}", serde_json::to_string_pretty(&report)?);

    if let Some(report_path) = report_path {
        if let Some(parent) = report_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(report_path, serde_json::to_string_pretty(&report)?)?;
    }

    Ok(())
}

pub fn read_manifest(path: &Path) -> Result<SrLabManifest, Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn inspect_manifest(
    manifest: &SrLabManifest,
) -> Result<SrLabInspectReport, Box<dyn std::error::Error>> {
    validate_manifest(manifest)?;

    let unsupported_ops: Vec<String> = manifest
        .layers
        .iter()
        .filter(|layer| !layer.kind.is_tiny_wgsl_supported())
        .map(|layer| format!("{}:{:?}", layer.name, layer.kind))
        .collect();
    let mut warnings = Vec::new();
    if manifest.family == SrLabFamily::Span
        && manifest
            .layers
            .iter()
            .all(|layer| layer.kind != SrLabLayerKind::SpanAttention)
    {
        warnings.push(
            "SPAN-family manifests normally need attention ops; this manifest has none".to_owned(),
        );
    }
    let license_lower = manifest.license.to_ascii_lowercase();
    if license_lower.contains("noncommercial") || license_lower.contains("cc-by-nc") {
        warnings.push("NonCommercial model weights must not be bundled".to_owned());
    }

    Ok(SrLabInspectReport {
        name: manifest.name.clone(),
        family: manifest.family.label().to_owned(),
        scale: manifest.scale,
        layer_count: manifest.layers.len(),
        weights_format: manifest.weights_format.clone(),
        weights_sha256: manifest.weights_sha256.clone(),
        source: manifest.source.clone(),
        license: manifest.license.clone(),
        tiny_wgsl_supported: unsupported_ops.is_empty(),
        unsupported_ops,
        warnings,
    })
}

fn validate_manifest(manifest: &SrLabManifest) -> Result<(), Box<dyn std::error::Error>> {
    if manifest.name.trim().is_empty() {
        return Err("SR Lab manifest name is empty".into());
    }
    if !matches!(manifest.scale, 2..=4) {
        return Err(format!("unsupported SR scale: {}", manifest.scale).into());
    }
    if manifest.input_channels == 0 || manifest.output_channels == 0 {
        return Err("input/output channels must be positive".into());
    }
    if manifest.weights_format != "suisui-srlab-v1" {
        return Err(format!("unsupported weights format: {}", manifest.weights_format).into());
    }
    if !is_hex_sha256(&manifest.weights_sha256) {
        return Err("weights_sha256 must be a 64-character hex SHA-256".into());
    }
    if manifest.source.trim().is_empty() {
        return Err("SR Lab manifest source is empty".into());
    }
    if manifest.license.trim().is_empty() {
        return Err("SR Lab manifest license is empty".into());
    }
    if manifest.layers.is_empty() {
        return Err("SR Lab manifest has no layers".into());
    }

    let mut names = BTreeSet::new();
    for layer in &manifest.layers {
        if layer.name.trim().is_empty() {
            return Err("SR Lab layer name is empty".into());
        }
        if !names.insert(layer.name.as_str()) {
            return Err(format!("duplicate SR Lab layer name: {}", layer.name).into());
        }
    }

    Ok(())
}

fn is_hex_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn default_sr_lab_report_path() -> PathBuf {
    PathBuf::from("perf-fixtures").join("sr-lab-inspect.json")
}

#[cfg(test)]
mod tests {
    use super::{inspect_manifest, SrLabFamily, SrLabLayer, SrLabLayerKind, SrLabManifest};

    fn base_manifest(layers: Vec<SrLabLayer>) -> SrLabManifest {
        SrLabManifest {
            name: "tiny rfdn smoke".to_owned(),
            family: SrLabFamily::Rfdn,
            scale: 2,
            input_channels: 3,
            output_channels: 3,
            weights_format: "suisui-srlab-v1".to_owned(),
            weights_sha256: "0".repeat(64),
            source: "local-test".to_owned(),
            license: "MIT".to_owned(),
            layers,
        }
    }

    #[test]
    fn simple_manifest_is_tiny_wgsl_supported() {
        let report = inspect_manifest(&base_manifest(vec![
            SrLabLayer {
                name: "conv0".to_owned(),
                kind: SrLabLayerKind::Conv2d3x3,
                input_channels: Some(3),
                output_channels: Some(16),
            },
            SrLabLayer {
                name: "relu0".to_owned(),
                kind: SrLabLayerKind::Relu,
                input_channels: None,
                output_channels: None,
            },
            SrLabLayer {
                name: "shuffle".to_owned(),
                kind: SrLabLayerKind::PixelShuffle2x,
                input_channels: Some(12),
                output_channels: Some(3),
            },
        ]))
        .unwrap();

        assert!(report.tiny_wgsl_supported);
        assert!(report.unsupported_ops.is_empty());
    }

    #[test]
    fn span_attention_blocks_tiny_wgsl_support() {
        let mut manifest = base_manifest(vec![SrLabLayer {
            name: "attention0".to_owned(),
            kind: SrLabLayerKind::SpanAttention,
            input_channels: Some(16),
            output_channels: Some(16),
        }]);
        manifest.family = SrLabFamily::Span;

        let report = inspect_manifest(&manifest).unwrap();

        assert!(!report.tiny_wgsl_supported);
        assert_eq!(report.unsupported_ops.len(), 1);
    }

    #[test]
    fn noncommercial_license_warns_without_accepting_bundling() {
        let mut manifest = base_manifest(vec![SrLabLayer {
            name: "conv0".to_owned(),
            kind: SrLabLayerKind::Conv2d1x1,
            input_channels: Some(3),
            output_channels: Some(3),
        }]);
        manifest.license = "CC-BY-NC-4.0".to_owned();

        let report = inspect_manifest(&manifest).unwrap();

        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("NonCommercial")));
    }
}
