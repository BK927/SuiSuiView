//! Registrations from builds before per-installation namespaces: one shared
//! "SuiSuiView" entry in Default Apps plus `SuiSuiView.<ext>` ProgIDs. They are
//! removed after the next register or unregister. A ProgID that may still be
//! the user's default is kept, so double-clicking those files keeps working.
//! Read failures keep entries rather than block.
use super::registry::Registry;
use super::{normalize, Change, REGISTERED_APPS};

const NAME: &str = "SuiSuiView";
const CAPABILITIES: &str = "Software\\SuiSuiView\\Capabilities";

pub(super) fn cleanup(registry: &impl Registry, known: &[&str]) -> Vec<Change> {
    let associations = format!("{CAPABILITIES}\\FileAssociations");
    let mut changes = vec![
        removal(CAPABILITIES, "ApplicationName"),
        removal(CAPABILITIES, "ApplicationDescription"),
    ];
    for extension in known.iter().filter_map(|e| normalize(e).ok()) {
        let prog_id = format!("{NAME}.{}", &extension[1..]);
        if !may_be_default(registry, &extension, &prog_id) {
            let class = format!("Software\\Classes\\{prog_id}");
            changes.extend([
                removal(&class, ""),
                removal(&class, "FriendlyTypeName"),
                removal(&format!("{class}\\DefaultIcon"), ""),
                removal(&format!("{class}\\shell\\open\\command"), ""),
            ]);
        }
        changes.push(removal(&associations, &extension));
    }
    changes.retain(|change| matches!(registry.read(&change.key, &change.name), Ok(Some(_))));
    // Unpublish the Default Apps entry first, and only ours: another app could
    // use the same name.
    let published = registry
        .read(REGISTERED_APPS, NAME)
        .ok()
        .flatten()
        .and_then(|value| value.as_string());
    if published.as_deref() == Some(CAPABILITIES) {
        changes.insert(0, removal(REGISTERED_APPS, NAME));
    }
    changes
}

fn removal(key: &str, name: &str) -> Change {
    Change {
        key: key.to_owned(),
        name: name.to_owned(),
        value: None,
    }
}

/// `UserChoice` holds the user's default; newer Windows builds add
/// `UserChoiceLatest`, whose layout is undocumented, so both shapes count.
fn may_be_default(registry: &impl Registry, extension: &str, prog_id: &str) -> bool {
    let file_exts =
        format!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\{extension}");
    [
        (format!("{file_exts}\\UserChoice"), "ProgId"),
        (format!("{file_exts}\\UserChoiceLatest"), "ProgId"),
        (format!("{file_exts}\\UserChoiceLatest\\ProgId"), "ProgId"),
        (format!("Software\\Classes\\{extension}"), ""),
    ]
    .iter()
    .any(|(key, name)| match registry.read(key, name) {
        Ok(value) => value
            .and_then(|value| value.as_string())
            .is_some_and(|value| value.eq_ignore_ascii_case(prog_id)),
        Err(_) => true,
    })
}
