#![allow(unsafe_code)]

use crate::core::state::StateStore;
use eframe::egui;
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

        let placement = store.window_placement();
        let window_size =
            valid_window_size(placement, min_window_size).unwrap_or(default_window_size);
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
        if let Some(position) = valid_window_position(placement) {
            attributes = attributes.with_position(winit::dpi::LogicalPosition::new(
                f64::from(position[0]),
                f64::from(position[1]),
            ));
        }
        if placement.maximized {
            attributes = attributes.with_maximized(true);
        }

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

    pub(super) fn into_window_after_context_destroy(self) -> Result<winit::window::Window, String> {
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

fn window_icon(icon: egui::IconData) -> Option<winit::window::Icon> {
    winit::window::Icon::from_rgba(icon.rgba, icon.width, icon.height).ok()
}

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
