
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupReveal {
    HostManaged,
    AfterFirstFrame,
}

pub(crate) struct AppRuntime {
    egui_ctx: egui::Context,
    startup_reveal: StartupReveal,
}

impl AppRuntime {
    pub(crate) fn new(egui_ctx: egui::Context, startup_reveal: StartupReveal) -> Self {
        Self {
            egui_ctx,
            startup_reveal,
        }
    }

    pub(crate) fn egui_ctx(&self) -> &egui::Context {
        &self.egui_ctx
    }

    pub(crate) fn startup_reveal(&self) -> StartupReveal {
        self.startup_reveal
    }
}
