use super::{required_path, unknown_arg, CliCommand, CliError};
use std::ffi::OsString;

pub(super) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<CliCommand, CliError> {
    let input_path = required_path(
        &mut args,
        "usage: suisuiview-cli --artcnn-c4f16-render <image> --artcnn-output <png>",
    )?;
    let mut output_path = None;

    while let Some(arg) = args.next() {
        if arg == "--artcnn-output" {
            output_path = Some(required_path(
                &mut args,
                "--artcnn-output requires a PNG output path",
            )?);
        } else {
            return Err(unknown_arg(arg));
        }
    }

    Ok(CliCommand::ArtcnnC4F16Render {
        input_path,
        output_path: output_path
            .ok_or_else(|| CliError::new("--artcnn-c4f16-render requires --artcnn-output <png>"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{parse_args, CliAction, CliCommand};
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn artcnn_c4f16_render_parses_output() {
        let action = parse_args(vec![
            OsString::from("--artcnn-c4f16-render"),
            OsString::from("source.png"),
            OsString::from("--artcnn-output"),
            OsString::from("out.png"),
        ])
        .unwrap();

        let CliAction::Command(CliCommand::ArtcnnC4F16Render {
            input_path,
            output_path,
        }) = action
        else {
            panic!("expected ArtCNN C4F16 render command");
        };
        assert_eq!(input_path, PathBuf::from("source.png"));
        assert_eq!(output_path, PathBuf::from("out.png"));
    }

    #[test]
    fn artcnn_c4f16_render_requires_output() {
        let error = parse_args(vec![
            OsString::from("--artcnn-c4f16-render"),
            OsString::from("source.png"),
        ])
        .unwrap_err();

        assert!(error
            .message()
            .contains("--artcnn-c4f16-render requires --artcnn-output"));
    }
}
