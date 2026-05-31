use super::{required_path, required_u32, required_usize, unknown_arg, CliCommand, CliError};
use std::ffi::OsString;

pub(super) fn parse_inspect(
    mut args: impl Iterator<Item = OsString>,
) -> Result<CliCommand, CliError> {
    let manifest_path = required_path(
        &mut args,
        "usage: suisuiview-cli --sr-lab-inspect <manifest.json>",
    )?;
    let mut report_path = None;

    while let Some(arg) = args.next() {
        if arg == "--sr-lab-report" {
            report_path = Some(required_path(&mut args, "--sr-lab-report requires a path")?);
        } else if arg == "--sr-lab-report-default" {
            report_path = Some(crate::core::sr_lab::default_sr_lab_report_path());
        } else {
            return Err(unknown_arg(arg));
        }
    }

    Ok(CliCommand::SrLabInspect {
        manifest_path,
        report_path,
    })
}

pub(super) fn parse_span_cpu_reference(
    args: impl Iterator<Item = OsString>,
) -> Result<CliCommand, CliError> {
    let args = parse_span_reference_args(args, SpanReferenceKind::Cpu)?;
    Ok(CliCommand::SrLabSpanCpuReference {
        manifest_path: args.manifest_path,
        input_path: args.input_path,
        long_edge: args.long_edge,
        output_path: args.output_path,
        report_path: args.report_path,
    })
}

pub(super) fn parse_span_gpu_reference(
    args: impl Iterator<Item = OsString>,
) -> Result<CliCommand, CliError> {
    let args = parse_span_reference_args(args, SpanReferenceKind::Gpu)?;
    Ok(CliCommand::SrLabSpanGpuReference {
        manifest_path: args.manifest_path,
        input_path: args.input_path,
        long_edge: args.long_edge,
        output_path: args.output_path,
        report_path: args.report_path,
        compare_cpu: args.compare_cpu,
    })
}

pub(super) fn parse_span_session_bench(
    mut args: impl Iterator<Item = OsString>,
) -> Result<CliCommand, CliError> {
    let usage = "usage: suisuiview-cli --sr-lab-span-session-bench <manifest.json> <image>";
    let manifest_path = required_path(&mut args, usage)?;
    let input_path = required_path(&mut args, usage)?;
    let mut long_edge = None;
    let mut warmups = crate::core::sr_lab::gpu::DEFAULT_SPAN_SESSION_WARMUPS;
    let mut iterations = crate::core::sr_lab::gpu::DEFAULT_SPAN_SESSION_ITERATIONS;
    let mut report_path = None;

    while let Some(arg) = args.next() {
        if arg == "--sr-lab-long-edge" {
            long_edge = Some(required_u32(&mut args, "--sr-lab-long-edge")?);
        } else if arg == "--sr-lab-warmups" {
            warmups = required_usize(&mut args, "--sr-lab-warmups")?;
        } else if arg == "--sr-lab-iterations" {
            iterations = required_usize(&mut args, "--sr-lab-iterations")?;
            if iterations == 0 {
                return Err(CliError::new(
                    "--sr-lab-iterations requires a positive integer",
                ));
            }
        } else if arg == "--sr-lab-report" {
            report_path = Some(required_path(&mut args, "--sr-lab-report requires a path")?);
        } else if arg == "--sr-lab-report-default" {
            report_path = Some(crate::core::sr_lab::default_span_gpu_session_report_path());
        } else {
            return Err(unknown_arg(arg));
        }
    }

    Ok(CliCommand::SrLabSpanSessionBench {
        manifest_path,
        input_path,
        long_edge,
        warmups,
        iterations,
        report_path,
    })
}

struct SpanReferenceArgs {
    manifest_path: std::path::PathBuf,
    input_path: std::path::PathBuf,
    long_edge: Option<u32>,
    output_path: Option<std::path::PathBuf>,
    report_path: Option<std::path::PathBuf>,
    compare_cpu: bool,
}

#[derive(Clone, Copy)]
enum SpanReferenceKind {
    Cpu,
    Gpu,
}

impl SpanReferenceKind {
    fn command(self) -> &'static str {
        match self {
            Self::Cpu => "--sr-lab-span-cpu-reference",
            Self::Gpu => "--sr-lab-span-gpu-reference",
        }
    }
}

fn parse_span_reference_args(
    mut args: impl Iterator<Item = OsString>,
    kind: SpanReferenceKind,
) -> Result<SpanReferenceArgs, CliError> {
    let usage = match kind {
        SpanReferenceKind::Cpu => {
            "usage: suisuiview-cli --sr-lab-span-cpu-reference <manifest.json> <image>"
        }
        SpanReferenceKind::Gpu => {
            "usage: suisuiview-cli --sr-lab-span-gpu-reference <manifest.json> <image>"
        }
    };
    let manifest_path = required_path(&mut args, usage)?;
    let input_path = required_path(&mut args, usage)?;
    let mut long_edge = None;
    let mut output_path = None;
    let mut report_path = None;
    let mut compare_cpu = false;

    while let Some(arg) = args.next() {
        if arg == "--sr-lab-long-edge" {
            long_edge = Some(required_u32(&mut args, "--sr-lab-long-edge")?);
        } else if arg == "--sr-lab-output" {
            output_path = Some(required_path(&mut args, "--sr-lab-output requires a path")?);
        } else if arg == "--sr-lab-report" {
            report_path = Some(required_path(&mut args, "--sr-lab-report requires a path")?);
        } else if arg == "--sr-lab-report-default" {
            report_path = Some(default_span_reference_report_path(kind));
        } else if arg == "--sr-lab-compare-cpu" && matches!(kind, SpanReferenceKind::Gpu) {
            compare_cpu = true;
        } else if arg == "--sr-lab-compare-cpu" {
            return Err(CliError::new(format!(
                "--sr-lab-compare-cpu is only valid with {}",
                SpanReferenceKind::Gpu.command()
            )));
        } else {
            return Err(unknown_arg(arg));
        }
    }

    Ok(SpanReferenceArgs {
        manifest_path,
        input_path,
        long_edge,
        output_path,
        report_path,
        compare_cpu,
    })
}

fn default_span_reference_report_path(kind: SpanReferenceKind) -> std::path::PathBuf {
    match kind {
        SpanReferenceKind::Cpu => crate::core::sr_lab::default_span_cpu_reference_report_path(),
        SpanReferenceKind::Gpu => crate::core::sr_lab::default_span_gpu_reference_report_path(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{parse_args, CliAction, CliCommand};
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn sr_lab_inspect_preserves_report_flag() {
        let action = parse_args(vec![
            OsString::from("--sr-lab-inspect"),
            OsString::from("model.json"),
            OsString::from("--sr-lab-report"),
            OsString::from("report.json"),
        ])
        .unwrap();

        let CliAction::Command(CliCommand::SrLabInspect {
            manifest_path,
            report_path,
        }) = action
        else {
            panic!("expected SR Lab inspect command");
        };

        assert_eq!(manifest_path, PathBuf::from("model.json"));
        assert_eq!(report_path, Some(PathBuf::from("report.json")));
    }

    #[test]
    fn sr_lab_span_cpu_reference_preserves_flags() {
        let action = parse_args(vec![
            OsString::from("--sr-lab-span-cpu-reference"),
            OsString::from("manifest.json"),
            OsString::from("input.png"),
            OsString::from("--sr-lab-long-edge"),
            OsString::from("32"),
            OsString::from("--sr-lab-output"),
            OsString::from("out.png"),
            OsString::from("--sr-lab-report"),
            OsString::from("report.json"),
        ])
        .unwrap();

        let CliAction::Command(CliCommand::SrLabSpanCpuReference {
            manifest_path,
            input_path,
            long_edge,
            output_path,
            report_path,
        }) = action
        else {
            panic!("expected SPAN CPU reference command");
        };

        assert_eq!(manifest_path, PathBuf::from("manifest.json"));
        assert_eq!(input_path, PathBuf::from("input.png"));
        assert_eq!(long_edge, Some(32));
        assert_eq!(output_path, Some(PathBuf::from("out.png")));
        assert_eq!(report_path, Some(PathBuf::from("report.json")));
    }

    #[test]
    fn sr_lab_span_gpu_reference_preserves_flags() {
        let action = parse_args(vec![
            OsString::from("--sr-lab-span-gpu-reference"),
            OsString::from("manifest.json"),
            OsString::from("input.png"),
            OsString::from("--sr-lab-long-edge"),
            OsString::from("32"),
            OsString::from("--sr-lab-output"),
            OsString::from("out.png"),
            OsString::from("--sr-lab-report"),
            OsString::from("report.json"),
            OsString::from("--sr-lab-compare-cpu"),
        ])
        .unwrap();

        let CliAction::Command(CliCommand::SrLabSpanGpuReference {
            manifest_path,
            input_path,
            long_edge,
            output_path,
            report_path,
            compare_cpu,
        }) = action
        else {
            panic!("expected SPAN GPU reference command");
        };

        assert_eq!(manifest_path, PathBuf::from("manifest.json"));
        assert_eq!(input_path, PathBuf::from("input.png"));
        assert_eq!(long_edge, Some(32));
        assert_eq!(output_path, Some(PathBuf::from("out.png")));
        assert_eq!(report_path, Some(PathBuf::from("report.json")));
        assert!(compare_cpu);
    }

    #[test]
    fn sr_lab_span_session_bench_preserves_flags() {
        let action = parse_args(vec![
            OsString::from("--sr-lab-span-session-bench"),
            OsString::from("manifest.json"),
            OsString::from("input.png"),
            OsString::from("--sr-lab-long-edge"),
            OsString::from("64"),
            OsString::from("--sr-lab-warmups"),
            OsString::from("2"),
            OsString::from("--sr-lab-iterations"),
            OsString::from("7"),
            OsString::from("--sr-lab-report"),
            OsString::from("report.json"),
        ])
        .unwrap();

        let CliAction::Command(CliCommand::SrLabSpanSessionBench {
            manifest_path,
            input_path,
            long_edge,
            warmups,
            iterations,
            report_path,
        }) = action
        else {
            panic!("expected SPAN session benchmark command");
        };

        assert_eq!(manifest_path, PathBuf::from("manifest.json"));
        assert_eq!(input_path, PathBuf::from("input.png"));
        assert_eq!(long_edge, Some(64));
        assert_eq!(warmups, 2);
        assert_eq!(iterations, 7);
        assert_eq!(report_path, Some(PathBuf::from("report.json")));
    }

    #[test]
    fn sr_lab_span_session_bench_rejects_zero_iterations() {
        let error = parse_args(vec![
            OsString::from("--sr-lab-span-session-bench"),
            OsString::from("manifest.json"),
            OsString::from("input.png"),
            OsString::from("--sr-lab-iterations"),
            OsString::from("0"),
        ])
        .unwrap_err();

        assert_eq!(
            error.message(),
            "--sr-lab-iterations requires a positive integer"
        );
    }
}
