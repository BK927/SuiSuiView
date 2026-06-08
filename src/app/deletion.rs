use super::{
    commands::DeleteMode, opening::OpenViewFallback, perf, OpenOrigin, PendingDeleteDialog,
    SuiSuiViewApp,
};
use crate::core::natural::cmp_natural;
use crate::core::source::{classify_path, BookSource, SourceKind};
use crate::core::worker::NavigationDirection;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) struct DeleteAfterPlan {
    pub(in crate::app) target: PathBuf,
    success: Option<DeleteOpenPlan>,
    restore: DeleteOpenPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) struct DeleteOpenPlan {
    path: PathBuf,
    direction: NavigationDirection,
    explicit_page: Option<usize>,
}

impl DeleteOpenPlan {
    fn open(self, app: &mut SuiSuiViewApp, view_fallback: OpenViewFallback) {
        app.open_path_with_explicit_page(
            self.path,
            self.direction,
            self.explicit_page,
            Some(view_fallback),
        );
    }

    fn open_after_successful_delete(
        self,
        app: &mut SuiSuiViewApp,
        view_fallback: OpenViewFallback,
    ) {
        if self.can_use_adjacent_seed() {
            if let Some(cache) =
                app.take_adjacent_seed_for_successor(&self.path, self.direction, self.explicit_page)
            {
                app.install_adjacent_seed_cache(
                    cache,
                    self.direction,
                    view_fallback,
                    self.explicit_page,
                );
                return;
            }
            perf::record_adjacent_seed_prefetch_hit(false, app.target_long_edge);
        }

        app.open_path_after_successful_delete(
            self.path,
            self.direction,
            self.explicit_page,
            Some(view_fallback),
        );
    }

    fn can_use_adjacent_seed(&self) -> bool {
        self.explicit_page.is_none()
            && matches!(
                classify_path(&self.path),
                SourceKind::Folder | SourceKind::ZipCbz
            )
    }
}

impl SuiSuiViewApp {
    pub(in crate::app) fn delete_current_file(&mut self, mode: DeleteMode) {
        let Some(plan) = self.current_delete_after_plan() else {
            self.notify("No current file to delete.");
            return;
        };

        if should_confirm_delete(mode, self.settings.confirm_delete) {
            self.edge_prompt = None;
            self.close_bookmark_popover();
            self.pending_delete_dialog = Some(PendingDeleteDialog::new(mode, plan));
            return;
        }

        self.execute_delete_plan(mode, plan);
    }

    pub(in crate::app) fn execute_delete_plan(&mut self, mode: DeleteMode, plan: DeleteAfterPlan) {
        if !self.worker.clear_book_blocking() {
            self.notify(
                "Background decode is still finishing; deletion was not attempted. Try again soon.",
            );
            return;
        }

        let view_fallback = self.open_view_fallback();
        let result = delete_file(mode, &plan.target);
        let message = delete_result_message(mode, &plan.target, &result);
        match result {
            Ok(()) => {
                if let Some(next) = plan.success {
                    next.open_after_successful_delete(self, view_fallback);
                    self.notify(message);
                } else {
                    self.clear_local_book_state(message);
                }
            }
            Err(_) => {
                if !self.reload_current_book_after_delete_failure() {
                    plan.restore.open(self, view_fallback);
                }
                self.notify(message);
            }
        }
    }

    fn current_delete_after_plan(&self) -> Option<DeleteAfterPlan> {
        let source = self.source.as_ref()?;
        delete_after_plan_for(self.open_origin?, source.as_ref(), self.current_page)
    }

    fn reload_current_book_after_delete_failure(&mut self) -> bool {
        let Some(source) = self.source.clone() else {
            return false;
        };
        self.worker.load_book(
            source,
            self.worker_center_page(),
            self.last_nav_direction,
            self.target_long_edge,
            self.visible_page_count(),
            self.worker_options(),
        );
        true
    }
}

pub(in crate::app) fn delete_target_for(
    origin: OpenOrigin,
    source: &dyn BookSource,
    current_page: usize,
) -> Option<PathBuf> {
    match origin {
        OpenOrigin::ZipCbz => Some(source.source_path().to_path_buf()),
        OpenOrigin::Folder | OpenOrigin::SingleImage => source.page_file_path(current_page),
    }
}

fn delete_after_plan_for(
    origin: OpenOrigin,
    source: &dyn BookSource,
    current_page: usize,
) -> Option<DeleteAfterPlan> {
    let target = delete_target_for(origin, source, current_page)?;
    let restore = restore_plan_for(origin, source, current_page, &target);
    let success = match origin {
        OpenOrigin::ZipCbz => adjacent_book_after_delete(&target),
        OpenOrigin::Folder => folder_after_page_delete(source, current_page),
        OpenOrigin::SingleImage => adjacent_image_after_delete(&target),
    };
    Some(DeleteAfterPlan {
        target,
        success,
        restore,
    })
}

fn restore_plan_for(
    origin: OpenOrigin,
    source: &dyn BookSource,
    current_page: usize,
    target: &Path,
) -> DeleteOpenPlan {
    let direction = NavigationDirection::Forward;
    match origin {
        OpenOrigin::ZipCbz => DeleteOpenPlan {
            path: source.source_path().to_path_buf(),
            direction,
            explicit_page: Some(current_page),
        },
        OpenOrigin::Folder => DeleteOpenPlan {
            path: source.source_path().to_path_buf(),
            direction,
            explicit_page: Some(current_page),
        },
        OpenOrigin::SingleImage => DeleteOpenPlan {
            path: target.to_path_buf(),
            direction,
            explicit_page: None,
        },
    }
}

fn folder_after_page_delete(
    source: &dyn BookSource,
    current_page: usize,
) -> Option<DeleteOpenPlan> {
    let remaining_pages = source.page_count().checked_sub(1)?;
    if remaining_pages == 0 {
        return None;
    }
    let explicit_page = current_page.min(remaining_pages.saturating_sub(1));
    let direction = if explicit_page < current_page {
        NavigationDirection::Backward
    } else {
        NavigationDirection::Forward
    };
    Some(DeleteOpenPlan {
        path: source.source_path().to_path_buf(),
        direction,
        explicit_page: Some(explicit_page),
    })
}

fn adjacent_book_after_delete(target: &Path) -> Option<DeleteOpenPlan> {
    let entries = sorted_sibling_entries(target, |path| {
        matches!(classify_path(path), SourceKind::Folder | SourceKind::ZipCbz)
    })?;
    adjacent_entry_after_delete(&entries, target).map(|(path, direction)| DeleteOpenPlan {
        path,
        direction,
        explicit_page: None,
    })
}

fn adjacent_image_after_delete(target: &Path) -> Option<DeleteOpenPlan> {
    let entries = sorted_sibling_entries(target, |path| {
        matches!(classify_path(path), SourceKind::SingleImage)
    })?;
    adjacent_entry_after_delete(&entries, target).map(|(path, direction)| DeleteOpenPlan {
        path,
        direction,
        explicit_page: None,
    })
}

fn sorted_sibling_entries(current: &Path, keep: impl Fn(&Path) -> bool) -> Option<Vec<PathBuf>> {
    let parent = current.parent()?;
    let mut entries = fs::read_dir(parent)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| keep(path))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        let left_name = left
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        let right_name = right
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        cmp_natural(&left_name, &right_name)
    });
    Some(entries)
}

fn adjacent_entry_after_delete(
    entries: &[PathBuf],
    current: &Path,
) -> Option<(PathBuf, NavigationDirection)> {
    if entries.len() <= 1 {
        return None;
    }
    let current_index = current_index(entries, current)?;
    if let Some(next) = entries.get(current_index + 1) {
        return Some((next.clone(), NavigationDirection::Forward));
    }
    current_index
        .checked_sub(1)
        .and_then(|index| entries.get(index))
        .cloned()
        .map(|path| (path, NavigationDirection::Backward))
}

fn current_index(entries: &[PathBuf], current: &Path) -> Option<usize> {
    entries
        .iter()
        .position(|path| same_path(path, current))
        .or_else(|| entries.iter().position(|path| path == current))
        .or_else(|| {
            let current_name = current.file_name()?;
            entries
                .iter()
                .position(|path| path.file_name().is_some_and(|name| name == current_name))
        })
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn should_confirm_delete(mode: DeleteMode, confirm_delete: bool) -> bool {
    mode == DeleteMode::Permanent || confirm_delete
}

fn delete_file(mode: DeleteMode, target: &Path) -> Result<(), String> {
    match mode {
        DeleteMode::Recycle => trash::delete(target).map_err(|error| error.to_string()),
        DeleteMode::Permanent => fs::remove_file(target).map_err(|error| error.to_string()),
    }
}

fn delete_result_message(mode: DeleteMode, target: &Path, result: &Result<(), String>) -> String {
    match result {
        Ok(()) => match mode {
            DeleteMode::Recycle => format!("Moved to Recycle Bin: {}", target.display()),
            DeleteMode::Permanent => format!("Permanently deleted: {}", target.display()),
        },
        Err(error) => format!("Could not delete {}: {error}", target.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        adjacent_book_after_delete, adjacent_entry_after_delete, adjacent_image_after_delete,
        delete_after_plan_for, delete_target_for, should_confirm_delete, DeleteOpenPlan,
    };
    use crate::app::{commands::DeleteMode, OpenOrigin};
    use crate::core::source::{BookSource, SourceError};
    use crate::core::worker::NavigationDirection;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn adjacent_entry_after_delete_picks_next_without_wrap() {
        let entries = vec![path("001.jpg"), path("002.jpg"), path("003.jpg")];

        assert_eq!(
            adjacent_entry_after_delete(&entries, Path::new("002.jpg")),
            Some((path("003.jpg"), NavigationDirection::Forward))
        );
    }

    #[test]
    fn adjacent_entry_after_delete_picks_previous_for_last() {
        let entries = vec![path("001.jpg"), path("002.jpg"), path("003.jpg")];

        assert_eq!(
            adjacent_entry_after_delete(&entries, Path::new("003.jpg")),
            Some((path("002.jpg"), NavigationDirection::Backward))
        );
    }

    #[test]
    fn adjacent_entry_after_delete_does_not_wrap_single_entry() {
        let entries = vec![path("001.jpg")];

        assert_eq!(
            adjacent_entry_after_delete(&entries, Path::new("001.jpg")),
            None
        );
    }

    #[test]
    fn recycle_delete_can_skip_confirmation() {
        assert!(!should_confirm_delete(DeleteMode::Recycle, false));
        assert!(should_confirm_delete(DeleteMode::Recycle, true));
    }

    #[test]
    fn permanent_delete_always_confirms() {
        assert!(should_confirm_delete(DeleteMode::Permanent, false));
        assert!(should_confirm_delete(DeleteMode::Permanent, true));
    }

    #[test]
    fn delete_successor_seed_candidates_are_book_siblings_only() {
        let archive = DeleteOpenPlan {
            path: path("next.cbz"),
            direction: NavigationDirection::Forward,
            explicit_page: None,
        };
        let shifted_folder_page = DeleteOpenPlan {
            path: path("folder"),
            direction: NavigationDirection::Forward,
            explicit_page: Some(1),
        };
        let image = DeleteOpenPlan {
            path: path("next.jpg"),
            direction: NavigationDirection::Forward,
            explicit_page: None,
        };

        assert!(archive.can_use_adjacent_seed());
        assert!(!shifted_folder_page.can_use_adjacent_seed());
        assert!(!image.can_use_adjacent_seed());
    }

    #[test]
    fn adjacent_image_after_delete_uses_natural_sort() {
        let dir = temp_test_dir("delete-images-natural");
        fs::write(dir.join("image-1.jpg"), b"").unwrap();
        fs::write(dir.join("image-2.jpg"), b"").unwrap();
        fs::write(dir.join("image-10.jpg"), b"").unwrap();

        let plan = adjacent_image_after_delete(&dir.join("image-1.jpg")).unwrap();

        assert_eq!(plan.path, dir.join("image-2.jpg"));
        assert_eq!(plan.direction, NavigationDirection::Forward);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn adjacent_book_after_delete_uses_natural_sort_without_wrap() {
        let dir = temp_test_dir("delete-books-natural");
        fs::write(dir.join("book-1.cbz"), b"").unwrap();
        fs::create_dir_all(dir.join("book-2")).unwrap();
        fs::write(dir.join("book-10.cbz"), b"").unwrap();

        let middle = adjacent_book_after_delete(&dir.join("book-2")).unwrap();
        let last = adjacent_book_after_delete(&dir.join("book-10.cbz")).unwrap();

        assert_eq!(middle.path, dir.join("book-10.cbz"));
        assert_eq!(middle.direction, NavigationDirection::Forward);
        assert_eq!(last.path, dir.join("book-2"));
        assert_eq!(last.direction, NavigationDirection::Backward);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn folder_delete_reopens_same_folder_at_shifted_page() {
        let source = FakeSource {
            source_path: path("folder"),
            page_files: vec![path("001.jpg"), path("002.jpg"), path("003.jpg")],
        };

        let plan = delete_after_plan_for(OpenOrigin::Folder, &source, 1).unwrap();

        assert_eq!(plan.target, path("002.jpg"));
        assert_eq!(
            plan.success,
            Some(DeleteOpenPlan {
                path: path("folder"),
                direction: NavigationDirection::Forward,
                explicit_page: Some(1),
            })
        );
    }

    #[test]
    fn folder_delete_last_page_reopens_previous_page() {
        let source = FakeSource {
            source_path: path("folder"),
            page_files: vec![path("001.jpg"), path("002.jpg"), path("003.jpg")],
        };

        let plan = delete_after_plan_for(OpenOrigin::Folder, &source, 2).unwrap();

        assert_eq!(
            plan.success,
            Some(DeleteOpenPlan {
                path: path("folder"),
                direction: NavigationDirection::Backward,
                explicit_page: Some(1),
            })
        );
    }

    #[test]
    fn folder_delete_only_page_has_no_successor() {
        let source = FakeSource {
            source_path: path("folder"),
            page_files: vec![path("001.jpg")],
        };

        let plan = delete_after_plan_for(OpenOrigin::Folder, &source, 0).unwrap();

        assert_eq!(plan.success, None);
    }

    #[test]
    fn delete_failure_restore_plan_reopens_original_target() {
        let source = FakeSource {
            source_path: path("folder"),
            page_files: vec![path("001.jpg"), path("002.jpg"), path("003.jpg")],
        };

        let folder = delete_after_plan_for(OpenOrigin::Folder, &source, 2).unwrap();
        let single_image = delete_after_plan_for(OpenOrigin::SingleImage, &source, 1).unwrap();
        let archive = delete_after_plan_for(OpenOrigin::ZipCbz, &source, 0).unwrap();

        assert_eq!(
            folder.restore,
            DeleteOpenPlan {
                path: path("folder"),
                direction: NavigationDirection::Forward,
                explicit_page: Some(2),
            }
        );
        assert_eq!(
            single_image.restore,
            DeleteOpenPlan {
                path: path("002.jpg"),
                direction: NavigationDirection::Forward,
                explicit_page: None,
            }
        );
        assert_eq!(
            archive.restore,
            DeleteOpenPlan {
                path: path("folder"),
                direction: NavigationDirection::Forward,
                explicit_page: Some(0),
            }
        );
    }

    #[test]
    fn delete_target_tracks_origin() {
        let source = FakeSource {
            source_path: path("book.cbz"),
            page_files: vec![path("page-001.jpg")],
        };

        assert_eq!(
            delete_target_for(OpenOrigin::ZipCbz, &source, 0),
            Some(path("book.cbz"))
        );
        assert_eq!(
            delete_target_for(OpenOrigin::Folder, &source, 0),
            Some(path("page-001.jpg"))
        );
        assert_eq!(
            delete_target_for(OpenOrigin::SingleImage, &source, 0),
            Some(path("page-001.jpg"))
        );
    }

    struct FakeSource {
        source_path: PathBuf,
        page_files: Vec<PathBuf>,
    }

    impl BookSource for FakeSource {
        fn title(&self) -> &str {
            "fake"
        }

        fn source_path(&self) -> &Path {
            &self.source_path
        }

        fn book_id(&self) -> &str {
            "fake"
        }

        fn page_count(&self) -> usize {
            self.page_files.len()
        }

        fn page_name(&self, _index: usize) -> Option<&str> {
            None
        }

        fn page_file_path(&self, index: usize) -> Option<PathBuf> {
            self.page_files.get(index).cloned()
        }

        fn read_page(&self, _index: usize) -> Result<Vec<u8>, SourceError> {
            unreachable!("delete planning tests do not read pages")
        }
    }

    fn path(value: &str) -> PathBuf {
        PathBuf::from(value)
    }

    fn temp_test_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "suisuiview-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
