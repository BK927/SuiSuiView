use super::{required_path, unknown_arg, upscale_method_arg, CliCommand, CliError};
use std::ffi::OsString;

pub(super) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<CliCommand, CliError> {
    let method = upscale_method_arg::required(&mut args, "--upscale-render")?;
    let input_path = required_path(
        &mut args,
        "usage: suisuiview-cli --upscale-render <method> <image> --upscale-output <png> --upscale-output-size <width>x<height>",
    )?;
    let mut output_path = None;
    let mut output_size = None;

    while let Some(arg) = args.next() {
        if arg == "--upscale-output" {
            output_path = Some(required_path(
                &mut args,
                "--upscale-output requires a PNG path",
            )?);
        } else if arg == "--upscale-output-size" {
            output_size = Some(required_output_size(&mut args, "--upscale-output-size")?);
        } else {
            return Err(unknown_arg(arg));
        }
    }

    Ok(CliCommand::UpscaleRender {
        method,
        input_path,
        output_path: output_path
            .ok_or_else(|| CliError::new("--upscale-render requires --upscale-output <png>"))?,
        output_size: output_size.ok_or_else(|| {
            CliError::new("--upscale-render requires --upscale-output-size <width>x<height>")
        })?,
    })
}

fn required_output_size(
    args: &mut impl Iterator<Item = OsString>,
    flag: &'static str,
) -> Result<[usize; 2], CliError> {
    let value = args
        .next()
        .ok_or_else(|| CliError::new(format!("{flag} requires <width>x<height>")))?;
    let value = value.to_string_lossy();
    let Some((width, height)) = value.split_once('x').or_else(|| value.split_once('X')) else {
        return Err(CliError::new(format!(
            "{flag} requires <width>x<height>, got {value}"
        )));
    };
    let width = width
        .parse::<usize>()
        .ok()
        .filter(|width| *width > 0)
        .ok_or_else(|| CliError::new(format!("{flag} width must be positive")))?;
    let height = height
        .parse::<usize>()
        .ok()
        .filter(|height| *height > 0)
        .ok_or_else(|| CliError::new(format!("{flag} height must be positive")))?;
    Ok([width, height])
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::cli::CliCommand;
    use crate::core::state::DisplayUpscaler;
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn upscale_render_parses_method_output_and_size() {
        let command = parse(
            [
                "anime4k_v32_cnn_x2_s",
                "source.png",
                "--upscale-output",
                "out.png",
                "--upscale-output-size",
                "1280x720",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .unwrap();

        let CliCommand::UpscaleRender {
            method,
            input_path,
            output_path,
            output_size,
        } = command
        else {
            panic!("expected upscale render command");
        };
        assert_eq!(method, DisplayUpscaler::WgslAnime4kV32CnnX2S);
        assert_eq!(input_path, PathBuf::from("source.png"));
        assert_eq!(output_path, PathBuf::from("out.png"));
        assert_eq!(output_size, [1280, 720]);
    }
}
