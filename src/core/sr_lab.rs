use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

pub mod blob;
pub mod cpu;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SrLabFamily {
    Rfdn,
    RepRfn,
    Span,
    SpanS,
}

impl SrLabFamily {
    fn label(self) -> &'static str {
        match self {
            Self::Rfdn => "RFDN",
            Self::RepRfn => "RepRFN",
            Self::Span => "SPAN",
            Self::SpanS => "SPAN-S",
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
    PixelShuffle3x,
    PixelShuffle4x,
    MeanShift,
    Silu,
    SpanGate,
    Concat4,
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
    #[serde(default)]
    pub variant: Option<String>,
    pub scale: u32,
    pub input_channels: u32,
    pub output_channels: u32,
    pub weights_format: String,
    #[serde(default)]
    pub weights_file: Option<String>,
    pub weights_sha256: String,
    pub source: String,
    #[serde(default)]
    pub source_commit: Option<String>,
    #[serde(default)]
    pub source_checkpoint_url: Option<String>,
    #[serde(default)]
    pub source_checkpoint_archive_sha256: Option<String>,
    #[serde(default)]
    pub source_checkpoint_file: Option<String>,
    #[serde(default)]
    pub source_checkpoint_sha256: Option<String>,
    pub license: String,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub span: Option<SrLabSpanMetadata>,
    pub layers: Vec<SrLabLayer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SrLabSpanMetadata {
    pub feature_channels: u32,
    pub block_count: u32,
    pub reparameterized_conv3xc: bool,
    pub img_range: f32,
    pub rgb_mean: [f32; 3],
}

#[derive(Debug, Clone, Serialize)]
pub struct SrLabInspectReport {
    pub name: String,
    pub family: String,
    pub variant: Option<String>,
    pub scale: u32,
    pub layer_count: usize,
    pub weights_format: String,
    pub weights_file: Option<String>,
    pub weights_sha256: String,
    pub source: String,
    pub source_commit: Option<String>,
    pub license: String,
    pub span: Option<SrLabSpanSummary>,
    pub tiny_wgsl_supported: bool,
    pub unsupported_ops: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SrLabSpanSummary {
    pub feature_channels: u32,
    pub block_count: u32,
    pub reparameterized_conv3xc: bool,
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
    if matches!(manifest.family, SrLabFamily::Span | SrLabFamily::SpanS)
        && manifest
            .layers
            .iter()
            .all(|layer| layer.kind != SrLabLayerKind::SpanGate)
    {
        warnings.push(
            "SPAN-family manifests normally need span_gate ops; this manifest has none".to_owned(),
        );
    }
    if matches!(manifest.family, SrLabFamily::Span | SrLabFamily::SpanS) && manifest.span.is_none()
    {
        warnings.push("SPAN-family manifests should include span metadata".to_owned());
    }
    let license_lower = manifest.license.to_ascii_lowercase();
    if license_lower.contains("noncommercial") || license_lower.contains("cc-by-nc") {
        warnings.push("NonCommercial model weights must not be bundled".to_owned());
    }

    Ok(SrLabInspectReport {
        name: manifest.name.clone(),
        family: manifest.family.label().to_owned(),
        variant: manifest.variant.clone(),
        scale: manifest.scale,
        layer_count: manifest.layers.len(),
        weights_format: manifest.weights_format.clone(),
        weights_file: manifest.weights_file.clone(),
        weights_sha256: manifest.weights_sha256.clone(),
        source: manifest.source.clone(),
        source_commit: manifest.source_commit.clone(),
        license: manifest.license.clone(),
        span: manifest.span.as_ref().map(|span| SrLabSpanSummary {
            feature_channels: span.feature_channels,
            block_count: span.block_count,
            reparameterized_conv3xc: span.reparameterized_conv3xc,
        }),
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
    if let Some(weights_file) = &manifest.weights_file {
        if weights_file.trim().is_empty() {
            return Err("SR Lab manifest weights_file is empty".into());
        }
    }
    if let Some(span) = &manifest.span {
        if span.feature_channels == 0 || span.block_count == 0 {
            return Err("SPAN metadata channels and block count must be positive".into());
        }
        if !span.img_range.is_finite() || span.img_range <= 0.0 {
            return Err("SPAN metadata img_range must be positive and finite".into());
        }
        if !span.rgb_mean.iter().all(|value| value.is_finite()) {
            return Err("SPAN metadata rgb_mean values must be finite".into());
        }
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

pub fn default_span_cpu_reference_report_path() -> PathBuf {
    PathBuf::from("perf-fixtures").join("sr-lab-span-cpu-reference.json")
}

#[cfg(test)]
mod tests {
    use super::{inspect_manifest, SrLabFamily, SrLabLayer, SrLabLayerKind, SrLabManifest};

    fn base_manifest(layers: Vec<SrLabLayer>) -> SrLabManifest {
        SrLabManifest {
            name: "tiny rfdn smoke".to_owned(),
            family: SrLabFamily::Rfdn,
            variant: None,
            scale: 2,
            input_channels: 3,
            output_channels: 3,
            weights_format: "suisui-srlab-v1".to_owned(),
            weights_file: None,
            weights_sha256: "0".repeat(64),
            source: "local-test".to_owned(),
            source_commit: None,
            source_checkpoint_url: None,
            source_checkpoint_archive_sha256: None,
            source_checkpoint_file: None,
            source_checkpoint_sha256: None,
            license: "MIT".to_owned(),
            notes: Vec::new(),
            span: None,
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
    fn span_gate_blocks_tiny_wgsl_support() {
        let mut manifest = base_manifest(vec![SrLabLayer {
            name: "attention0".to_owned(),
            kind: SrLabLayerKind::SpanGate,
            input_channels: Some(16),
            output_channels: Some(16),
        }]);
        manifest.family = SrLabFamily::Span;
        manifest.span = Some(super::SrLabSpanMetadata {
            feature_channels: 16,
            block_count: 1,
            reparameterized_conv3xc: true,
            img_range: 255.0,
            rgb_mean: [0.4488, 0.4371, 0.4040],
        });

        let report = inspect_manifest(&manifest).unwrap();

        assert!(!report.tiny_wgsl_supported);
        assert_eq!(report.unsupported_ops.len(), 1);
    }

    #[test]
    fn converted_span_s_manifest_shape_is_accepted() {
        let manifest: SrLabManifest = serde_json::from_str(
            r#"{
                "name": "SPAN-S x2",
                "family": "span-s",
                "variant": "SPAN-S",
                "scale": 2,
                "input_channels": 3,
                "output_channels": 3,
                "weights_format": "suisui-srlab-v1",
                "weights_file": "weights.srlab",
                "weights_sha256": "506ca7af17f69988dfddb951cf934ba060057d39860c8960779c7bc2790267b9",
                "source": "https://github.com/hongyuanyu/SPAN",
                "source_commit": "c77a5917759f09e66fbc7124220c5afc5ee221e5",
                "license": "Apache-2.0",
                "span": {
                    "feature_channels": 48,
                    "block_count": 6,
                    "reparameterized_conv3xc": true,
                    "img_range": 255.0,
                    "rgb_mean": [0.4488, 0.4371, 0.4040]
                },
                "layers": [
                    {"name": "mean_shift", "kind": "mean_shift", "input_channels": 3, "output_channels": 3},
                    {"name": "conv_1", "kind": "conv2d3x3", "input_channels": 3, "output_channels": 48},
                    {"name": "block_1.act1", "kind": "silu"},
                    {"name": "block_1.gate", "kind": "span_gate", "input_channels": 48, "output_channels": 48},
                    {"name": "concat_feature_b6_b1_b5_2", "kind": "concat4", "input_channels": 48, "output_channels": 192},
                    {"name": "pixel_shuffle2x", "kind": "pixel_shuffle2x", "input_channels": 12, "output_channels": 3}
                ]
            }"#,
        )
        .unwrap();

        let report = inspect_manifest(&manifest).unwrap();

        assert_eq!(report.family, "SPAN-S");
        assert_eq!(report.variant.as_deref(), Some("SPAN-S"));
        assert_eq!(
            report.span.as_ref().map(|span| span.feature_channels),
            Some(48)
        );
        assert!(report
            .unsupported_ops
            .iter()
            .any(|op| op.contains("MeanShift")));
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
