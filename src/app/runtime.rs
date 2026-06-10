use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScreenRenderer {
    Glow,
    Wgpu { target_format: wgpu::TextureFormat },
}

impl ScreenRenderer {
    pub(crate) fn wgpu_target_format(self) -> Option<wgpu::TextureFormat> {
        match self {
            Self::Glow => None,
            Self::Wgpu { target_format } => Some(target_format),
        }
    }

    pub(crate) fn supports_wgsl_paint(self) -> bool {
        self.wgpu_target_format().is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupReveal {
    HostManaged,
    AfterFirstFrame,
}

impl StartupReveal {
    pub(crate) fn for_eframe_host() -> Self {
        Self::HostManaged
    }
}

pub(crate) struct AppRuntime {
    egui_ctx: egui::Context,
    screen_renderer: ScreenRenderer,
    startup_reveal: StartupReveal,
}

impl AppRuntime {
    pub(crate) fn new(
        egui_ctx: egui::Context,
        screen_renderer: ScreenRenderer,
        startup_reveal: StartupReveal,
    ) -> Self {
        Self {
            egui_ctx,
            screen_renderer,
            startup_reveal,
        }
    }

    pub(crate) fn egui_ctx(&self) -> &egui::Context {
        &self.egui_ctx
    }

    pub(crate) fn screen_renderer(&self) -> ScreenRenderer {
        self.screen_renderer
    }

    pub(crate) fn startup_reveal(&self) -> StartupReveal {
        self.startup_reveal
    }
}

#[cfg(test)]
mod tests {
    use super::StartupReveal;

    #[test]
    fn eframe_host_reveal_policy_matches_platform() {
        let policy = StartupReveal::for_eframe_host();

        assert_eq!(policy, StartupReveal::HostManaged);
    }
}
