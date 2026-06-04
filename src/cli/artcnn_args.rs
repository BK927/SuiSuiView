use super::{required_path, unknown_arg, CliCommand, CliError};
use crate::core::artcnn::ArtcnnVariant;
use crate::core::state::WgpuUpscaleMethod;
use std::ffi::OsString;

pub(super) fn parse_c4f16(
    mut args: impl Iterator<Item = OsString>,
) -> Result<CliCommand, CliError> {
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

    Ok(CliCommand::ArtcnnRender {
        variant: ArtcnnVariant::C4F16,
        method: WgpuUpscaleMethod::WgslArtcnnC4F16,
        input_path,
        output_path: output_path
            .ok_or_else(|| CliError::new("--artcnn-c4f16-render requires --artcnn-output <png>"))?,
    })
}

pub(super) fn parse_variant(
    mut args: impl Iterator<Item = OsString>,
) -> Result<CliCommand, CliError> {
    let variant_token = args.next().ok_or_else(|| {
        CliError::new(
            "usage: suisuiview-cli --artcnn-render <variant> <image> --artcnn-output <png>",
        )
    })?;
    let (variant, method) =
        parse_artcnn_variant(&variant_token.to_string_lossy()).ok_or_else(|| {
            CliError::new(format!(
                "unknown ArtCNN variant: {}",
                variant_token.to_string_lossy()
            ))
        })?;
    let input_path = required_path(
        &mut args,
        "usage: suisuiview-cli --artcnn-render <variant> <image> --artcnn-output <png>",
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

    Ok(CliCommand::ArtcnnRender {
        variant,
        method,
        input_path,
        output_path: output_path
            .ok_or_else(|| CliError::new("--artcnn-render requires --artcnn-output <png>"))?,
    })
}

fn parse_artcnn_variant(token: &str) -> Option<(ArtcnnVariant, WgpuUpscaleMethod)> {
    match token {
        "artcnn_c4f16" | "c4f16" => {
            Some((ArtcnnVariant::C4F16, WgpuUpscaleMethod::WgslArtcnnC4F16))
        }
        "artcnn_c4f16_dn" | "c4f16_dn" => {
            Some((ArtcnnVariant::C4F16Dn, WgpuUpscaleMethod::WgslArtcnnC4F16Dn))
        }
        "artcnn_c4f16_ds" | "c4f16_ds" => {
            Some((ArtcnnVariant::C4F16Ds, WgpuUpscaleMethod::WgslArtcnnC4F16Ds))
        }
        "artcnn_c4f32" | "c4f32" => {
            Some((ArtcnnVariant::C4F32, WgpuUpscaleMethod::WgslArtcnnC4F32))
        }
        "artcnn_c4f32_dn" | "c4f32_dn" => {
            Some((ArtcnnVariant::C4F32Dn, WgpuUpscaleMethod::WgslArtcnnC4F32Dn))
        }
        "artcnn_c4f32_ds" | "c4f32_ds" => {
            Some((ArtcnnVariant::C4F32Ds, WgpuUpscaleMethod::WgslArtcnnC4F32Ds))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{parse_args, CliAction, CliCommand};
    use crate::core::artcnn::ArtcnnVariant;
    use crate::core::state::WgpuUpscaleMethod;
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

        let CliAction::Command(CliCommand::ArtcnnRender {
            variant,
            method,
            input_path,
            output_path,
        }) = action
        else {
            panic!("expected ArtCNN C4F16 render command");
        };
        assert_eq!(variant, ArtcnnVariant::C4F16);
        assert_eq!(method, WgpuUpscaleMethod::WgslArtcnnC4F16);
        assert_eq!(input_path, PathBuf::from("source.png"));
        assert_eq!(output_path, PathBuf::from("out.png"));
    }

    #[test]
    fn artcnn_render_parses_all_variant_tokens() {
        for (token, expected_variant, expected_method) in [
            (
                "artcnn_c4f16_dn",
                ArtcnnVariant::C4F16Dn,
                WgpuUpscaleMethod::WgslArtcnnC4F16Dn,
            ),
            (
                "artcnn_c4f16_ds",
                ArtcnnVariant::C4F16Ds,
                WgpuUpscaleMethod::WgslArtcnnC4F16Ds,
            ),
            (
                "artcnn_c4f32",
                ArtcnnVariant::C4F32,
                WgpuUpscaleMethod::WgslArtcnnC4F32,
            ),
            (
                "artcnn_c4f32_dn",
                ArtcnnVariant::C4F32Dn,
                WgpuUpscaleMethod::WgslArtcnnC4F32Dn,
            ),
            (
                "artcnn_c4f32_ds",
                ArtcnnVariant::C4F32Ds,
                WgpuUpscaleMethod::WgslArtcnnC4F32Ds,
            ),
        ] {
            let action = parse_args(vec![
                OsString::from("--artcnn-render"),
                OsString::from(token),
                OsString::from("source.png"),
                OsString::from("--artcnn-output"),
                OsString::from("out.png"),
            ])
            .unwrap();

            let CliAction::Command(CliCommand::ArtcnnRender {
                variant, method, ..
            }) = action
            else {
                panic!("expected ArtCNN render command");
            };
            assert_eq!(variant, expected_variant);
            assert_eq!(method, expected_method);
        }
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
