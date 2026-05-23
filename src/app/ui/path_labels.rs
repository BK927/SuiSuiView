use crate::core::formats::descriptor_for_extension;
use std::path::{Path, PathBuf};

pub(in crate::app) const RECENT_PATH_LABEL_CHARS: usize = 180;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) struct BookmarkDisplayParts {
    pub(in crate::app) title: String,
    pub(in crate::app) context: String,
    pub(in crate::app) full: String,
}

pub(in crate::app) fn compact_start(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_owned();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }

    let tail: String = text.chars().skip(count - (max_chars - 3)).collect();
    format!("...{tail}")
}

pub(in crate::app) fn compact_start_for_two_lines(text: &str, max_chars: usize) -> String {
    compact_start(text, max_chars)
}

pub(in crate::app) fn bookmark_display_parts(
    known_path: Option<&str>,
    page_name: Option<&str>,
    fallback_title: &str,
) -> BookmarkDisplayParts {
    let page_name = useful_page_name(page_name, fallback_title);
    match (known_path.filter(|path| !path.trim().is_empty()), page_name) {
        (Some(path), Some(page_name)) if is_archive_path(path) => BookmarkDisplayParts {
            title: page_file_name(page_name),
            context: path.to_owned(),
            full: format!("{path} | {page_name}"),
        },
        (Some(path), Some(page_name)) => {
            let full = folder_page_display_path(path, page_name);
            BookmarkDisplayParts {
                title: page_file_name(page_name),
                context: parent_display(&full).unwrap_or_else(|| path.to_owned()),
                full,
            }
        }
        (Some(path), None) => BookmarkDisplayParts {
            title: path_file_name(path).unwrap_or_else(|| path.to_owned()),
            context: parent_display(path).unwrap_or_default(),
            full: path.to_owned(),
        },
        (None, Some(page_name)) => BookmarkDisplayParts {
            title: page_file_name(page_name),
            context: parent_display(page_name).unwrap_or_default(),
            full: page_name.to_owned(),
        },
        (None, None) => BookmarkDisplayParts {
            title: fallback_title.to_owned(),
            context: String::new(),
            full: fallback_title.to_owned(),
        },
    }
}

fn folder_page_display_path(path: &str, page_name: &str) -> String {
    let path = Path::new(path);
    let root = if is_image_path(path) {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    root.join(page_name).display().to_string()
}

fn useful_page_name<'a>(page_name: Option<&'a str>, fallback_title: &'a str) -> Option<&'a str> {
    page_name
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            let title = fallback_title.trim();
            (!title.is_empty() && !looks_like_page_label(title)).then_some(title)
        })
}

fn looks_like_page_label(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    lower.starts_with("p.") || lower.starts_with("p. ")
}

fn is_archive_path(path: &str) -> bool {
    matches!(extension(path).as_deref(), Some("zip" | "cbz"))
}

fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .and_then(descriptor_for_extension)
        .is_some_and(|descriptor| descriptor.is_image_page())
}

fn extension(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_lowercase())
}

fn page_file_name(path: &str) -> String {
    path_file_name(path).unwrap_or_else(|| path.to_owned())
}

fn path_file_name(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

fn parent_display(path: &str) -> Option<String> {
    PathBuf::from(path)
        .parent()
        .map(|parent| parent.display().to_string())
        .filter(|parent| !parent.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        bookmark_display_parts, compact_start, compact_start_for_two_lines, RECENT_PATH_LABEL_CHARS,
    };

    #[test]
    fn compact_start_preserves_tail() {
        assert_eq!(
            compact_start("C:/books/series/page-001.png", 18),
            "...es/page-001.png"
        );
        assert_eq!(compact_start("short.png", 18), "short.png");
    }

    #[test]
    fn recent_path_label_limit_keeps_common_windows_paths_readable() {
        assert_eq!(
            compact_start(
                "C:/Users/dead4/Pictures/wallpaper/kanata03.png",
                RECENT_PATH_LABEL_CHARS
            ),
            "C:/Users/dead4/Pictures/wallpaper/kanata03.png"
        );
    }

    #[test]
    fn compact_start_for_two_lines_preserves_long_tail() {
        assert_eq!(
            compact_start_for_two_lines(
                "C:/Users/dead4/Pictures/아주 긴 폴더 이름/wallpaper/kanata03.png",
                34,
            ),
            "... 긴 폴더 이름/wallpaper/kanata03.png"
        );
    }

    #[test]
    fn bookmark_display_uses_archive_separator() {
        assert_eq!(
            bookmark_display_parts(
                Some("C:/books/book.cbz"),
                Some("chapter/page-001.jpg"),
                "ignored",
            )
            .full,
            "C:/books/book.cbz | chapter/page-001.jpg"
        );
    }

    #[test]
    fn bookmark_display_parts_prioritize_file_name() {
        let folder = bookmark_display_parts(
            Some("C:/books/series"),
            Some("chapter/page-001.jpg"),
            "ignored",
        );
        assert_eq!(folder.title, "page-001.jpg");
        assert!(folder.context.contains("series"));

        let archive = bookmark_display_parts(
            Some("C:/books/book.cbz"),
            Some("chapter/page-001.jpg"),
            "ignored",
        );
        assert_eq!(archive.title, "page-001.jpg");
        assert_eq!(archive.context, "C:/books/book.cbz");
    }

    #[test]
    fn bookmark_display_combines_folder_and_page_name() {
        let display = bookmark_display_parts(
            Some("C:/books/series"),
            Some("chapter/page-001.jpg"),
            "ignored",
        )
        .full;
        assert!(
            display.ends_with("chapter\\page-001.jpg") || display.ends_with("chapter/page-001.jpg")
        );
    }

    #[test]
    fn bookmark_display_falls_back_without_page_name() {
        assert_eq!(
            bookmark_display_parts(Some("C:/books/series"), None, "p. 006").full,
            "C:/books/series"
        );
        assert_eq!(
            bookmark_display_parts(None, None, "cover.png").full,
            "cover.png"
        );
    }
}
