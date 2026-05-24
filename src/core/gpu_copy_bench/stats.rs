use super::{GpuCopyBenchCase, GpuCopyBenchSummary};

#[derive(Default)]
pub(super) struct SummarySet {
    cases: Vec<SummaryAccumulator>,
}

impl SummarySet {
    pub(super) fn push(&mut self, case: &GpuCopyBenchCase) {
        if let Some(summary) = self
            .cases
            .iter_mut()
            .find(|summary| summary.case == case.case)
        {
            summary.push(case);
        } else {
            let mut summary = SummaryAccumulator::new(case.case.clone());
            summary.push(case);
            self.cases.push(summary);
        }
    }

    pub(super) fn finish(self) -> Vec<GpuCopyBenchSummary> {
        self.cases
            .into_iter()
            .map(SummaryAccumulator::finish)
            .collect()
    }
}

struct SummaryAccumulator {
    case: String,
    pages: usize,
    samples: usize,
    bytes_total: usize,
    bind_group_creates_total: usize,
    uniform_buffer_creates_total: usize,
    values: Vec<f64>,
}

impl SummaryAccumulator {
    fn new(case: String) -> Self {
        Self {
            case,
            pages: 0,
            samples: 0,
            bytes_total: 0,
            bind_group_creates_total: 0,
            uniform_buffer_creates_total: 0,
            values: Vec::new(),
        }
    }

    fn push(&mut self, case: &GpuCopyBenchCase) {
        self.pages += 1;
        self.samples += case.samples;
        self.bytes_total = self.bytes_total.saturating_add(case.bytes);
        self.bind_group_creates_total = self.bind_group_creates_total.saturating_add(
            case.bind_group_creates_per_iteration
                .saturating_mul(case.samples),
        );
        self.uniform_buffer_creates_total = self.uniform_buffer_creates_total.saturating_add(
            case.uniform_buffer_creates_per_iteration
                .saturating_mul(case.samples),
        );
        self.values.extend(case.raw_samples_ms.iter().copied());
    }

    fn finish(self) -> GpuCopyBenchSummary {
        let stats = SampleStats::from_samples(self.values);
        let avg_bytes = if self.pages == 0 {
            0
        } else {
            self.bytes_total / self.pages
        };
        GpuCopyBenchSummary {
            interpretation: interpretation_for_case(&self.case).to_owned(),
            case: self.case,
            samples: self.samples,
            avg_ms: stats.avg_ms,
            median_ms: stats.median_ms,
            p95_ms: stats.p95_ms,
            max_ms: stats.max_ms,
            avg_bytes,
            avg_bind_group_creates: average_count(self.bind_group_creates_total, self.samples),
            avg_uniform_buffer_creates: average_count(
                self.uniform_buffer_creates_total,
                self.samples,
            ),
        }
    }
}

fn average_count(total: usize, samples: usize) -> f64 {
    if samples == 0 {
        0.0
    } else {
        total as f64 / samples as f64
    }
}

pub(super) struct SampleStats {
    pub(super) samples: usize,
    pub(super) samples_ms: Vec<f64>,
    pub(super) avg_ms: f64,
    pub(super) median_ms: f64,
    pub(super) p95_ms: f64,
    pub(super) max_ms: f64,
}

impl SampleStats {
    pub(super) fn from_samples(mut samples: Vec<f64>) -> Self {
        if samples.is_empty() {
            return Self {
                samples: 0,
                samples_ms: Vec::new(),
                avg_ms: 0.0,
                median_ms: 0.0,
                p95_ms: 0.0,
                max_ms: 0.0,
            };
        }
        samples.sort_by(|a, b| a.total_cmp(b));
        let count = samples.len();
        let total = samples.iter().sum::<f64>();
        Self {
            samples: count,
            samples_ms: samples.clone(),
            avg_ms: total / count as f64,
            median_ms: percentile(&samples, 0.50),
            p95_ms: percentile(&samples, 0.95),
            max_ms: samples[count - 1],
        }
    }
}

fn interpretation_for_case(case: &str) -> &str {
    match case {
        "color_image_to_rgba" => "CPU-side ColorImage to RGBA byte conversion.",
        "source_texture_create_view" => "CPU cost of creating a source texture and view.",
        "write_texture_reused_texture" => {
            "CPU-side queue.write_texture cost with bytes already available."
        }
        "precomputed_first_upload" => {
            "Estimated source cache miss if upload bytes are precomputed off the paint path."
        }
        "legacy_combined_bind_group_create" => {
            "Old-style combined texture+uniform bind group creation cost."
        }
        "texture_bind_group_create" => {
            "Texture-only bind group creation cost for reusable texture bindings."
        }
        "params_bind_group_create" => "Per-draw uniform buffer and params bind group creation cost.",
        "current_first_upload" => {
            "Current source cache miss shape: texture creation, RGBA conversion, upload, view creation."
        }
        "fsr1_intermediate_texture_create_view" => {
            "CPU cost currently paid by the two-pass FSR1 path when creating its intermediate target."
        }
        "fsr1_twopass_recreate_intermediate" => {
            "Two-pass FSR1 render cost when the intermediate target is recreated every sample."
        }
        "fsr1_twopass_reuse_intermediate" => {
            "Two-pass FSR1 render cost when the same intermediate target is reused."
        }
        _ => "GPU copy benchmark case.",
    }
}

fn percentile(samples: &[f64], percentile: f64) -> f64 {
    let last = samples.len().saturating_sub(1);
    let index = ((last as f64) * percentile).ceil() as usize;
    samples[index.min(last)]
}

#[cfg(test)]
mod tests {
    use super::{percentile, SampleStats};

    #[test]
    fn percentile_uses_sorted_ceiling_index() {
        let samples = [1.0, 2.0, 3.0, 4.0];

        assert_eq!(percentile(&samples, 0.50), 3.0);
        assert_eq!(percentile(&samples, 0.95), 4.0);
    }

    #[test]
    fn sample_stats_handles_empty_samples() {
        let stats = SampleStats::from_samples(Vec::new());

        assert_eq!(stats.samples, 0);
        assert_eq!(stats.avg_ms, 0.0);
        assert_eq!(stats.max_ms, 0.0);
    }
}
