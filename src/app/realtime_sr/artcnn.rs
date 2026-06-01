use super::RealtimeSrOutput;
use crate::core::artcnn_c4f16::{
    exact_output_size, ArtcnnC4F16, ArtcnnC4F16RenderOptions, ArtcnnC4F16Workspace,
};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use crossbeam_channel::{bounded, Receiver, TryRecvError};
use std::thread;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::{Duration, Instant};

const TRANSIENT_BYTES_LIMIT: u64 = 256 * 1024 * 1024;
const WORKSPACE_CACHE_BYTES_LIMIT: u64 = 192 * 1024 * 1024;
const WORKSPACE_CACHE_SLOTS_LIMIT: usize = 4;

pub(super) struct ArtcnnRenderer {
    state: ArtcnnRendererState,
}

enum ArtcnnRendererState {
    Pending,
    Loading(Receiver<LoadedArtcnnRenderer>),
    Ready(Box<LoadedArtcnnRenderer>),
    Disabled,
}

struct LoadedArtcnnRenderer {
    core: ArtcnnC4F16,
    workspaces: Vec<ArtcnnWorkspaceSlot>,
    workspace_bytes: u64,
}

struct ArtcnnWorkspaceSlot {
    workspace: ArtcnnC4F16Workspace,
}

impl ArtcnnRenderer {
    pub(super) fn new() -> Self {
        Self {
            state: ArtcnnRendererState::Pending,
        }
    }

    pub(super) fn render(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        source_size: [usize; 2],
    ) -> Option<RealtimeSrOutput> {
        self.start_loading_if_needed(device);
        self.finish_loading_if_ready();

        let ArtcnnRendererState::Ready(renderer) = &mut self.state else {
            return None;
        };
        renderer.render(device, encoder, source_view, source_size)
    }

    pub(super) fn is_loading(&self) -> bool {
        matches!(self.state, ArtcnnRendererState::Loading(_))
    }

    pub(super) fn warm_up(&mut self, device: &wgpu::Device) {
        self.start_loading_if_needed(device);
        self.finish_loading_if_ready();
    }

    fn start_loading_if_needed(&mut self, device: &wgpu::Device) {
        if !matches!(self.state, ArtcnnRendererState::Pending) {
            return;
        }
        self.state = LoadedArtcnnRenderer::spawn_loader(device.clone())
            .map(ArtcnnRendererState::Loading)
            .unwrap_or(ArtcnnRendererState::Disabled);
    }

    fn finish_loading_if_ready(&mut self) {
        let ArtcnnRendererState::Loading(receiver) = &self.state else {
            return;
        };
        let next_state = match receiver.try_recv() {
            Ok(renderer) => ArtcnnRendererState::Ready(Box::new(renderer)),
            Err(TryRecvError::Disconnected) => ArtcnnRendererState::Disabled,
            Err(TryRecvError::Empty) => return,
        };
        self.state = next_state;
    }
}

impl LoadedArtcnnRenderer {
    fn spawn_loader(device: wgpu::Device) -> Option<Receiver<Self>> {
        let (sender, receiver) = bounded(1);
        let _loader = thread::Builder::new()
            .name("suisuiview-artcnn-display-loader".to_owned())
            .spawn(move || {
                let _ = sender.send(Self::new(&device));
            })
            .ok()?;
        Some(receiver)
    }

    fn new(device: &wgpu::Device) -> Self {
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let load_started = Instant::now();
        let core = ArtcnnC4F16::new(device);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf_trace::record_duration_if_at_least(
            "artcnn_display_load",
            load_started.elapsed(),
            Duration::from_millis(16),
            &[
                PerfField::Str("method", "artcnn_c4f16"),
                PerfField::Usize("pipelines", 8),
            ],
        );
        Self {
            core,
            workspaces: Vec::new(),
            workspace_bytes: 0,
        }
    }

    fn render(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        source_size: [usize; 2],
    ) -> Option<RealtimeSrOutput> {
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let render_started = Instant::now();
        let output_size = exact_output_size(source_size).ok()?;
        let options = ArtcnnC4F16RenderOptions {
            output_size,
            output_usage: wgpu::TextureUsages::TEXTURE_BINDING,
            transient_limit: TRANSIENT_BYTES_LIMIT,
            readback_padded_bytes_per_row: None,
        };
        #[cfg_attr(
            not(any(feature = "perf-dev", feature = "perf-diagnostics")),
            allow(unused_variables)
        )]
        let (slot, reused_workspace) = match self.take_workspace(source_size) {
            Some(slot) => (slot, true),
            None => (
                self.create_workspace_slot(device, source_size, &options)?,
                false,
            ),
        };
        let should_cache_workspace = slot.workspace.byte_size <= WORKSPACE_CACHE_BYTES_LIMIT;
        let output = self
            .core
            .render_to_texture_with_workspace(
                device,
                encoder,
                source_view,
                &slot.workspace,
                options,
            )
            .ok();
        if should_cache_workspace {
            self.store_workspace(slot);
        }
        let output = output?;
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_artcnn_display_encode(
            render_started.elapsed(),
            source_size,
            output_size,
            reused_workspace,
            should_cache_workspace,
            self.workspaces.len(),
            self.workspace_bytes,
        );
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        Some(RealtimeSrOutput {
            texture: output.texture,
            view,
            size: output.size,
            byte_size: output.size[0]
                .saturating_mul(output.size[1])
                .saturating_mul(4),
        })
    }

    fn create_workspace_slot(
        &self,
        device: &wgpu::Device,
        source_size: [usize; 2],
        options: &ArtcnnC4F16RenderOptions,
    ) -> Option<ArtcnnWorkspaceSlot> {
        self.core
            .create_workspace(device, source_size, options)
            .ok()
            .map(|workspace| ArtcnnWorkspaceSlot { workspace })
    }

    fn take_workspace(&mut self, source_size: [usize; 2]) -> Option<ArtcnnWorkspaceSlot> {
        let index = self
            .workspaces
            .iter()
            .position(|slot| slot.workspace.source_size == source_size)?;
        let slot = self.workspaces.remove(index);
        self.workspace_bytes = self
            .workspace_bytes
            .saturating_sub(slot.workspace.byte_size);
        Some(slot)
    }

    fn store_workspace(&mut self, slot: ArtcnnWorkspaceSlot) {
        let byte_size = slot.workspace.byte_size;
        while self.workspaces.len() >= WORKSPACE_CACHE_SLOTS_LIMIT
            || self
                .workspace_bytes
                .checked_add(byte_size)
                .is_none_or(|bytes| bytes > WORKSPACE_CACHE_BYTES_LIMIT)
        {
            let evicted = self.workspaces.remove(0);
            self.workspace_bytes = self
                .workspace_bytes
                .saturating_sub(evicted.workspace.byte_size);
        }
        self.workspace_bytes = self.workspace_bytes.saturating_add(byte_size);
        self.workspaces.push(slot);
    }
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_artcnn_display_encode(
    duration: Duration,
    source_size: [usize; 2],
    output_size: [usize; 2],
    reused_workspace: bool,
    cached_workspace: bool,
    workspace_slots: usize,
    workspace_bytes: u64,
) {
    perf_trace::record_duration(
        "artcnn_display_encode",
        duration,
        &[
            PerfField::Str("method", "artcnn_c4f16"),
            PerfField::Usize("source_width", source_size[0]),
            PerfField::Usize("source_height", source_size[1]),
            PerfField::Usize("output_width", output_size[0]),
            PerfField::Usize("output_height", output_size[1]),
            PerfField::Bool("reused_workspace", reused_workspace),
            PerfField::Bool("cached_workspace", cached_workspace),
            PerfField::Usize("workspace_slots", workspace_slots),
            PerfField::Usize(
                "workspace_cache_bytes",
                usize::try_from(workspace_bytes).unwrap_or(usize::MAX),
            ),
        ],
    );
}
