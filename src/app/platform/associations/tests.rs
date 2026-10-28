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

#[test]
fn windows_registry_round_trip_uses_only_a_unique_test_root() {
    use windows_sys::Win32::System::Registry::{RegDeleteTreeW, HKEY_CURRENT_USER};
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = format!("Software\\SuiSuiViewTests\\{}-{stamp}", std::process::id());
    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let wide: Vec<_> = self.0.encode_utf16().chain([0]).collect();
            unsafe {
                RegDeleteTreeW(HKEY_CURRENT_USER, wide.as_ptr());
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
