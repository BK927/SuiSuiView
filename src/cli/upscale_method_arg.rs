use super::CliError;
use std::ffi::OsString;

pub(super) fn required(
    args: &mut impl Iterator<Item = OsString>,
    flag: &'static str,
) -> Result<crate::core::state::WgpuUpscaleMethod, CliError> {
    let token = args
        .next()
        .ok_or_else(|| CliError::new(format!("{flag} requires a token")))?;
    let token = token.to_string_lossy();
    crate::core::state::WgpuUpscaleMethod::GPU_METHODS
        .iter()
        .copied()
        .find(|method| method.token() == token)
        .ok_or_else(|| CliError::new(format!("unknown upscale method token: {token}")))
}
