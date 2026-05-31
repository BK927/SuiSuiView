use crate::core::natural::cmp_natural;
use crate::core::source::{classify_path, SourceKind};
use crate::core::worker::NavigationDirection;
use std::fs;
use std::path::{Path, PathBuf};

pub(in crate::app) fn sibling_book_path(current: &Path, direction: isize) -> Option<PathBuf> {
    let entries = sibling_book_entries(current)?;
    if entries.len() <= 1 {
        return None;
    }
    let current_index = sibling_book_current_index(&entries, current);
    let next_index = if direction >= 0 {
        (current_index + 1) % entries.len()
    } else {
        (current_index + entries.len() - 1) % entries.len()
    };
    Some(entries[next_index].clone())
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
    if entries.len() <= 1 {
        return Vec::new();
    }
    let current_index = sibling_book_current_index(&entries, current);
    let mut siblings = Vec::with_capacity(2);
    let ordered_directions = match first_direction {
        NavigationDirection::Forward => [(1, "next"), (-1, "previous")],
        NavigationDirection::Backward => [(-1, "previous"), (1, "next")],
    };
    for (direction, label) in ordered_directions {
        let index = if direction >= 0 {
            (current_index + 1) % entries.len()
        } else {
            (current_index + entries.len() - 1) % entries.len()
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

fn sibling_book_current_index(entries: &[PathBuf], current: &Path) -> usize {
    entries
        .iter()
        .position(|path| same_path(path, current))
        .unwrap_or_else(|| {
            entries
                .iter()
                .position(|path| path == current)
                .unwrap_or_default()
        })
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}
