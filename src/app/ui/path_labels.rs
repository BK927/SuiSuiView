use crate::core::formats::descriptor_for_extension;
use std::path::Path;

pub(in crate::app) const RECENT_PATH_LABEL_CHARS: usize = 46;
pub(in crate::app) const BOOKMARK_PATH_LABEL_CHARS: usize = 42;

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

pub(in crate::app) fn bookmark_display_name(
    known_path: Option<&str>,
    page_name: Option<&str>,
    fallback_title: &str,
) -> String {
    let page_name = useful_page_name(page_name, fallback_title);
    match (known_path.filter(|path| !path.trim().is_empty()), page_name) {
        (Some(path), Some(page_name)) if is_archive_path(path) => {
            format!("{path} | {page_name}")
        }
        (Some(path), Some(page_name)) => folder_page_display_path(path, page_name),
        (Some(path), None) => path.to_owned(),
        (None, Some(page_name)) => page_name.to_owned(),
        (None, None) => fallback_title.to_owned(),
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

#[cfg(test)]
mod tests {
    use super::{bookmark_display_name, compact_start};

    #[test]
    fn compact_start_preserves_tail() {
        assert_eq!(
            compact_start("C:/books/series/page-001.png", 18),
            "...es/page-001.png"
        );
        assert_eq!(compact_start("short.png", 18), "short.png");
    }

    #[test]
    fn bookmark_display_uses_archive_separator() {
        assert_eq!(
            bookmark_display_name(
                Some("C:/books/book.cbz"),
                Some("chapter/page-001.jpg"),
                "ignored",
            ),
            "C:/books/book.cbz | chapter/page-001.jpg"
        );
    }

    #[test]
    fn bookmark_display_combines_folder_and_page_name() {
        let display = bookmark_display_name(
            Some("C:/books/series"),
            Some("chapter/page-001.jpg"),
            "ignored",
        );
        assert!(
            display.ends_with("chapter\\page-001.jpg") || display.ends_with("chapter/page-001.jpg")
        );
    }

    #[test]
    fn bookmark_display_falls_back_without_page_name() {
        assert_eq!(
            bookmark_display_name(Some("C:/books/series"), None, "p. 006"),
            "C:/books/series"
        );
        assert_eq!(bookmark_display_name(None, None, "cover.png"), "cover.png");
    }
}
