#![allow(unsafe_code)]

use std::ffi::{c_void, CString};
use std::num::NonZeroU32;

use egui_winit::winit;
use winit::raw_window_handle::HasWindowHandle as _;

const WINDOW_SIZE: [f64; 2] = [720.0, 420.0];

pub(crate) struct GlutinWindowContext {
    window: winit::window::Window,
    gl_context: glutin::context::PossiblyCurrentContext,
    gl_display: glutin::display::Display,
    gl_surface: glutin::surface::Surface<glutin::surface::WindowSurface>,
}

impl GlutinWindowContext {
    unsafe fn new(event_loop: &winit::event_loop::ActiveEventLoop) -> Result<Self, String> {
        use glutin::context::NotCurrentGlContext as _;
        use glutin::display::GetGlDisplay as _;
        use glutin::display::GlDisplay as _;
        use glutin::prelude::GlSurface as _;

        let window_attributes = winit::window::WindowAttributes::default()
            .with_resizable(true)
            .with_inner_size(winit::dpi::LogicalSize::new(WINDOW_SIZE[0], WINDOW_SIZE[1]))
            .with_title("SuiSuiView runtime probe")
            .with_visible(false);

        let config_template = glutin::config::ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(None)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_transparency(false);

        let (mut window, gl_config) = glutin_winit::DisplayBuilder::new()
            .with_preference(glutin_winit::ApiPreference::FallbackEgl)
            .with_window_attributes(Some(window_attributes.clone()))
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
            glutin_winit::finalize_window(event_loop, window_attributes.clone(), &gl_config)
                .expect("failed to finalize GL window")
        });
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

    pub(crate) fn window(&self) -> &winit::window::Window {
        &self.window
    }

    pub(crate) fn resize(&self, physical_size: winit::dpi::PhysicalSize<u32>) {
        use glutin::surface::GlSurface as _;
        let Some(width) = NonZeroU32::new(physical_size.width) else {
            return;
        };
        let Some(height) = NonZeroU32::new(physical_size.height) else {
            return;
        };
        self.gl_surface.resize(&self.gl_context, width, height);
    }

    pub(crate) fn swap_buffers(&self) -> Result<(), String> {
        use glutin::surface::GlSurface as _;
        self.gl_surface
            .swap_buffers(&self.gl_context)
            .map_err(|error| format!("failed to swap GL buffers: {error}"))
    }

    pub(crate) fn into_window_after_context_destroy(self) -> Result<winit::window::Window, String> {
        use glutin::context::PossiblyCurrentGlContext as _;

        let Self {
            window,
            gl_context,
            gl_display,
            gl_surface,
        } = self;
        let not_current = gl_context
            .make_not_current()
            .map_err(|error| format!("failed to make GL context not current: {error}"))?;
        drop(gl_surface);
        drop(not_current);
        drop(gl_display);
        Ok(window)
    }

    fn get_proc_address(&self, addr: &std::ffi::CStr) -> *const c_void {
        use glutin::display::GlDisplay as _;
        self.gl_display.get_proc_address(addr)
    }
}

pub(crate) fn create_display(
    event_loop: &winit::event_loop::ActiveEventLoop,
) -> Result<(GlutinWindowContext, glow::Context), String> {
    let glutin_window = unsafe { GlutinWindowContext::new(event_loop)? };
    let gl = unsafe {
        glow::Context::from_loader_function(|symbol| {
            let symbol = CString::new(symbol).expect("GL symbol names should not contain NUL");
            glutin_window.get_proc_address(&symbol)
        })
    };
    Ok((glutin_window, gl))
}
