use crate::core::state::TopBarItems;

const MORE_BUTTON_WIDTH: f32 = 44.0;
const SEPARATOR_WIDTH: f32 = 18.0;
const OPEN_GROUP_WIDTH: f32 = 112.0;
const VIEW_GROUP_WIDTH: f32 = 300.0;
const ADJUST_GROUP_WIDTH: f32 = 120.0;
const COMPARE_IDLE_GROUP_WIDTH: f32 = 92.0;
const COMPARE_ACTIVE_GROUP_WIDTH: f32 = 520.0;
const BOOKMARKS_GROUP_WIDTH: f32 = 78.0;
const PAGE_BASE_WIDTH: f32 = 148.0;
const PAGE_DIGIT_WIDTH: f32 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app::ui) enum TopBarGroup {
    Open,
    Page,
    View,
    Adjust,
    Compare,
    Bookmarks,
}

impl TopBarGroup {
    const ALL: [Self; 6] = [
        Self::Open,
        Self::Page,
        Self::View,
        Self::Adjust,
        Self::Compare,
        Self::Bookmarks,
    ];

    fn is_visible(self, items: TopBarItems) -> bool {
        match self {
            Self::Open => items.open,
            Self::Page => items.page,
            Self::View => items.view,
            Self::Adjust => items.adjust,
            Self::Compare => items.compare,
            Self::Bookmarks => items.bookmarks,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app::ui) struct TopBarLayout {
    pub(in crate::app::ui) inline_groups: Vec<TopBarGroup>,
    pub(in crate::app::ui) overflow_groups: Vec<TopBarGroup>,
}

pub(in crate::app::ui) fn visible_top_bar_groups(
    items: TopBarItems,
) -> impl Iterator<Item = TopBarGroup> {
    TopBarGroup::ALL
        .into_iter()
        .filter(move |group| group.is_visible(items))
}

pub(in crate::app::ui) fn responsive_top_bar_layout(
    items: TopBarItems,
    available_width: f32,
    compare_enabled: bool,
    page_count: usize,
) -> TopBarLayout {
    let visible_groups: Vec<_> = visible_top_bar_groups(items).collect();
    let mut inline_groups = visible_groups.clone();

    if layout_width(&inline_groups, false, compare_enabled, page_count) <= available_width {
        return TopBarLayout {
            inline_groups,
            overflow_groups: Vec::new(),
        };
    }

    for group in [
        TopBarGroup::Compare,
        TopBarGroup::Adjust,
        TopBarGroup::View,
        TopBarGroup::Open,
        TopBarGroup::Bookmarks,
        TopBarGroup::Page,
    ] {
        if let Some(index) = inline_groups
            .iter()
            .position(|candidate| *candidate == group)
        {
            inline_groups.remove(index);
        }
        if layout_width(&inline_groups, true, compare_enabled, page_count) <= available_width {
            break;
        }
    }

    let overflow_groups = visible_groups
        .into_iter()
        .filter(|group| !inline_groups.contains(group))
        .collect();
    TopBarLayout {
        inline_groups,
        overflow_groups,
    }
}

fn layout_width(
    inline_groups: &[TopBarGroup],
    has_overflow: bool,
    compare_enabled: bool,
    page_count: usize,
) -> f32 {
    let inline_width = groups_width(inline_groups, compare_enabled, page_count);
    if !has_overflow {
        return inline_width;
    }
    if inline_groups.is_empty() {
        MORE_BUTTON_WIDTH
    } else {
        inline_width + SEPARATOR_WIDTH + MORE_BUTTON_WIDTH
    }
}

fn groups_width(groups: &[TopBarGroup], compare_enabled: bool, page_count: usize) -> f32 {
    let controls_width = groups
        .iter()
        .copied()
        .map(|group| group_width(group, compare_enabled, page_count))
        .sum::<f32>();
    let separators_width = groups.len().saturating_sub(1) as f32 * SEPARATOR_WIDTH;
    controls_width + separators_width
}

fn group_width(group: TopBarGroup, compare_enabled: bool, page_count: usize) -> f32 {
    match group {
        TopBarGroup::Open => OPEN_GROUP_WIDTH,
        TopBarGroup::Page => page_group_width(page_count),
        TopBarGroup::View => VIEW_GROUP_WIDTH,
        TopBarGroup::Adjust => ADJUST_GROUP_WIDTH,
        TopBarGroup::Compare if compare_enabled => COMPARE_ACTIVE_GROUP_WIDTH,
        TopBarGroup::Compare => COMPARE_IDLE_GROUP_WIDTH,
        TopBarGroup::Bookmarks => BOOKMARKS_GROUP_WIDTH,
    }
}

fn page_group_width(page_count: usize) -> f32 {
    let digits = page_count.max(1).to_string().len() as f32;
    PAGE_BASE_WIDTH + digits * PAGE_DIGIT_WIDTH
}

#[cfg(test)]
mod tests {
    use super::{responsive_top_bar_layout, visible_top_bar_groups, TopBarGroup};
    use crate::core::state::TopBarItems;

    fn all_items() -> TopBarItems {
        TopBarItems {
            open: true,
            page: true,
            view: true,
            adjust: true,
            compare: true,
            bookmarks: true,
        }
    }

    #[test]
    fn visible_top_bar_groups_preserve_toolbar_order() {
        let items = TopBarItems {
            open: false,
            page: true,
            view: false,
            adjust: true,
            compare: false,
            bookmarks: true,
        };

        let groups: Vec<_> = visible_top_bar_groups(items).collect();

        assert_eq!(
            groups,
            vec![
                TopBarGroup::Page,
                TopBarGroup::Adjust,
                TopBarGroup::Bookmarks
            ]
        );
    }

    #[test]
    fn visible_top_bar_groups_can_be_empty() {
        let groups: Vec<_> = visible_top_bar_groups(TopBarItems {
            open: false,
            page: false,
            view: false,
            adjust: false,
            compare: false,
            bookmarks: false,
        })
        .collect();

        assert!(groups.is_empty());
    }

    #[test]
    fn responsive_top_bar_layout_keeps_all_groups_inline_when_wide() {
        let layout = responsive_top_bar_layout(all_items(), 1400.0, false, 120);

        assert_eq!(
            layout.inline_groups,
            vec![
                TopBarGroup::Open,
                TopBarGroup::Page,
                TopBarGroup::View,
                TopBarGroup::Adjust,
                TopBarGroup::Compare,
                TopBarGroup::Bookmarks,
            ]
        );
        assert!(layout.overflow_groups.is_empty());
    }

    #[test]
    fn responsive_top_bar_layout_overflows_secondary_groups_near_min_width() {
        let layout = responsive_top_bar_layout(all_items(), 708.0, false, 120);

        assert_eq!(
            layout.inline_groups,
            vec![TopBarGroup::Open, TopBarGroup::Page, TopBarGroup::Bookmarks]
        );
        assert_eq!(
            layout.overflow_groups,
            vec![TopBarGroup::View, TopBarGroup::Adjust, TopBarGroup::Compare]
        );
    }

    #[test]
    fn responsive_top_bar_layout_keeps_only_more_when_very_narrow() {
        let layout = responsive_top_bar_layout(all_items(), 60.0, false, 9999);

        assert!(layout.inline_groups.is_empty());
        assert_eq!(
            layout.overflow_groups,
            vec![
                TopBarGroup::Open,
                TopBarGroup::Page,
                TopBarGroup::View,
                TopBarGroup::Adjust,
                TopBarGroup::Compare,
                TopBarGroup::Bookmarks,
            ]
        );
    }

    #[test]
    fn responsive_top_bar_layout_omits_disabled_groups() {
        let layout = responsive_top_bar_layout(
            TopBarItems {
                open: false,
                page: true,
                view: false,
                adjust: true,
                compare: false,
                bookmarks: true,
            },
            60.0,
            false,
            10,
        );

        assert!(layout.inline_groups.is_empty());
        assert_eq!(
            layout.overflow_groups,
            vec![
                TopBarGroup::Page,
                TopBarGroup::Adjust,
                TopBarGroup::Bookmarks
            ]
        );
    }

    #[test]
    fn responsive_top_bar_layout_preserves_order_in_both_regions() {
        let layout = responsive_top_bar_layout(all_items(), 500.0, false, 80);

        assert_eq!(
            layout.inline_groups,
            vec![TopBarGroup::Open, TopBarGroup::Page, TopBarGroup::Bookmarks]
        );
        assert_eq!(
            layout.overflow_groups,
            vec![TopBarGroup::View, TopBarGroup::Adjust, TopBarGroup::Compare]
        );
    }

    #[test]
    fn responsive_top_bar_layout_prioritizes_active_compare_for_overflow() {
        let layout = responsive_top_bar_layout(all_items(), 820.0, true, 120);

        assert!(!layout.inline_groups.contains(&TopBarGroup::Compare));
        assert!(layout.overflow_groups.contains(&TopBarGroup::Compare));
    }
}
