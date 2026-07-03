use super::{DecodeBackend, PreparedPage};
use crate::core::source::SharedSource;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::{perf_trace, perf_trace::PerfField};
use std::env;
use std::sync::OnceLock;
use std::time::Duration;

// Experiment status (2026-07): graduated. With the env var unset the mode is
// Adaptive, so decode-ahead is active in the product by default; the variable
// remains only as an override (off/forced) for diagnosis.
const DECODE_AHEAD_ENV: &str = "SUISUIVIEW_EXPERIMENT_DECODE_AHEAD";
const ADAPTIVE_SLOW_PREPARE: Duration = Duration::from_millis(750);
const ADAPTIVE_FAST_PREPARE: Duration = Duration::from_millis(180);
const ADAPTIVE_FAST_DISABLE_COUNT: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DecodeAheadMode {
    Disabled,
    Forced,
    Adaptive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DecodeAheadCandidate {
    Any,
    WebpCluster,
}

impl DecodeAheadCandidate {
    pub(super) fn matches_job(self, source: &SharedSource, index: usize) -> bool {
        match self {
            Self::Any => true,
            Self::WebpCluster => {
                is_webp_page(source.page_name(index))
                    && ((index > 0 && is_webp_page(source.page_name(index - 1)))
                        || index
                            .checked_add(1)
                            .is_some_and(|next| is_webp_page(source.page_name(next))))
            }
        }
    }
}

pub(super) struct DecodeAheadPolicy {
    mode: DecodeAheadMode,
    fast_prepare_count: u8,
    adaptive_active: bool,
}

impl DecodeAheadPolicy {
    pub(super) fn from_env() -> Self {
        Self::new(decode_ahead_mode())
    }

    fn new(mode: DecodeAheadMode) -> Self {
        Self {
            mode,
            fast_prepare_count: 0,
            adaptive_active: false,
        }
    }

    pub(super) fn candidate(&self) -> Option<DecodeAheadCandidate> {
        match self.mode {
            DecodeAheadMode::Disabled => None,
            DecodeAheadMode::Forced => Some(DecodeAheadCandidate::Any),
            DecodeAheadMode::Adaptive if self.adaptive_active => {
                Some(DecodeAheadCandidate::WebpCluster)
            }
            DecodeAheadMode::Adaptive => None,
        }
    }

    pub(super) fn needs_prepare_timing_for(&self, source: &SharedSource, index: usize) -> bool {
        match self.mode {
            DecodeAheadMode::Disabled | DecodeAheadMode::Forced => false,
            DecodeAheadMode::Adaptive if self.adaptive_active => true,
            DecodeAheadMode::Adaptive => is_webp_page(source.page_name(index)),
        }
    }

    pub(super) fn observe_prepare(
        &mut self,
        source: &SharedSource,
        index: usize,
        page: &PreparedPage,
        duration: Option<Duration>,
    ) {
        if self.mode != DecodeAheadMode::Adaptive {
            return;
        }
        let Some(duration) = duration else {
            return;
        };

        if is_adaptive_slow_backend(page.decode_backend) && duration >= ADAPTIVE_SLOW_PREPARE {
            self.fast_prepare_count = 0;
            if self.adaptive_active {
                return;
            }
            if !DecodeAheadCandidate::WebpCluster.matches_job(source, index) {
                return;
            }
            self.adaptive_active = true;
            record_adaptive_state("enable_slow_webp", true, self.fast_prepare_count);
            return;
        }

        if !self.adaptive_active || duration > ADAPTIVE_FAST_PREPARE {
            self.fast_prepare_count = 0;
            return;
        }

        self.fast_prepare_count = self.fast_prepare_count.saturating_add(1);
        if self.fast_prepare_count >= ADAPTIVE_FAST_DISABLE_COUNT {
            let disabled_at_count = self.fast_prepare_count;
            self.fast_prepare_count = 0;
            self.adaptive_active = false;
            record_adaptive_state("disable_fast_window", false, disabled_at_count);
        }
    }

    pub(super) fn reset_context(&mut self) {
        if self.mode != DecodeAheadMode::Adaptive {
            return;
        }
        if self.adaptive_active || self.fast_prepare_count != 0 {
            self.adaptive_active = false;
            self.fast_prepare_count = 0;
            record_adaptive_state("context", false, self.fast_prepare_count);
        }
    }
}

pub(super) fn decode_ahead_mode() -> DecodeAheadMode {
    static MODE: OnceLock<DecodeAheadMode> = OnceLock::new();
    *MODE.get_or_init(|| {
        decode_ahead_mode_value(
            env::var(DECODE_AHEAD_ENV).ok().as_deref(),
            DecodeAheadMode::Adaptive,
        )
    })
}

fn decode_ahead_mode_value(value: Option<&str>, default_mode: DecodeAheadMode) -> DecodeAheadMode {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        None => default_mode,
        Some(value)
            if value.eq_ignore_ascii_case("0")
                || value.eq_ignore_ascii_case("false")
                || value.eq_ignore_ascii_case("off")
                || value.eq_ignore_ascii_case("no")
                || value.eq_ignore_ascii_case("disabled")
                || value.eq_ignore_ascii_case("disable")
                || value.eq_ignore_ascii_case("none") =>
        {
            DecodeAheadMode::Disabled
        }
        Some(value)
            if value.eq_ignore_ascii_case("1")
                || value.eq_ignore_ascii_case("true")
                || value.eq_ignore_ascii_case("on")
                || value.eq_ignore_ascii_case("yes")
                || value.eq_ignore_ascii_case("forced")
                || value.eq_ignore_ascii_case("force") =>
        {
            DecodeAheadMode::Forced
        }
        Some(value)
            if value.eq_ignore_ascii_case("adaptive") || value.eq_ignore_ascii_case("auto") =>
        {
            DecodeAheadMode::Adaptive
        }
        Some(_) => DecodeAheadMode::Disabled,
    }
}

fn is_adaptive_slow_backend(backend: DecodeBackend) -> bool {
    matches!(
        backend,
        DecodeBackend::LibWebp | DecodeBackend::LibWebpScaled | DecodeBackend::ImageWebp
    )
}

fn is_webp_page(page_name: Option<&str>) -> bool {
    page_name
        .and_then(|name| std::path::Path::new(name).extension())
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("webp"))
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_adaptive_state(reason: &'static str, active: bool, fast_prepare_count: u8) {
    perf_trace::record_duration(
        "page_decode_ahead_adaptive_state",
        Duration::ZERO,
        &[
            PerfField::Str("reason", reason),
            PerfField::Bool("active", active),
            PerfField::Usize("fast_prepare_count", fast_prepare_count as usize),
        ],
    );
}

#[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
fn record_adaptive_state(_reason: &'static str, _active: bool, _fast_prepare_count: u8) {}

#[cfg(test)]
mod tests {
    use super::{
        decode_ahead_mode_value, DecodeAheadCandidate, DecodeAheadMode, DecodeAheadPolicy,
    };
    use crate::core::source::SharedSource;
    use crate::core::worker::{DecodeBackend, PagePixels, PreparedPage};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn decode_ahead_mode_parses_forced_adaptive_and_disabled_values() {
        assert_eq!(
            decode_ahead_mode_value(Some("1"), DecodeAheadMode::Adaptive),
            DecodeAheadMode::Forced
        );
        assert_eq!(
            decode_ahead_mode_value(Some("forced"), DecodeAheadMode::Adaptive),
            DecodeAheadMode::Forced
        );
        assert_eq!(
            decode_ahead_mode_value(Some("adaptive"), DecodeAheadMode::Disabled),
            DecodeAheadMode::Adaptive
        );
        assert_eq!(
            decode_ahead_mode_value(Some("auto"), DecodeAheadMode::Disabled),
            DecodeAheadMode::Adaptive
        );
        assert_eq!(
            decode_ahead_mode_value(Some("0"), DecodeAheadMode::Adaptive),
            DecodeAheadMode::Disabled
        );
        assert_eq!(
            decode_ahead_mode_value(Some("off"), DecodeAheadMode::Adaptive),
            DecodeAheadMode::Disabled
        );
    }

    #[test]
    fn decode_ahead_mode_uses_supplied_default_when_unset_or_empty() {
        assert_eq!(
            decode_ahead_mode_value(None, DecodeAheadMode::Adaptive),
            DecodeAheadMode::Adaptive
        );
        assert_eq!(
            decode_ahead_mode_value(Some(""), DecodeAheadMode::Adaptive),
            DecodeAheadMode::Adaptive
        );
        assert_eq!(
            decode_ahead_mode_value(None, DecodeAheadMode::Disabled),
            DecodeAheadMode::Disabled
        );
    }

    #[test]
    fn decode_ahead_mode_treats_unknown_values_as_disabled() {
        assert_eq!(
            decode_ahead_mode_value(Some("unknown"), DecodeAheadMode::Adaptive),
            DecodeAheadMode::Disabled
        );
    }

    #[test]
    fn adaptive_policy_activates_only_after_slow_webp_prepare() {
        let mut policy = DecodeAheadPolicy::new(DecodeAheadMode::Adaptive);
        let source = named_source(vec!["page-0.jpg", "page-1.webp", "page-2.webp"]);

        policy.observe_prepare(
            &source,
            0,
            &prepared_page(DecodeBackend::PngSampled),
            Some(Duration::from_millis(900)),
        );
        assert_eq!(policy.candidate(), None);

        policy.observe_prepare(
            &source,
            1,
            &prepared_page(DecodeBackend::LibWebpScaled),
            Some(Duration::from_millis(900)),
        );
        assert_eq!(policy.candidate(), Some(DecodeAheadCandidate::WebpCluster));
    }

    #[test]
    fn adaptive_policy_ignores_missing_prepare_timing() {
        let mut policy = DecodeAheadPolicy::new(DecodeAheadMode::Adaptive);
        let source = named_source(vec!["page-0.webp", "page-1.webp"]);

        policy.observe_prepare(
            &source,
            0,
            &prepared_page(DecodeBackend::LibWebpScaled),
            None,
        );

        assert_eq!(policy.candidate(), None);
    }

    #[test]
    fn adaptive_policy_turns_off_after_fast_prepare_window() {
        let mut policy = DecodeAheadPolicy::new(DecodeAheadMode::Adaptive);
        let source = named_source(vec!["page-0.webp", "page-1.webp"]);
        policy.observe_prepare(
            &source,
            0,
            &prepared_page(DecodeBackend::LibWebpScaled),
            Some(Duration::from_millis(900)),
        );

        for _ in 0..super::ADAPTIVE_FAST_DISABLE_COUNT {
            policy.observe_prepare(
                &source,
                0,
                &prepared_page(DecodeBackend::ZuneJpeg),
                Some(Duration::from_millis(40)),
            );
        }

        assert_eq!(policy.candidate(), None);
    }

    #[test]
    fn adaptive_policy_times_only_webp_while_inactive() {
        let policy = DecodeAheadPolicy::new(DecodeAheadMode::Adaptive);
        let source = named_source(vec!["page-0.jpg", "page-1.webp"]);

        assert!(!policy.needs_prepare_timing_for(&source, 0));
        assert!(policy.needs_prepare_timing_for(&source, 1));
    }

    #[test]
    fn adaptive_policy_times_all_pages_while_active() {
        let mut policy = DecodeAheadPolicy::new(DecodeAheadMode::Adaptive);
        let source = named_source(vec!["page-0.webp", "page-1.webp", "page-2.jpg"]);
        policy.observe_prepare(
            &source,
            0,
            &prepared_page(DecodeBackend::LibWebpScaled),
            Some(Duration::from_millis(900)),
        );

        assert!(policy.needs_prepare_timing_for(&source, 2));
    }

    #[test]
    fn adaptive_policy_ignores_isolated_slow_webp() {
        let mut policy = DecodeAheadPolicy::new(DecodeAheadMode::Adaptive);
        let source = named_source(vec!["page-0.jpg", "page-1.webp", "page-2.png"]);

        policy.observe_prepare(
            &source,
            1,
            &prepared_page(DecodeBackend::LibWebpScaled),
            Some(Duration::from_millis(900)),
        );

        assert_eq!(policy.candidate(), None);
    }

    #[test]
    fn webp_cluster_candidate_requires_neighboring_webp_page() {
        let source = named_source(vec![
            "page-0.jpg",
            "page-1.WEBP",
            "page-2.webp",
            "page-3.png",
        ]);

        assert!(DecodeAheadCandidate::WebpCluster.matches_job(&source, 1));
        assert!(DecodeAheadCandidate::WebpCluster.matches_job(&source, 2));
        assert!(!DecodeAheadCandidate::WebpCluster.matches_job(&source, 0));
        assert!(!DecodeAheadCandidate::WebpCluster.matches_job(&source, 3));
        assert!(DecodeAheadCandidate::Any.matches_job(&source, 3));
    }

    fn named_source(page_names: Vec<&'static str>) -> SharedSource {
        Arc::new(NamedSource { page_names })
    }

    struct NamedSource {
        page_names: Vec<&'static str>,
    }

    impl crate::core::source::BookSource for NamedSource {
        fn title(&self) -> &str {
            "named"
        }

        fn source_path(&self) -> &std::path::Path {
            std::path::Path::new("named-source")
        }

        fn book_id(&self) -> &str {
            "named-source"
        }

        fn page_count(&self) -> usize {
            self.page_names.len()
        }

        fn page_name(&self, index: usize) -> Option<&str> {
            self.page_names.get(index).copied()
        }

        fn read_page(&self, index: usize) -> Result<Vec<u8>, crate::core::source::SourceError> {
            if index < self.page_count() {
                Ok(Vec::new())
            } else {
                Err(crate::core::source::SourceError::InvalidPage {
                    index,
                    page_count: self.page_count(),
                })
            }
        }
    }

    fn prepared_page(decode_backend: DecodeBackend) -> PreparedPage {
        PreparedPage {
            pixels: PagePixels::Rgba(Arc::from([0, 0, 0, 255])),
            original_width: 1,
            original_height: 1,
            display_width: 1,
            display_height: 1,
            byte_size: 4,
            target_long_edge: 1024,
            decode_backend,
            notice: None,
        }
    }
}
