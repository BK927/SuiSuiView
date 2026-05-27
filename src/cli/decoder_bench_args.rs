use super::{required_path, required_usize, unknown_arg, CliCommand, CliError};
use std::ffi::OsString;

pub(super) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<CliCommand, CliError> {
    let path = required_path(&mut args, "usage: suisuiview-cli --decoder-bench <path>")?;
    let mut iterations = crate::core::decoder_bench::DEFAULT_DECODER_BENCH_ITERATIONS;
    let mut max_pages = crate::core::decoder_bench::DEFAULT_DECODER_BENCH_MAX_PAGES;
    let mut report_path = None;

    while let Some(arg) = args.next() {
        if arg == "--decoder-iterations" {
            iterations = required_usize(&mut args, "--decoder-iterations")?;
        } else if arg == "--decoder-max-pages" {
            max_pages = required_usize(&mut args, "--decoder-max-pages")?;
        } else if arg == "--decoder-report" {
            report_path = Some(required_path(
                &mut args,
                "--decoder-report requires a path",
            )?);
        } else if arg == "--decoder-report-default" {
            report_path = Some(crate::core::decoder_bench::default_decoder_report_path());
        } else {
            return Err(unknown_arg(arg));
        }
    }

    Ok(CliCommand::DecoderBench {
        path,
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
    fn decoder_bench_preserves_iteration_flags() {
        let action = parse_args(vec![
            OsString::from("--decoder-bench"),
            OsString::from("book.cbz"),
            OsString::from("--decoder-iterations"),
            OsString::from("5"),
            OsString::from("--decoder-max-pages"),
            OsString::from("4"),
            OsString::from("--decoder-report"),
            OsString::from("decoder.json"),
        ])
        .unwrap();

        let CliAction::Command(CliCommand::DecoderBench {
            path,
            iterations,
            max_pages,
            report_path,
        }) = action
        else {
            panic!("expected decoder bench command");
        };

        assert_eq!(path, PathBuf::from("book.cbz"));
        assert_eq!(iterations, 5);
        assert_eq!(max_pages, 4);
        assert_eq!(report_path, Some(PathBuf::from("decoder.json")));
    }
}
