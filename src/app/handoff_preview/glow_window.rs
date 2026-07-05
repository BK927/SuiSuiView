#![allow(unsafe_code)]

use crate::core::state::{StateStore, WindowPlacement};
use egui_winit::winit;
use std::ffi::{c_void, CString};
use std::num::NonZeroU32;
use winit::raw_window_handle::HasWindowHandle as _;

pub(super) struct GlutinWindowContext {
    window: winit::window::Window,
    gl_context: glutin::context::PossiblyCurrentContext,
    gl_display: glutin::display::Display,
    gl_surface: glutin::surface::Surface<glutin::surface::WindowSurface>,
}

impl GlutinWindowContext {
    pub(super) unsafe fn new(
        event_loop: &winit::event_loop::ActiveEventLoop,
        store: &StateStore,
        icon: egui::IconData,
        default_window_size: [f32; 2],
        min_window_size: [f32; 2],
    ) -> Result<Self, String> {
        use glutin::context::NotCurrentGlContext as _;
        use glutin::display::GetGlDisplay as _;
        use glutin::display::GlDisplay as _;
        use glutin::prelude::GlSurface as _;

        let attributes =
            startup_window_attributes(store, icon, default_window_size, min_window_size);

        let config_template = glutin::config::ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(None)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_transparency(false);

        let (mut window, gl_config) = glutin_winit::DisplayBuilder::new()
            .with_preference(glutin_winit::ApiPreference::FallbackEgl)
            .with_window_attributes(Some(attributes.clone()))
            .build(event_loop, config_template, |mut configs| {
                configs
                    .next()
                    .expect("at least one GL config should be available")
            })
            .map_err(|error| format!("failed to create GL config: {error}"))?;

        let gl_display = gl_config.display();
        let raw_window_handle = window
            .as_ref()
            .map(|window| window.window_handle().map(|handle| handle.as_raw()))
            .transpose()
            .map_err(|error| format!("failed to get raw window handle: {error}"))?;
        let context_attributes =
            glutin::context::ContextAttributesBuilder::new().build(raw_window_handle);
        let fallback_attributes = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::Gles(None))
            .build(raw_window_handle);

        let not_current = gl_display
            .create_context(&gl_config, &context_attributes)
            .or_else(|_| gl_display.create_context(&gl_config, &fallback_attributes))
            .map_err(|error| format!("failed to create GL context: {error}"))?;

        let window = window.take().unwrap_or_else(|| {
            glutin_winit::finalize_window(event_loop, attributes.clone(), &gl_config)
                .expect("failed to finalize GL window")
        });
        apply_dark_startup_chrome(&window);
        let size = window.inner_size();
        let width = NonZeroU32::new(size.width).unwrap_or(NonZeroU32::MIN);
        let height = NonZeroU32::new(size.height).unwrap_or(NonZeroU32::MIN);
        let surface_attributes =
            glutin::surface::SurfaceAttributesBuilder::<glutin::surface::WindowSurface>::new()
                .build(
                    window
                        .window_handle()
                        .map_err(|error| format!("failed to get window handle: {error}"))?
                        .as_raw(),
                    width,
                    height,
                );
        let gl_surface = gl_display
            .create_window_surface(&gl_config, &surface_attributes)
            .map_err(|error| format!("failed to create GL surface: {error}"))?;
        let gl_context = not_current
            .make_current(&gl_surface)
            .map_err(|error| format!("failed to make GL context current: {error}"))?;
        gl_surface
            .set_swap_interval(
                &gl_context,
                glutin::surface::SwapInterval::Wait(NonZeroU32::MIN),
            )
            .map_err(|error| format!("failed to set swap interval: {error}"))?;

        Ok(Self {
            window,
            gl_context,
            gl_display,
            gl_surface,
        })
    }

    pub(super) fn window(&self) -> &winit::window::Window {
        &self.window
    }

    pub(super) fn reveal_after_first_frame(&self) {
        crate::startup_window::reveal_main_windows();
        self.window.set_visible(true);
    }

    pub(super) fn resize(&self, physical_size: winit::dpi::PhysicalSize<u32>) {
        use glutin::surface::GlSurface as _;
        let Some(width) = NonZeroU32::new(physical_size.width) else {
            return;
        };
        let Some(height) = NonZeroU32::new(physical_size.height) else {
            return;
        };
        self.gl_surface.resize(&self.gl_context, width, height);
    }

    pub(super) fn swap_buffers(&self) -> Result<(), String> {
        use glutin::surface::GlSurface as _;
        self.gl_surface
            .swap_buffers(&self.gl_context)
            .map_err(|error| format!("failed to swap GL buffers: {error}"))
    }

    fn get_proc_address(&self, addr: &std::ffi::CStr) -> *const c_void {
        use glutin::display::GlDisplay as _;
        self.gl_display.get_proc_address(addr)
    }
}

pub(super) fn create_gl_display(
    event_loop: &winit::event_loop::ActiveEventLoop,
    store: &StateStore,
    icon: egui::IconData,
    default_window_size: [f32; 2],
    min_window_size: [f32; 2],
) -> Result<(GlutinWindowContext, glow::Context), String> {
    let glutin_window = unsafe {
        GlutinWindowContext::new(
            event_loop,
            store,
            icon,
            default_window_size,
            min_window_size,
        )?
    };
    let gl = unsafe {
        glow::Context::from_loader_function(|symbol| {
            let symbol = CString::new(symbol).expect("GL symbol names should not contain NUL");
            glutin_window.get_proc_address(&symbol)
        })
    };
    Ok((glutin_window, gl))
}

/// Build the shared startup window attributes (placement/size/icon, created
/// hidden with dark chrome) used by both the Glow and WGPU-direct paths.
fn startup_window_attributes(
    store: &StateStore,
    icon: egui::IconData,
    default_window_size: [f32; 2],
    min_window_size: [f32; 2],
) -> winit::window::WindowAttributes {
    let placement = store.window_placement();
    let window_size = valid_window_size(placement, min_window_size).unwrap_or(default_window_size);
    let mut attributes = winit::window::WindowAttributes::default()
        .with_resizable(true)
        .with_inner_size(winit::dpi::LogicalSize::new(
            f64::from(window_size[0]),
            f64::from(window_size[1]),
        ))
        .with_min_inner_size(winit::dpi::LogicalSize::new(
            f64::from(min_window_size[0]),
            f64::from(min_window_size[1]),
        ))
        .with_title("SuiSuiView")
        .with_window_icon(window_icon(icon))
        .with_visible(false);
    match startup_position(placement) {
        StartupPosition::Physical([x, y]) => {
            attributes = attributes.with_position(winit::dpi::PhysicalPosition::new(x, y));
        }
        StartupPosition::Logical([x, y]) => {
            attributes = attributes
                .with_position(winit::dpi::LogicalPosition::new(f64::from(x), f64::from(y)));
        }
        StartupPosition::OsDefault => {}
    }
    if placement.maximized {
        attributes = attributes.with_maximized(true);
    }
    attributes
}

/// How the saved outer position should be restored. Physical is preferred
/// because it is scale-free on Windows and survives mixed-DPI restarts; the
/// legacy logical value is a fallback for state saved by an older build, and
/// `OsDefault` leaves placement to the OS when nothing valid was saved.
#[derive(Debug, Clone, Copy, PartialEq)]
enum StartupPosition {
    Physical([i32; 2]),
    Logical([f32; 2]),
    OsDefault,
}

/// Choose the restore position, preferring the scale-free physical value over
/// the legacy logical one. The physical value is only probed on Windows, where
/// `valid_window_position_px` checks it still lands on the connected desktop.
fn startup_position(placement: &WindowPlacement) -> StartupPosition {
    #[cfg(target_os = "windows")]
    let valid_px = valid_window_position_px(placement);
    #[cfg(not(target_os = "windows"))]
    let valid_px = None;
    // The legacy logical value predates the physical field and can carry
    // garbage accumulated by the old drift bug (observed: -2321 logical from a
    // desktop whose leftmost physical edge is -1440). Its exact physical spot
    // depends on a scale we cannot know here, but scales are bounded, so
    // probing the raw value against the virtual screen rejects far-off-screen
    // garbage while keeping sane one-time migrations.
    #[cfg(target_os = "windows")]
    let valid_logical = valid_window_position(placement).filter(|&[x, y]| {
        virtual_screen_rect()
            .is_some_and(|screen| position_probe_on_virtual_screen([x as i32, y as i32], screen))
    });
    #[cfg(not(target_os = "windows"))]
    let valid_logical = valid_window_position(placement);
    select_startup_position(valid_px, valid_logical)
}

/// Pure position-selection order (physical over legacy logical over OS
/// default), split out so the preference is unit-testable without Win32.
fn select_startup_position(
    valid_px: Option<[i32; 2]>,
    valid_logical: Option<[f32; 2]>,
) -> StartupPosition {
    if let Some(position) = valid_px {
        return StartupPosition::Physical(position);
    }
    if let Some(position) = valid_logical {
        return StartupPosition::Logical(position);
    }
    StartupPosition::OsDefault
}

/// Create a plain winit window (no OpenGL context) for the WGPU-direct startup
/// path, using the same placement/visibility/dark-chrome as the Glow window.
pub(super) fn create_plain_window(
    event_loop: &winit::event_loop::ActiveEventLoop,
    store: &StateStore,
    icon: egui::IconData,
    default_window_size: [f32; 2],
    min_window_size: [f32; 2],
) -> Result<winit::window::Window, String> {
    let attributes = startup_window_attributes(store, icon, default_window_size, min_window_size);
    let window = event_loop
        .create_window(attributes)
        .map_err(|error| format!("failed to create window: {error}"))?;
    apply_dark_startup_chrome(&window);
    Ok(window)
}

fn window_icon(icon: egui::IconData) -> Option<winit::window::Icon> {
    winit::window::Icon::from_rgba(icon.rgba, icon.width, icon.height).ok()
}

/// Give the window dark chrome from creation so the default light title bar and
/// the white pre-render background buffer never show on this dark-themed app:
/// a dark immersive title bar, and a dark class background brush so the frame
/// Windows paints before the first GL frame is composited is the app background
/// color instead of white (winit leaves `hbrBackground` NULL, which is what
/// lets the white startup buffer show through).
#[cfg(target_os = "windows")]
fn apply_dark_startup_chrome(window: &winit::window::Window) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let winit::raw_window_handle::RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    let hwnd = win32.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    unsafe {
        let enabled: i32 = 1;
        let _ = windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            windows_sys::Win32::Graphics::Dwm::DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &enabled as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
        // COLORREF is 0x00BBGGRR; the viewer clears to ~rgb(4, 4, 5).
        let brush = windows_sys::Win32::Graphics::Gdi::CreateSolidBrush(0x0005_0404);
        if !brush.is_null() {
            windows_sys::Win32::UI::WindowsAndMessaging::SetClassLongPtrW(
                hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::GCLP_HBRBACKGROUND,
                brush as isize,
            );
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn apply_dark_startup_chrome(_window: &winit::window::Window) {}

fn valid_window_size(
    placement: &crate::core::state::WindowPlacement,
    min_window_size: [f32; 2],
) -> Option<[f32; 2]> {
    let [width, height] = placement.inner_size?;
    (width.is_finite()
        && height.is_finite()
        && width >= min_window_size[0]
        && height >= min_window_size[1])
        .then_some([width, height])
}

fn valid_window_position(placement: &crate::core::state::WindowPlacement) -> Option<[f32; 2]> {
    let [x, y] = placement.outer_position?;
    (x.is_finite() && y.is_finite()).then_some([x, y])
}

// TEMP diagnostic: log DPI-relevant window events to a file when SUISUI_DPIDBG
// is set, to characterize the cross-monitor drag instability.
pub(super) fn dbg_dpi_event(event: &winit::event::WindowEvent) {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    if std::env::var_os("SUISUI_DPIDBG").is_none() {
        return;
    }
    let line = match event {
        winit::event::WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
            format!("ScaleFactorChanged scale={scale_factor}")
        }
        winit::event::WindowEvent::Resized(size) => {
            format!("Resized {}x{}", size.width, size.height)
        }
        winit::event::WindowEvent::Moved(pos) => format!("Moved {},{}", pos.x, pos.y),
        _ => return,
    };
    let t = START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
        * 1000.0;
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join("suisui_dpidbg.log"))
    {
        let _ = writeln!(f, "t={t:9.2}ms {line}");
    }
}

/// Physical position of the virtual screen (the union of all monitors), in the
/// same scale-free coordinate space as the saved physical outer position.
#[derive(Debug, Clone, Copy)]
struct ScreenRect {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

/// Saved physical position, if it still lands on the connected desktop. The
/// probe point (a spot inside the title bar) must lie within the virtual
/// screen, so a stale position from a disconnected monitor is discarded and
/// the OS default placement is used instead.
#[cfg(target_os = "windows")]
fn valid_window_position_px(placement: &WindowPlacement) -> Option<[i32; 2]> {
    let position = placement.outer_position_px?;
    let screen = virtual_screen_rect()?;
    position_probe_on_virtual_screen(position, screen).then_some(position)
}

/// The desktop bounding box spanning every monitor, read from Win32. Returns
/// `None` if the reported size is degenerate (no usable desktop).
#[cfg(target_os = "windows")]
fn virtual_screen_rect() -> Option<ScreenRect> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    let (left, top, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    (width > 0 && height > 0).then_some(ScreenRect {
        left,
        top,
        width,
        height,
    })
}

/// Pure check that a saved physical position still lands on the desktop: a
/// point just inside the title bar (32 px right, 16 px down from the corner)
/// must fall within `screen`. The right/bottom edges are exclusive, matching
/// the half-open virtual-screen span.
fn position_probe_on_virtual_screen(position: [i32; 2], screen: ScreenRect) -> bool {
    let probe_x = position[0] + 32;
    let probe_y = position[1] + 16;
    probe_x >= screen.left
        && probe_y >= screen.top
        && probe_x < screen.left + screen.width
        && probe_y < screen.top + screen.height
}

#[cfg(test)]
mod tests {
    use super::{
        position_probe_on_virtual_screen, select_startup_position, ScreenRect, StartupPosition,
    };

    const SCREEN: ScreenRect = ScreenRect {
        left: 0,
        top: 0,
        width: 1920,
        height: 1080,
    };

    #[test]
    fn probe_accepts_position_inside_virtual_screen() {
        assert!(position_probe_on_virtual_screen([100, 120], SCREEN));
    }

    #[test]
    fn probe_rejects_position_left_of_virtual_screen() {
        // Probe adds +32 to x; -64 leaves the probe point at x = -32.
        assert!(!position_probe_on_virtual_screen([-64, 120], SCREEN));
    }

    #[test]
    fn probe_rejects_position_below_virtual_screen() {
        // Probe adds +16 to y; 1080 leaves the probe point at y = 1096.
        assert!(!position_probe_on_virtual_screen([100, 1080], SCREEN));
    }

    #[test]
    fn probe_accepts_position_with_probe_point_exactly_on_top_left_edge() {
        // Probe point [-32 + 32, -16 + 16] = [0, 0] lands on the inclusive
        // top-left edge of the screen.
        assert!(position_probe_on_virtual_screen([-32, -16], SCREEN));
    }

    #[test]
    fn probe_rejects_position_with_probe_point_on_exclusive_right_edge() {
        // Probe point x = 1920 sits on the exclusive right edge.
        assert!(!position_probe_on_virtual_screen([1888, 120], SCREEN));
    }

    #[test]
    fn probe_rejects_drift_garbage_from_the_field() {
        // Regression: a real state file carried legacy logical -2321,953
        // accumulated by the old drift bug; the desktop's leftmost physical
        // edge was -1440. The legacy fallback must reject it (as-if-physical
        // probe) instead of restoring the window off-screen.
        let two_monitor = ScreenRect {
            left: -1440,
            top: 0,
            width: 1440 + 3840,
            height: 2560,
        };
        assert!(!position_probe_on_virtual_screen([-2321, 953], two_monitor));
        // A sane on-screen legacy value keeps working.
        assert!(position_probe_on_virtual_screen([200, 150], two_monitor));
    }

    #[test]
    fn startup_position_prefers_physical_when_both_present() {
        assert_eq!(
            select_startup_position(Some([200, 150]), Some([120.0, 90.0])),
            StartupPosition::Physical([200, 150])
        );
    }

    #[test]
    fn startup_position_uses_legacy_logical_when_physical_absent() {
        assert_eq!(
            select_startup_position(None, Some([120.0, 90.0])),
            StartupPosition::Logical([120.0, 90.0])
        );
    }

    #[test]
    fn startup_position_falls_back_to_legacy_when_physical_probe_fails() {
        // `startup_position` passes `None` for the physical slot when the probe
        // rejects a stale off-screen position, so the legacy value is used.
        assert_eq!(
            select_startup_position(None, Some([120.0, 90.0])),
            StartupPosition::Logical([120.0, 90.0])
        );
    }

    #[test]
    fn startup_position_uses_os_default_when_nothing_valid() {
        assert_eq!(
            select_startup_position(None, None),
            StartupPosition::OsDefault
        );
    }
}
