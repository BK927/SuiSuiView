use crate::core::sr_lab::{
    self, blob,
    cpu::FeatureMap,
    gpu::tiled::{SpanGpuTiledRunner, DEFAULT_SPAN_TILE_EDGE},
    SrLabFamily, SrLabManifest,
};
use egui::ColorImage;
use image::{imageops::FilterType, RgbaImage};
use std::env;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const EXPERIMENT_SPAN_MANIFEST_ENV: &str = "SUISUIVIEW_EXPERIMENT_SPAN_MANIFEST";
const SR_LAB_SPAN_MANIFEST_ENV: &str = "SUISUIVIEW_SR_LAB_SPAN_MANIFEST";
const DEFAULT_SPAN_MANIFEST_PATHS: [&str; 2] = [
    "models/span/span_s_x2_ch48/manifest.json",
    "models/span/span_x2_ch52/manifest.json",
];

pub(super) struct SpanBench {
    manifest: SrLabManifest,
    runner: SpanGpuTiledRunner,
    tile_edge: usize,
}

pub(super) struct SpanBenchOutput {
    pub(super) image: ColorImage,
    pub(super) elapsed: Duration,
}

impl SpanBench {
    pub(super) fn try_new() -> Option<Self> {
        match Self::new() {
            Ok(bench) => Some(bench),
            Err(error) => {
                eprintln!("SR Lab SPAN x2 bench candidate disabled: {error}");
                None
            }
        }
    }

    fn new() -> Result<Self, String> {
        let manifest_path = span_manifest_path()
            .ok_or_else(|| "set SUISUIVIEW_SR_LAB_SPAN_MANIFEST to a SPAN manifest".to_owned())?;
        let manifest = sr_lab::read_manifest(&manifest_path).map_err(|error| error.to_string())?;
        validate_span_bench_manifest(&manifest)?;
        sr_lab::validate_span_graph_contract(&manifest)?;
        let weights = blob::read_checked_weights(&manifest_path, &manifest, "SPAN quality bench")?;
        let runner = SpanGpuTiledRunner::new(&weights)?;
        Ok(Self {
            manifest,
            runner,
            tile_edge: DEFAULT_SPAN_TILE_EDGE,
        })
    }

    pub(super) fn apply(
        &self,
        image: &ColorImage,
        output_size: [usize; 2],
    ) -> Result<SpanBenchOutput, String> {
        let started = Instant::now();
        let scale = self.manifest.scale as usize;
        let exact_output = [
            image.size[0]
                .checked_mul(scale)
                .ok_or_else(|| "SR Lab SPAN x2 output width overflowed".to_owned())?,
            image.size[1]
                .checked_mul(scale)
                .ok_or_else(|| "SR Lab SPAN x2 output height overflowed".to_owned())?,
        ];
        if !is_near_exact_output(output_size, exact_output) {
            return Err(format!(
                "SR Lab SPAN x2 requires near-{scale}x output, got {}x{} -> {}x{}",
                image.size[0], image.size[1], output_size[0], output_size[1]
            ));
        }

        let input = color_image_to_feature_map(image);
        let run = self.runner.run(&self.manifest, &input, self.tile_edge)?;
        if run.output.width != exact_output[0] || run.output.height != exact_output[1] {
            return Err(format!(
                "SR Lab SPAN x2 output size mismatch: expected {}x{}, got {}x{}",
                exact_output[0], exact_output[1], run.output.width, run.output.height
            ));
        }
        let mut output = feature_map_to_color_image(&run.output)?;
        if output.size != output_size {
            output = resize_color_image(&output, output_size)?;
        }

        Ok(SpanBenchOutput {
            image: output,
            elapsed: started.elapsed(),
        })
    }
}

fn span_manifest_path() -> Option<PathBuf> {
    env::var_os(EXPERIMENT_SPAN_MANIFEST_ENV)
        .or_else(|| env::var_os(SR_LAB_SPAN_MANIFEST_ENV))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(default_span_manifest_path)
}

fn default_span_manifest_path() -> Option<PathBuf> {
    DEFAULT_SPAN_MANIFEST_PATHS
        .iter()
        .map(Path::new)
        .find(|path| path.exists())
        .map(Path::to_path_buf)
}

fn validate_span_bench_manifest(manifest: &SrLabManifest) -> Result<(), String> {
    if !matches!(manifest.family, SrLabFamily::Span | SrLabFamily::SpanS) {
        return Err("SPAN quality bench requires a SPAN-family manifest".to_owned());
    }
    if manifest.scale != 2 || manifest.input_channels != 3 || manifest.output_channels != 3 {
        return Err("SPAN quality bench requires a 3-channel x2 RGB manifest".to_owned());
    }
    Ok(())
}

fn color_image_to_feature_map(image: &ColorImage) -> FeatureMap {
    let mut input = FeatureMap::zeros(3, image.size[1], image.size[0]);
    for y in 0..image.size[1] {
        for x in 0..image.size[0] {
            let pixel = image.pixels[y * image.size[0] + x].to_array();
            for (channel, value) in pixel.iter().take(3).enumerate() {
                input.set(channel, y, x, f32::from(*value) / 255.0);
            }
        }
    }
    input
}

fn is_near_exact_output(output_size: [usize; 2], exact_output: [usize; 2]) -> bool {
    output_size[0].abs_diff(exact_output[0]) <= 1 && output_size[1].abs_diff(exact_output[1]) <= 1
}

fn resize_color_image(image: &ColorImage, output_size: [usize; 2]) -> Result<ColorImage, String> {
    let rgba = RgbaImage::from_raw(
        image.size[0] as u32,
        image.size[1] as u32,
        color_image_to_rgba(image),
    )
    .ok_or_else(|| "failed to build SR Lab SPAN x2 resize input image".to_owned())?;
    let resized = image::imageops::resize(
        &rgba,
        output_size[0] as u32,
        output_size[1] as u32,
        FilterType::Lanczos3,
    );
    Ok(ColorImage::from_rgba_unmultiplied(
        output_size,
        &resized.into_raw(),
    ))
}

fn color_image_to_rgba(image: &ColorImage) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
    for pixel in &image.pixels {
        bytes.extend_from_slice(&pixel.to_srgba_unmultiplied());
    }
    bytes
}

fn feature_map_to_color_image(output: &FeatureMap) -> Result<ColorImage, String> {
    if output.channels != 3 {
        return Err(format!(
            "SR Lab SPAN x2 expected 3 output channels, got {}",
            output.channels
        ));
    }

    let mut bytes = Vec::with_capacity(output.width * output.height * 4);
    for y in 0..output.height {
        for x in 0..output.width {
            for channel in 0..3 {
                let value = output.get(channel, y as isize, x as isize);
                bytes.push((value * 255.0).round().clamp(0.0, 255.0) as u8);
            }
            bytes.push(255);
        }
    }
    Ok(ColorImage::from_rgba_unmultiplied(
        [output.width, output.height],
        &bytes,
    ))
}
