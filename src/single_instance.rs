use std::path::{Path, PathBuf};

use crossbeam_channel::Receiver;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IpcRequest {
    pub path: Option<String>,
}

pub(crate) fn pipe_name_for_key(key: &str) -> String {
    let hash = blake3::hash(key.as_bytes()).to_hex().to_string();
    format!(r"\\.\pipe\SuiSuiView-{}", &hash[..16])
}

pub(crate) fn encode_request(path: Option<&Path>) -> Vec<u8> {
    serde_json::to_vec(&IpcRequest {
        path: path.map(|path| path.to_string_lossy().to_string()),
    })
    .unwrap_or_default()
}

pub(crate) fn decode_request(bytes: &[u8]) -> Option<IpcRequest> {
    serde_json::from_slice(bytes).ok()
}

#[cfg(target_os = "windows")]
pub(crate) fn start_listener(pipe_name: String) -> Receiver<Option<PathBuf>> {
    let (tx, rx) = crossbeam_channel::unbounded::<Option<PathBuf>>();
    std::thread::Builder::new()
        .name("suisuiview-ipc".to_owned())
        .spawn(move || ipc_listener_loop(pipe_name, tx))
        .ok();
    rx
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn start_listener(_pipe_name: String) -> Receiver<Option<PathBuf>> {
    let (_tx, rx) = crossbeam_channel::unbounded();
    rx
}

#[cfg(target_os = "windows")]
pub(crate) fn send_open_request(pipe_name: &str, path: Option<&Path>) -> bool {
    windows_pipe::send(pipe_name, &encode_request(path))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn send_open_request(_pipe_name: &str, _path: Option<&Path>) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn ipc_listener_loop(pipe_name: String, tx: crossbeam_channel::Sender<Option<PathBuf>>) {
    loop {
        let Some(bytes) = windows_pipe::read_one_message(&pipe_name) else {
            std::thread::sleep(std::time::Duration::from_millis(250));
            continue;
        };
        let Some(request) = decode_request(&bytes) else {
            continue;
        };
        let _ = tx.send(request.path.map(PathBuf::from));
    }
}

#[cfg(target_os = "windows")]
mod windows_pipe {
    use std::ffi::OsStr;
    use std::iter;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE, OPEN_EXISTING,
        PIPE_ACCESS_INBOUND,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, WaitNamedPipeW,
        PIPE_READMODE_MESSAGE, PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    const PIPE_BUFFER_SIZE: u32 = 64 * 1024;
    const WAIT_MS: u32 = 200;

    pub(super) fn send(pipe_name: &str, bytes: &[u8]) -> bool {
        let pipe_name = wide(pipe_name);
        let mut handle = unsafe {
            CreateFileW(
                pipe_name.as_ptr(),
                FILE_GENERIC_WRITE,
                0,
                ptr::null_mut(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };

        if handle == INVALID_HANDLE_VALUE {
            let error = unsafe { GetLastError() };
            if error != ERROR_PIPE_BUSY {
                return false;
            }
            let waited = unsafe { WaitNamedPipeW(pipe_name.as_ptr(), WAIT_MS) } != 0;
            if !waited {
                return false;
            }
            handle = unsafe {
                CreateFileW(
                    pipe_name.as_ptr(),
                    FILE_GENERIC_WRITE,
                    0,
                    ptr::null_mut(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    ptr::null_mut(),
                )
            };
        }

        if handle == INVALID_HANDLE_VALUE {
            return false;
        }
        let ok = write_all(handle, bytes);
        unsafe {
            CloseHandle(handle);
        }
        ok
    }

    pub(super) fn read_one_message(pipe_name: &str) -> Option<Vec<u8>> {
        let pipe_name = wide(pipe_name);
        let handle = unsafe {
            CreateNamedPipeW(
                pipe_name.as_ptr(),
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                PIPE_BUFFER_SIZE,
                PIPE_BUFFER_SIZE,
                0,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return None;
        }

        let connected = unsafe { ConnectNamedPipe(handle, ptr::null_mut()) } != 0
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if !connected {
            unsafe {
                CloseHandle(handle);
            }
            return None;
        }

        let mut buffer = vec![0_u8; PIPE_BUFFER_SIZE as usize];
        let mut read = 0_u32;
        let ok = unsafe {
            ReadFile(
                handle,
                buffer.as_mut_ptr().cast(),
                PIPE_BUFFER_SIZE,
                &mut read,
                ptr::null_mut(),
            )
        } != 0;
        unsafe {
            DisconnectNamedPipe(handle);
            CloseHandle(handle);
        }
        if !ok || read == 0 {
            return None;
        }
        buffer.truncate(read as usize);
        Some(buffer)
    }

    fn write_all(handle: HANDLE, mut bytes: &[u8]) -> bool {
        while !bytes.is_empty() {
            let chunk_len = bytes.len().min(PIPE_BUFFER_SIZE as usize);
            let mut written = 0_u32;
            let ok = unsafe {
                WriteFile(
                    handle,
                    bytes.as_ptr().cast(),
                    chunk_len as u32,
                    &mut written,
                    ptr::null_mut(),
                )
            } != 0;
            if !ok || written == 0 {
                return false;
            }
            bytes = &bytes[written as usize..];
        }
        true
    }

    fn wide(text: &str) -> Vec<u16> {
        OsStr::new(text)
            .encode_wide()
            .chain(iter::once(0))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_request, encode_request, pipe_name_for_key};
    use std::path::Path;

    #[test]
    fn ipc_request_round_trips_path() {
        let encoded = encode_request(Some(Path::new(r"C:\images\a.png")));
        let decoded = decode_request(&encoded).unwrap();

        assert_eq!(decoded.path.as_deref(), Some(r"C:\images\a.png"));
    }

    #[test]
    fn pipe_name_hash_is_stable_and_namespaced() {
        let first = pipe_name_for_key(r"C:\Users\me\AppData\Local\SuiSuiView\state.json");
        let second = pipe_name_for_key(r"C:\Users\me\AppData\Local\SuiSuiView\state.json");

        assert_eq!(first, second);
        assert!(first.starts_with(r"\\.\pipe\SuiSuiView-"));
        assert_ne!(first, pipe_name_for_key("other"));
    }
}
