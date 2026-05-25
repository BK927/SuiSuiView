use super::{
    clamp_target_long_edge, preview_prefetch_indices, NavigationDirection,
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
    visible_pages: usize,
    prefetch_enabled: bool,
    progressive_preview_enabled: bool,
) -> Vec<PageJob> {
    let target_long_edge = clamp_target_long_edge(target_long_edge);
    let full_indices = if prefetch_enabled {
        prioritized_indices(center, page_count, direction)
    } else {
        visible_indices(center, page_count, visible_pages)
    };
    let preview_capacity = if progressive_preview_enabled
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

    if progressive_preview_enabled && target_long_edge > PREVIEW_TARGET_LONG_EDGE {
        let preview_indices = if prefetch_enabled {
            preview_prefetch_indices(center, page_count, direction, visible_pages)
        } else {
            visible_indices(center, page_count, visible_pages)
        };
        for index in preview_indices {
            push_job(&mut jobs, index, PREVIEW_TARGET_LONG_EDGE);
        }
    }

    for index in full_indices {
        push_job(&mut jobs, index, target_long_edge);
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

fn is_visible_page_index(index: usize, center: usize, visible_pages: usize) -> bool {
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
) -> Vec<usize> {
    if page_count == 0 {
        return Vec::new();
    }

    let mut indices = Vec::with_capacity(5);
    push_index(&mut indices, center, page_count);

    match direction {
        NavigationDirection::Forward => {
            for offset in 1..=3 {
                if let Some(index) = center.checked_add(offset) {
                    push_index(&mut indices, index, page_count);
                }
            }
            if let Some(index) = center.checked_sub(1) {
                push_index(&mut indices, index, page_count);
            }
        }
        NavigationDirection::Backward => {
            for offset in 1..=3 {
                if let Some(index) = center.checked_sub(offset) {
                    push_index(&mut indices, index, page_count);
                }
            }
            if let Some(index) = center.checked_add(1) {
                push_index(&mut indices, index, page_count);
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
    use crate::core::worker::{NavigationDirection, PREVIEW_TARGET_LONG_EDGE};

    #[test]
    fn priority_tracks_forward_reading() {
        assert_eq!(
            prioritized_indices(5, 12, NavigationDirection::Forward),
            vec![5, 6, 7, 8, 4]
        );
    }

    #[test]
    fn priority_tracks_backward_reading() {
        assert_eq!(
            prioritized_indices(5, 12, NavigationDirection::Backward),
            vec![5, 4, 3, 2, 6]
        );
    }

    #[test]
    fn preview_jobs_are_prioritized_for_visible_pages() {
        assert_eq!(
            prioritized_jobs(5, 12, NavigationDirection::Forward, 2048, 2, true, true,),
            vec![
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
                    index: 4,
                    target_long_edge: 2048
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
                2,
                true,
                true,
            ),
            prioritized_indices(5, 12, NavigationDirection::Forward)
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
            prioritized_jobs(5, 12, NavigationDirection::Forward, 2048, 2, false, false,),
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
