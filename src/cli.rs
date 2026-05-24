use crate::core::worker::{DecodeStrategy, DEFAULT_TARGET_LONG_EDGE, MIN_TARGET_LONG_EDGE};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

const CLI_NAME: &str = "suisuiview-cli";

pub const REDIRECT_MESSAGE: &str =
    "CLI 명령은 suisuiview-cli를 사용하세요.\n예: suisuiview-cli --perf-scan <path>";

const HELP: &str = "\
SuiSuiView CLI

Usage:
  suisuiview-cli --perf-scan <path> [--perf-report <report.json>] [--perf-report-default] [--target-long-edge <px>] [--decode-strategy auto|image-crate]
  suisuiview-cli --quality-scan <path> [--target-long-edge <px>] [--quality-report <report.json>]
  suisuiview-cli --effect-bench <path> [--target-long-edge <px>] [--effect-report <report.json>] [--effect-report-default]
  suisuiview-cli --upscale-bench <path> [--source-long-edge <px>] [--target-long-edge <px>] [--upscale-report <report.json>] [--upscale-report-default]
  suisuiview-cli --upscale-quality-scan <path> [--source-long-edge <px>] [--target-long-edge <px>] [--upscale-quality-report <report.json>] [--upscale-quality-report-default] [--upscale-quality-visuals <dir>]
  suisuiview-cli --gpu-copy-bench <path> [--target-long-edge <px>] [--gpu-copy-iterations <count>] [--gpu-copy-max-pages <count>] [--gpu-copy-report <report.json>] [--gpu-copy-report-default]

Options:
  -h, --help    Show this help.
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
        report_path: Option<PathBuf>,
    },
    UpscaleQualityScan {
        path: PathBuf,
        source_long_edge: u32,
        target_long_edge: u32,
        report_path: Option<PathBuf>,
        visual_dir: Option<PathBuf>,
    },
    GpuCopyBench {
        path: PathBuf,
        target_long_edge: u32,
        iterations: usize,
        max_pages: usize,
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
    if first == "--gpu-copy-bench" {
        return parse_gpu_copy_bench(args).map(CliAction::Command);
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
        || arg == "--gpu-copy-bench"
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
                report_path,
            } => crate::core::upscale_bench::run_upscale_bench(
                &path,
                report_path.as_deref(),
                source_long_edge,
                target_long_edge,
            )
            .map_err(|error| format!("upscale bench failed: {error}")),
            Self::UpscaleQualityScan {
                path,
                source_long_edge,
                target_long_edge,
                report_path,
                visual_dir,
            } => crate::core::upscale_quality::run_upscale_quality_scan(
                &path,
                report_path.as_deref(),
                visual_dir.as_deref(),
                source_long_edge,
                target_long_edge,
            )
            .map_err(|error| format!("upscale quality scan failed: {error}")),
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
    let mut report_path = None;

    while let Some(arg) = args.next() {
        if arg == "--target-long-edge" {
            target_long_edge = required_u32(&mut args, "--target-long-edge")?;
        } else if arg == "--source-long-edge" {
            source_long_edge = Some(required_u32(&mut args, "--source-long-edge")?);
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
        report_path,
    })
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
    let mut report_path = None;
    let mut visual_dir = None;

    while let Some(arg) = args.next() {
        if arg == "--target-long-edge" {
            target_long_edge = required_u32(&mut args, "--target-long-edge")?;
        } else if arg == "--source-long-edge" {
            source_long_edge = Some(required_u32(&mut args, "--source-long-edge")?);
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
        report_path,
        visual_dir,
    })
}

fn parse_gpu_copy_bench(mut args: impl Iterator<Item = OsString>) -> Result<CliCommand, CliError> {
    let path = required_path(&mut args, "usage: suisuiview-cli --gpu-copy-bench <path>")?;
    let mut target_long_edge = DEFAULT_TARGET_LONG_EDGE;
    let mut iterations = crate::core::gpu_copy_bench::default_gpu_copy_iterations();
    let mut max_pages = crate::core::gpu_copy_bench::default_gpu_copy_max_pages();
    let mut report_path = None;

    while let Some(arg) = args.next() {
        if arg == "--target-long-edge" {
            target_long_edge = required_u32(&mut args, "--target-long-edge")?;
        } else if arg == "--gpu-copy-iterations" {
            iterations = required_usize(&mut args, "--gpu-copy-iterations")?;
        } else if arg == "--gpu-copy-max-pages" {
            max_pages = required_usize(&mut args, "--gpu-copy-max-pages")?;
        } else if arg == "--gpu-copy-report" {
            report_path = Some(required_path(
                &mut args,
                "--gpu-copy-report requires a path",
            )?);
        } else if arg == "--gpu-copy-report-default" {
            report_path = Some(crate::core::gpu_copy_bench::default_gpu_copy_report_path());
        } else {
            return Err(unknown_arg(arg));
        }
    }

    Ok(CliCommand::GpuCopyBench {
        path,
        target_long_edge,
        iterations: iterations.max(1),
        max_pages: max_pages.max(1),
        report_path,
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
