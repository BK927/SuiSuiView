use std::time::Instant;

use super::wgpu_worker::{run_wgpu_probe_blocking, WgpuProbeInput};

pub(crate) fn run_headless_worker(started_at: Instant) -> Result<(), String> {
    let report = run_wgpu_probe_blocking(started_at, WgpuProbeInput::Synthetic);
    println!(
        "runtime_probe headless_worker=true wgpu_worker_started_ms={:.3} wgpu_init_ms={:.3} wgpu_compute_readback_ms={:.3} backend={} device_type={} checksum={} mode={} error={}",
        report.worker_started_ms,
        report.init_ms.unwrap_or(-1.0),
        report.compute_readback_ms.unwrap_or(-1.0),
        report.backend.unwrap_or("unknown"),
        report.device_type.unwrap_or("unknown"),
        report.checksum.unwrap_or_default(),
        report.mode,
        report.error.as_deref().unwrap_or("none")
    );
    if let Some(error) = report.error {
        Err(error)
    } else {
        Ok(())
    }
}
