#[cfg(target_os = "windows")]
mod imp {
    use std::sync::mpsc::{self, Sender};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
        SetStretchBltMode, StretchDIBits, UpdateWindow, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        COLORONCOLOR, DIB_RGB_COLORS, HBRUSH, PAINTSTRUCT, SRCCOPY,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent};
    use windows_sys::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows,
        GetClassNameW, GetClientRect, GetWindowLongPtrW, GetWindowRect, GetWindowTextW,
        GetWindowThreadProcessId, MsgWaitForMultipleObjectsEx, PeekMessageW, RegisterClassW,
        SetLayeredWindowAttributes, SetWindowLongPtrW, ShowWindow, TranslateMessage, CS_HREDRAW,
        CS_VREDRAW, CW_USEDEFAULT, EVENT_OBJECT_CREATE, EVENT_OBJECT_SHOW, GWLP_USERDATA,
        GWL_EXSTYLE, LWA_ALPHA, MSG, MWMO_INPUTAVAILABLE, OBJID_WINDOW, PM_REMOVE, QS_ALLINPUT,
        SW_MAXIMIZE, SW_SHOW, WINEVENT_OUTOFCONTEXT, WM_CLOSE, WM_DESTROY, WM_PAINT, WNDCLASSW,
        WS_EX_LAYERED, WS_OVERLAPPEDWINDOW,
    };

    const WINIT_EVENT_TARGET_CLASS: &str = "Winit Thread Event Target";
    const PREVIEW_WINDOW_CLASS: &str = "SuiSuiView Startup Preview";
    const FLASH_GUARD_DURATION: Duration = Duration::from_millis(1500);
    const FLASH_GUARD_POLL_INTERVAL: Duration = Duration::from_millis(2);
    const PREVIEW_WINDOW_DURATION: Duration = Duration::from_millis(3000);
    const PREVIEW_WINDOW_POLL_INTERVAL: Duration = Duration::from_millis(4);
    const PREVIEW_BACKGROUND: u32 = 0x0012_1212;

    pub(crate) struct StartupFlashGuard {
        stop: Arc<AtomicBool>,
        join: Option<JoinHandle<()>>,
    }

    impl StartupFlashGuard {
        pub(crate) fn start() -> Self {
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = stop.clone();
            let pid = std::process::id();
            let join = thread::Builder::new()
                .name("suisuiview-startup-window-guard".to_owned())
                .spawn(move || run_flash_guard(pid, worker_stop))
                .ok();

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

    pub(crate) struct StartupPreviewConfig {
        pub(crate) enabled: bool,
        pub(crate) title: String,
        pub(crate) inner_size: [f32; 2],
        pub(crate) position: Option<[f32; 2]>,
        pub(crate) maximized: bool,
        pub(crate) wait_for_first_image: bool,
    }

    pub(crate) struct StartupPreviewImage {
        bgra: Arc<[u8]>,
        width: usize,
        height: usize,
    }

    impl StartupPreviewImage {
        pub(crate) fn from_rgba(width: usize, height: usize, rgba: &[u8]) -> Self {
            let mut bgra = Vec::with_capacity(rgba.len());
            for pixel in rgba.chunks_exact(4) {
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
            Self {
                bgra: bgra.into(),
                width,
                height,
            }
        }
    }

    pub(crate) struct StartupPreviewWindow {
        page_tx: Option<Sender<StartupPreviewImage>>,
        stop: Arc<AtomicBool>,
        join: Option<JoinHandle<()>>,
    }

    impl StartupPreviewWindow {
        pub(crate) fn start(config: StartupPreviewConfig) -> Self {
            if !config.enabled {
                return Self::disabled();
            }
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = stop.clone();
            let (page_tx, page_rx) = mpsc::channel();
            let join = thread::Builder::new()
                .name("suisuiview-startup-preview-window".to_owned())
                .spawn(move || run_preview_window(config, page_rx, worker_stop))
                .ok();

            Self {
                page_tx: join.as_ref().map(|_| page_tx),
                stop,
                join,
            }
        }

        fn disabled() -> Self {
            Self {
                page_tx: None,
                stop: Arc::new(AtomicBool::new(false)),
                join: None,
            }
        }

        pub(crate) fn page_sender(&self) -> Option<Sender<StartupPreviewImage>> {
            self.page_tx.clone()
        }
    }

    impl Drop for StartupPreviewWindow {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            self.page_tx = None;
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    struct PreviewState {
        image: Option<StartupPreviewImage>,
    }

    fn run_flash_guard(pid: u32, stop: Arc<AtomicBool>) {
        let deadline = Instant::now() + FLASH_GUARD_DURATION;
        let event_hook = startup_window_event_hook(pid);
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

    fn run_preview_window(
        config: StartupPreviewConfig,
        page_rx: mpsc::Receiver<StartupPreviewImage>,
        stop: Arc<AtomicBool>,
    ) {
        set_process_dpi_awareness_for_preview();
        let Some(hwnd) = create_preview_window(&config) else {
            return;
        };
        let state = Arc::new(Mutex::new(PreviewState { image: None }));
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Arc::as_ptr(&state) as isize);
        }

        if config.wait_for_first_image {
            wait_for_initial_preview_image(&page_rx, &state);
        }

        let pid = std::process::id();
        let deadline = Instant::now() + PREVIEW_WINDOW_DURATION;
        let image_ready = state
            .lock()
            .map(|state| state.image.is_some())
            .unwrap_or(false);
        record_startup_preview_shown(image_ready);
        unsafe {
            ShowWindow(
                hwnd,
                if config.maximized {
                    SW_MAXIMIZE
                } else {
                    SW_SHOW
                },
            );
            UpdateWindow(hwnd);
        }
        while Instant::now() < deadline && !stop.load(Ordering::Acquire) {
            while let Ok(image) = page_rx.try_recv() {
                if let Ok(mut state) = state.lock() {
                    state.image = Some(image);
                }
                unsafe {
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            pump_startup_window_events();
            if real_main_window_visible(pid, hwnd) {
                break;
            }
            wait_for_preview_window_event();
        }

        unsafe {
            DestroyWindow(hwnd);
        }
        pump_startup_window_events();
        drop(state);
    }

    fn set_process_dpi_awareness_for_preview() {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    }

    fn wait_for_initial_preview_image(
        page_rx: &mpsc::Receiver<StartupPreviewImage>,
        state: &Mutex<PreviewState>,
    ) {
        let deadline = Instant::now() + Duration::from_millis(80);
        while Instant::now() < deadline {
            match page_rx.try_recv() {
                Ok(image) => {
                    if let Ok(mut state) = state.lock() {
                        state.image = Some(image);
                    }
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }
    }

    fn create_preview_window(config: &StartupPreviewConfig) -> Option<HWND> {
        let class_name = wide_null(PREVIEW_WINDOW_CLASS);
        let title = wide_null(&config.title);
        let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };
        if hinstance.is_null() {
            return None;
        }
        let wnd_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(preview_window_proc),
            hInstance: hinstance,
            lpszClassName: class_name.as_ptr(),
            ..unsafe { std::mem::zeroed() }
        };
        unsafe {
            RegisterClassW(&wnd_class);
        }

        let width = config.inner_size[0].round().max(320.0) as i32;
        let height = config.inner_size[1].round().max(240.0) as i32;
        let (x, y) = config
            .position
            .map(|[x, y]| (x.round() as i32, y.round() as i32))
            .unwrap_or((CW_USEDEFAULT, CW_USEDEFAULT));
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                x,
                y,
                width,
                height,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinstance,
                std::ptr::null(),
            )
        };
        (!hwnd.is_null()).then_some(hwnd)
    }

    unsafe extern "system" fn preview_window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_PAINT => {
                paint_preview_window(hwnd);
                0
            }
            WM_CLOSE | WM_DESTROY => 0,
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe fn paint_preview_window(hwnd: HWND) {
        let mut paint = std::mem::zeroed::<PAINTSTRUCT>();
        let hdc = BeginPaint(hwnd, &mut paint);
        if hdc.is_null() {
            return;
        }

        let mut rect = std::mem::zeroed::<RECT>();
        GetClientRect(hwnd, &mut rect);
        let brush: HBRUSH = CreateSolidBrush(PREVIEW_BACKGROUND);
        if !brush.is_null() {
            FillRect(hdc, &rect, brush);
            DeleteObject(brush);
        }

        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Mutex<PreviewState>;
        if !state_ptr.is_null() {
            if let Ok(state) = (*state_ptr).lock() {
                if let Some(image) = state.image.as_ref() {
                    paint_preview_image(hdc, &rect, image);
                }
            }
        }

        EndPaint(hwnd, &paint);
    }

    unsafe fn paint_preview_image(
        hdc: windows_sys::Win32::Graphics::Gdi::HDC,
        rect: &RECT,
        image: &StartupPreviewImage,
    ) {
        let client_width = (rect.right - rect.left).max(1);
        let client_height = (rect.bottom - rect.top).max(1);
        let image_width = image.width.max(1) as i32;
        let image_height = image.height.max(1) as i32;
        let scale = (client_width as f32 / image_width as f32)
            .min(client_height as f32 / image_height as f32)
            .max(0.01);
        let dst_width = (image_width as f32 * scale).round().max(1.0) as i32;
        let dst_height = (image_height as f32 * scale).round().max(1.0) as i32;
        let dst_x = rect.left + (client_width - dst_width) / 2;
        let dst_y = rect.top + (client_height - dst_height) / 2;
        let mut bitmap_info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: image_width,
                biHeight: -image_height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            },
            ..std::mem::zeroed()
        };
        SetStretchBltMode(hdc, COLORONCOLOR);
        StretchDIBits(
            hdc,
            dst_x,
            dst_y,
            dst_width,
            dst_height,
            0,
            0,
            image_width,
            image_height,
            image.bgra.as_ptr().cast(),
            &mut bitmap_info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }

    fn real_main_window_visible(pid: u32, preview_hwnd: HWND) -> bool {
        let mut data = MainWindowSearch {
            pid,
            preview_hwnd,
            visible: false,
        };
        unsafe {
            EnumWindows(
                Some(enum_windows_for_real_main_window),
                (&mut data as *mut MainWindowSearch) as LPARAM,
            );
        }
        data.visible
    }

    struct MainWindowSearch {
        pid: u32,
        preview_hwnd: HWND,
        visible: bool,
    }

    unsafe extern "system" fn enum_windows_for_real_main_window(hwnd: HWND, lparam: LPARAM) -> i32 {
        let data = &mut *(lparam as *mut MainWindowSearch);
        if hwnd != data.preview_hwnd
            && window_process_id(hwnd) == data.pid
            && is_real_main_window(hwnd)
        {
            data.visible = true;
            return 0;
        }
        1
    }

    unsafe fn is_real_main_window(hwnd: HWND) -> bool {
        if !is_window_visible(hwnd) || window_class_name(hwnd) == WINIT_EVENT_TARGET_CLASS {
            return false;
        }
        let [left, top, right, bottom] = window_rect(hwnd);
        let width = right - left;
        let height = bottom - top;
        if width < 300 || height < 200 {
            return false;
        }
        let title = window_title(hwnd);
        title.contains("SuiSuiView") || window_class_name(hwnd) == "Window Class"
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

    fn startup_window_event_hook(pid: u32) -> windows_sys::Win32::UI::Accessibility::HWINEVENTHOOK {
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
        _hook: windows_sys::Win32::UI::Accessibility::HWINEVENTHOOK,
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

    fn wait_for_preview_window_event() {
        unsafe {
            let _ = MsgWaitForMultipleObjectsEx(
                0,
                std::ptr::null(),
                PREVIEW_WINDOW_POLL_INTERVAL.as_millis() as u32,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            );
        }
    }

    unsafe fn mask_startup_window(hwnd: HWND) {
        if window_class_name(hwnd) == WINIT_EVENT_TARGET_CLASS {
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if ex_style & WS_EX_LAYERED as isize == 0 {
                let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style | WS_EX_LAYERED as isize);
            }
            let _ = SetLayeredWindowAttributes(hwnd, 0, 0, LWA_ALPHA);
        }
    }

    unsafe fn window_process_id(hwnd: HWND) -> u32 {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        pid
    }

    unsafe fn is_window_visible(hwnd: HWND) -> bool {
        windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd) != 0
    }

    unsafe fn window_rect(hwnd: HWND) -> [i32; 4] {
        let mut rect = std::mem::zeroed::<RECT>();
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return [0, 0, 0, 0];
        }
        [rect.left, rect.top, rect.right, rect.bottom]
    }

    unsafe fn window_class_name(hwnd: HWND) -> String {
        let mut buffer = [0u16; 128];
        let len = GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        String::from_utf16_lossy(&buffer[..usize::try_from(len).unwrap_or(0)])
    }

    unsafe fn window_title(hwnd: HWND) -> String {
        let mut buffer = [0u16; 256];
        let len = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        String::from_utf16_lossy(&buffer[..usize::try_from(len).unwrap_or(0)])
    }

    fn wide_null(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    fn record_startup_preview_shown(image_ready: bool) {
        crate::core::perf_trace::record_duration(
            "startup_preview_shown",
            Duration::ZERO,
            &[crate::core::perf_trace::PerfField::Bool(
                "image_ready",
                image_ready,
            )],
        );
    }

    #[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
    fn record_startup_preview_shown(_image_ready: bool) {}
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use std::sync::mpsc::Sender;

    pub(crate) struct StartupFlashGuard;

    impl StartupFlashGuard {
        pub(crate) fn start() -> Self {
            Self
        }
    }

    pub(crate) struct StartupPreviewConfig {
        pub(crate) enabled: bool,
        pub(crate) title: String,
        pub(crate) inner_size: [f32; 2],
        pub(crate) position: Option<[f32; 2]>,
        pub(crate) maximized: bool,
        pub(crate) wait_for_first_image: bool,
    }

    pub(crate) struct StartupPreviewImage;

    impl StartupPreviewImage {
        pub(crate) fn from_rgba(_width: usize, _height: usize, _rgba: &[u8]) -> Self {
            Self
        }
    }

    pub(crate) struct StartupPreviewWindow;

    impl StartupPreviewWindow {
        pub(crate) fn start(_config: StartupPreviewConfig) -> Self {
            Self
        }

        pub(crate) fn page_sender(&self) -> Option<Sender<StartupPreviewImage>> {
            None
        }
    }
}

pub(crate) use imp::StartupFlashGuard;
pub(crate) use imp::{StartupPreviewConfig, StartupPreviewImage, StartupPreviewWindow};

pub(crate) fn start_flash_guard() -> StartupFlashGuard {
    StartupFlashGuard::start()
}

pub(crate) fn start_preview_window(config: StartupPreviewConfig) -> StartupPreviewWindow {
    StartupPreviewWindow::start(config)
}
