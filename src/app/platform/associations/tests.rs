use super::*;
use std::collections::BTreeMap;

#[derive(Default)]
struct MemoryRegistry {
    values: BTreeMap<(String, String), Value>,
    writes: usize,
    fail_at: Option<usize>,
}
impl Registry for MemoryRegistry {
    fn read(&self, key: &str, name: &str) -> Result<Option<Value>, String> {
        Ok(self.values.get(&(key.into(), name.into())).cloned())
    }
    fn write(&mut self, key: &str, name: &str, value: Option<&Value>) -> Result<(), String> {
        self.writes += 1;
        if self.fail_at == Some(self.writes) {
            return Err("injected write failure".into());
        }
        let key = (key.into(), name.into());
        if let Some(value) = value {
            self.values.insert(key, value.clone());
        } else {
            self.values.remove(&key);
        }
        Ok(())
    }
}

#[test]
fn invalid_extensions_are_rejected_before_mutation() {
    let context = Context::for_executable(Path::new("C:\\Viewer\\suisuiview.exe"), false).unwrap();
    for invalid in ["", "jpg\\shell", "a\0b", "png\"", "not-supported"] {
        assert!(plan(&context, &[invalid], &["png", "cbz"]).is_err());
    }
}

#[test]
fn failure_restores_all_original_values_at_every_write_boundary() {
    let context =
        Context::for_executable(Path::new("C:\\한글 경로\\suisuiview.exe"), false).unwrap();
    let first = plan(&context, &["png"], &["png", "cbz"]).unwrap();
    let second = plan(&context, &["cbz"], &["png", "cbz"]).unwrap();
    for failure in 1..=second.len() {
        let mut registry = MemoryRegistry::default();
        apply(&mut registry, &first).unwrap();
        let before = registry.values.clone();
        registry.fail_at = Some(registry.writes + failure);
        assert!(apply(&mut registry, &second).is_err());
        assert_eq!(registry.values, before, "failed write {failure}");
    }
}

#[test]
fn installed_and_multiple_portable_copies_preserve_each_other_and_user_choice() {
    let installed =
        Context::for_executable(Path::new("C:\\Program Files\\suisuiview.exe"), true).unwrap();
    let portable =
        Context::for_executable(Path::new("C:\\한글 폴더\\suisuiview.exe"), false).unwrap();
    let other = Context::for_executable(Path::new("D:\\copy\\suisuiview.exe"), false).unwrap();
    assert_ne!(portable.id, other.id);
    let mut registry = MemoryRegistry::default();
    let user_choice = (
        "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.png\\UserChoice".into(),
        "ProgId".into(),
    );
    registry
        .values
        .insert(user_choice.clone(), Value::string("Other.App"));
    for context in [&installed, &portable, &other] {
        apply(
            &mut registry,
            &plan(context, &["png"], &["png", "zip"]).unwrap(),
        )
        .unwrap();
    }
    let command = registry
        .read(
            &format!(
                "Software\\Classes\\{}.png\\shell\\open\\command",
                portable.id
            ),
            "",
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        command.as_string().unwrap(),
        "\"C:\\한글 폴더\\suisuiview.exe\" \"%1\""
    );
    assert!(registry
        .read(
            "Software\\Classes\\.png\\OpenWithProgids",
            &format!("{}.png", portable.id)
        )
        .unwrap()
        .is_some());
    apply(
        &mut registry,
        &plan(&installed, &[], &["png", "zip"]).unwrap(),
    )
    .unwrap();
    assert!(registry
        .read(REGISTERED_APPS, &installed.id)
        .unwrap()
        .is_none());
    assert!(registry
        .read(REGISTERED_APPS, &portable.id)
        .unwrap()
        .is_some());
    assert!(registry.read(REGISTERED_APPS, &other.id).unwrap().is_some());
    assert_eq!(registry.values[&user_choice], Value::string("Other.App"));
}

const LEGACY_CAPABILITIES: &str = "Software\\SuiSuiView\\Capabilities";
const PNG_USER_CHOICE: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\.png\\UserChoice";

/// What builds before per-installation namespaces wrote on registration.
fn seed_legacy_registration(registry: &mut MemoryRegistry, extensions: &[&str]) {
    let mut set = |key: &str, name: &str, value: &str| {
        registry
            .values
            .insert((key.into(), name.into()), Value::string(value));
    };
    set(LEGACY_CAPABILITIES, "ApplicationName", "SuiSuiView");
    set(
        LEGACY_CAPABILITIES,
        "ApplicationDescription",
        "Fast native image and comic viewer",
    );
    set(REGISTERED_APPS, "SuiSuiView", LEGACY_CAPABILITIES);
    for extension in extensions {
        let class = format!("Software\\Classes\\SuiSuiView.{extension}");
        set(
            &format!("{LEGACY_CAPABILITIES}\\FileAssociations"),
            &format!(".{extension}"),
            &format!("SuiSuiView.{extension}"),
        );
        set(&class, "", "SuiSuiView file");
        set(&class, "FriendlyTypeName", "SuiSuiView file");
        set(
            &format!("{class}\\DefaultIcon"),
            "",
            "\"C:\\old\\suisuiview.exe\",0",
        );
        set(
            &format!("{class}\\shell\\open\\command"),
            "",
            "\"C:\\old\\suisuiview.exe\" \"%1\"",
        );
    }
}

fn legacy_values(registry: &MemoryRegistry) -> Vec<(String, String)> {
    registry
        .values
        .keys()
        .filter(|(key, name)| {
            key.starts_with("Software\\SuiSuiView\\")
                || key.starts_with("Software\\Classes\\SuiSuiView.png")
                || key.starts_with("Software\\Classes\\SuiSuiView.cbz")
                || (key == REGISTERED_APPS && name == "SuiSuiView")
        })
        .cloned()
        .collect()
}

const KNOWN: [&str; 2] = ["png", "cbz"];

#[test]
fn legacy_registration_is_removed_on_register_and_unregister() {
    let context = Context::for_executable(Path::new("C:\\Viewer\\suisuiview.exe"), false).unwrap();
    for selected in [&["png"][..], &[]] {
        let mut registry = MemoryRegistry::default();
        seed_legacy_registration(&mut registry, &KNOWN);

        write_registration(&mut registry, &context, selected, &KNOWN).unwrap();

        assert_eq!(legacy_values(&registry), Vec::<(String, String)>::new());
        assert_eq!(
            registry
                .read(REGISTERED_APPS, &context.id)
                .unwrap()
                .is_some(),
            !selected.is_empty()
        );
    }
}

#[test]
fn legacy_prog_id_survives_while_it_may_be_the_users_default() {
    let context = Context::for_executable(Path::new("C:\\Viewer\\suisuiview.exe"), false).unwrap();
    let mut registry = MemoryRegistry::default();
    seed_legacy_registration(&mut registry, &KNOWN);
    registry.values.insert(
        (PNG_USER_CHOICE.into(), "ProgId".into()),
        Value::string("SuiSuiView.png"),
    );

    write_registration(&mut registry, &context, &["png"], &KNOWN).unwrap();

    let command = "Software\\Classes\\SuiSuiView.png\\shell\\open\\command";
    assert!(registry.read(command, "").unwrap().is_some());
    assert!(registry
        .read("Software\\Classes\\SuiSuiView.cbz", "")
        .unwrap()
        .is_none());
    assert!(registry
        .read(REGISTERED_APPS, "SuiSuiView")
        .unwrap()
        .is_none());
    assert!(registry
        .read(LEGACY_CAPABILITIES, "ApplicationName")
        .unwrap()
        .is_none());
}

#[test]
fn unreadable_user_choice_keeps_the_legacy_prog_id() {
    struct DeniedUserChoice(MemoryRegistry);
    impl Registry for DeniedUserChoice {
        fn read(&self, key: &str, name: &str) -> Result<Option<Value>, String> {
            if key.contains("\\UserChoice") {
                return Err("access denied".into());
            }
            self.0.read(key, name)
        }
        fn write(&mut self, key: &str, name: &str, value: Option<&Value>) -> Result<(), String> {
            self.0.write(key, name, value)
        }
    }
    let context = Context::for_executable(Path::new("C:\\Viewer\\suisuiview.exe"), false).unwrap();
    let mut registry = DeniedUserChoice(MemoryRegistry::default());
    seed_legacy_registration(&mut registry.0, &["png"]);

    write_registration(&mut registry, &context, &["png"], &KNOWN).unwrap();

    assert!(registry
        .read("Software\\Classes\\SuiSuiView.png", "")
        .unwrap()
        .is_some());
    assert!(registry
        .read(REGISTERED_APPS, "SuiSuiView")
        .unwrap()
        .is_none());
}

#[test]
fn a_failed_legacy_cleanup_never_undoes_the_new_registration() {
    let context = Context::for_executable(Path::new("C:\\Viewer\\suisuiview.exe"), false).unwrap();
    let mut seeded = MemoryRegistry::default();
    seed_legacy_registration(&mut seeded, &KNOWN);
    let registration = plan(&context, &["png"], &KNOWN).unwrap().len();
    let cleanup = legacy::cleanup(&seeded, &KNOWN).len();
    assert!(cleanup > 0);
    for failure in 1..=registration + cleanup {
        let mut registry = MemoryRegistry {
            values: seeded.values.clone(),
            fail_at: Some(failure),
            ..MemoryRegistry::default()
        };
        let result = write_registration(&mut registry, &context, &["png"], &KNOWN);
        if failure <= registration {
            assert!(result.is_err());
            assert_eq!(registry.values, seeded.values, "failed write {failure}");
        } else {
            result.unwrap();
            assert!(registry
                .read(REGISTERED_APPS, &context.id)
                .unwrap()
                .is_some());
            let legacy = |r: &MemoryRegistry| {
                legacy_values(r)
                    .into_iter()
                    .map(|key| r.values[&key].clone())
                    .collect::<Vec<_>>()
            };
            assert_eq!(legacy(&registry), legacy(&seeded), "failed write {failure}");
        }
    }
}

#[test]
fn nothing_legacy_means_no_extra_writes_and_foreign_entries_stay() {
    let mut registry = MemoryRegistry::default();
    assert!(legacy::cleanup(&registry, &KNOWN).is_empty());

    registry.values.insert(
        (REGISTERED_APPS.into(), "SuiSuiView".into()),
        Value::string("Software\\SomeoneElse\\Capabilities"),
    );
    assert!(legacy::cleanup(&registry, &KNOWN).is_empty());
}

#[test]
fn windows_registry_round_trip_uses_only_a_unique_test_root() {
    use windows_sys::Win32::System::Registry::{RegDeleteKeyW, RegDeleteTreeW, HKEY_CURRENT_USER};
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = format!("Software\\SuiSuiViewTests\\{}-{stamp}", std::process::id());
    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let wide: Vec<_> = self.0.encode_utf16().chain([0]).collect();
            // The shared parent goes too; RegDeleteKeyW refuses while another
            // run still has a subkey there.
            let parent: Vec<_> = "Software\\SuiSuiViewTests"
                .encode_utf16()
                .chain([0])
                .collect();
            unsafe {
                RegDeleteTreeW(HKEY_CURRENT_USER, wide.as_ptr());
                RegDeleteKeyW(HKEY_CURRENT_USER, parent.as_ptr());
            }
        }
    }
    let _cleanup = Cleanup(root.clone());
    let context =
        Context::for_executable(Path::new("C:\\한글 경로\\suisuiview.exe"), false).unwrap();
    let scoped = |selected: &[&str]| {
        plan(&context, selected, &["png", "cbz"])
            .unwrap()
            .into_iter()
            .map(|mut c| {
                c.key = format!("{root}\\{}", c.key);
                c
            })
            .collect::<Vec<_>>()
    };
    let changes = scoped(&["png", "cbz"]);
    let mut registry = WindowsRegistry;
    apply(&mut registry, &changes).unwrap();
    for change in &changes {
        assert_eq!(
            registry.read(&change.key, &change.name).unwrap(),
            change.value
        );
    }
    apply(&mut registry, &scoped(&[])).unwrap();
    for change in &changes {
        assert!(registry.read(&change.key, &change.name).unwrap().is_none());
    }
}
