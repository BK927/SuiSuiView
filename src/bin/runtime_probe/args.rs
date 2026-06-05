use std::path::PathBuf;

use super::gpu_effect_worker::WgpuEffectScenario;
use super::wgpu_worker::WgpuProbeInput;

#[derive(Clone, Copy, Debug)]
pub(crate) enum ImageWorkerMode {
    Copy,
    Effect(WgpuEffectScenario),
}

impl ImageWorkerMode {
    pub(crate) fn input(self, image_size: [usize; 2], rgba: Vec<u8>) -> WgpuProbeInput {
        match self {
            Self::Copy => WgpuProbeInput::Rgba { image_size, rgba },
            Self::Effect(scenario) => WgpuProbeInput::Effect {
                image_size,
                rgba,
                scenario,
            },
        }
    }
}

pub(crate) fn input_path_from_args(args: &[String]) -> Option<PathBuf> {
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--input" {
            return args.get(index + 1).map(PathBuf::from);
        }
        index += 1;
    }
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--auto-close-after-report" | "--headless-worker" => index += 1,
            "--target-long-edge" | "--worker-mode" => index += 2,
            arg if arg.starts_with("--") => index += 1,
            arg => return Some(PathBuf::from(arg)),
        }
    }
    None
}

pub(crate) fn target_long_edge_from_args(args: &[String]) -> Option<u32> {
    args.windows(2)
        .find(|pair| pair[0] == "--target-long-edge")
        .and_then(|pair| pair[1].parse().ok())
        .filter(|target| *target > 0)
}

pub(crate) fn image_worker_mode_from_args(args: &[String]) -> Option<ImageWorkerMode> {
    let value = args
        .windows(2)
        .find(|pair| pair[0] == "--worker-mode")
        .map(|pair| pair[1].as_str())?;
    if value.eq_ignore_ascii_case("copy") || value.eq_ignore_ascii_case("rgba-copy") {
        return Some(ImageWorkerMode::Copy);
    }
    WgpuEffectScenario::from_token(value).map(ImageWorkerMode::Effect)
}
