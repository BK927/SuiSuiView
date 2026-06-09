#[cfg(target_os = "windows")]
mod imp {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    };
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, EnumWindows, GetClassNameW, GetWindowLongPtrW, GetWindowThreadProcessId,
        MsgWaitForMultipleObjectsEx, PeekMessageW, SetLayeredWindowAttributes, SetWindowLongPtrW,
        SetWindowPos, TranslateMessage, EVENT_OBJECT_CREATE, EVENT_OBJECT_SHOW, GWL_EXSTYLE,
        LWA_ALPHA, MSG, MWMO_INPUTAVAILABLE, OBJID_WINDOW, PM_REMOVE, QS_ALLINPUT,
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        WINEVENT_OUTOFCONTEXT, WS_EX_LAYERED,
    };

    const MAIN_WINDOW_CLASS: &str = "Window Class";
    const WINIT_EVENT_TARGET_CLASS: &str = "Winit Thread Event Target";
    const FLASH_GUARD_DURATION: Duration = Duration::from_millis(1500);
    const FLASH_GUARD_POLL_INTERVAL: Duration = Duration::from_millis(2);
    static MAIN_WINDOW_REVEALED: AtomicBool = AtomicBool::new(false);

    pub(crate) struct StartupFlashGuard {
        stop: Arc<AtomicBool>,
        join: Option<JoinHandle<()>>,
    }

    impl StartupFlashGuard {
        pub(crate) fn start() -> Self {
            MAIN_WINDOW_REVEALED.store(false, Ordering::Release);
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = stop.clone();
            let pid = std::process::id();
            let (ready_tx, ready_rx) = mpsc::channel();
            let join = thread::Builder::new()
                .name("suisuiview-startup-window-guard".to_owned())
                .spawn(move || run_flash_guard(pid, worker_stop, ready_tx))
                .ok();
            if join.is_some() {
                let _ = ready_rx.recv_timeout(Duration::from_millis(100));
            }

            Self { stop, join }
        }
    }

    impl Drop for StartupFlashGuard {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    fn run_flash_guard(pid: u32, stop: Arc<AtomicBool>, ready: mpsc::Sender<()>) {
        let deadline = Instant::now() + FLASH_GUARD_DURATION;
        let event_hook = startup_window_event_hook(pid);
        mask_startup_windows(pid);
        let _ = ready.send(());
        while Instant::now() < deadline && !stop.load(Ordering::Acquire) {
            mask_startup_windows(pid);
            pump_startup_window_events();
            wait_for_startup_window_event();
        }
        if !event_hook.is_null() {
            unsafe {
                let _ = UnhookWinEvent(event_hook);
            }
        }
    }

    fn mask_startup_windows(pid: u32) {
        unsafe {
            EnumWindows(Some(enum_windows_for_startup_mask), pid as LPARAM);
        }
    }

    unsafe extern "system" fn enum_windows_for_startup_mask(hwnd: HWND, lparam: LPARAM) -> i32 {
        let pid = lparam as u32;
        if window_process_id(hwnd) == pid {
            mask_startup_window(hwnd);
        }
        1
    }

    fn startup_window_event_hook(pid: u32) -> HWINEVENTHOOK {
        unsafe {
            SetWinEventHook(
                EVENT_OBJECT_CREATE,
                EVENT_OBJECT_SHOW,
                std::ptr::null_mut(),
                Some(startup_window_event_callback),
                pid,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        }
    }

    unsafe extern "system" fn startup_window_event_callback(
        _hook: HWINEVENTHOOK,
        _event: u32,
        hwnd: HWND,
        id_object: i32,
        id_child: i32,
        _event_thread: u32,
        _event_time: u32,
    ) {
        if id_object == OBJID_WINDOW && id_child == 0 {
            mask_startup_window(hwnd);
        }
    }

    fn pump_startup_window_events() {
        unsafe {
            let mut message = std::mem::zeroed::<MSG>();
            while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }

    fn wait_for_startup_window_event() {
        unsafe {
            let _ = MsgWaitForMultipleObjectsEx(
                0,
                std::ptr::null(),
                FLASH_GUARD_POLL_INTERVAL.as_millis() as u32,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            );
        }
    }

    unsafe fn mask_startup_window(hwnd: HWND) {
        let class_name = window_class_name(hwnd);
        if class_name == WINIT_EVENT_TARGET_CLASS
            || (class_name == MAIN_WINDOW_CLASS && !MAIN_WINDOW_REVEALED.load(Ordering::Acquire))
        {
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if ex_style & WS_EX_LAYERED as isize == 0 {
                let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style | WS_EX_LAYERED as isize);
            }
            let _ = SetLayeredWindowAttributes(hwnd, 0, 0, LWA_ALPHA);
        }
    }

    pub(crate) fn reveal_main_windows() {
        MAIN_WINDOW_REVEALED.store(true, Ordering::Release);
        unsafe {
            EnumWindows(
                Some(enum_windows_for_startup_reveal),
                std::process::id() as LPARAM,
            );
        }
    }

    unsafe extern "system" fn enum_windows_for_startup_reveal(hwnd: HWND, lparam: LPARAM) -> i32 {
        let pid = lparam as u32;
        if window_process_id(hwnd) == pid && window_class_name(hwnd) == MAIN_WINDOW_CLASS {
            reveal_main_window(hwnd);
        }
        1
    }

    unsafe fn reveal_main_window(hwnd: HWND) {
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        if ex_style & WS_EX_LAYERED as isize == 0 {
            return;
        }
        let _ = SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
        let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style & !(WS_EX_LAYERED as isize));
        let _ = SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }

    unsafe fn window_process_id(hwnd: HWND) -> u32 {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        pid
    }

    unsafe fn window_class_name(hwnd: HWND) -> String {
        let mut buffer = [0u16; 128];
        let len = GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        String::from_utf16_lossy(&buffer[..usize::try_from(len).unwrap_or(0)])
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    pub(crate) struct StartupFlashGuard;

    impl StartupFlashGuard {
        pub(crate) fn start() -> Self {
            Self
        }
    }

    pub(crate) fn reveal_main_windows() {}
}

pub(crate) use imp::StartupFlashGuard;

pub(crate) fn start_flash_guard() -> StartupFlashGuard {
    StartupFlashGuard::start()
}

pub(crate) fn reveal_main_windows() {
    imp::reveal_main_windows();
}
