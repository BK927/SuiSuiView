use super::Change;
use std::collections::BTreeSet;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Value { kind: u32, bytes: Vec<u8> }

impl Value {
    pub(super) fn string(value: &str) -> Self {
        Self { kind: REG_SZ, bytes: wide(value).iter().flat_map(|c| c.to_le_bytes()).collect() }
    }
    pub(super) fn as_string(&self) -> Option<String> {
        if self.kind != REG_SZ || self.bytes.len() % 2 != 0 { return None; }
        let words: Vec<_> = self.bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
            .take_while(|&c| c != 0).collect();
        String::from_utf16(&words).ok()
    }
}

pub(super) trait Registry {
    fn read(&self, key: &str, name: &str) -> Result<Option<Value>, String>;
    fn write(&mut self, key: &str, name: &str, value: Option<&Value>) -> Result<(), String>;
    fn remove_empty_keys(&mut self, _changes: &[Change]) {}
}

pub(super) struct WindowsRegistry;
struct Key(HKEY);
impl Drop for Key { fn drop(&mut self) { unsafe { RegCloseKey(self.0); } } }
fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain([0]).collect() }
fn error(action: &str, status: u32) -> String { format!("Registry {action} failed (Windows error {status})") }

impl WindowsRegistry {
    fn open(path: &str, access: u32, create: bool) -> Result<Option<Key>, String> {
        let mut key = std::ptr::null_mut();
        let path = wide(path);
        let result = unsafe {
            if create { RegCreateKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, std::ptr::null(),
                REG_OPTION_NON_VOLATILE, access, std::ptr::null(), &mut key, std::ptr::null_mut()) }
            else { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, access, &mut key) }
        };
        match result {
            ERROR_SUCCESS => Ok(Some(Key(key))),
            ERROR_FILE_NOT_FOUND => Ok(None),
            _ => Err(error("open", result)),
        }
    }
}

impl Registry for WindowsRegistry {
    fn read(&self, key: &str, name: &str) -> Result<Option<Value>, String> {
        let Some(key) = Self::open(key, KEY_QUERY_VALUE, false)? else { return Ok(None); };
        let name = wide(name);
        let (mut kind, mut length) = (0, 0);
        let result = unsafe { RegQueryValueExW(key.0, name.as_ptr(), std::ptr::null(), &mut kind, std::ptr::null_mut(), &mut length) };
        if result == ERROR_FILE_NOT_FOUND { return Ok(None); }
        if result != ERROR_SUCCESS { return Err(error("read size", result)); }
        if length > 65536 { return Err("Unexpectedly large file association value".into()); }
        let mut bytes = vec![0; length as usize];
        let result = unsafe { RegQueryValueExW(key.0, name.as_ptr(), std::ptr::null(), &mut kind, bytes.as_mut_ptr(), &mut length) };
        if result != ERROR_SUCCESS { return Err(error("read", result)); }
        bytes.truncate(length as usize);
        Ok(Some(Value { kind, bytes }))
    }

    fn write(&mut self, key: &str, name: &str, value: Option<&Value>) -> Result<(), String> {
        let Some(key) = Self::open(key, KEY_SET_VALUE, value.is_some())? else { return Ok(()); };
        let name = wide(name);
        let result = unsafe { match value {
            Some(value) => RegSetValueExW(key.0, name.as_ptr(), 0, value.kind, value.bytes.as_ptr(), value.bytes.len() as u32),
            None => RegDeleteValueW(key.0, name.as_ptr()),
        } };
        if result == ERROR_SUCCESS || (value.is_none() && result == ERROR_FILE_NOT_FOUND) { Ok(()) }
        else { Err(error("write", result)) }
    }

    fn remove_empty_keys(&mut self, changes: &[Change]) {
        // RegDeleteKey deletes values too. Check both counts first, and never
        // delete shared keys (another app could add a value after our check).
        let mut keys: BTreeSet<String> = changes.iter().map(|c| c.key.clone()).collect();
        for change in changes {
            if let Some(class) = change.key.strip_suffix("\\shell\\open\\command") {
                keys.insert(format!("{class}\\shell\\open"));
                keys.insert(format!("{class}\\shell"));
            }
            if let Some(owner) = change.key.strip_suffix("\\Capabilities") { keys.insert(owner.into()); }
        }
        keys.remove("Software\\RegisteredApplications");
        let mut keys: Vec<_> = keys.into_iter().collect();
        keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
        for path in keys {
            if path.ends_with("\\OpenWithProgids") || path.ends_with("\\RegisteredApplications") {
                continue;
            }
            let Ok(Some(key)) = Self::open(&path, KEY_READ, false) else { continue; };
            let (mut subkeys, mut values) = (0, 0);
            let status = unsafe { RegQueryInfoKeyW(key.0, std::ptr::null_mut(), std::ptr::null_mut(),
                std::ptr::null(), &mut subkeys, std::ptr::null_mut(), std::ptr::null_mut(),
                &mut values, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
            drop(key);
            if status == ERROR_SUCCESS && subkeys == 0 && values == 0 {
                unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, wide(&path).as_ptr()); }
            }
        }
    }
}
