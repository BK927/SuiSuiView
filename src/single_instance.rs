use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use serde::{Deserialize, Serialize};

type WakeCallback = Arc<dyn Fn() + Send + Sync + 'static>;

#[derive(Default)]
struct ListenerWake {
    callback: Mutex<Option<WakeCallback>>,
}

impl ListenerWake {
    fn set_callback(&self, callback: WakeCallback) {
        *self.callback.lock().unwrap() = Some(callback);
    }

    fn notify(&self) {
        let callback = self.callback.lock().unwrap().clone();
        if let Some(callback) = callback {
            callback();
        }
    }
}

pub(crate) struct IpcListener {
    receiver: Receiver<Option<PathBuf>>,
    wake: Arc<ListenerWake>,
    #[cfg(target_os = "windows")]
    stop: Option<ListenerStop>,
}

#[cfg(target_os = "windows")]
struct ListenerStop {
    pipe_name: String,
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl IpcListener {
    fn channel() -> (Self, Sender<Option<PathBuf>>, Weak<ListenerWake>) {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let wake = Arc::new(ListenerWake::default());
        let weak_wake = Arc::downgrade(&wake);
        (
            Self {
                receiver,
                wake,
                #[cfg(target_os = "windows")]
                stop: None,
            },
            sender,
            weak_wake,
        )
    }

    /// Attach the UI wake-up after winit/egui has created its event loop.
    /// Requests can arrive before that point; if one is already queued, wake
    /// immediately after installing the callback so it is not left idle.
    pub(crate) fn set_wake_callback(&self, callback: impl Fn() + Send + Sync + 'static) {
        let callback: WakeCallback = Arc::new(callback);
        self.wake.set_callback(callback.clone());
        if !self.receiver.is_empty() {
            callback();
        }
    }

    pub(crate) fn try_recv(&self) -> Result<Option<PathBuf>, TryRecvError> {
        self.receiver.try_recv()
    }
}

#[cfg(target_os = "windows")]
impl Drop for IpcListener {
    fn drop(&mut self) {
        let Some(stop) = &self.stop else {
            return;
        };
        use std::sync::atomic::Ordering;
        stop.running.store(false, Ordering::Release);
        // Connect once to release a listener blocked in ConnectNamedPipe/ReadFile.
        // The byte is deliberately not valid JSON and is never delivered to the
        // app; the loop observes `running == false` before decoding it.
        let _ = windows_pipe::send(&stop.pipe_name, b"\0");
    }
}

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
pub(crate) fn start_listener(pipe_name: String) -> IpcListener {
    use std::sync::atomic::AtomicBool;

    let (mut listener, sender, wake) = IpcListener::channel();
    let running = Arc::new(AtomicBool::new(true));
    listener.stop = Some(ListenerStop {
        pipe_name: pipe_name.clone(),
        running: running.clone(),
    });
    std::thread::Builder::new()
        .name("suisuiview-ipc".to_owned())
        .spawn(move || ipc_listener_loop(pipe_name, sender, wake, running))
        .ok();
    listener
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn start_listener(_pipe_name: String) -> IpcListener {
    let (listener, _sender, _wake) = IpcListener::channel();
    listener
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
fn ipc_listener_loop(
    pipe_name: String,
    sender: Sender<Option<PathBuf>>,
    wake: Weak<ListenerWake>,
    running: Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;

    while running.load(Ordering::Acquire) {
        let Some(bytes) = windows_pipe::read_one_message(&pipe_name) else {
            if !running.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
            continue;
        };
        if !running.load(Ordering::Acquire) {
            break;
        }
        let Some(request) = decode_request(&bytes) else {
            continue;
        };
        if !enqueue_request(&sender, &wake, request.path.map(PathBuf::from)) {
            break;
        }
    }
}

fn enqueue_request(
    sender: &Sender<Option<PathBuf>>,
    wake: &Weak<ListenerWake>,
    request: Option<PathBuf>,
) -> bool {
    if sender.send(request).is_err() {
        return false;
    }
    if let Some(wake) = wake.upgrade() {
        wake.notify();
    }
    true
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
    use super::{decode_request, encode_request, enqueue_request, pipe_name_for_key, IpcListener};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

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

    #[test]
    fn queued_request_wakes_when_callback_is_attached_late() {
        let (listener, sender, wake) = IpcListener::channel();
        assert!(enqueue_request(
            &sender,
            &wake,
            Some(PathBuf::from("first.cbz"))
        ));

        let wake_count = Arc::new(AtomicUsize::new(0));
        let callback_count = wake_count.clone();
        listener.set_wake_callback(move || {
            callback_count.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(wake_count.load(Ordering::Relaxed), 1);

        assert!(enqueue_request(&sender, &wake, None));
        assert_eq!(wake_count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn no_path_request_does_not_discard_neighboring_file_requests() {
        let (listener, sender, wake) = IpcListener::channel();
        let first = PathBuf::from("first.cbz");
        let second = PathBuf::from("second.cbz");
        assert!(enqueue_request(&sender, &wake, Some(first.clone())));
        assert!(enqueue_request(&sender, &wake, None));
        assert!(enqueue_request(&sender, &wake, Some(second.clone())));

        assert_eq!(listener.try_recv().unwrap(), Some(first));
        assert_eq!(listener.try_recv().unwrap(), None);
        assert_eq!(listener.try_recv().unwrap(), Some(second));
        assert!(listener.try_recv().is_err());
    }
}
