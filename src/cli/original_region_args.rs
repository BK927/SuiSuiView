use super::{required_path, required_usize, unknown_arg, CliCommand, CliError};
use crate::core::worker::OriginalRegion;
use std::ffi::OsString;

pub(super) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<CliCommand, CliError> {
    let path = required_path(
        &mut args,
        "usage: suisuiview-cli --original-region-bench <path> --region <x,y,width,height>",
    )?;
    let mut page_index = 0usize;
    let mut region = None;
    let mut iterations = 5usize;
    let mut report_path = None;

    while let Some(arg) = args.next() {
        if arg == "--page-index" {
            page_index = required_usize(&mut args, "--page-index")?;
        } else if arg == "--region" {
            region = Some(required_region(&mut args, "--region")?);
        } else if arg == "--region-iterations" {
            iterations = required_usize(&mut args, "--region-iterations")?.max(1);
        } else if arg == "--region-report" {
            report_path = Some(required_path(&mut args, "--region-report requires a path")?);
        } else if arg == "--region-report-default" {
            report_path =
                Some(crate::core::original_region_bench::default_original_region_report_path());
        } else {
            return Err(unknown_arg(arg));
        }
    }

    Ok(CliCommand::OriginalRegionBench {
        path,
        page_index,
        region: region.ok_or_else(|| CliError::new("--region is required"))?,
        iterations,
        report_path,
    })
}

fn required_region(
    args: &mut impl Iterator<Item = OsString>,
    flag: &'static str,
) -> Result<OriginalRegion, CliError> {
    let value = args
        .next()
        .ok_or_else(|| CliError::new(format!("{flag} requires x,y,width,height")))?;
    parse_region(&value.to_string_lossy())
        .ok_or_else(|| CliError::new(format!("{flag} requires x,y,width,height")))
}

fn parse_region(value: &str) -> Option<OriginalRegion> {
    let mut parts = value.split(',');
    let region = OriginalRegion {
        x: parts.next()?.parse().ok()?,
        y: parts.next()?.parse().ok()?,
        width: parts.next()?.parse().ok()?,
        height: parts.next()?.parse().ok()?,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(region)
}

#[cfg(test)]
mod tests {
    use super::super::{parse_args, CliAction, CliCommand};
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn original_region_bench_parses_region_and_iterations() {
        let action = parse_args(vec![
            OsString::from("--original-region-bench"),
            OsString::from("book.cbz"),
            OsString::from("--page-index"),
            OsString::from("2"),
            OsString::from("--region"),
            OsString::from("10,20,300,400"),
            OsString::from("--region-iterations"),
            OsString::from("9"),
            OsString::from("--region-report"),
            OsString::from("region.json"),
        ])
        .unwrap();

        let CliAction::Command(CliCommand::OriginalRegionBench {
            path,
            page_index,
            region,
            iterations,
            report_path,
        }) = action
        else {
            panic!("expected original region bench command");
        };

        assert_eq!(path, PathBuf::from("book.cbz"));
        assert_eq!(page_index, 2);
        assert_eq!(region.x, 10);
        assert_eq!(region.y, 20);
        assert_eq!(region.width, 300);
        assert_eq!(region.height, 400);
        assert_eq!(iterations, 9);
        assert_eq!(report_path, Some(PathBuf::from("region.json")));
    }

    #[test]
    fn original_region_bench_requires_region() {
        let error = parse_args(vec![
            OsString::from("--original-region-bench"),
            OsString::from("book.cbz"),
        ])
        .unwrap_err();

        assert!(error.message().contains("--region is required"));
    }
}
