use serde::Serialize;

#[derive(Debug, Default, Clone, Serialize)]
pub struct TimingStats {
    pub samples: Vec<f64>,
    pub total_ms: f64,
    pub mean_ms: f64,
    pub min_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

impl TimingStats {
    pub fn from_samples(mut samples: Vec<f64>) -> Self {
        if samples.is_empty() {
            return Self::default();
        }

        let total_ms = samples.iter().sum::<f64>();
        let mean_ms = total_ms / samples.len() as f64;
        samples.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
        let min_ms = samples[0];
        let max_ms = samples[samples.len() - 1];
        let p50_ms = percentile(&samples, 0.50);
        let p95_ms = percentile(&samples, 0.95);
        let p99_ms = percentile(&samples, 0.99);

        Self {
            samples,
            total_ms,
            mean_ms,
            min_ms,
            p50_ms,
            p95_ms,
            p99_ms,
            max_ms,
        }
    }
}

fn percentile(values: &[f64], quantile: f64) -> f64 {
    let index = ((values.len() - 1) as f64 * quantile.clamp(0.0, 1.0)).round() as usize;
    values[index]
}
