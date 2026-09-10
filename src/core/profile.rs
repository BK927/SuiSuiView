//! Process-scoped state isolation for local diagnostics. Validation happens
//! before state loading; an invalid override can never fall back to user data.
use directories::ProjectDirs;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

pub const PROFILE_DIR_ENV: &str = "SUISUIVIEW_PROFILE_DIR";
static PROFILE_OVERRIDE: OnceLock<Result<Option<PathBuf>, ProfileError>> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct ProfileError(String);

impl std::fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{PROFILE_DIR_ENV}: {}", self.0)
    }
}

impl std::error::Error for ProfileError {}

/// Freeze the environment once per process, including self-restarts which
/// inherit the override. This does not create or rename any directory.
pub fn initialize() -> Result<(), ProfileError> {
    override_directory().map(|_| ())
}

pub fn override_directory() -> Result<Option<&'static Path>, ProfileError> {
    PROFILE_OVERRIDE
        .get_or_init(|| {
            let Some(value) = std::env::var_os(PROFILE_DIR_ENV) else {
                return Ok(None);
            };
            let normal = ProjectDirs::from("", "", "SuiSuiView");
            let directory = validate_override(&value, normal.as_ref().map(|dirs| dirs.data_dir()))?;
            if let Some(normal) = normal.as_ref() {
                validate_override(&value, Some(normal.cache_dir()))?;
            }
            Ok(Some(directory))
        })
        .as_ref()
        .map(|value| value.as_deref())
        .map_err(Clone::clone)
}

fn validate_override(
    value: &OsStr,
    normal_directory: Option<&Path>,
) -> Result<PathBuf, ProfileError> {
    let path = Path::new(value);
    if value.is_empty() || !path.is_absolute() {
        return Err(ProfileError(
            "expected a nonempty absolute directory path".into(),
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ProfileError(
            "parent-directory components are not allowed".into(),
        ));
    }
    if path.parent().is_none() {
        return Err(ProfileError(
            "a filesystem root is not an isolated profile".into(),
        ));
    }
    let directory = resolve_existing_ancestor(path)?;
    if directory.exists() && !directory.is_dir() {
        return Err(ProfileError("the profile path is not a directory".into()));
    }
    if let Some(normal) = normal_directory {
        let normal = resolve_existing_ancestor(normal)?;
        if contains_path(&directory, &normal) || contains_path(&normal, &directory) {
            return Err(ProfileError(
                "the override overlaps the normal user profile".into(),
            ));
        }
    }
    // Existing links must not redirect a nominally isolated profile's state or
    // write locks back into another directory. Missing entries remain lazy.
    for entry in [
        "state.json",
        "state.json.write.lock",
        "books",
        "books/.write.lock",
        "cache",
        "cache/bookmark-thumbnails",
    ] {
        let resolved = resolve_existing_ancestor(&directory.join(entry))?;
        if !contains_path(&directory, &resolved) {
            return Err(ProfileError(format!(
                "{entry} points outside the isolated profile"
            )));
        }
    }
    Ok(directory)
}

fn resolve_existing_ancestor(path: &Path) -> Result<PathBuf, ProfileError> {
    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => {
                let mut resolved = std::fs::canonicalize(ancestor).map_err(|error| {
                    ProfileError(format!("cannot resolve the profile path: {error}"))
                })?;
                if !missing.is_empty() && !resolved.is_dir() {
                    return Err(ProfileError("a profile parent is not a directory".into()));
                }
                for component in missing.into_iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let component = ancestor.file_name().ok_or_else(|| {
                    ProfileError("the profile has no accessible existing parent".into())
                })?;
                missing.push(component);
                ancestor = ancestor.parent().ok_or_else(|| {
                    ProfileError("the profile has no accessible existing parent".into())
                })?;
            }
            Err(error) => {
                return Err(ProfileError(format!(
                    "cannot inspect the profile path: {error}"
                )))
            }
        }
    }
}

fn contains_path(directory: &Path, path: &Path) -> bool {
    #[cfg(windows)]
    {
        // Canonicalization resolves aliases; case folding also covers the
        // not-yet-created suffix on Windows' case-insensitive default volumes.
        PathBuf::from(path.to_string_lossy().to_lowercase())
            .starts_with(PathBuf::from(directory.to_string_lossy().to_lowercase()))
    }
    #[cfg(not(windows))]
    path.starts_with(directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    const CHILD_ENV: &str = "SUISUIVIEW_PROFILE_TEST_CHILD";

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "suisuiview-profile-validation-{}",
            std::process::id()
        ))
    }

    #[test]
    fn profile_override_rejects_empty_relative_parent_and_root_paths() {
        for path in [
            PathBuf::new(),
            PathBuf::from("relative-profile"),
            root().join("../escape"),
        ] {
            assert!(validate_override(path.as_os_str(), None).is_err());
        }
        let root_path = root().ancestors().last().unwrap().to_path_buf();
        assert!(validate_override(root_path.as_os_str(), None).is_err());
    }

    #[test]
    fn profile_override_rejects_normal_profile_overlap() {
        let normal = root().join("personal");
        for path in [normal.clone(), normal.join("test"), root()] {
            assert!(validate_override(path.as_os_str(), Some(&normal)).is_err());
        }
    }

    #[test]
    fn profile_override_accepts_missing_isolated_directory_without_creating_it() {
        let path = root().join("isolated");
        let normal = root().join("personal");
        let resolved = validate_override(path.as_os_str(), Some(&normal)).unwrap();
        assert!(resolved.is_absolute());
        assert!(!path.exists());
        assert!(resolved.ends_with("isolated"));
    }

    #[test]
    fn profile_override_rejects_existing_file_as_directory() {
        let executable = std::env::current_exe().unwrap();
        assert!(validate_override(executable.as_os_str(), None).is_err());
        assert!(validate_override(executable.join("child").as_os_str(), None).is_err());
    }

    #[test]
    fn profile_child_state_roundtrip() {
        let Some(mode) = std::env::var_os(CHILD_ENV) else {
            return;
        };
        if mode == "invalid" {
            assert!(initialize().is_err());
            return;
        }
        initialize().unwrap();
        let directory = override_directory()
            .unwrap()
            .expect("child must be isolated");
        let mut store = crate::core::state::StateStore::load();
        assert_eq!(store.path(), directory.join("state.json"));
        let mut settings = store.settings().clone();
        settings.single_instance = true;
        store.update_settings(settings).unwrap();
        assert!(directory.join("state.json").is_file());
        assert!(
            crate::core::state::StateStore::load()
                .settings()
                .single_instance
        );
    }

    #[test]
    fn profile_override_is_process_scoped_and_invalid_values_fail_before_state_load() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "suisuiview-isolated-profile-{}-{unique}",
            std::process::id()
        ));
        // create_dir (not create_dir_all) proves this test owns a new directory.
        std::fs::create_dir(&directory).unwrap();
        let run_child = |value: &OsStr, mode: &str| {
            std::process::Command::new(std::env::current_exe().unwrap())
                .arg("profile_child_state_roundtrip")
                .arg("--nocapture")
                .env(CHILD_ENV, mode)
                .env(PROFILE_DIR_ENV, value)
                .output()
                .unwrap()
        };
        let isolated = run_child(directory.as_os_str(), "isolated");
        let invalid = run_child(OsStr::new("relative-profile-must-not-load"), "invalid");
        // Only the newly created, absolute temporary directory is removed.
        assert!(directory.is_absolute());
        assert!(directory.starts_with(std::env::temp_dir()));
        std::fs::remove_dir_all(&directory).unwrap();
        assert!(
            isolated.status.success(),
            "{}",
            String::from_utf8_lossy(&isolated.stdout)
        );
        assert!(
            invalid.status.success(),
            "{}",
            String::from_utf8_lossy(&invalid.stdout)
        );
    }
}
