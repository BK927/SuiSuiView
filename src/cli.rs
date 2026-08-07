use crate::core::artcnn::ArtcnnVariant;
use crate::core::worker::{
    DecodeStrategy, OriginalRegion, DEFAULT_TARGET_LONG_EDGE, MIN_TARGET_LONG_EDGE,
};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

mod artcnn_args;
mod decoder_bench_args;
mod gpu_copy_args;
mod original_region_args;
mod sr_lab_args;
mod upscale_method_arg;
mod upscale_render_args;

const CLI_NAME: &str = "suisuiview-cli";

pub const REDIRECT_MESSAGE: &str =
    "CLI 명령은 suisuiview-cli를 사용하세요.\n예: suisuiview-cli --perf-scan <path>";

const HELP: &str = "\
SuiSuiView CLI

Usage:
  suisuiview-cli --perf-scan <path> [--perf-report <report.json>] [--perf-report-default] [--target-long-edge <px>] [--decode-strategy auto|image-crate]
  suisuiview-cli --quality-scan <path> [--target-long-edge <px>] [--quality-report <report.json>]
  suisuiview-cli --effect-bench <path> [--target-long-edge <px>] [--effect-report <report.json>] [--effect-report-default]
  suisuiview-cli --upscale-bench <path> [--source-long-edge <px>] [--target-long-edge <px>] [--upscale-method <token>] [--upscale-max-pages <count>] [--upscale-report <report.json>] [--upscale-report-default]
  suisuiview-cli --upscale-quality-scan <path> [--source-long-edge <px>] [--target-long-edge <px>] [--upscale-quality-method <token>] [--upscale-quality-max-pages <count>] [--upscale-quality-report <report.json>] [--upscale-quality-report-default] [--upscale-quality-visuals <dir>]
  suisuiview-cli --cunny-probe --probe-method <token> [--probe-edge <px>] [--probe-report <report.json>] [--probe-report-default]
  suisuiview-cli --cunny-stage-stats <image> --stage-method <token> [--stage-long-edge <px>] [--stage-report <report.json>] [--stage-report-default]

  suisuiview-cli --upscale-render <method> <image> --upscale-output <png> --upscale-output-size <width>x<height>
  suisuiview-cli --gpu-copy-bench <path> [--target-long-edge <px>] [--gpu-copy-iterations <count>] [--gpu-copy-max-pages <count>] [--gpu-copy-report <report.json>] [--gpu-copy-report-default]
  suisuiview-cli --decoder-bench <path> [--decoder-iterations <count>] [--decoder-max-pages <count>] [--decoder-report <report.json>] [--decoder-report-default]
  suisuiview-cli --original-region-bench <path> --region <x,y,width,height> [--page-index <index>] [--region-iterations <count>] [--region-report <report.json>] [--region-report-default]
  suisuiview-cli --artcnn-render <variant> <image> --artcnn-output <png>
  suisuiview-cli --artcnn-c4f16-render <image> --artcnn-output <png>
  suisuiview-cli --sr-lab-inspect <manifest.json> [--sr-lab-report <report.json>] [--sr-lab-report-default]
  suisuiview-cli --sr-lab-span-cpu-reference <manifest.json> <image> [--sr-lab-long-edge <px>] [--sr-lab-max-long-edge <px>] [--sr-lab-output <png>] [--sr-lab-report <report.json>] [--sr-lab-report-default]
  suisuiview-cli --sr-lab-span-gpu-reference <manifest.json> <image> [--sr-lab-long-edge <px>] [--sr-lab-max-long-edge <px>] [--sr-lab-output <png>] [--sr-lab-report <report.json>] [--sr-lab-report-default] [--sr-lab-compare-cpu]
  suisuiview-cli --sr-lab-span-session-bench <manifest.json> <image> [--sr-lab-long-edge <px>] [--sr-lab-max-long-edge <px>] [--sr-lab-warmups <count>] [--sr-lab-iterations <count>] [--sr-lab-report <report.json>] [--sr-lab-report-default]
  suisuiview-cli --sr-lab-span-gpu-tiled-reference <manifest.json> <image> [--sr-lab-long-edge <px>] [--sr-lab-max-long-edge <px>] [--sr-lab-tile-edge <px>] [--sr-lab-output <png>] [--sr-lab-report <report.json>] [--sr-lab-report-default] [--sr-lab-compare-cpu]

Options:
  -h, --help    Show this help.

Decoder bench accepts a single file, CBZ/ZIP, or a recursive folder and does
not change production decoder defaults.
";

#[derive(Debug, Clone)]
pub enum CliAction {
    Help,
    Command(CliCommand),
}

#[derive(Debug, Clone)]
pub enum CliCommand {
    PerfScan {
        path: PathBuf,
        report_path: Option<PathBuf>,
        target_long_edge: u32,
        decode_strategy: DecodeStrategy,
    },
    QualityScan {
        path: PathBuf,
        target_long_edge: u32,
        report_path: Option<PathBuf>,
    },
    EffectBench {
        path: PathBuf,
        target_long_edge: u32,
        report_path: Option<PathBuf>,
    },
    UpscaleBench {
        path: PathBuf,
        source_long_edge: u32,
        target_long_edge: u32,
        method_filter: Option<crate::core::state::WgpuUpscaleMethod>,
        max_pages: Option<usize>,
        report_path: Option<PathBuf>,
    },
    UpscaleQualityScan {
        path: PathBuf,
        source_long_edge: u32,
        target_long_edge: u32,
        method_filter: Option<crate::core::state::WgpuUpscaleMethod>,
        max_pages: Option<usize>,
        report_path: Option<PathBuf>,
        visual_dir: Option<PathBuf>,
    },
    /// Synthetic structural probes for one CuNNy variant: impulse, flat field
    /// and per-axis edges, for telling a wiring fault from a weights fault.
    CunnyProbe {
        method: crate::core::state::WgpuUpscaleMethod,
        probe_edge: u32,
        report_path: Option<PathBuf>,
    },
    /// Per-pass activation statistics for one CuNNy variant: the check that
    /// localises a silently mis-ported convolution chain to a pass index.
    CunnyStageStats {
        image: PathBuf,
        method: crate::core::state::WgpuUpscaleMethod,
        long_edge: u32,
        report_path: Option<PathBuf>,
    },
    GpuCopyBench {
        path: PathBuf,
        target_long_edge: u32,
        iterations: usize,
        max_pages: usize,
        report_path: Option<PathBuf>,
    },
    DecoderBench {
        path: PathBuf,
        iterations: usize,
        max_pages: usize,
        report_path: Option<PathBuf>,
    },
    OriginalRegionBench {
        path: PathBuf,
        page_index: usize,
        region: OriginalRegion,
        iterations: usize,
        report_path: Option<PathBuf>,
    },
    ArtcnnRender {
        variant: ArtcnnVariant,
        method: crate::core::state::WgpuUpscaleMethod,
        input_path: PathBuf,
        output_path: PathBuf,
    },
    UpscaleRender {
        method: crate::core::state::WgpuUpscaleMethod,
        input_path: PathBuf,
        output_path: PathBuf,
        output_size: [usize; 2],
    },
    SrLabInspect {
        manifest_path: PathBuf,
        report_path: Option<PathBuf>,
    },
    SrLabSpanCpuReference {
        manifest_path: PathBuf,
        input_path: PathBuf,
        long_edge: Option<u32>,
        max_long_edge: Option<u32>,
        output_path: Option<PathBuf>,
        report_path: Option<PathBuf>,
    },
    SrLabSpanGpuReference {
        manifest_path: PathBuf,
        input_path: PathBuf,
        long_edge: Option<u32>,
        max_long_edge: Option<u32>,
        output_path: Option<PathBuf>,
        report_path: Option<PathBuf>,
        compare_cpu: bool,
    },
    SrLabSpanGpuTiledReference {
        manifest_path: PathBuf,
        input_path: PathBuf,
        long_edge: Option<u32>,
        max_long_edge: Option<u32>,
        tile_edge: usize,
        output_path: Option<PathBuf>,
        report_path: Option<PathBuf>,
        compare_cpu: bool,
    },
    SrLabSpanSessionBench {
        manifest_path: PathBuf,
        input_path: PathBuf,
        long_edge: Option<u32>,
        max_long_edge: Option<u32>,
        warmups: usize,
        iterations: usize,
        report_path: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    message: String,
}

impl CliError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

pub fn run_from_env() -> ExitCode {
    match parse_args(std::env::args_os().skip(1).collect()) {
        Ok(CliAction::Help) => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Ok(CliAction::Command(command)) => match command.run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(1)
            }
        },
        Err(error) => {
            eprintln!("{}", error.message());
            eprintln!();
            eprintln!("{HELP}");
            ExitCode::from(2)
        }
    }
}

pub fn parse_args(args: Vec<OsString>) -> Result<CliAction, CliError> {
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(CliAction::Help);
    };

    if is_help_arg(&first) {
        return Ok(CliAction::Help);
    }
    if first == "--perf-scan" {
        return parse_perf_scan(args).map(CliAction::Command);
    }
    if first == "--quality-scan" {
        return parse_quality_scan(args).map(CliAction::Command);
    }
    if first == "--effect-bench" {
        return parse_effect_bench(args).map(CliAction::Command);
    }
    if first == "--upscale-bench" {
        return parse_upscale_bench(args).map(CliAction::Command);
    }
    if first == "--upscale-quality-scan" {
        return parse_upscale_quality_scan(args).map(CliAction::Command);
    }
    if first == "--upscale-render" {
        return upscale_render_args::parse(args).map(CliAction::Command);
    }
    if first == "--cunny-probe" {
        return parse_cunny_probe(args).map(CliAction::Command);
    }
    if first == "--cunny-stage-stats" {
        return parse_cunny_stage_stats(args).map(CliAction::Command);
    }
    if first == "--gpu-copy-bench" {
        return gpu_copy_args::parse(args).map(CliAction::Command);
    }
    if first == "--decoder-bench" {
        return decoder_bench_args::parse(args).map(CliAction::Command);
    }
    if first == "--original-region-bench" {
        return original_region_args::parse(args).map(CliAction::Command);
    }
    if first == "--artcnn-c4f16-render" {
        return artcnn_args::parse_c4f16(args).map(CliAction::Command);
    }
    if first == "--artcnn-render" {
        return artcnn_args::parse_variant(args).map(CliAction::Command);
    }
    if first == "--sr-lab-inspect" {
        return sr_lab_args::parse_inspect(args).map(CliAction::Command);
    }
    if first == "--sr-lab-span-cpu-reference" {
        return sr_lab_args::parse_span_cpu_reference(args).map(CliAction::Command);
    }
    if first == "--sr-lab-span-gpu-reference" {
        return sr_lab_args::parse_span_gpu_reference(args).map(CliAction::Command);
    }
    if first == "--sr-lab-span-gpu-tiled-reference" {
        return sr_lab_args::parse_span_gpu_tiled_reference(args).map(CliAction::Command);
    }
    if first == "--sr-lab-span-session-bench" {
        return sr_lab_args::parse_span_session_bench(args).map(CliAction::Command);
    }

    Err(CliError::new(format!(
        "unknown {CLI_NAME} command: {}",
        first.to_string_lossy()
    )))
}

pub fn is_gui_cli_redirect_arg(arg: &OsString) -> bool {
    is_help_arg(arg) || is_cli_command_arg(arg)
}

fn is_help_arg(arg: &OsString) -> bool {
    arg == "--help" || arg == "-h" || arg == "help"
}

fn is_cli_command_arg(arg: &OsString) -> bool {
    arg == "--perf-scan"
        || arg == "--quality-scan"
        || arg == "--effect-bench"
        || arg == "--upscale-bench"
        || arg == "--upscale-quality-scan"
        || arg == "--cunny-stage-stats"
        || arg == "--cunny-probe"
        || arg == "--upscale-render"
        || arg == "--gpu-copy-bench"
        || arg == "--decoder-bench"
        || arg == "--original-region-bench"
        || arg == "--artcnn-render"
        || arg == "--artcnn-c4f16-render"
        || arg == "--sr-lab-inspect"
        || arg == "--sr-lab-span-cpu-reference"
        || arg == "--sr-lab-span-gpu-reference"
        || arg == "--sr-lab-span-gpu-tiled-reference"
        || arg == "--sr-lab-span-session-bench"
}

impl CliCommand {
    fn run(self) -> Result<(), String> {
        match self {
            Self::PerfScan {
                path,
                report_path,
                target_long_edge,
                decode_strategy,
            } => crate::core::perf::run_perf_scan(
                &path,
                report_path.as_deref(),
                target_long_edge,
                decode_strategy,
            )
            .map_err(|error| format!("perf scan failed: {error}")),
            Self::QualityScan {
                path,
                target_long_edge,
                report_path,
            } => crate::core::quality::run_quality_scan(
                &path,
                target_long_edge,
                report_path.as_deref(),
            )
            .map_err(|error| format!("quality scan failed: {error}")),
            Self::EffectBench {
                path,
                target_long_edge,
                report_path,
            } => crate::core::effect_bench::run_effect_bench(
                &path,
                report_path.as_deref(),
                target_long_edge,
            )
            .map_err(|error| format!("effect bench failed: {error}")),
            Self::UpscaleBench {
                path,
                source_long_edge,
                target_long_edge,
                method_filter,
                max_pages,
                report_path,
            } => crate::core::upscale_bench::run_upscale_bench(
                &path,
                report_path.as_deref(),
                source_long_edge,
                target_long_edge,
                method_filter,
                max_pages,
            )
            .map_err(|error| format!("upscale bench failed: {error}")),
            Self::UpscaleQualityScan {
                path,
                source_long_edge,
                target_long_edge,
                method_filter,
                max_pages,
                report_path,
                visual_dir,
            } => crate::core::upscale_quality::run_upscale_quality_scan(
                &path,
                report_path.as_deref(),
                visual_dir.as_deref(),
                source_long_edge,
                target_long_edge,
                method_filter,
                max_pages,
            )
            .map_err(|error| format!("upscale quality scan failed: {error}")),
            Self::CunnyProbe {
                method,
                probe_edge,
                report_path,
            } => crate::core::cunny_probe::run_cunny_probe(method, probe_edge)
                .map_err(|error| format!("cunny probe failed: {error}"))
                .and_then(|report| {
                    crate::core::cunny_probe::print_cunny_probe_report(&report);
                    match report_path {
                        Some(path) => write_json_report(&path, &report),
                        None => Ok(()),
                    }
                }),
            Self::CunnyStageStats {
                image,
                method,
                long_edge,
                report_path,
            } => crate::core::cunny_stage_stats::run_cunny_stage_stats(&image, method, long_edge)
                .map_err(|error| format!("cunny stage stats failed: {error}"))
                .and_then(|report| {
                    crate::core::cunny_stage_stats::print_cunny_stage_report(&report);
                    match report_path {
                        Some(path) => write_json_report(&path, &report),
                        None => Ok(()),
                    }
                }),
            Self::GpuCopyBench {
                path,
                target_long_edge,
                iterations,
                max_pages,
                report_path,
            } => crate::core::gpu_copy_bench::run_gpu_copy_bench(
                &path,
                report_path.as_deref(),
                target_long_edge,
                iterations,
                max_pages,
            )
            .map_err(|error| format!("gpu copy bench failed: {error}")),
            Self::DecoderBench {
                path,
                iterations,
                max_pages,
                report_path,
            } => crate::core::decoder_bench::run_decoder_bench(
                &path,
                report_path.as_deref(),
                iterations,
                max_pages,
            )
            .map_err(|error| format!("decoder bench failed: {error}")),
            Self::OriginalRegionBench {
                path,
                page_index,
                region,
                iterations,
                report_path,
            } => crate::core::original_region_bench::run_original_region_bench(
                &path,
                report_path.as_deref(),
                page_index,
                region,
                iterations,
            )
            .map_err(|error| format!("original region bench failed: {error}")),
            Self::ArtcnnRender {
                variant,
                method,
                input_path,
                output_path,
            } => crate::core::upscale_bench::run_artcnn_render(
                variant,
                method,
                &input_path,
                &output_path,
            )
            .map_err(|error| format!("{} render failed: {error}", variant.label())),
            Self::UpscaleRender {
                method,
                input_path,
                output_path,
                output_size,
            } => crate::core::upscale_bench::run_upscale_render(
                method,
                &input_path,
                &output_path,
                output_size,
            )
            .map_err(|error| format!("{} render failed: {error}", method.label())),
            Self::SrLabInspect {
                manifest_path,
                report_path,
            } => crate::core::sr_lab::run_sr_lab_inspect(&manifest_path, report_path.as_deref())
                .map_err(|error| format!("SR Lab inspect failed: {error}")),
            Self::SrLabSpanCpuReference {
                manifest_path,
                input_path,
                long_edge,
                max_long_edge,
                output_path,
                report_path,
            } => crate::core::sr_lab::cpu::run_span_cpu_reference(
                &manifest_path,
                &input_path,
                long_edge,
                max_long_edge,
                output_path.as_deref(),
                report_path.as_deref(),
            )
            .map_err(|error| format!("SR Lab SPAN CPU reference failed: {error}")),
            Self::SrLabSpanGpuReference {
                manifest_path,
                input_path,
                long_edge,
                max_long_edge,
                output_path,
                report_path,
                compare_cpu,
            } => crate::core::sr_lab::gpu::run_span_gpu_reference(
                &manifest_path,
                &input_path,
                long_edge,
                max_long_edge,
                output_path.as_deref(),
                report_path.as_deref(),
                compare_cpu,
            )
            .map_err(|error| format!("SR Lab SPAN GPU reference failed: {error}")),
            Self::SrLabSpanGpuTiledReference {
                manifest_path,
                input_path,
                long_edge,
                max_long_edge,
                tile_edge,
                output_path,
                report_path,
                compare_cpu,
            } => crate::core::sr_lab::gpu::tiled::run_span_gpu_tiled_reference(
                &manifest_path,
                &input_path,
                long_edge,
                max_long_edge,
                tile_edge,
                output_path.as_deref(),
                report_path.as_deref(),
                compare_cpu,
            )
            .map_err(|error| format!("SR Lab SPAN GPU tiled reference failed: {error}")),
            Self::SrLabSpanSessionBench {
                manifest_path,
                input_path,
                long_edge,
                max_long_edge,
                warmups,
                iterations,
                report_path,
            } => crate::core::sr_lab::gpu::run_span_gpu_session_bench(
                &manifest_path,
                &input_path,
                long_edge,
                max_long_edge,
                warmups,
                iterations,
                report_path.as_deref(),
            )
            .map_err(|error| format!("SR Lab SPAN GPU session benchmark failed: {error}")),
        }
    }
}

fn parse_perf_scan(mut args: impl Iterator<Item = OsString>) -> Result<CliCommand, CliError> {
    let path = required_path(&mut args, "usage: suisuiview-cli --perf-scan <path>")?;
    let mut report_path = None;
    let mut target_long_edge = DEFAULT_TARGET_LONG_EDGE;
    let mut decode_strategy = DecodeStrategy::Auto;

    while let Some(arg) = args.next() {
        if arg == "--perf-report" {
            report_path = Some(required_path(&mut args, "--perf-report requires a path")?);
        } else if arg == "--perf-report-default" {
            report_path = Some(crate::core::perf::default_report_path());
        } else if arg == "--target-long-edge" {
            target_long_edge = required_u32(&mut args, "--target-long-edge")?;
        } else if arg == "--decode-strategy" {
            decode_strategy = args
                .next()
                .and_then(|value| DecodeStrategy::parse_cli(&value.to_string_lossy()))
                .ok_or_else(|| {
                    CliError::new("--decode-strategy requires one of: auto, image-crate")
                })?;
        } else {
            return Err(unknown_arg(arg));
        }
    }

    Ok(CliCommand::PerfScan {
        path,
        report_path,
        target_long_edge,
        decode_strategy,
    })
}

fn parse_quality_scan(mut args: impl Iterator<Item = OsString>) -> Result<CliCommand, CliError> {
    let path = required_path(&mut args, "usage: suisuiview-cli --quality-scan <path>")?;
    let mut target_long_edge = DEFAULT_TARGET_LONG_EDGE;
    let mut report_path = None;

    while let Some(arg) = args.next() {
        if arg == "--target-long-edge" {
            target_long_edge = required_u32(&mut args, "--target-long-edge")?;
        } else if arg == "--quality-report" {
            report_path = Some(required_path(
                &mut args,
                "--quality-report requires a path",
            )?);
        } else {
            return Err(unknown_arg(arg));
        }
    }

    Ok(CliCommand::QualityScan {
        path,
        target_long_edge,
        report_path,
    })
}

fn parse_effect_bench(mut args: impl Iterator<Item = OsString>) -> Result<CliCommand, CliError> {
    let path = required_path(&mut args, "usage: suisuiview-cli --effect-bench <path>")?;
    let mut target_long_edge = DEFAULT_TARGET_LONG_EDGE;
    let mut report_path = None;

    while let Some(arg) = args.next() {
        if arg == "--target-long-edge" {
            target_long_edge = required_u32(&mut args, "--target-long-edge")?;
        } else if arg == "--effect-report" {
            report_path = Some(required_path(&mut args, "--effect-report requires a path")?);
        } else if arg == "--effect-report-default" {
            report_path = Some(crate::core::effect_bench::default_effect_report_path());
        } else {
            return Err(unknown_arg(arg));
        }
    }

    Ok(CliCommand::EffectBench {
        path,
        target_long_edge,
        report_path,
    })
}

fn parse_upscale_bench(mut args: impl Iterator<Item = OsString>) -> Result<CliCommand, CliError> {
    let path = required_path(&mut args, "usage: suisuiview-cli --upscale-bench <path>")?;
    let mut target_long_edge = DEFAULT_TARGET_LONG_EDGE;
    let mut source_long_edge = None;
    let mut method_filter = None;
    let mut max_pages = None;
    let mut report_path = None;

    while let Some(arg) = args.next() {
        if arg == "--target-long-edge" {
            target_long_edge = required_u32(&mut args, "--target-long-edge")?;
        } else if arg == "--source-long-edge" {
            source_long_edge = Some(required_u32(&mut args, "--source-long-edge")?);
        } else if arg == "--upscale-method" {
            method_filter = Some(upscale_method_arg::required(&mut args, "--upscale-method")?);
        } else if arg == "--upscale-max-pages" {
            max_pages = Some(required_usize(&mut args, "--upscale-max-pages")?);
        } else if arg == "--upscale-report" {
            report_path = Some(required_path(
                &mut args,
                "--upscale-report requires a path",
            )?);
        } else if arg == "--upscale-report-default" {
            report_path = Some(crate::core::upscale_bench::default_upscale_report_path());
        } else {
            return Err(unknown_arg(arg));
        }
    }

    Ok(CliCommand::UpscaleBench {
        path,
        source_long_edge: source_long_edge
            .unwrap_or_else(|| (target_long_edge / 2).max(MIN_TARGET_LONG_EDGE)),
        target_long_edge,
        method_filter,
        max_pages,
        report_path,
    })
}

fn parse_cunny_probe(mut args: impl Iterator<Item = OsString>) -> Result<CliCommand, CliError> {
    let mut method = None;
    let mut probe_edge = crate::core::cunny_probe::DEFAULT_PROBE_EDGE;
    let mut report_path = None;
    while let Some(arg) = args.next() {
        if arg == "--probe-method" {
            method = Some(upscale_method_arg::required(&mut args, "--probe-method")?);
        } else if arg == "--probe-edge" {
            probe_edge = required_u32(&mut args, "--probe-edge")?;
        } else if arg == "--probe-report" {
            report_path = Some(required_path(&mut args, "--probe-report requires a path")?);
        } else if arg == "--probe-report-default" {
            report_path = Some(crate::core::cunny_probe::default_cunny_probe_report_path());
        } else {
            return Err(unknown_arg(arg));
        }
    }
    let method = method.ok_or_else(|| CliError::new("--cunny-probe requires --probe-method"))?;
    Ok(CliCommand::CunnyProbe {
        method,
        probe_edge,
        report_path,
    })
}

fn parse_cunny_stage_stats(
    mut args: impl Iterator<Item = OsString>,
) -> Result<CliCommand, CliError> {
    let image = required_path(
        &mut args,
        "usage: suisuiview-cli --cunny-stage-stats <image> --stage-method <token>",
    )?;
    let mut method = None;
    let mut long_edge = crate::core::cunny_stage_stats::DEFAULT_STAGE_LONG_EDGE;
    let mut report_path = None;

    while let Some(arg) = args.next() {
        if arg == "--stage-method" {
            method = Some(upscale_method_arg::required(&mut args, "--stage-method")?);
        } else if arg == "--stage-long-edge" {
            long_edge = required_u32(&mut args, "--stage-long-edge")?;
        } else if arg == "--stage-report" {
            report_path = Some(required_path(&mut args, "--stage-report requires a path")?);
        } else if arg == "--stage-report-default" {
            report_path = Some(crate::core::cunny_stage_stats::default_cunny_stage_report_path());
        } else {
            return Err(unknown_arg(arg));
        }
    }

    let method =
        method.ok_or_else(|| CliError::new("--cunny-stage-stats requires --stage-method"))?;
    Ok(CliCommand::CunnyStageStats {
        image,
        method,
        long_edge,
        report_path,
    })
}

fn write_json_report<T: serde::Serialize>(
    path: &std::path::Path,
    report: &T,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    let text =
        serde_json::to_string_pretty(report).map_err(|error| format!("stage report: {error}"))?;
    std::fs::write(path, text).map_err(|error| format!("{}: {error}", path.display()))?;
    println!("Report: {}", path.display());
    Ok(())
}

fn parse_upscale_quality_scan(
    mut args: impl Iterator<Item = OsString>,
) -> Result<CliCommand, CliError> {
    let path = required_path(
        &mut args,
        "usage: suisuiview-cli --upscale-quality-scan <path>",
    )?;
    let mut target_long_edge = DEFAULT_TARGET_LONG_EDGE;
    let mut source_long_edge = None;
    let mut method_filter = None;
    let mut max_pages = None;
    let mut report_path = None;
    let mut visual_dir = None;

    while let Some(arg) = args.next() {
        if arg == "--target-long-edge" {
            target_long_edge = required_u32(&mut args, "--target-long-edge")?;
        } else if arg == "--source-long-edge" {
            source_long_edge = Some(required_u32(&mut args, "--source-long-edge")?);
        } else if arg == "--upscale-quality-method" {
            method_filter = Some(upscale_method_arg::required(
                &mut args,
                "--upscale-quality-method",
            )?);
        } else if arg == "--upscale-quality-max-pages" {
            max_pages = Some(required_usize(&mut args, "--upscale-quality-max-pages")?);
        } else if arg == "--upscale-quality-report" {
            report_path = Some(required_path(
                &mut args,
                "--upscale-quality-report requires a path",
            )?);
        } else if arg == "--upscale-quality-report-default" {
            report_path = Some(crate::core::upscale_quality::default_upscale_quality_report_path());
        } else if arg == "--upscale-quality-visuals" {
            visual_dir = Some(required_path(
                &mut args,
                "--upscale-quality-visuals requires a directory",
            )?);
        } else {
            return Err(unknown_arg(arg));
        }
    }

    Ok(CliCommand::UpscaleQualityScan {
        path,
        source_long_edge: source_long_edge
            .unwrap_or_else(|| (target_long_edge / 2).max(MIN_TARGET_LONG_EDGE)),
        target_long_edge,
        method_filter,
        max_pages,
        report_path,
        visual_dir,
    })
}

fn required_path(
    args: &mut impl Iterator<Item = OsString>,
    message: &'static str,
) -> Result<PathBuf, CliError> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| CliError::new(message))
}

fn required_u32(
    args: &mut impl Iterator<Item = OsString>,
    flag: &'static str,
) -> Result<u32, CliError> {
    args.next()
        .and_then(|value| value.to_string_lossy().parse().ok())
        .ok_or_else(|| CliError::new(format!("{flag} requires a positive integer")))
}

fn required_usize(
    args: &mut impl Iterator<Item = OsString>,
    flag: &'static str,
) -> Result<usize, CliError> {
    args.next()
        .and_then(|value| value.to_string_lossy().parse().ok())
        .ok_or_else(|| CliError::new(format!("{flag} requires a positive integer")))
}

fn unknown_arg(arg: OsString) -> CliError {
    CliError::new(format!("unknown argument: {}", arg.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::{is_gui_cli_redirect_arg, parse_args, CliAction, CliCommand};
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn help_is_default_action() {
        assert!(matches!(parse_args(vec![]).unwrap(), CliAction::Help));
        assert!(matches!(
            parse_args(vec![OsString::from("--help")]).unwrap(),
            CliAction::Help
        ));
    }

    #[test]
    fn perf_scan_preserves_existing_flags() {
        let action = parse_args(vec![
            OsString::from("--perf-scan"),
            OsString::from("book.cbz"),
            OsString::from("--target-long-edge"),
            OsString::from("2048"),
            OsString::from("--perf-report"),
            OsString::from("report.json"),
        ])
        .unwrap();

        let CliAction::Command(CliCommand::PerfScan {
            path,
            report_path,
            target_long_edge,
            ..
        }) = action
        else {
            panic!("expected perf scan command");
        };

        assert_eq!(path, PathBuf::from("book.cbz"));
        assert_eq!(report_path, Some(PathBuf::from("report.json")));
        assert_eq!(target_long_edge, 2048);
    }

    #[test]
    fn upscale_source_long_edge_defaults_from_target() {
        let action = parse_args(vec![
            OsString::from("--upscale-bench"),
            OsString::from("book.cbz"),
            OsString::from("--target-long-edge"),
            OsString::from("4096"),
        ])
        .unwrap();

        let CliAction::Command(CliCommand::UpscaleBench {
            source_long_edge, ..
        }) = action
        else {
            panic!("expected upscale bench command");
        };

        assert_eq!(source_long_edge, 2048);
    }

    #[test]
    fn upscale_bench_accepts_method_and_max_pages() {
        let action = parse_args(vec![
            OsString::from("--upscale-bench"),
            OsString::from("book.cbz"),
            OsString::from("--upscale-method"),
            OsString::from("artcnn_c4f16"),
            OsString::from("--upscale-max-pages"),
            OsString::from("1"),
        ])
        .unwrap();

        let CliAction::Command(CliCommand::UpscaleBench {
            method_filter,
            max_pages,
            ..
        }) = action
        else {
            panic!("expected upscale bench command");
        };
        assert_eq!(
            method_filter,
            Some(crate::core::state::WgpuUpscaleMethod::WgslArtcnnC4F16)
        );
        assert_eq!(max_pages, Some(1));
    }

    #[test]
    fn upscale_quality_accepts_method_and_max_pages() {
        let action = parse_args(vec![
            OsString::from("--upscale-quality-scan"),
            OsString::from("book.cbz"),
            OsString::from("--upscale-quality-method"),
            OsString::from("srlab_span_x2"),
            OsString::from("--upscale-quality-max-pages"),
            OsString::from("2"),
        ])
        .unwrap();

        let CliAction::Command(CliCommand::UpscaleQualityScan {
            method_filter,
            max_pages,
            ..
        }) = action
        else {
            panic!("expected upscale quality scan command");
        };
        assert_eq!(
            method_filter,
            Some(crate::core::state::WgpuUpscaleMethod::WgslSrLabSpanX2)
        );
        assert_eq!(max_pages, Some(2));
    }

    #[test]
    fn upscale_quality_method_reports_its_flag_name_when_missing_token() {
        let error = parse_args(vec![
            OsString::from("--upscale-quality-scan"),
            OsString::from("book.cbz"),
            OsString::from("--upscale-quality-method"),
        ])
        .unwrap_err();

        assert!(error
            .message()
            .contains("--upscale-quality-method requires a token"));
    }

    #[test]
    fn gui_redirect_matches_existing_cli_entrypoints() {
        assert!(is_gui_cli_redirect_arg(&OsString::from("--perf-scan")));
        assert!(is_gui_cli_redirect_arg(&OsString::from("--gpu-copy-bench")));
        assert!(is_gui_cli_redirect_arg(&OsString::from("--help")));
        assert!(!is_gui_cli_redirect_arg(&OsString::from(
            "C:/books/book.cbz"
        )));
    }

    #[test]
    fn gpu_copy_bench_preserves_iteration_flags() {
        let action = parse_args(vec![
            OsString::from("--gpu-copy-bench"),
            OsString::from("book.cbz"),
            OsString::from("--target-long-edge"),
            OsString::from("4096"),
            OsString::from("--gpu-copy-iterations"),
            OsString::from("7"),
            OsString::from("--gpu-copy-max-pages"),
            OsString::from("3"),
            OsString::from("--gpu-copy-report"),
            OsString::from("gpu-copy.json"),
        ])
        .unwrap();

        let CliAction::Command(CliCommand::GpuCopyBench {
            path,
            target_long_edge,
            iterations,
            max_pages,
            report_path,
        }) = action
        else {
            panic!("expected gpu copy bench command");
        };

        assert_eq!(path, PathBuf::from("book.cbz"));
        assert_eq!(target_long_edge, 4096);
        assert_eq!(iterations, 7);
        assert_eq!(max_pages, 3);
        assert_eq!(report_path, Some(PathBuf::from("gpu-copy.json")));
    }
}
