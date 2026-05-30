use super::{required_path, required_u32, required_usize, unknown_arg, CliCommand, CliError};
use crate::core::worker::DEFAULT_TARGET_LONG_EDGE;
use std::ffi::OsString;

pub(super) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<CliCommand, CliError> {
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

#[cfg(test)]
mod tests {
    use super::super::{parse_args, CliAction, CliCommand};
    use std::ffi::OsString;
    use std::path::PathBuf;

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
