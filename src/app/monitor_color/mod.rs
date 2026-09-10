//! Optional final-scene SDR color management for the WGPU host.
//! No worker, LUT, or offscreen scene is created while the setting is disabled.
use egui_wgpu::winit::Painter;
use egui_winit::winit;

mod gpu;
mod loader;
mod scene;
pub(crate) const SCENE_FORMAT: wgpu::TextureFormat = scene::FORMAT;
#[cfg(test)]
mod tests;
#[cfg(target_os = "windows")]
mod windows;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum MonitorColorStatus {
    #[default]
    Off,
    Loading,
    Managed {
        profile: String,
    },
    SystemSrgb,
    Fallback {
        reason: String,
    },
}

impl MonitorColorStatus {
    pub(crate) fn label(&self, i18n: crate::core::i18n::I18n) -> String {
        let korean = i18n.language() == crate::core::i18n::ResolvedLanguage::KoKr;
        let text = |ko: &str, en: &str| if korean { ko.to_owned() } else { en.to_owned() };
        match self {
            Self::Off => text("모니터 색 관리 꺼짐", "Monitor color management off"),
            Self::Loading => text("모니터 프로필 준비 중", "Preparing monitor profile"),
            Self::Managed { profile } => {
                format!("{}: {profile}", text("적용 프로필", "Display profile"))
            }
            Self::SystemSrgb => text("Windows 기본 sRGB 출력", "Windows default sRGB output"),
            Self::Fallback { .. } => text(
                "프로필 적용 불가 · sRGB 출력",
                "Profile unavailable · sRGB output",
            ),
        }
    }

    pub(crate) fn detail(&self) -> Option<&str> {
        match self {
            Self::Fallback { reason } => Some(reason),
            _ => None,
        }
    }
}

pub(crate) fn status(ctx: &egui::Context) -> MonitorColorStatus {
    ctx.data(|data| {
        data.get_temp::<MonitorColorStatus>(status_id())
            .unwrap_or_default()
    })
}

fn status_id() -> egui::Id {
    egui::Id::new("monitor-color-output-status")
}

fn set_status(ctx: &egui::Context, value: MonitorColorStatus) {
    if status(ctx) != value {
        ctx.data_mut(|data| data.insert_temp(status_id(), value));
        ctx.request_repaint();
    }
}

#[derive(Default)]
pub(crate) struct MonitorColor {
    loader: Option<loader::ProfileLoader>,
    display: Option<String>,
    ready: Option<loader::LoadedProfile>,
    scene: Option<gpu::SceneTexture>,
    start_failed: bool,
}

impl MonitorColor {
    pub(crate) fn disable(ctx: &egui::Context) {
        set_status(ctx, MonitorColorStatus::Off);
    }

    fn update_profile(
        &mut self,
        ctx: &egui::Context,
        window: &winit::window::Window,
        painter: &Painter,
    ) {
        let Some(state) = painter.render_state() else {
            return;
        };
        if self.loader.is_none() && !self.start_failed {
            match loader::ProfileLoader::start(ctx.clone(), state) {
                Ok(loader) => self.loader = Some(loader),
                Err(reason) => {
                    self.start_failed = true;
                    set_status(ctx, MonitorColorStatus::Fallback { reason });
                }
            }
        }
        let display = window.current_monitor().and_then(|monitor| monitor.name());
        if display != self.display {
            self.display = display;
            self.ready = None;
            self.scene = None;
            if let (Some(display), Some(loader)) = (&self.display, &self.loader) {
                if loader.requests.send(display.clone()).is_ok() {
                    set_status(ctx, MonitorColorStatus::Loading);
                }
            }
        }
        if self.display.is_none() {
            self.ready = None;
            set_status(
                ctx,
                MonitorColorStatus::Fallback {
                    reason: "No active display".to_owned(),
                },
            );
        }
        loop {
            let Some(loader) = &self.loader else {
                break;
            };
            match loader.results.try_recv() {
                Ok(loaded) => self.accept_loaded(ctx, loaded),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.loader = None;
                    self.ready = None;
                    self.scene = None;
                    self.start_failed = true;
                    set_status(
                        ctx,
                        MonitorColorStatus::Fallback {
                            reason: "The display profile worker stopped".to_owned(),
                        },
                    );
                    break;
                }
            }
        }
    }

    fn accept_loaded(&mut self, ctx: &egui::Context, loaded: loader::LoadedProfile) {
        if Some(&loaded.display) == self.display.as_ref() {
            set_status(ctx, loaded.status.clone());
            self.ready = Some(loaded);
        }
    }

    /// Choose the attachment before the app creates format-specific callbacks.
    /// UI setting changes take effect at the next frame boundary.
    pub(crate) fn prepare(
        &mut self,
        ctx: &egui::Context,
        window: &winit::window::Window,
        painter: &Painter,
    ) -> bool {
        self.update_profile(ctx, window, painter);
        let size = window.inner_size();
        let size = [size.width, size.height];
        let Some(ready) = &self.ready else {
            return false;
        };
        let Some(lut) = &ready.lut else {
            self.scene = None;
            return false;
        };
        let Some(state) = painter.render_state() else {
            return false;
        };
        if ready
            .renderer
            .as_ref()
            .is_none_or(|renderer| renderer.is_poisoned())
        {
            self.scene = None;
            set_status(
                ctx,
                MonitorColorStatus::Fallback {
                    reason: "The display composition renderer is unavailable".to_owned(),
                },
            );
            return false;
        }
        if size.contains(&0) {
            return false;
        }
        if size
            .iter()
            .any(|value| *value > state.device.limits().max_texture_dimension_2d)
        {
            self.scene = None;
            set_status(
                ctx,
                MonitorColorStatus::Fallback {
                    reason: "The window exceeds the GPU texture limit".to_owned(),
                },
            );
            return false;
        }
        let scene_bytes = u64::from(size[0])
            * u64::from(size[1])
            * u64::from(SCENE_FORMAT.block_copy_size(None).unwrap_or(8));
        if scene_bytes > 128 * 1024 * 1024 {
            self.scene = None;
            set_status(
                ctx,
                MonitorColorStatus::Fallback {
                    reason: "Display color correction exceeds the 128 MiB scene budget".to_owned(),
                },
            );
            return false;
        }
        set_status(ctx, ready.status.clone());
        if self
            .scene
            .as_ref()
            .is_none_or(|scene| scene.size != size || scene.fingerprint != ready.fingerprint)
        {
            self.scene = Some(gpu::SceneTexture::new(
                &state.device,
                size,
                ready.fingerprint,
                lut,
            ));
        }
        true
    }

    /// Called only after `prepare` selected an FP16 frame. Textures and callback
    /// resources stay owned by the normal renderer so fallback never reuploads.
    pub(crate) fn paint(
        &mut self,
        painter: &mut Painter,
        pixels_per_point: f32,
        clear: [f32; 4],
        primitives: &mut [egui::ClippedPrimitive],
        delta: &egui::TexturesDelta,
    ) {
        let ready = self.ready.as_ref().unwrap();
        let lut = ready.lut.as_ref().unwrap();
        let state = painter.render_state().unwrap();
        let scene = self.scene.as_ref().unwrap();
        let size = scene.size;
        let mut composition = ready.renderer.as_ref().unwrap().lock().unwrap();
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size,
            pixels_per_point,
        };
        let mut encoder = state
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("display-color-compose"),
            });
        let before = {
            let mut surface_renderer = state.renderer.write();
            for (id, image) in &delta.set {
                surface_renderer.update_texture(&state.device, &state.queue, *id, image);
            }
            composition.borrow_textures(&state.device, &surface_renderer, primitives);
            std::mem::swap(
                &mut surface_renderer.callback_resources,
                &mut composition.renderer.callback_resources,
            );
            composition.renderer.update_buffers(
                &state.device,
                &state.queue,
                &mut encoder,
                primitives,
                &screen,
            )
        };
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("display-color-compose"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &scene.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear[0] as f64,
                            g: clear[1] as f64,
                            b: clear[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            composition
                .renderer
                .render(&mut pass.forget_lifetime(), primitives, &screen);
        }
        std::mem::swap(
            &mut state.renderer.write().callback_resources,
            &mut composition.renderer.callback_resources,
        );
        drop(composition);
        // Composition and output execute in order on the same device/queue.
        state
            .queue
            .submit(before.into_iter().chain([encoder.finish()]));
        let rect = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(size[0] as f32, size[1] as f32) / pixels_per_point,
        );
        let callback = egui_wgpu::Callback::new_paint_callback(
            rect,
            gpu::OutputCallback {
                lut: lut.clone(),
                binding: scene.binding.clone(),
            },
        );
        painter.paint_and_update_textures(
            egui::ViewportId::ROOT,
            pixels_per_point,
            clear,
            &[egui::ClippedPrimitive {
                clip_rect: rect,
                primitive: egui::epaint::Primitive::Callback(callback),
            }],
            &egui::TexturesDelta {
                set: Vec::new(),
                free: delta.free.clone(),
            },
            Vec::new(),
        );
    }
}
