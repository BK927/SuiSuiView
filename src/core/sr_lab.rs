use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

pub mod blob;
pub mod cpu;
pub mod gpu;
#[allow(dead_code)]
pub(crate) mod sha256;

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

    if matches!(manifest.family, SrLabFamily::Span | SrLabFamily::SpanS) {
        if let Err(error) = validate_span_graph_contract(manifest) {
            return Err(error.into());
        }
    }

    Ok(())
}

pub(crate) fn validate_span_graph_contract(manifest: &SrLabManifest) -> Result<(), String> {
    if !matches!(manifest.family, SrLabFamily::Span | SrLabFamily::SpanS) {
        return Err("SPAN graph contract requires a SPAN-family manifest".to_owned());
    }
    if manifest.scale != 2 {
        return Err(format!(
            "SPAN graph executor currently supports x2 pixel shuffle only, got x{}",
            manifest.scale
        ));
    }
    if manifest.input_channels != 3 || manifest.output_channels != 3 {
        return Err(format!(
            "SPAN graph executor requires RGB input/output channels, got {}/{}",
            manifest.input_channels, manifest.output_channels
        ));
    }
    let span = manifest
        .span
        .as_ref()
        .ok_or_else(|| "SPAN graph executor requires span metadata".to_owned())?;
    if !span.reparameterized_conv3xc {
        return Err("SPAN graph executor requires reparameterized Conv3XC manifests".to_owned());
    }
    let expected_len = expected_span_layer_count(span.block_count)?;
    if manifest.layers.len() != expected_len {
        return Err(format!(
            "SPAN graph executor expected {} layers for {} SPAB blocks, got {}",
            expected_len,
            span.block_count,
            manifest.layers.len()
        ));
    }
    let feature_channels = span.feature_channels;
    let output_channels = manifest.output_channels;
    let joined_channels = feature_channels
        .checked_mul(4)
        .ok_or_else(|| "SPAN graph feature channel count overflowed".to_owned())?;
    let upsample_channels = output_channels
        .checked_mul(4)
        .ok_or_else(|| "SPAN graph output channel count overflowed".to_owned())?;

    let mut index = 0usize;
    validate_span_layer_contract(
        &mut index,
        manifest,
        "mean_shift",
        SrLabLayerKind::MeanShift,
        Some(3),
        Some(3),
    )?;
    validate_span_layer_contract(
        &mut index,
        manifest,
        "conv_1",
        SrLabLayerKind::Conv2d3x3,
        Some(3),
        Some(feature_channels),
    )?;
    for block in 1..=span.block_count {
        validate_span_layer_contract(
            &mut index,
            manifest,
            &format!("block_{block}.c1_r"),
            SrLabLayerKind::Conv2d3x3,
            Some(feature_channels),
            Some(feature_channels),
        )?;
        validate_span_layer_contract(
            &mut index,
            manifest,
            &format!("block_{block}.act1"),
            SrLabLayerKind::Silu,
            None,
            None,
        )?;
        validate_span_layer_contract(
            &mut index,
            manifest,
            &format!("block_{block}.c2_r"),
            SrLabLayerKind::Conv2d3x3,
            Some(feature_channels),
            Some(feature_channels),
        )?;
        validate_span_layer_contract(
            &mut index,
            manifest,
            &format!("block_{block}.act2"),
            SrLabLayerKind::Silu,
            None,
            None,
        )?;
        validate_span_layer_contract(
            &mut index,
            manifest,
            &format!("block_{block}.c3_r"),
            SrLabLayerKind::Conv2d3x3,
            Some(feature_channels),
            Some(feature_channels),
        )?;
        validate_span_layer_contract(
            &mut index,
            manifest,
            &format!("block_{block}.gate"),
            SrLabLayerKind::SpanGate,
            Some(feature_channels),
            Some(feature_channels),
        )?;
    }
    validate_span_layer_contract(
        &mut index,
        manifest,
        "conv_2",
        SrLabLayerKind::Conv2d3x3,
        Some(feature_channels),
        Some(feature_channels),
    )?;
    validate_span_layer_contract(
        &mut index,
        manifest,
        "concat_feature_b6_b1_b5_2",
        SrLabLayerKind::Concat4,
        Some(feature_channels),
        Some(joined_channels),
    )?;
    validate_span_layer_contract(
        &mut index,
        manifest,
        "conv_cat",
        SrLabLayerKind::Conv2d1x1,
        Some(joined_channels),
        Some(feature_channels),
    )?;
    validate_span_layer_contract(
        &mut index,
        manifest,
        "upsampler.0",
        SrLabLayerKind::Conv2d3x3,
        Some(feature_channels),
        Some(upsample_channels),
    )?;
    validate_span_layer_contract(
        &mut index,
        manifest,
        "pixel_shuffle2x",
        SrLabLayerKind::PixelShuffle2x,
        Some(upsample_channels),
        Some(output_channels),
    )?;
    Ok(())
}

fn expected_span_layer_count(block_count: u32) -> Result<usize, String> {
    (block_count as usize)
        .checked_mul(6)
        .and_then(|count| count.checked_add(7))
        .ok_or_else(|| "SPAN graph layer count overflowed".to_owned())
}

fn validate_span_layer_contract(
    index: &mut usize,
    manifest: &SrLabManifest,
    expected_name: &str,
    expected_kind: SrLabLayerKind,
    expected_input_channels: Option<u32>,
    expected_output_channels: Option<u32>,
) -> Result<(), String> {
    let position = *index + 1;
    let layer = manifest
        .layers
        .get(*index)
        .ok_or_else(|| format!("SPAN graph layer {position} is missing"))?;
    if layer.name != expected_name {
        return Err(format!(
            "SPAN graph layer {position} expected '{}', got '{}'",
            expected_name, layer.name
        ));
    }
    if layer.kind != expected_kind {
        return Err(format!(
            "SPAN graph layer {position} ('{}') expected kind {:?}, got {:?}",
            layer.name, expected_kind, layer.kind
        ));
    }
    if let Some(input_channels) = expected_input_channels {
        if layer.input_channels != Some(input_channels) {
            return Err(format!(
                "SPAN graph layer {position} ('{}') expected input_channels {}, got {:?}",
                layer.name, input_channels, layer.input_channels
            ));
        }
    }
    if let Some(output_channels) = expected_output_channels {
        if layer.output_channels != Some(output_channels) {
            return Err(format!(
                "SPAN graph layer {position} ('{}') expected output_channels {}, got {:?}",
                layer.name, output_channels, layer.output_channels
            ));
        }
    }
    *index += 1;
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

pub fn default_span_gpu_reference_report_path() -> PathBuf {
    PathBuf::from("perf-fixtures").join("sr-lab-span-gpu-reference.json")
}

pub fn default_span_gpu_session_report_path() -> PathBuf {
    PathBuf::from("perf-fixtures").join("sr-lab-span-session-bench.json")
}

pub fn default_span_gpu_tiled_reference_report_path() -> PathBuf {
    PathBuf::from("perf-fixtures").join("sr-lab-span-gpu-tiled-reference.json")
}

#[cfg(test)]
mod tests {
    use super::{
        inspect_manifest, validate_span_graph_contract, SrLabFamily, SrLabLayer, SrLabLayerKind,
        SrLabManifest, SrLabSpanMetadata,
    };

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

    fn span_layers(feature_channels: u32, block_count: u32) -> Vec<SrLabLayer> {
        let joined_channels = feature_channels * 4;
        let upsample_channels = 12;
        let mut layers = Vec::with_capacity(super::expected_span_layer_count(block_count).unwrap());
        push_span_layer(
            &mut layers,
            "mean_shift",
            SrLabLayerKind::MeanShift,
            Some(3),
            Some(3),
        );
        push_span_layer(
            &mut layers,
            "conv_1",
            SrLabLayerKind::Conv2d3x3,
            Some(3),
            Some(feature_channels),
        );
        for block in 1..=block_count {
            push_span_layer(
                &mut layers,
                &format!("block_{block}.c1_r"),
                SrLabLayerKind::Conv2d3x3,
                Some(feature_channels),
                Some(feature_channels),
            );
            push_span_layer(
                &mut layers,
                &format!("block_{block}.act1"),
                SrLabLayerKind::Silu,
                None,
                None,
            );
            push_span_layer(
                &mut layers,
                &format!("block_{block}.c2_r"),
                SrLabLayerKind::Conv2d3x3,
                Some(feature_channels),
                Some(feature_channels),
            );
            push_span_layer(
                &mut layers,
                &format!("block_{block}.act2"),
                SrLabLayerKind::Silu,
                None,
                None,
            );
            push_span_layer(
                &mut layers,
                &format!("block_{block}.c3_r"),
                SrLabLayerKind::Conv2d3x3,
                Some(feature_channels),
                Some(feature_channels),
            );
            push_span_layer(
                &mut layers,
                &format!("block_{block}.gate"),
                SrLabLayerKind::SpanGate,
                Some(feature_channels),
                Some(feature_channels),
            );
        }
        push_span_layer(
            &mut layers,
            "conv_2",
            SrLabLayerKind::Conv2d3x3,
            Some(feature_channels),
            Some(feature_channels),
        );
        push_span_layer(
            &mut layers,
            "concat_feature_b6_b1_b5_2",
            SrLabLayerKind::Concat4,
            Some(feature_channels),
            Some(joined_channels),
        );
        push_span_layer(
            &mut layers,
            "conv_cat",
            SrLabLayerKind::Conv2d1x1,
            Some(joined_channels),
            Some(feature_channels),
        );
        push_span_layer(
            &mut layers,
            "upsampler.0",
            SrLabLayerKind::Conv2d3x3,
            Some(feature_channels),
            Some(upsample_channels),
        );
        push_span_layer(
            &mut layers,
            "pixel_shuffle2x",
            SrLabLayerKind::PixelShuffle2x,
            Some(upsample_channels),
            Some(3),
        );
        layers
    }

    fn push_span_layer(
        layers: &mut Vec<SrLabLayer>,
        name: &str,
        kind: SrLabLayerKind,
        input_channels: Option<u32>,
        output_channels: Option<u32>,
    ) {
        layers.push(SrLabLayer {
            name: name.to_owned(),
            kind,
            input_channels,
            output_channels,
        });
    }

    fn span_manifest(feature_channels: u32, block_count: u32) -> SrLabManifest {
        let mut manifest = base_manifest(span_layers(feature_channels, block_count));
        manifest.name = "SPAN-S x2".to_owned();
        manifest.family = SrLabFamily::SpanS;
        manifest.variant = Some("SPAN-S".to_owned());
        manifest.license = "Apache-2.0".to_owned();
        manifest.weights_file = Some("weights.srlab".to_owned());
        manifest.source = "https://github.com/hongyuanyu/SPAN".to_owned();
        manifest.source_commit = Some("c77a5917759f09e66fbc7124220c5afc5ee221e5".to_owned());
        manifest.span = Some(SrLabSpanMetadata {
            feature_channels,
            block_count,
            reparameterized_conv3xc: true,
            img_range: 255.0,
            rgb_mean: [0.4488, 0.4371, 0.4040],
        });
        manifest
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
        let manifest = span_manifest(16, 1);

        let report = inspect_manifest(&manifest).unwrap();

        assert!(!report.tiny_wgsl_supported);
        assert!(report
            .unsupported_ops
            .iter()
            .any(|op| op.contains("SpanGate")));
    }

    #[test]
    fn converted_span_s_manifest_shape_is_accepted() {
        let manifest = span_manifest(48, 6);

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
    fn span_manifest_rejects_missing_executor_layers() {
        let mut manifest = span_manifest(48, 6);
        manifest.layers.pop();

        let error = validate_span_graph_contract(&manifest).unwrap_err();

        assert!(error.contains("expected 43 layers"));
    }

    #[test]
    fn span_manifest_rejects_wrong_executor_layer_kind() {
        let mut manifest = span_manifest(48, 6);
        manifest.layers[2].kind = SrLabLayerKind::Conv2d1x1;

        let error = validate_span_graph_contract(&manifest).unwrap_err();

        assert!(error.contains("block_1.c1_r"));
        assert!(error.contains("expected kind Conv2d3x3"));
    }

    #[test]
    fn span_manifest_rejects_non_x2_contract() {
        let mut manifest = span_manifest(48, 6);
        manifest.scale = 4;

        let error = validate_span_graph_contract(&manifest).unwrap_err();

        assert!(error.contains("x2 pixel shuffle"));
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
