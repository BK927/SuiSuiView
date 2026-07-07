use egui_winit::winit;
#[cfg(target_os = "windows")]
use std::time::Duration;

#[cfg(target_os = "windows")]
pub(super) fn sync_visible_window_title(window: &winit::window::Window) {
    let title = window.title();
    let title = if title.is_empty() {
        "SuiSuiView"
    } else {
        title.as_str()
    };
    set_process_visible_window_title(title);
}

#[cfg(not(target_os = "windows"))]
pub(super) fn sync_visible_window_title(_window: &winit::window::Window) {}

#[cfg(target_os = "windows")]
pub(super) fn schedule_process_visible_window_title(title: String) {
    let generation = next_title_sync_generation();
    set_process_visible_window_title(&title);
    std::thread::Builder::new()
        .name("suisuiview-handoff-title-sync".to_owned())
        .spawn(move || {
            for delay in [
                Duration::from_millis(80),
                Duration::from_millis(320),
                Duration::from_millis(900),
            ] {
                std::thread::sleep(delay);
                if !title_sync_generation_matches(generation) {
                    break;
                }
                set_process_visible_window_title(&title);
            }
        })
        .ok();
}

#[cfg(not(target_os = "windows"))]
pub(super) fn schedule_process_visible_window_title(_title: String) {}

#[cfg(target_os = "windows")]
static TITLE_SYNC_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(target_os = "windows")]
fn next_title_sync_generation() -> u64 {
    use std::sync::atomic::Ordering;

    TITLE_SYNC_GENERATION.fetch_add(1, Ordering::Relaxed) + 1
}

#[cfg(target_os = "windows")]
fn title_sync_generation_matches(generation: u64) -> bool {
    use std::sync::atomic::Ordering;

    TITLE_SYNC_GENERATION.load(Ordering::Relaxed) == generation
}

#[cfg(target_os = "windows")]
fn set_process_visible_window_title(title: &str) {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible, SetWindowTextW,
    };

    struct TitleUpdate {
        process_id: u32,
        title: *const u16,
    }

    unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> i32 {
        let update = &*(lparam as *const TitleUpdate);
        let mut window_process_id = 0;
        GetWindowThreadProcessId(hwnd, &mut window_process_id);
        if window_process_id == update.process_id && IsWindowVisible(hwnd) != 0 {
            SetWindowTextW(hwnd, update.title);
        }
        1
    }

    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let update = TitleUpdate {
        process_id: std::process::id(),
        title: title.as_ptr(),
    };
    unsafe {
        EnumWindows(Some(enum_window), &update as *const TitleUpdate as isize);
    }
}

#[cfg(not(target_os = "windows"))]
fn set_process_visible_window_title(_title: &str) {}
