//! Draw-time kernel upscaling for the Glow (OpenGL) renderer — the Honeyview
//! model. When Glow is the active backend and the CPU upscale filter maps to a
//! resampling kernel, a small GL fragment shader enlarges the page at draw time
//! from the native-size prepared texture instead of pre-enlarging it on the CPU.
//! Non-kernel filters keep egui's built-in sampler / the CPU pre-enlarge path.
//!
//! Driven through an `egui::PaintCallback` carrying an `egui_glow::CallbackFn`.
//! egui_glow sets the GL viewport to the callback rect and the scissor to the
//! clip rect before the callback runs, and restores its own painting state
//! afterward, so the callback only compiles-once, binds the egui texture, and
//! draws a full-viewport triangle. On compile/link failure the shared state
//! latches `failed` and the routing helper permanently degrades to the plain path.

use super::viewer::ViewMode;
use super::{SuiSuiViewApp, TextureSampling};
use crate::core::state::{CpuScaleFilter, FitMode};
use egui::{Color32, PaintCallback, Rect, TextureHandle, TextureId};
use egui_glow::glow::{self, HasContext as _};
use std::sync::{Arc, OnceLock};

/// Which resampling kernel the shader runs, derived from the user's CPU upscale
/// filter. Only the filters with a faithful GPU analogue map here; every other
/// filter routes to `None` (plain sampler / CPU pre-enlarge fallback).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum KernelChoice {
    /// Catmull-Rom cubic — BC-cubic with B=0, C=0.5.
    CatmullRom,
    /// Mitchell-Netravali cubic — BC-cubic with B=1/3, C=1/3.
    Mitchell,
    /// Lanczos with radius 2.
    Lanczos2,
    /// Lanczos with radius 3.
    Lanczos3,
}

impl KernelChoice {
    /// The kernel for a CPU upscale filter, or `None` for the filters that have
    /// no faithful GPU kernel (Bilinear, Nearest, Box, Hamming, Gaussian) — those
    /// keep their existing sampler / CPU pre-enlarge behavior.
    pub(in crate::app) fn from_filter(filter: CpuScaleFilter) -> Option<Self> {
        match filter {
            CpuScaleFilter::CatmullRom => Some(Self::CatmullRom),
            CpuScaleFilter::Mitchell => Some(Self::Mitchell),
            CpuScaleFilter::Lanczos2 => Some(Self::Lanczos2),
            CpuScaleFilter::Lanczos3 => Some(Self::Lanczos3),
            CpuScaleFilter::Nearest
            | CpuScaleFilter::Box
            | CpuScaleFilter::Bilinear
            | CpuScaleFilter::Hamming
            | CpuScaleFilter::Gaussian => None,
        }
    }

    /// Technical label shown on the top-bar chip when this kernel drew the page.
    pub(in crate::app) fn label(self) -> &'static str {
        match self {
            Self::CatmullRom => "CatmullRom",
            Self::Mitchell => "Mitchell",
            Self::Lanczos2 => "Lanczos2",
            Self::Lanczos3 => "Lanczos3",
        }
    }

    /// Shader `u_kernel` selector: 0 = BC-cubic, 1 = Lanczos.
    fn kernel_id(self) -> i32 {
        match self {
            Self::CatmullRom | Self::Mitchell => 0,
            Self::Lanczos2 | Self::Lanczos3 => 1,
        }
    }

    /// BC-cubic (B, C) coefficients; unused by the Lanczos kernels (kernel_id 1).
    fn b_c(self) -> (f32, f32) {
        match self {
            Self::CatmullRom => (0.0, 0.5),
            Self::Mitchell => (1.0 / 3.0, 1.0 / 3.0),
            Self::Lanczos2 | Self::Lanczos3 => (0.0, 0.0),
        }
    }

    /// Lanczos radius; unused by the cubic kernels (kernel_id 0).
    fn radius(self) -> f32 {
        match self {
            Self::CatmullRom | Self::Mitchell => 0.0,
            Self::Lanczos2 => 2.0,
            Self::Lanczos3 => 3.0,
        }
    }
}

/// Lazily compiled GL program shared between the app (routing health check) and
/// the paint callbacks (compile + draw). Behind an `Arc<Mutex<_>>` on the app.
pub(in crate::app) struct GlowScaleState {
    kernel: Option<CompiledKernel>,
    /// Latched once compile/link fails on a broken driver: the app must then
    /// never emit the callback again and behavior degrades to the plain path.
    failed: bool,
}

impl GlowScaleState {
    pub(in crate::app) fn new() -> Self {
        Self {
            kernel: None,
            failed: false,
        }
    }
}

struct CompiledKernel {
    program: glow::Program,
    vao: Option<glow::VertexArray>,
    vbo: glow::Buffer,
    a_pos: u32,
    u_sampler: Option<glow::UniformLocation>,
    u_src_size: Option<glow::UniformLocation>,
    u_kernel: Option<glow::UniformLocation>,
    u_b: Option<glow::UniformLocation>,
    u_c: Option<glow::UniformLocation>,
    u_radius: Option<glow::UniformLocation>,
}

/// `SUISUIVIEW_GLOW_SCALE_LOG=1` traces shader compile once and each kernel draw.
/// Read once via `OnceLock` so the hot path pays nothing when unset.
fn log_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("SUISUIVIEW_GLOW_SCALE_LOG").is_some())
}

impl SuiSuiViewApp {
    /// The resampling kernel to draw fit-mode / strip pages with in Glow, or
    /// `None` when the plain sampler path must be used. `Some` only when ALL hold:
    /// the Glow backend is live at runtime (never derived from settings alone),
    /// the view is a fit mode or the vertical strip, the CPU upscale filter maps
    /// to a kernel, and the shader has not failed.
    pub(in crate::app) fn glow_upscale_kernel(&self) -> Option<KernelChoice> {
        if !self.glow_is_active_backend() {
            return None;
        }
        if self.glow_scale.lock().unwrap().failed {
            return None;
        }
        let fit_ok = self.view_mode == ViewMode::VerticalStrip
            || matches!(
                self.fit_mode,
                FitMode::FitPage | FitMode::FitWidth | FitMode::FitHeight
            );
        if !fit_ok {
            return None;
        }
        KernelChoice::from_filter(self.settings.cpu_upscale_filter)
    }

    /// Whether Glow (not wgpu) is the runtime renderer this frame. The wgpu stage
    /// is the only place that sets `gpu_effects_available` / `gpu_target_format`,
    /// so their absence is the authoritative "Glow is live" signal — the same
    /// state the paint path reads to route WGSL callbacks vs the CPU texture path.
    pub(in crate::app) fn glow_is_active_backend(&self) -> bool {
        !self.gpu_effects_available && self.gpu_target_format.is_none()
    }

    /// True when `glow_upscale_kernel` would return `Some` — the routing signal
    /// threaded into the CPU display-upscale policy so the worker stops
    /// pre-enlarging pages the shader will enlarge at draw time.
    pub(in crate::app) fn glow_kernel_available(&self) -> bool {
        self.glow_upscale_kernel().is_some()
    }

    /// Draw one native-size page texture into `page_rect`, enlarging it with the
    /// kernel shader when the view enlarges it (dest px > source px) and the
    /// routing allows; otherwise fall back to `painter.image` (plain sampler).
    /// Returns the kernel that drew, or `None` when the plain path drew.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::app) fn paint_page_image_kernel_or_plain(
        &self,
        painter: &egui::Painter,
        texture: &TextureHandle,
        page_rect: Rect,
        tint: Color32,
        pixels_per_point: f32,
        allow_kernel: bool,
        sampling: TextureSampling,
        page_index: usize,
    ) -> Option<KernelChoice> {
        let uv = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        let kernel = self.kernel_for_draw(
            allow_kernel,
            sampling,
            tint,
            texture.size(),
            page_rect,
            pixels_per_point,
        );
        let Some((choice, src_size, scale)) = kernel else {
            painter.image(texture.id(), page_rect, uv, tint);
            return None;
        };
        if log_enabled() {
            eprintln!(
                "[glow-scale] page={page_index} scale={scale:.2} kernel={}",
                choice.label()
            );
        }
        self.emit_kernel_callback(painter, texture.id(), page_rect, src_size, choice);
        Some(choice)
    }

    /// Resolve the kernel + source size + scale for a draw, or `None` to take the
    /// plain path. Enforces the draw-site preconditions: kernel routing is on,
    /// the texture is linear-sampled (not the nearest source-inspection texture),
    /// the tint is opaque (transitions keep the plain path), and the view
    /// genuinely enlarges the page (scale > 1).
    fn kernel_for_draw(
        &self,
        allow_kernel: bool,
        sampling: TextureSampling,
        tint: Color32,
        src_texels: [usize; 2],
        page_rect: Rect,
        pixels_per_point: f32,
    ) -> Option<(KernelChoice, [f32; 2], f32)> {
        if !allow_kernel || sampling != TextureSampling::Linear || tint != Color32::WHITE {
            return None;
        }
        let choice = self.glow_upscale_kernel()?;
        let src = [src_texels[0].max(1) as f32, src_texels[1].max(1) as f32];
        let dest = [
            page_rect.width() * pixels_per_point,
            page_rect.height() * pixels_per_point,
        ];
        let scale = (dest[0] / src[0]).min(dest[1] / src[1]);
        (scale > 1.0).then_some((choice, src, scale))
    }

    fn emit_kernel_callback(
        &self,
        painter: &egui::Painter,
        texture_id: TextureId,
        page_rect: Rect,
        src_size: [f32; 2],
        choice: KernelChoice,
    ) {
        let state = self.glow_scale.clone();
        let ctx = painter.ctx().clone();
        let callback = PaintCallback {
            rect: page_rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                let gl = painter.gl();
                let mut state = state.lock().unwrap();
                if state.failed {
                    return;
                }
                if state.kernel.is_none() {
                    match compile_kernel(gl) {
                        Ok(kernel) => {
                            if log_enabled() {
                                eprintln!("[glow-scale] shader compiled");
                            }
                            state.kernel = Some(kernel);
                        }
                        Err(error) => {
                            eprintln!("[glow-scale] shader compile/link failed: {error}");
                            state.failed = true;
                            // Next frame the routing helper takes the plain path;
                            // this one page-frame draws nothing on broken drivers.
                            ctx.request_repaint();
                            return;
                        }
                    }
                }
                let Some(kernel) = state.kernel.as_ref() else {
                    return;
                };
                let Some(texture) = painter.texture(texture_id) else {
                    return;
                };
                // SAFETY: single GL context, called from egui_glow's paint pass
                // with the callback viewport/scissor already set; egui_glow
                // restores its own state after the callback returns.
                unsafe { draw_kernel(gl, kernel, texture, src_size, choice) };
            })),
        };
        painter.add(callback);
    }
}

/// GLSL body (sans version header) shared vertex+fragment source. The version
/// declaration and `NEW_SHADER_INTERFACE` define are prepended at compile time
/// from the detected `ShaderVersion`, mirroring egui_glow so the program builds
/// on GL 2.1 / GLES2-level drivers (this renderer is the weak-machine fallback).
const VERT_SRC: &str = r#"
#if NEW_SHADER_INTERFACE
    #define I in
    #define O out
#else
    #define I attribute
    #define O varying
#endif
#ifdef GL_ES
    precision highp float;
#endif
I vec2 a_pos;
O vec2 v_uv;
void main() {
    gl_Position = vec4(a_pos, 0.0, 1.0);
    // NDC top (+1) maps to the top of the callback viewport; the egui texture
    // has row 0 at the top, so flip Y into a top-left-origin uv.
    v_uv = vec2(0.5 + 0.5 * a_pos.x, 0.5 - 0.5 * a_pos.y);
}
"#;

const FRAG_SRC: &str = r#"
#ifdef GL_ES
    #if defined(GL_FRAGMENT_PRECISION_HIGH) && GL_FRAGMENT_PRECISION_HIGH == 1
        precision highp float;
    #else
        precision mediump float;
    #endif
#endif
uniform sampler2D u_sampler;
uniform vec2 u_src_size;
uniform int u_kernel;   // 0 = BC-cubic, 1 = Lanczos
uniform float u_b;
uniform float u_c;
uniform float u_radius;
#if NEW_SHADER_INTERFACE
    in vec2 v_uv;
    out vec4 f_color;
    #define gl_FragColor f_color
    #define texture2D texture
#else
    varying vec2 v_uv;
#endif

// BC-cubic (Mitchell-Netravali family). Mirrors glow_scale::cubic_bc_weight.
float cubic_weight(float x) {
    x = abs(x);
    float x2 = x * x;
    float x3 = x2 * x;
    if (x < 1.0) {
        return ((12.0 - 9.0 * u_b - 6.0 * u_c) * x3
              + (-18.0 + 12.0 * u_b + 6.0 * u_c) * x2
              + (6.0 - 2.0 * u_b)) * (1.0 / 6.0);
    } else if (x < 2.0) {
        return ((-u_b - 6.0 * u_c) * x3
              + (6.0 * u_b + 30.0 * u_c) * x2
              + (-12.0 * u_b - 48.0 * u_c) * x
              + (8.0 * u_b + 24.0 * u_c)) * (1.0 / 6.0);
    }
    return 0.0;
}

float sinc(float x) {
    if (abs(x) < 1.0e-4) return 1.0;
    float p = 3.14159265358979 * x;
    return sin(p) / p;
}

// Lanczos. Mirrors glow_scale::lanczos_weight.
float lanczos_weight(float x) {
    if (abs(x) >= u_radius) return 0.0;
    return sinc(x) * sinc(x / u_radius);
}

float kernel_weight(float x) {
    if (u_kernel == 0) return cubic_weight(x);
    return lanczos_weight(x);
}

void main() {
    // Continuous source coordinate in texel-index space (centers at integers).
    vec2 src = v_uv * u_src_size - 0.5;
    vec2 base = floor(src);
    vec2 frac = src - base;
    vec4 accum = vec4(0.0);
    float wsum = 0.0;
    // 6 taps/axis covers the 4-tap cubic (extra taps weigh 0) and 6-tap Lanczos3;
    // Lanczos2 trims its own support via the weight. Sampling at exact texel
    // centers makes LINEAR return true texels; sum in gamma, output gamma (egui
    // textures are sRGB-unaware) to match egui_glow.
    for (int j = -2; j <= 3; j++) {
        float wy = kernel_weight(frac.y - float(j));
        for (int i = -2; i <= 3; i++) {
            float w = kernel_weight(frac.x - float(i)) * wy;
            vec2 tap = (base + vec2(float(i), float(j)) + 0.5) / u_src_size;
            accum += w * texture2D(u_sampler, tap);
            wsum += w;
        }
    }
    // Negative-lobe kernels can undershoot; clamp. Pages are opaque, so the
    // premultiplied output egui blend expects is just (rgb, 1).
    vec3 rgb = clamp(accum.rgb / wsum, 0.0, 1.0);
    gl_FragColor = vec4(rgb, 1.0);
}
"#;

fn compile_kernel(gl: &glow::Context) -> Result<CompiledKernel, String> {
    let version = egui_glow::ShaderVersion::get(gl);
    let header = format!(
        "{}\n#define NEW_SHADER_INTERFACE {}\n",
        version.version_declaration(),
        version.is_new_shader_interface() as i32
    );
    unsafe {
        let vert = compile_shader(gl, glow::VERTEX_SHADER, &format!("{header}{VERT_SRC}"))?;
        let frag = compile_shader(gl, glow::FRAGMENT_SHADER, &format!("{header}{FRAG_SRC}"))
            .inspect_err(|_| gl.delete_shader(vert))?;
        // link_program deletes the program on failure; we still own the shaders.
        let program = link_program(gl, vert, frag).inspect_err(|_| {
            gl.delete_shader(vert);
            gl.delete_shader(frag);
        })?;
        gl.detach_shader(program, vert);
        gl.detach_shader(program, frag);
        gl.delete_shader(vert);
        gl.delete_shader(frag);

        let a_pos = gl
            .get_attrib_location(program, "a_pos")
            .ok_or_else(|| "missing a_pos attribute".to_owned())?;
        let vao = gl.create_vertex_array().ok();
        let vbo = gl.create_buffer()?;
        if let Some(vao) = vao {
            gl.bind_vertex_array(Some(vao));
        }
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        // Oversized fullscreen triangle in NDC covering the whole [-1, 1] square.
        let verts: [f32; 6] = [-1.0, -1.0, 3.0, -1.0, -1.0, 3.0];
        gl.buffer_data_u8_slice(
            glow::ARRAY_BUFFER,
            bytemuck::cast_slice(&verts),
            glow::STATIC_DRAW,
        );
        if vao.is_some() {
            gl.bind_vertex_array(None);
        }
        gl.bind_buffer(glow::ARRAY_BUFFER, None);

        Ok(CompiledKernel {
            program,
            vao,
            vbo,
            a_pos,
            u_sampler: gl.get_uniform_location(program, "u_sampler"),
            u_src_size: gl.get_uniform_location(program, "u_src_size"),
            u_kernel: gl.get_uniform_location(program, "u_kernel"),
            u_b: gl.get_uniform_location(program, "u_b"),
            u_c: gl.get_uniform_location(program, "u_c"),
            u_radius: gl.get_uniform_location(program, "u_radius"),
        })
    }
}

unsafe fn compile_shader(
    gl: &glow::Context,
    shader_type: u32,
    source: &str,
) -> Result<glow::Shader, String> {
    unsafe {
        let shader = gl.create_shader(shader_type)?;
        gl.shader_source(shader, source);
        gl.compile_shader(shader);
        if gl.get_shader_compile_status(shader) {
            Ok(shader)
        } else {
            let log = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            Err(log)
        }
    }
}

unsafe fn link_program(
    gl: &glow::Context,
    vert: glow::Shader,
    frag: glow::Shader,
) -> Result<glow::Program, String> {
    unsafe {
        let program = gl.create_program()?;
        gl.attach_shader(program, vert);
        gl.attach_shader(program, frag);
        gl.link_program(program);
        if gl.get_program_link_status(program) {
            Ok(program)
        } else {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            Err(log)
        }
    }
}

unsafe fn draw_kernel(
    gl: &glow::Context,
    kernel: &CompiledKernel,
    texture: glow::Texture,
    src_size: [f32; 2],
    choice: KernelChoice,
) {
    unsafe {
        gl.use_program(Some(kernel.program));
        if let Some(vao) = kernel.vao {
            gl.bind_vertex_array(Some(vao));
        }
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(kernel.vbo));
        gl.enable_vertex_attrib_array(kernel.a_pos);
        gl.vertex_attrib_pointer_f32(kernel.a_pos, 2, glow::FLOAT, false, 0, 0);

        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.uniform_1_i32(kernel.u_sampler.as_ref(), 0);
        gl.uniform_2_f32(kernel.u_src_size.as_ref(), src_size[0], src_size[1]);
        gl.uniform_1_i32(kernel.u_kernel.as_ref(), choice.kernel_id());
        let (b, c) = choice.b_c();
        gl.uniform_1_f32(kernel.u_b.as_ref(), b);
        gl.uniform_1_f32(kernel.u_c.as_ref(), c);
        gl.uniform_1_f32(kernel.u_radius.as_ref(), choice.radius());

        gl.draw_arrays(glow::TRIANGLES, 0, 3);

        // Restore only what egui_glow does not: it rebinds its own program, VAO,
        // buffers and textures via prepare_painting after the callback, but we
        // disable our attribute array so a legacy (no-VAO) context does not carry
        // a dangling enabled attrib into egui's next mesh draw.
        gl.disable_vertex_attrib_array(kernel.a_pos);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        if kernel.vao.is_some() {
            gl.bind_vertex_array(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference BC-cubic weight the GLSL `cubic_weight` mirrors.
    fn cubic_bc_weight(x: f32, b: f32, c: f32) -> f32 {
        let x = x.abs();
        let x2 = x * x;
        let x3 = x2 * x;
        if x < 1.0 {
            ((12.0 - 9.0 * b - 6.0 * c) * x3 + (-18.0 + 12.0 * b + 6.0 * c) * x2 + (6.0 - 2.0 * b))
                / 6.0
        } else if x < 2.0 {
            ((-b - 6.0 * c) * x3
                + (6.0 * b + 30.0 * c) * x2
                + (-12.0 * b - 48.0 * c) * x
                + (8.0 * b + 24.0 * c))
                / 6.0
        } else {
            0.0
        }
    }

    fn sinc(x: f32) -> f32 {
        if x.abs() < 1.0e-4 {
            1.0
        } else {
            let p = std::f32::consts::PI * x;
            p.sin() / p
        }
    }

    /// Reference Lanczos weight the GLSL `lanczos_weight` mirrors.
    fn lanczos_weight(x: f32, a: f32) -> f32 {
        if x.abs() >= a {
            0.0
        } else {
            sinc(x) * sinc(x / a)
        }
    }

    #[test]
    fn filter_maps_only_kernel_filters() {
        assert_eq!(
            KernelChoice::from_filter(CpuScaleFilter::CatmullRom),
            Some(KernelChoice::CatmullRom)
        );
        assert_eq!(
            KernelChoice::from_filter(CpuScaleFilter::Mitchell),
            Some(KernelChoice::Mitchell)
        );
        assert_eq!(
            KernelChoice::from_filter(CpuScaleFilter::Lanczos2),
            Some(KernelChoice::Lanczos2)
        );
        assert_eq!(
            KernelChoice::from_filter(CpuScaleFilter::Lanczos3),
            Some(KernelChoice::Lanczos3)
        );
        for filter in [
            CpuScaleFilter::Nearest,
            CpuScaleFilter::Box,
            CpuScaleFilter::Bilinear,
            CpuScaleFilter::Hamming,
            CpuScaleFilter::Gaussian,
        ] {
            assert_eq!(KernelChoice::from_filter(filter), None);
        }
    }

    #[test]
    fn kernel_params_match_the_intended_families() {
        assert_eq!(KernelChoice::CatmullRom.kernel_id(), 0);
        assert_eq!(KernelChoice::CatmullRom.b_c(), (0.0, 0.5));
        assert_eq!(KernelChoice::Mitchell.kernel_id(), 0);
        assert_eq!(KernelChoice::Mitchell.b_c(), (1.0 / 3.0, 1.0 / 3.0));
        assert_eq!(KernelChoice::Lanczos2.kernel_id(), 1);
        assert_eq!(KernelChoice::Lanczos2.radius(), 2.0);
        assert_eq!(KernelChoice::Lanczos3.kernel_id(), 1);
        assert_eq!(KernelChoice::Lanczos3.radius(), 3.0);
    }

    #[test]
    fn catmull_rom_weights_are_pinned() {
        // Catmull-Rom (B=0, C=0.5): interpolating (w(0)=1, integer taps=0),
        // with the classic negative lobe at |x|=1.5.
        assert!((cubic_bc_weight(0.0, 0.0, 0.5) - 1.0).abs() < 1e-6);
        assert!(cubic_bc_weight(1.0, 0.0, 0.5).abs() < 1e-6);
        assert!(cubic_bc_weight(2.0, 0.0, 0.5).abs() < 1e-6);
        // w(0.5) = 0.5625, w(1.5) = -0.0625 for Catmull-Rom.
        assert!((cubic_bc_weight(0.5, 0.0, 0.5) - 0.5625).abs() < 1e-6);
        assert!((cubic_bc_weight(1.5, 0.0, 0.5) + 0.0625).abs() < 1e-6);
    }

    #[test]
    fn mitchell_weights_are_pinned() {
        let b = 1.0 / 3.0;
        let c = 1.0 / 3.0;
        // Mitchell is smoothing, not interpolating: w(0) = 8/9.
        assert!((cubic_bc_weight(0.0, b, c) - 8.0 / 9.0).abs() < 1e-6);
        assert!(cubic_bc_weight(2.0, b, c).abs() < 1e-6);
        // Partition of unity across a unit phase: taps at -1,0,1 sum to 1.
        let sum =
            cubic_bc_weight(-1.0, b, c) + cubic_bc_weight(0.0, b, c) + cubic_bc_weight(1.0, b, c);
        assert!((sum - 1.0).abs() < 1e-6);
    }

    #[test]
    fn lanczos_weights_are_pinned() {
        // Interpolating at integer taps, zero at/after the radius.
        assert!((lanczos_weight(0.0, 3.0) - 1.0).abs() < 1e-6);
        assert!(lanczos_weight(1.0, 3.0).abs() < 1e-6);
        assert!(lanczos_weight(2.0, 3.0).abs() < 1e-6);
        assert_eq!(lanczos_weight(3.0, 3.0), 0.0);
        assert_eq!(lanczos_weight(2.0, 2.0), 0.0);
        // sinc(0.5)=2/pi, sinc(0.5/3)=sinc(1/6): product is the Lanczos3 w(0.5).
        let expected = (2.0 / std::f32::consts::PI) * sinc(0.5 / 3.0);
        assert!((lanczos_weight(0.5, 3.0) - expected).abs() < 1e-6);
    }
}
