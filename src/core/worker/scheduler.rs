use super::{
    clamp_target_long_edge, preview_prefetch_indices, NavigationDirection, PreparedTargetIntent,
    FULL_QUALITY_PREFETCH_BACKWARD_PAGES, FULL_QUALITY_PREFETCH_FORWARD_PAGES,
    PREVIEW_PREFETCH_BACKWARD_PAGES, PREVIEW_PREFETCH_FORWARD_PAGES, PREVIEW_TARGET_LONG_EDGE,
};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PageJob {
    pub(super) index: usize,
    pub(super) target_long_edge: u32,
}

pub(super) fn prioritized_jobs(
    center: usize,
    page_count: usize,
    direction: NavigationDirection,
    target_long_edge: u32,
    target_intent: PreparedTargetIntent,
    visible_pages: usize,
    prefetch_enabled: bool,
    progressive_preview_enabled: bool,
) -> Vec<PageJob> {
    let target_long_edge = clamp_target_long_edge(target_long_edge);
    let full_indices = if target_intent.keeps_exact_prefetch_lightweight() {
        visible_indices(center, page_count, visible_pages)
    } else if prefetch_enabled {
        prioritized_indices(center, page_count, direction, visible_pages)
    } else {
        visible_indices(center, page_count, visible_pages)
    };
    let preview_capacity = if target_intent.is_original_inspection() {
        0
    } else if progressive_preview_enabled
        && target_long_edge > PREVIEW_TARGET_LONG_EDGE
        && prefetch_enabled
    {
        visible_pages
            .max(1)
            .saturating_add(PREVIEW_PREFETCH_FORWARD_PAGES)
            .saturating_add(PREVIEW_PREFETCH_BACKWARD_PAGES)
    } else if progressive_preview_enabled && target_long_edge > PREVIEW_TARGET_LONG_EDGE {
        visible_pages.max(1)
    } else {
        0
    };
    let mut jobs = Vec::with_capacity(full_indices.len().saturating_add(preview_capacity));

    for index in full_indices {
        push_job(&mut jobs, index, target_long_edge);
    }

    if !target_intent.is_original_inspection()
        && progressive_preview_enabled
        && target_long_edge > PREVIEW_TARGET_LONG_EDGE
    {
        let preview_indices = if prefetch_enabled {
            preview_prefetch_indices(center, page_count, direction, visible_pages)
        } else {
            visible_indices(center, page_count, visible_pages)
        };
        for index in preview_indices {
            push_job(&mut jobs, index, PREVIEW_TARGET_LONG_EDGE);
        }
    }

    jobs
}

pub(super) fn should_skip_ai_preview_or_prefetch(
    page_name: Option<&str>,
    center: usize,
    visible_pages: usize,
    index: usize,
    target_long_edge: u32,
) -> bool {
    if !page_name.is_some_and(is_ai_page_name) {
        return false;
    }
    if target_long_edge == PREVIEW_TARGET_LONG_EDGE {
        return true;
    }
    !is_visible_page_index(index, center, visible_pages)
}

fn visible_indices(center: usize, page_count: usize, visible_pages: usize) -> Vec<usize> {
    let mut indices = Vec::with_capacity(visible_pages.max(1));
    for offset in 0..visible_pages.max(1) {
        if let Some(index) = center.checked_add(offset) {
            push_index(&mut indices, index, page_count);
        }
    }
    indices
}

pub(super) fn is_visible_page_index(index: usize, center: usize, visible_pages: usize) -> bool {
    index >= center && index < center.saturating_add(visible_pages.max(1))
}

fn is_ai_page_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ai"))
}

fn push_job(jobs: &mut Vec<PageJob>, index: usize, target_long_edge: u32) {
    let job = PageJob {
        index,
        target_long_edge,
    };
    if !jobs.contains(&job) {
        jobs.push(job);
    }
}

pub(super) fn prioritized_indices(
    center: usize,
    page_count: usize,
    direction: NavigationDirection,
    visible_pages: usize,
) -> Vec<usize> {
    if page_count == 0 {
        return Vec::new();
    }

    let forward_prefetch_pages =
        FULL_QUALITY_PREFETCH_FORWARD_PAGES.max(visible_pages.saturating_sub(1));
    let backward_prefetch_pages = FULL_QUALITY_PREFETCH_BACKWARD_PAGES;
    let mut indices = Vec::with_capacity(
        forward_prefetch_pages
            .saturating_add(backward_prefetch_pages)
            .saturating_add(1),
    );
    push_index(&mut indices, center, page_count);

    match direction {
        NavigationDirection::Forward => {
            for offset in 1..=forward_prefetch_pages {
                if let Some(index) = center.checked_add(offset) {
                    push_index(&mut indices, index, page_count);
                }
            }
            for offset in 1..=backward_prefetch_pages {
                if let Some(index) = center.checked_sub(offset) {
                    push_index(&mut indices, index, page_count);
                }
            }
        }
        NavigationDirection::Backward => {
            for offset in 1..=forward_prefetch_pages {
                if let Some(index) = center.checked_sub(offset) {
                    push_index(&mut indices, index, page_count);
                }
            }
            for offset in 1..=backward_prefetch_pages {
                if let Some(index) = center.checked_add(offset) {
                    push_index(&mut indices, index, page_count);
                }
            }
        }
    }

    indices
}

fn push_index(indices: &mut Vec<usize>, index: usize, page_count: usize) {
    if index < page_count && !indices.contains(&index) {
        indices.push(index);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        prioritized_indices, prioritized_jobs, should_skip_ai_preview_or_prefetch, PageJob,
    };
    use crate::core::worker::{
        NavigationDirection, PreparedTargetIntent, MAX_TARGET_LONG_EDGE, PREVIEW_TARGET_LONG_EDGE,
    };

    #[test]
    fn priority_tracks_forward_reading() {
        assert_eq!(
            prioritized_indices(5, 12, NavigationDirection::Forward, 1),
            vec![5, 6, 7, 8, 9, 10, 11, 4]
        );
    }

    #[test]
    fn priority_tracks_backward_reading() {
        assert_eq!(
            prioritized_indices(5, 12, NavigationDirection::Backward, 1),
            vec![5, 4, 3, 2, 1, 0, 6]
        );
    }

    #[test]
    fn priority_can_extend_full_prefetch_for_queued_turns() {
        assert_eq!(
            prioritized_indices(5, 20, NavigationDirection::Forward, 6),
            vec![5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 4]
        );
    }

    #[test]
    fn exact_jobs_precede_preview_jobs_for_visible_pages() {
        assert_eq!(
            prioritized_jobs(
                5,
                12,
                NavigationDirection::Forward,
                2048,
                PreparedTargetIntent::NormalNavigation,
                2,
                true,
                true,
            ),
            vec![
                PageJob {
                    index: 5,
                    target_long_edge: 2048
                },
                PageJob {
                    index: 6,
                    target_long_edge: 2048
                },
                PageJob {
                    index: 7,
                    target_long_edge: 2048
                },
                PageJob {
                    index: 8,
                    target_long_edge: 2048
                },
                PageJob {
                    index: 9,
                    target_long_edge: 2048
                },
                PageJob {
                    index: 10,
                    target_long_edge: 2048
                },
                PageJob {
                    index: 11,
                    target_long_edge: 2048
                },
                PageJob {
                    index: 4,
                    target_long_edge: 2048
                },
                PageJob {
                    index: 5,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
                PageJob {
                    index: 6,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
                PageJob {
                    index: 7,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
                PageJob {
                    index: 8,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
                PageJob {
                    index: 9,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
                PageJob {
                    index: 10,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
                PageJob {
                    index: 11,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
                PageJob {
                    index: 4,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
                PageJob {
                    index: 3,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
            ]
        );
    }

    #[test]
    fn preview_jobs_are_skipped_when_target_is_preview_sized() {
        assert_eq!(
            prioritized_jobs(
                5,
                12,
                NavigationDirection::Forward,
                PREVIEW_TARGET_LONG_EDGE,
                PreparedTargetIntent::NormalNavigation,
                2,
                true,
                true,
            ),
            prioritized_indices(5, 12, NavigationDirection::Forward, 2)
                .into_iter()
                .map(|index| PageJob {
                    index,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                })
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn preview_and_prefetch_can_be_disabled() {
        assert_eq!(
            prioritized_jobs(
                5,
                12,
                NavigationDirection::Forward,
                2048,
                PreparedTargetIntent::NormalNavigation,
                2,
                false,
                false,
            ),
            vec![
                PageJob {
                    index: 5,
                    target_long_edge: 2048
                },
                PageJob {
                    index: 6,
                    target_long_edge: 2048
                },
            ]
        );
    }

    #[test]
    fn original_inspection_jobs_stay_visible_and_exact() {
        assert_eq!(
            prioritized_jobs(
                5,
                12,
                NavigationDirection::Forward,
                MAX_TARGET_LONG_EDGE + 1,
                PreparedTargetIntent::OriginalInspection,
                2,
                true,
                true,
            ),
            vec![
                PageJob {
                    index: 5,
                    target_long_edge: MAX_TARGET_LONG_EDGE + 1
                },
                PageJob {
                    index: 6,
                    target_long_edge: MAX_TARGET_LONG_EDGE + 1
                },
            ]
        );
    }

    #[test]
    fn large_fit_display_keeps_high_target_jobs_visible_but_allows_preview_prefetch() {
        assert_eq!(
            prioritized_jobs(
                5,
                12,
                NavigationDirection::Forward,
                MAX_TARGET_LONG_EDGE + 512,
                PreparedTargetIntent::LargeFitDisplay,
                2,
                true,
                true,
            ),
            vec![
                PageJob {
                    index: 5,
                    target_long_edge: MAX_TARGET_LONG_EDGE + 512
                },
                PageJob {
                    index: 6,
                    target_long_edge: MAX_TARGET_LONG_EDGE + 512
                },
                PageJob {
                    index: 5,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
                PageJob {
                    index: 6,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
                PageJob {
                    index: 7,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
                PageJob {
                    index: 8,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
                PageJob {
                    index: 9,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
                PageJob {
                    index: 10,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
                PageJob {
                    index: 11,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
                PageJob {
                    index: 4,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
                PageJob {
                    index: 3,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE
                },
            ]
        );
    }

    #[test]
    fn ai_preview_and_prefetch_jobs_are_skipped() {
        assert!(should_skip_ai_preview_or_prefetch(
            Some("art.ai"),
            1,
            1,
            1,
            PREVIEW_TARGET_LONG_EDGE
        ));
        assert!(!should_skip_ai_preview_or_prefetch(
            Some("art.ai"),
            1,
            1,
            1,
            2048
        ));
        assert!(should_skip_ai_preview_or_prefetch(
            Some("art.ai"),
            0,
            1,
            1,
            2048
        ));
        assert!(!should_skip_ai_preview_or_prefetch(
            Some("page-0001.png"),
            0,
            1,
            0,
            2048
        ));
    }
}
