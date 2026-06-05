use std::time::Instant;

use super::args::input_path_from_args;
use super::handoff::run_handoff_probe;
use super::handoff_image::run_image_handoff_probe;

pub(crate) fn try_run_handoff_mode(
    started_at: Instant,
    args: &[String],
    auto_close_after_report: bool,
    target_long_edge: u32,
) -> Result<bool, String> {
    if args
        .iter()
        .any(|arg| arg == "--handoff-image" || arg == "--handoff-image-prewarm-wgpu")
    {
        let prewarm_wgpu = args
            .iter()
            .any(|arg| arg == "--handoff-prewarm-wgpu" || arg == "--handoff-image-prewarm-wgpu");
        let Some(input_path) = input_path_from_args(args) else {
            return Err("--handoff-image requires --input <path>".to_owned());
        };
        run_image_handoff_probe(
            started_at,
            auto_close_after_report,
            prewarm_wgpu,
            input_path,
            target_long_edge,
        )?;
        return Ok(true);
    }

    if args
        .iter()
        .any(|arg| arg == "--handoff-probe" || arg == "--handoff-prewarm-wgpu")
    {
        let prewarm_wgpu = args.iter().any(|arg| arg == "--handoff-prewarm-wgpu");
        run_handoff_probe(started_at, auto_close_after_report, prewarm_wgpu)?;
        return Ok(true);
    }

    Ok(false)
}
