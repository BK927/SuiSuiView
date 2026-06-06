use crate::core::state::TopBarItems;

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

pub(in crate::app::ui) fn visible_top_bar_groups(
    items: TopBarItems,
) -> impl Iterator<Item = TopBarGroup> {
    TopBarGroup::ALL
        .into_iter()
        .filter(move |group| group.is_visible(items))
}

#[cfg(test)]
mod tests {
    use super::{visible_top_bar_groups, TopBarGroup};
    use crate::core::state::TopBarItems;

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
}
