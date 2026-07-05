use crate::core::natural::cmp_natural;
use crate::core::source::{classify_path, SourceKind};
use crate::core::worker::NavigationDirection;
use std::fs;
use std::path::{Path, PathBuf};

/// Where the current book sits in the sibling list: at an index, or — when its
/// path no longer exists on disk (deleted externally) — at the natural-sort
/// position it would occupy (`Gap`), so next/previous still resolve.
enum SiblingAnchor {
    Present(usize),
    Gap(usize),
}

pub(in crate::app) fn sibling_book_path(current: &Path, direction: isize) -> Option<PathBuf> {
    let entries = sibling_book_entries(current)?;
    match sibling_book_anchor(&entries, current)? {
        SiblingAnchor::Present(index) => {
            if entries.len() <= 1 {
                return None;
            }
            let next_index = if direction >= 0 {
                (index + 1) % entries.len()
            } else {
                (index + entries.len() - 1) % entries.len()
            };
            Some(entries[next_index].clone())
        }
        SiblingAnchor::Gap(insertion) => {
            if entries.is_empty() {
                return None;
            }
            let index = if direction >= 0 {
                insertion % entries.len()
            } else {
                (insertion + entries.len() - 1) % entries.len()
            };
            Some(entries[index].clone())
        }
    }
}

#[cfg(test)]
pub(in crate::app) fn adjacent_sibling_book_paths(
    current: &Path,
) -> Vec<(PathBuf, isize, &'static str)> {
    adjacent_sibling_book_paths_ordered(current, NavigationDirection::Forward)
}

pub(in crate::app) fn adjacent_sibling_book_paths_ordered(
    current: &Path,
    first_direction: NavigationDirection,
) -> Vec<(PathBuf, isize, &'static str)> {
    let Some(entries) = sibling_book_entries(current) else {
        return Vec::new();
    };
    let Some(anchor) = sibling_book_anchor(&entries, current) else {
        return Vec::new();
    };
    match anchor {
        SiblingAnchor::Present(_) => {
            if entries.len() <= 1 {
                return Vec::new();
            }
        }
        SiblingAnchor::Gap(_) => {
            if entries.is_empty() {
                return Vec::new();
            }
        }
    }
    let mut siblings = Vec::with_capacity(2);
    let ordered_directions = match first_direction {
        NavigationDirection::Forward => [(1, "next"), (-1, "previous")],
        NavigationDirection::Backward => [(-1, "previous"), (1, "next")],
    };
    for (direction, label) in ordered_directions {
        let index = match anchor {
            SiblingAnchor::Present(current_index) => {
                if direction >= 0 {
                    (current_index + 1) % entries.len()
                } else {
                    (current_index + entries.len() - 1) % entries.len()
                }
            }
            SiblingAnchor::Gap(insertion) => {
                if direction >= 0 {
                    insertion % entries.len()
                } else {
                    (insertion + entries.len() - 1) % entries.len()
                }
            }
        };
        let path = entries[index].clone();
        if siblings
            .iter()
            .any(|(existing, _, _): &(PathBuf, isize, &'static str)| same_path(existing, &path))
        {
            continue;
        }
        siblings.push((path, direction, label));
    }
    siblings
}

fn sibling_book_entries(current: &Path) -> Option<Vec<PathBuf>> {
    let parent = current.parent()?;
    let mut entries = fs::read_dir(parent)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| matches!(classify_path(path), SourceKind::Folder | SourceKind::ZipCbz))
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

fn sibling_book_current_index(entries: &[PathBuf], current: &Path) -> Option<usize> {
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

fn sibling_book_anchor(entries: &[PathBuf], current: &Path) -> Option<SiblingAnchor> {
    if let Some(index) = sibling_book_current_index(entries, current) {
        return Some(SiblingAnchor::Present(index));
    }
    if current.exists() {
        return None;
    }
    let insertion = entries.partition_point(|entry| {
        cmp_natural(&name(entry), &name(current)) == std::cmp::Ordering::Less
    });
    Some(SiblingAnchor::Gap(insertion))
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(in crate::app) fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

#[cfg(test)]
mod tests {
    use super::sibling_book_path;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn deleted_current_in_middle_resolves_both_neighbors() {
        let dir = temp_test_dir("sibling-gap-middle");
        fs::write(dir.join("book-1.cbz"), b"").unwrap();
        fs::write(dir.join("book-3.cbz"), b"").unwrap();

        let current = dir.join("book-2.cbz");
        assert_eq!(sibling_book_path(&current, 1), Some(dir.join("book-3.cbz")));
        assert_eq!(
            sibling_book_path(&current, -1),
            Some(dir.join("book-1.cbz"))
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn deleted_current_before_first_wraps_previous() {
        let dir = temp_test_dir("sibling-gap-before");
        fs::write(dir.join("book-1.cbz"), b"").unwrap();
        fs::write(dir.join("book-3.cbz"), b"").unwrap();

        let current = dir.join("book-0.cbz");
        assert_eq!(sibling_book_path(&current, 1), Some(dir.join("book-1.cbz")));
        assert_eq!(
            sibling_book_path(&current, -1),
            Some(dir.join("book-3.cbz"))
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn deleted_current_after_last_wraps_next() {
        let dir = temp_test_dir("sibling-gap-after");
        fs::write(dir.join("book-1.cbz"), b"").unwrap();
        fs::write(dir.join("book-3.cbz"), b"").unwrap();

        let current = dir.join("book-9.cbz");
        assert_eq!(sibling_book_path(&current, 1), Some(dir.join("book-1.cbz")));
        assert_eq!(
            sibling_book_path(&current, -1),
            Some(dir.join("book-3.cbz"))
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn deleted_current_with_single_survivor_resolves_to_it() {
        let dir = temp_test_dir("sibling-gap-single");
        fs::write(dir.join("book-1.cbz"), b"").unwrap();

        let current = dir.join("book-2.cbz");
        assert_eq!(sibling_book_path(&current, 1), Some(dir.join("book-1.cbz")));
        assert_eq!(
            sibling_book_path(&current, -1),
            Some(dir.join("book-1.cbz"))
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn existing_non_book_current_has_no_sibling() {
        let dir = temp_test_dir("sibling-non-book");
        fs::write(dir.join("book-1.cbz"), b"").unwrap();
        let photo = dir.join("photo.jpg");
        fs::write(&photo, b"").unwrap();

        assert_eq!(sibling_book_path(&photo, 1), None);
        assert_eq!(sibling_book_path(&photo, -1), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn deleted_current_uses_natural_sort_position() {
        let dir = temp_test_dir("sibling-gap-natural");
        fs::write(dir.join("book-2.cbz"), b"").unwrap();
        fs::write(dir.join("book-10.cbz"), b"").unwrap();

        let current = dir.join("book-3.cbz");
        assert_eq!(
            sibling_book_path(&current, 1),
            Some(dir.join("book-10.cbz"))
        );
        assert_eq!(
            sibling_book_path(&current, -1),
            Some(dir.join("book-2.cbz"))
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn present_single_book_still_has_no_sibling() {
        let dir = temp_test_dir("sibling-present-single");
        let current = dir.join("book-1.cbz");
        fs::write(&current, b"").unwrap();

        assert_eq!(sibling_book_path(&current, 1), None);
        assert_eq!(sibling_book_path(&current, -1), None);
        let _ = fs::remove_dir_all(dir);
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
