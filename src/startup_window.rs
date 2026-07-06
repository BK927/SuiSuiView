#[cfg(target_os = "windows")]
mod imp {
    use std::sync::mpsc;
    use std::sync::{
        atomic::{AtomicBool, AtomicIsize, Ordering},
        Arc,
    };
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, EnumWindows, GetClassNameW, GetWindowLongPtrW,
        GetWindowThreadProcessId, PeekMessageW, SetLayeredWindowAttributes, SetWindowLongPtrW,
        SetWindowPos, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, CWPRETSTRUCT,
        CWPSTRUCT, EVENT_OBJECT_CREATE, EVENT_OBJECT_SHOW, GWL_EXSTYLE, HHOOK, LWA_ALPHA, MSG,
        OBJID_WINDOW, PM_REMOVE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SWP_NOZORDER, WH_CALLWNDPROC, WH_CALLWNDPROCRET, WINEVENT_OUTOFCONTEXT, WS_EX_LAYERED,
    };

    const MAIN_WINDOW_CLASS: &str = "Window Class";
    const WINIT_EVENT_TARGET_CLASS: &str = "Winit Thread Event Target";
    const AUXILIARY_GUARD_DURATION: Duration = Duration::from_millis(1_500);
    const MAXIMIZED_GUARD_DURATION: Duration = Duration::from_millis(3_000);
    const AUXILIARY_GUARD_POLL_INTERVAL: Duration = Duration::from_millis(2);
    const MAIN_REVEAL_FORCE_DURATION: Duration = Duration::from_millis(500);
    static MASK_MAIN_WINDOWS: AtomicBool = AtomicBool::new(false);

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum StartupWindowGuardMode {
        AuxiliaryOnly,
        /// Mask the main window synchronously (the hook catches the show that
        /// happens *inside* winit's `create_window`), then hold the mask until
        /// the host reveals it on the first rendered frame via
        /// `reveal_main_windows()`. No maximize, no stability timer: the retired
        /// MaskMainUntilStable mode revealed on the first raw-visible state,
        /// which is winit's transient pre-paint show inside `create_window`, so
        /// it blinked an unpainted frame.
        MaskMainUntilRevealed,
    }

    impl StartupWindowGuardMode {
        fn masks_main_window(self) -> bool {
            self == Self::MaskMainUntilRevealed
        }
    }

    pub(crate) struct StartupFlashGuard {
        stop: Arc<AtomicBool>,
        join: Option<JoinHandle<()>>,
        hooks: Arc<MainThreadHooks>,
    }

    impl StartupFlashGuard {
        pub(crate) fn start(mode: StartupWindowGuardMode) -> Self {
            MASK_MAIN_WINDOWS.store(mode.masks_main_window(), Ordering::Release);
            let (callwnd_hook, callwnd_ret_hook) = install_main_thread_hooks(mode);
            let hooks = Arc::new(MainThreadHooks::new(callwnd_hook, callwnd_ret_hook));
            let worker_hooks = hooks.clone();
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = stop.clone();
            let pid = std::process::id();
            let (ready_tx, ready_rx) = mpsc::channel();
            let join = thread::Builder::new()
                .name("suisuiview-startup-window-guard".to_owned())
                .spawn(move || run_flash_guard(pid, worker_stop, mode, ready_tx, worker_hooks))
                .ok();
            if join.is_some() {
                let _ = ready_rx.recv_timeout(Duration::from_millis(100));
            }

            Self { stop, join, hooks }
        }
    }

    impl Drop for StartupFlashGuard {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            reveal_main_windows();
            self.hooks.unhook();
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
            MASK_MAIN_WINDOWS.store(false, Ordering::Release);
        }
    }

    fn install_main_thread_hooks(mode: StartupWindowGuardMode) -> (HHOOK, HHOOK) {
        if !mode.masks_main_window() {
            return (std::ptr::null_mut(), std::ptr::null_mut());
        }
        unsafe {
            let thread_id = GetCurrentThreadId();
            (
                SetWindowsHookExW(
                    WH_CALLWNDPROC,
                    Some(startup_callwndproc_hook),
                    std::ptr::null_mut(),
                    thread_id,
                ),
                SetWindowsHookExW(
                    WH_CALLWNDPROCRET,
                    Some(startup_callwndretproc_hook),
                    std::ptr::null_mut(),
                    thread_id,
                ),
            )
        }
    }

    fn unhook_slot(slot: &AtomicIsize) {
        let handle = slot.swap(0, Ordering::AcqRel);
        if handle != 0 {
            unsafe {
                let _ = UnhookWindowsHookEx(handle as HHOOK);
            }
        }
    }

    unsafe extern "system" fn startup_callwndproc_hook(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if code >= 0 && MASK_MAIN_WINDOWS.load(Ordering::Acquire) {
            let message = &*(lparam as *const CWPSTRUCT);
            mask_startup_window_from_hook(message.hwnd);
        }
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }

    unsafe extern "system" fn startup_callwndretproc_hook(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if code >= 0 && MASK_MAIN_WINDOWS.load(Ordering::Acquire) {
            let message = &*(lparam as *const CWPRETSTRUCT);
            mask_startup_window_from_hook(message.hwnd);
        }
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }

    unsafe fn mask_startup_window_from_hook(hwnd: HWND) {
        if hwnd.is_null() {
            return;
        }
        if window_class_is(hwnd, MAIN_WINDOW_CLASS) {
            mask_startup_window(hwnd, MAIN_WINDOW_CLASS);
        } else if window_class_is(hwnd, WINIT_EVENT_TARGET_CLASS) {
            // Mask the 15x15 winit event target synchronously as it is created on
            // the main thread, before the poll loop (or a trace sample) can catch
            // the brief WS_EX_LAYERED-without-attributes state that Windows does
            // not actually composite but IsWindowVisible reports as shown.
            mask_startup_window(hwnd, WINIT_EVENT_TARGET_CLASS);
        }
    }

    fn run_flash_guard(
        pid: u32,
        stop: Arc<AtomicBool>,
        mode: StartupWindowGuardMode,
        ready: mpsc::Sender<()>,
        main_thread_hooks: Arc<MainThreadHooks>,
    ) {
        let deadline = Instant::now() + guard_duration(mode);
        let mut main_reveal = MainRevealState::new(mode);
        let event_hook = startup_window_event_hook(pid);
        mask_startup_windows(pid);
        let _ = ready.send(());
        while Instant::now() < deadline && !stop.load(Ordering::Acquire) {
            mask_startup_windows(pid);
            match main_reveal.update(Instant::now()) {
                MainRevealAction::None => {}
                MainRevealAction::ForceReveal => force_reveal_main_windows(),
            }
            if mode.masks_main_window() && main_reveal.observe_external_reveal(Instant::now()) {
                main_thread_hooks.unhook();
                force_reveal_main_windows();
            }
            if mode.masks_main_window() && main_reveal.is_complete(Instant::now()) {
                break;
            }
            pump_startup_window_events();
            wait_for_startup_window_event();
        }
        reveal_main_windows();
        main_thread_hooks.unhook();
        if !event_hook.is_null() {
            unsafe {
                let _ = UnhookWinEvent(event_hook);
            }
        }
    }

    fn guard_duration(mode: StartupWindowGuardMode) -> Duration {
        if mode.masks_main_window() {
            MAXIMIZED_GUARD_DURATION
        } else {
            AUXILIARY_GUARD_DURATION
        }
    }

    struct MainThreadHooks {
        callwnd_hook: AtomicIsize,
        callwnd_ret_hook: AtomicIsize,
    }

    impl MainThreadHooks {
        fn new(callwnd_hook: HHOOK, callwnd_ret_hook: HHOOK) -> Self {
            Self {
                callwnd_hook: AtomicIsize::new(callwnd_hook as isize),
                callwnd_ret_hook: AtomicIsize::new(callwnd_ret_hook as isize),
            }
        }

        fn unhook(&self) {
            unhook_slot(&self.callwnd_hook);
            unhook_slot(&self.callwnd_ret_hook);
        }
    }

    fn mask_startup_windows(pid: u32) {
        unsafe {
            EnumWindows(Some(enum_windows_for_startup_mask), pid as LPARAM);
        }
    }

    unsafe extern "system" fn enum_windows_for_startup_mask(hwnd: HWND, lparam: LPARAM) -> i32 {
        if window_process_id(hwnd) == lparam as u32 {
            let class_name = window_class_name(hwnd);
            mask_startup_window(hwnd, &class_name);
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
            let class_name = window_class_name(hwnd);
            mask_startup_window(hwnd, &class_name);
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
        thread::sleep(AUXILIARY_GUARD_POLL_INTERVAL);
    }

    unsafe fn mask_startup_window(hwnd: HWND, class_name: &str) {
        let masks_main_window = class_name == MAIN_WINDOW_CLASS;
        if class_name == WINIT_EVENT_TARGET_CLASS
            || (masks_main_window && MASK_MAIN_WINDOWS.load(Ordering::Acquire))
        {
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if masks_main_window && !MASK_MAIN_WINDOWS.load(Ordering::Acquire) {
                return;
            }
            if ex_style & WS_EX_LAYERED as isize == 0 {
                let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style | WS_EX_LAYERED as isize);
            }
            if masks_main_window && !MASK_MAIN_WINDOWS.load(Ordering::Acquire) {
                return;
            }
            let _ = SetLayeredWindowAttributes(hwnd, 0, 0, LWA_ALPHA);
        }
    }

    pub(crate) fn reveal_main_windows() {
        if !MASK_MAIN_WINDOWS.swap(false, Ordering::AcqRel) {
            return;
        }
        force_reveal_main_windows();
    }

    fn force_reveal_main_windows() {
        unsafe {
            EnumWindows(
                Some(enum_windows_for_startup_reveal),
                std::process::id() as LPARAM,
            );
        }
    }

    unsafe extern "system" fn enum_windows_for_startup_reveal(hwnd: HWND, lparam: LPARAM) -> i32 {
        if window_process_id(hwnd) == lparam as u32 && window_class_name(hwnd) == MAIN_WINDOW_CLASS
        {
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

    unsafe fn window_class_is(hwnd: HWND, target: &str) -> bool {
        let mut buffer = [0u16; 64];
        let len = GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        let len = usize::try_from(len).unwrap_or(0);
        buffer[..len].iter().copied().eq(target.encode_utf16())
    }

    struct MainRevealState {
        revealed: bool,
        force_reveal_until: Option<Instant>,
    }

    impl MainRevealState {
        fn new(mode: StartupWindowGuardMode) -> Self {
            Self {
                // The mask mode starts masked (not yet revealed); AuxiliaryOnly
                // never masks main, so it is effectively already revealed.
                revealed: !mode.masks_main_window(),
                force_reveal_until: None,
            }
        }

        fn update(&mut self, now: Instant) -> MainRevealAction {
            if self.force_reveal_until.is_some_and(|until| now <= until) {
                return MainRevealAction::ForceReveal;
            }
            self.force_reveal_until = None;
            MainRevealAction::None
        }

        fn is_complete(&self, now: Instant) -> bool {
            self.revealed && self.force_reveal_until.is_none_or(|until| now > until)
        }

        fn observe_external_reveal(&mut self, now: Instant) -> bool {
            if self.revealed || MASK_MAIN_WINDOWS.load(Ordering::Acquire) {
                return false;
            }
            self.revealed = true;
            self.force_reveal_until = Some(now + MAIN_REVEAL_FORCE_DURATION);
            true
        }
    }

    enum MainRevealAction {
        None,
        ForceReveal,
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum StartupWindowGuardMode {
        AuxiliaryOnly,
        MaskMainUntilRevealed,
    }

    pub(crate) struct StartupFlashGuard;

    impl StartupFlashGuard {
        pub(crate) fn start(_mode: StartupWindowGuardMode) -> Self {
            Self
        }
    }

    pub(crate) fn reveal_main_windows() {}
}

pub(crate) use imp::StartupFlashGuard;
pub(crate) use imp::StartupWindowGuardMode;

pub(crate) fn start_flash_guard(mode: StartupWindowGuardMode) -> StartupFlashGuard {
    StartupFlashGuard::start(mode)
}

pub(crate) fn reveal_main_windows() {
    imp::reveal_main_windows();
}
