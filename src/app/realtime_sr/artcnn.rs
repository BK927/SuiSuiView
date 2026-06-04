use super::RealtimeSrOutput;
use crate::core::artcnn::{
    exact_output_size, Artcnn, ArtcnnRenderOptions, ArtcnnVariant, ArtcnnWorkspace,
};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use crate::core::state::WgpuUpscaleMethod;
use crossbeam_channel::{bounded, Receiver, TryRecvError};
use std::thread;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::{Duration, Instant};

const TRANSIENT_BYTES_LIMIT: u64 = 256 * 1024 * 1024;
const WORKSPACE_CACHE_BYTES_LIMIT: u64 = TRANSIENT_BYTES_LIMIT;
const WORKSPACE_CACHE_SLOTS_LIMIT: usize = 4;

pub(super) struct ArtcnnRenderer {
    state: ArtcnnRendererState,
}

enum ArtcnnRendererState {
    Pending,
    Loading {
        variant: ArtcnnVariant,
        receiver: Receiver<LoadedArtcnnRenderer>,
    },
    Ready(Box<LoadedArtcnnRenderer>),
    Disabled,
}

struct LoadedArtcnnRenderer {
    variant: ArtcnnVariant,
    core: Artcnn,
    workspaces: Vec<ArtcnnWorkspaceSlot>,
    workspace_bytes: u64,
}

struct ArtcnnWorkspaceSlot {
    workspace: ArtcnnWorkspace,
}

impl ArtcnnRenderer {
    pub(super) fn new() -> Self {
        Self {
            state: ArtcnnRendererState::Pending,
        }
    }

    pub(super) fn render(
        &mut self,
        method: WgpuUpscaleMethod,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        source_size: [usize; 2],
    ) -> Option<RealtimeSrOutput> {
        let variant = artcnn_variant_for_method(method)?;
        self.start_loading_if_needed(device, variant);
        self.finish_loading_if_ready();

        let ArtcnnRendererState::Ready(renderer) = &mut self.state else {
            return None;
        };
        renderer.render(device, encoder, source_view, source_size)
    }

    pub(super) fn is_loading(&self) -> bool {
        matches!(self.state, ArtcnnRendererState::Loading { .. })
    }

    pub(super) fn warm_up(&mut self, method: WgpuUpscaleMethod, device: &wgpu::Device) {
        let Some(variant) = artcnn_variant_for_method(method) else {
            return;
        };
        self.start_loading_if_needed(device, variant);
        self.finish_loading_if_ready();
    }

    fn start_loading_if_needed(&mut self, device: &wgpu::Device, variant: ArtcnnVariant) {
        if self.loaded_variant() != Some(variant) {
            self.state = ArtcnnRendererState::Pending;
        }
        if !matches!(self.state, ArtcnnRendererState::Pending) {
            return;
        }
        self.state = LoadedArtcnnRenderer::spawn_loader(device.clone(), variant)
            .map(|receiver| ArtcnnRendererState::Loading { variant, receiver })
            .unwrap_or(ArtcnnRendererState::Disabled);
    }

    fn loaded_variant(&self) -> Option<ArtcnnVariant> {
        match &self.state {
            ArtcnnRendererState::Loading { variant, .. } => Some(*variant),
            ArtcnnRendererState::Ready(renderer) => Some(renderer.variant),
            _ => None,
        }
    }

    fn finish_loading_if_ready(&mut self) {
        let ArtcnnRendererState::Loading { receiver, .. } = &self.state else {
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
    fn spawn_loader(device: wgpu::Device, variant: ArtcnnVariant) -> Option<Receiver<Self>> {
        let (sender, receiver) = bounded(1);
        let _loader = thread::Builder::new()
            .name("suisuiview-artcnn-display-loader".to_owned())
            .spawn(move || {
                let _ = sender.send(Self::new(&device, variant));
            })
            .ok()?;
        Some(receiver)
    }

    fn new(device: &wgpu::Device, variant: ArtcnnVariant) -> Self {
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let load_started = Instant::now();
        let core = Artcnn::new(device, variant);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf_trace::record_duration_if_at_least(
            "artcnn_display_load",
            load_started.elapsed(),
            Duration::from_millis(16),
            &[
                PerfField::Str("method", variant.token()),
                PerfField::Usize("pipelines", 8),
            ],
        );
        Self {
            variant,
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
        let output_size = match exact_output_size(self.variant, source_size) {
            Ok(output_size) => output_size,
            Err(error) => {
                record_artcnn_display_skip(
                    self.variant.token(),
                    artcnn_skip_reason(&error),
                    source_size,
                    [0, 0],
                    false,
                    self.workspaces.len(),
                    self.workspace_bytes,
                );
                return None;
            }
        };
        let options = ArtcnnRenderOptions {
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
            None => match self.create_workspace_slot(device, source_size, &options) {
                Ok(slot) => (slot, false),
                Err(error) => {
                    record_artcnn_display_skip(
                        self.variant.token(),
                        artcnn_skip_reason(&error),
                        source_size,
                        output_size,
                        false,
                        self.workspaces.len(),
                        self.workspace_bytes,
                    );
                    return None;
                }
            },
        };
        let workspace_byte_size = slot.workspace.byte_size;
        let should_cache_workspace = workspace_byte_size <= WORKSPACE_CACHE_BYTES_LIMIT;
        let output = self.core.render_to_texture_with_workspace(
            device,
            encoder,
            source_view,
            &slot.workspace,
            options,
        );
        if should_cache_workspace {
            self.store_workspace(slot);
        }
        let output = match output {
            Ok(output) => output,
            Err(error) => {
                record_artcnn_display_skip(
                    self.variant.token(),
                    artcnn_skip_reason(&error),
                    source_size,
                    output_size,
                    reused_workspace,
                    self.workspaces.len(),
                    self.workspace_bytes,
                );
                return None;
            }
        };
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_artcnn_display_encode(
            self.variant.token(),
            render_started.elapsed(),
            source_size,
            output_size,
            workspace_byte_size,
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
        options: &ArtcnnRenderOptions,
    ) -> Result<ArtcnnWorkspaceSlot, String> {
        self.core
            .create_workspace(device, source_size, options)
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

fn artcnn_skip_reason(error: &str) -> &'static str {
    if error.contains("non-empty source") {
        "empty_source"
    } else if error.contains("output width overflowed")
        || error.contains("output height overflowed")
        || error.contains("pixel count overflowed")
        || error.contains("byte size overflowed")
        || error.contains("size overflowed")
    {
        "size_overflow"
    } else if error.contains("exceeds adapter 2D texture limit") {
        "texture_limit"
    } else if error.contains("transient resources") {
        "transient_limit"
    } else if error.contains("workspace output shape mismatch") {
        "workspace_mismatch"
    } else {
        "render_error"
    }
}

pub(super) fn artcnn_variant_for_method(method: WgpuUpscaleMethod) -> Option<ArtcnnVariant> {
    match method {
        WgpuUpscaleMethod::WgslArtcnnC4F16 => Some(ArtcnnVariant::C4F16),
        WgpuUpscaleMethod::WgslArtcnnC4F16Dn => Some(ArtcnnVariant::C4F16Dn),
        WgpuUpscaleMethod::WgslArtcnnC4F16Ds => Some(ArtcnnVariant::C4F16Ds),
        WgpuUpscaleMethod::WgslArtcnnC4F32 => Some(ArtcnnVariant::C4F32),
        WgpuUpscaleMethod::WgslArtcnnC4F32Dn => Some(ArtcnnVariant::C4F32Dn),
        WgpuUpscaleMethod::WgslArtcnnC4F32Ds => Some(ArtcnnVariant::C4F32Ds),
        _ => None,
    }
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_artcnn_display_encode(
    method_token: &'static str,
    duration: Duration,
    source_size: [usize; 2],
    output_size: [usize; 2],
    workspace_byte_size: u64,
    reused_workspace: bool,
    cached_workspace: bool,
    workspace_slots: usize,
    workspace_bytes: u64,
) {
    perf_trace::record_duration(
        "artcnn_display_encode",
        duration,
        &[
            PerfField::Str("method", method_token),
            PerfField::Usize("source_width", source_size[0]),
            PerfField::Usize("source_height", source_size[1]),
            PerfField::Usize("output_width", output_size[0]),
            PerfField::Usize("output_height", output_size[1]),
            PerfField::Usize(
                "workspace_bytes",
                usize::try_from(workspace_byte_size).unwrap_or(usize::MAX),
            ),
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

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_artcnn_display_skip(
    method_token: &'static str,
    reason: &'static str,
    source_size: [usize; 2],
    output_size: [usize; 2],
    reused_workspace: bool,
    workspace_slots: usize,
    workspace_bytes: u64,
) {
    perf_trace::record_duration(
        "artcnn_display_skip",
        Duration::ZERO,
        &[
            PerfField::Str("method", method_token),
            PerfField::Str("reason", reason),
            PerfField::Usize("source_width", source_size[0]),
            PerfField::Usize("source_height", source_size[1]),
            PerfField::Usize("output_width", output_size[0]),
            PerfField::Usize("output_height", output_size[1]),
            PerfField::Bool("reused_workspace", reused_workspace),
            PerfField::Usize("workspace_slots", workspace_slots),
            PerfField::Usize(
                "workspace_cache_bytes",
                usize::try_from(workspace_bytes).unwrap_or(usize::MAX),
            ),
        ],
    );
}

#[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
fn record_artcnn_display_skip(
    _method_token: &'static str,
    _reason: &'static str,
    _source_size: [usize; 2],
    _output_size: [usize; 2],
    _reused_workspace: bool,
    _workspace_slots: usize,
    _workspace_bytes: u64,
) {
}

#[cfg(test)]
mod tests {
    use super::artcnn_skip_reason;

    #[test]
    fn artcnn_skip_reason_keeps_perf_labels_stable() {
        assert_eq!(
            artcnn_skip_reason("ArtCNN C4F16 requires a non-empty source image"),
            "empty_source"
        );
        assert_eq!(
            artcnn_skip_reason("ArtCNN C4F16 output width overflowed"),
            "size_overflow"
        );
        assert_eq!(
            artcnn_skip_reason(
                "ArtCNN C4F16 feature texture 99999x1 exceeds adapter 2D texture limit 8192"
            ),
            "texture_limit"
        );
        assert_eq!(
            artcnn_skip_reason("ArtCNN C4F16 transient resources would use about 300 MiB"),
            "transient_limit"
        );
        assert_eq!(
            artcnn_skip_reason("ArtCNN C4F16 workspace output shape mismatch"),
            "workspace_mismatch"
        );
        assert_eq!(
            artcnn_skip_reason("other validation failure"),
            "render_error"
        );
    }
}
