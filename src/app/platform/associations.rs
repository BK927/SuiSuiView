//! Per-user registration. Each installation owns a distinct namespace; UserChoice
//! and extension defaults are deliberately outside the write set.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};

mod registry;
use registry::{Registry, Value, WindowsRegistry};

const INSTALLER_KEY: &str = "Software\\BK927\\SuiSuiView\\Installer";
const REGISTERED_APPS: &str = "Software\\RegisteredApplications";

struct Context {
    id: String,
    executable: String,
}

impl Context {
    fn current() -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let location = WindowsRegistry
            .read(INSTALLER_KEY, "InstallLocation")?
            .and_then(|v| v.as_string())
            .map(PathBuf::from);
        let installed = location.as_deref().and_then(|p| p.canonicalize().ok())
            == exe.parent().and_then(|p| p.canonicalize().ok());
        Self::for_executable(&exe, location.is_some() && installed)
    }

    fn for_executable(exe: &Path, installed: bool) -> Result<Self, String> {
        let executable = exe
            .to_str()
            .ok_or("Executable path is not valid Unicode")?
            .to_owned();
        if executable.contains(['"', '\0']) {
            return Err("Invalid executable path".into());
        }
        let id = if installed {
            "SuiSuiView.Installed".to_owned()
        } else {
            let hash = blake3::hash(executable.to_lowercase().as_bytes()).to_hex();
            format!("SuiSuiView.Portable.{}", &hash[..16])
        };
        Ok(Self { id, executable })
    }

    fn capabilities(&self) -> String {
        format!(
            "Software\\BK927\\SuiSuiView\\Associations\\{}\\Capabilities",
            self.id
        )
    }
}

#[derive(Clone)]
struct Change {
    key: String,
    name: String,
    value: Option<Value>,
}

fn normalize(extension: &str) -> Result<String, String> {
    let extension = extension
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if extension.is_empty()
        || !extension
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(format!("Invalid file extension: {extension}"));
    }
    Ok(format!(".{extension}"))
}

fn plan(context: &Context, selected: &[&str], known: &[&str]) -> Result<Vec<Change>, String> {
    let known: BTreeSet<_> = known
        .iter()
        .map(|e| normalize(e))
        .collect::<Result<_, _>>()?;
    let selected: BTreeSet<_> = selected
        .iter()
        .map(|e| normalize(e))
        .collect::<Result<_, _>>()?;
    if !selected.is_subset(&known) {
        return Err("Unsupported file extension selected".into());
    }
    let active = !selected.is_empty();
    let capabilities = context.capabilities();
    let mut changes = Vec::new();
    let mut add = |key: String, name: &str, value: String, enabled: bool| {
        changes.push(Change {
            key,
            name: name.into(),
            value: enabled.then(|| Value::string(&value)),
        });
    };
    for (name, value) in [
        ("ApplicationName", "SuiSuiView".to_owned()),
        (
            "ApplicationDescription",
            "Image and comic viewer".to_owned(),
        ),
        ("ApplicationIcon", format!("\"{}\",0", context.executable)),
        ("OwnedExecutable", context.executable.clone()),
    ] {
        add(capabilities.clone(), name, value, active);
    }
    for extension in known {
        let enabled = selected.contains(&extension);
        let prog_id = format!("{}.{}", context.id, &extension[1..]);
        let class = format!("Software\\Classes\\{prog_id}");
        add(
            format!("{capabilities}\\FileAssociations"),
            &extension,
            prog_id.clone(),
            enabled,
        );
        add(
            format!("Software\\Classes\\{extension}\\OpenWithProgids"),
            &prog_id,
            String::new(),
            enabled,
        );
        add(
            class.clone(),
            "",
            format!("SuiSuiView {} file", &extension[1..]),
            enabled,
        );
        add(
            class.clone(),
            "FriendlyTypeName",
            format!("SuiSuiView {} file", &extension[1..]),
            enabled,
        );
        add(
            format!("{class}\\DefaultIcon"),
            "",
            format!("\"{}\",0", context.executable),
            enabled,
        );
        add(
            format!("{class}\\shell\\open\\command"),
            "",
            format!("\"{}\" \"%1\"", context.executable),
            enabled,
        );
    }
    // Publish the complete candidate only after its commands have been written.
    add(REGISTERED_APPS.into(), &context.id, capabilities, active);
    Ok(changes)
}

fn apply(registry: &mut impl Registry, changes: &[Change]) -> Result<(), String> {
    // Read every original value before the first mutation, including its type.
    let before = changes
        .iter()
        .map(|c| registry.read(&c.key, &c.name))
        .collect::<Result<Vec<_>, _>>()?;
    for (index, change) in changes.iter().enumerate() {
        if let Err(error) = registry.write(&change.key, &change.name, change.value.as_ref()) {
            let mut recovery_errors = Vec::new();
            for (change, original) in changes[..=index].iter().zip(&before).rev() {
                if let Err(e) = registry.write(&change.key, &change.name, original.as_ref()) {
                    recovery_errors.push(e);
                }
            }
            registry.remove_empty_keys(changes);
            return Err(if recovery_errors.is_empty() {
                error
            } else {
                format!(
                    "{error}; registry rollback also failed: {}",
                    recovery_errors.join("; ")
                )
            });
        }
    }
    registry.remove_empty_keys(changes);
    Ok(())
}

fn update(selected: &[&str], known: &[&str]) -> Result<(), String> {
    // Invalid requests must fail before reading or changing registrations.
    for extension in selected.iter().chain(known) {
        normalize(extension)?;
    }
    let context = Context::current()?;
    let changes = plan(&context, selected, known)?;
    let mut registry = WindowsRegistry;
    if let Some(owner) = registry
        .read(&context.capabilities(), "OwnedExecutable")?
        .and_then(|v| v.as_string())
    {
        if !owner.eq_ignore_ascii_case(&context.executable) {
            return Err("This file registration belongs to a different executable".into());
        }
    }
    apply(&mut registry, &changes)?;
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
    Ok(())
}

pub(in crate::app) fn register_file_associations(
    selected: &[&str],
    known: &[&str],
) -> Result<usize, String> {
    update(selected, known)?;
    Ok(selected
        .iter()
        .map(|e| normalize(e))
        .collect::<Result<BTreeSet<_>, _>>()?
        .len())
}

pub(in crate::app) fn unregister_file_associations(known: &[&str]) -> Result<(), String> {
    update(&[], known)
}

pub(in crate::app) fn open_windows_default_apps_for_suisuiview() -> Result<(), String> {
    let context = Context::current()?;
    Command::new("explorer.exe")
        .arg(format!(
            "ms-settings:defaultapps?registeredAppUser={}",
            context.id
        ))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
