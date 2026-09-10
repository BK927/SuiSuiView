//! Optional manual-SR refinement on the renderer's device and queue. UI calls
//! never poll the device, wait for a channel, or join a thread.
use super::{span::BackgroundSpanJob, RealtimeSrOutput};
use crate::core::state::WgpuUpscaleMethod;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering},
    Arc, Mutex, Weak,
};
use std::time::{Duration, Instant};

#[cfg(test)]
mod gpu_tests;
mod graph;
mod programs;
mod worker;
use worker::run_worker;

pub(in crate::app) const INPUT_QUIET: Duration = Duration::from_millis(150);
pub(in crate::app) const MAX_REQUESTS: usize = 8;

#[derive(Clone)]
pub(in crate::app) struct RefineRequest {
    /// Content-complete result identity, including selected manual model,
    /// dimensions, debanding settings and stack count. Never just a page index.
    pub key: u64,
    /// Stable identity of this input allocation/content for byte deduplication.
    pub source_key: u64,
    pub method: WgpuUpscaleMethod,
    pub _source_texture: wgpu::Texture,
    pub source_view: wgpu::TextureView,
    pub source_size: [usize; 2],
    pub source_byte_size: usize,
    pub stack_passes: usize,
    /// Must become true only after any source-producing render submission has
    /// completed. This is especially important for a new deband intermediate.
    pub source_ready: Arc<AtomicBool>,
}

pub(in crate::app) struct RefineSnapshot {
    pub generation: u64,
    /// Full active set (both spread pages and both comparison panes), not just
    /// whichever method was painted last. Maximum eight unique requests.
    pub requests: Vec<RefineRequest>,
    pub quiet_until: Instant,
    /// Shared allowance for pinned inputs, complete outputs and the one active
    /// full feature graph. The caller excludes unrelated foreground allocations.
    pub budget_bytes: usize,
}

pub(in crate::app) enum RefineEvent {
    Ready {
        generation: u64,
        key: u64,
        output: Arc<RealtimeSrOutput>,
    },
    Skipped {
        generation: u64,
        key: u64,
        reason: &'static str,
    },
}

struct Shared {
    snapshot: Mutex<Option<RefineSnapshot>>,
    generation: AtomicU64,
    quiet_micros: AtomicU64,
    stopped: AtomicBool,
    failure: AtomicU8,
    unclaimed_outputs: AtomicUsize,
    epoch: Instant,
}

pub(in crate::app) struct BackgroundRefiner {
    shared: Arc<Shared>,
    wake: Sender<()>,
    events: Receiver<RefineEvent>,
}

impl BackgroundRefiner {
    pub(in crate::app) fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        ctx: egui::Context,
    ) -> Self {
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(None),
            generation: AtomicU64::new(0),
            quiet_micros: AtomicU64::new(0),
            stopped: AtomicBool::new(false),
            failure: AtomicU8::new(0),
            unclaimed_outputs: AtomicUsize::new(0),
            epoch: Instant::now(),
        });
        let (wake, wakes) = bounded(1);
        let (events_tx, events) = bounded(MAX_REQUESTS);
        let worker_shared = shared.clone();
        let worker_ctx = ctx.clone();
        if std::thread::Builder::new()
            .name("suisuiview-background-refine".into())
            .spawn(move || {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_worker(
                        device,
                        queue,
                        worker_ctx.clone(),
                        worker_shared.clone(),
                        wakes,
                        events_tx,
                    )
                }))
                .is_err()
                {
                    worker_shared.failure.store(2, Ordering::Release);
                    worker_ctx.request_repaint();
                }
            })
            .is_err()
        {
            shared.failure.store(1, Ordering::Release);
            ctx.request_repaint();
        }
        Self {
            shared,
            wake,
            events,
        }
    }

    /// Retry on a later repaint if false; no unbounded command queue is built.
    pub(in crate::app) fn update(&self, snapshot: RefineSnapshot) -> bool {
        if self.failure_reason().is_some() {
            return true;
        }
        self.shared
            .generation
            .store(snapshot.generation, Ordering::Release);
        self.shared.quiet_micros.store(
            snapshot
                .quiet_until
                .saturating_duration_since(self.shared.epoch)
                .as_micros()
                .min(u64::MAX as u128) as u64,
            Ordering::Release,
        );
        if snapshot.requests.len() > MAX_REQUESTS {
            return false;
        }
        let Ok(mut pending) = self.shared.snapshot.try_lock() else {
            return false;
        };
        *pending = Some(snapshot);
        drop(pending);
        let _ = self.wake.try_send(());
        true
    }

    pub(in crate::app) fn drain(&self) -> Vec<RefineEvent> {
        self.events
            .try_iter()
            .inspect(|event| {
                if matches!(event, RefineEvent::Ready { .. }) {
                    self.shared.unclaimed_outputs.fetch_sub(1, Ordering::AcqRel);
                }
            })
            .collect()
    }

    pub(in crate::app) fn failure_reason(&self) -> Option<&'static str> {
        match self.shared.failure.load(Ordering::Acquire) {
            0 => None,
            1 => Some("worker_start_failed"),
            2 => Some("gpu_prepare_failed"),
            _ => Some("device_poll_failed"),
        }
    }

    pub(in crate::app) fn supports(method: WgpuUpscaleMethod) -> bool {
        programs::supported(method)
    }
}

impl Drop for BackgroundRefiner {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        let _ = self.wake.try_send(());
        // Detached worker retains device/resource ownership until its final
        // outstanding submission completes. UI shutdown never joins it.
    }
}

enum Graph {
    Texture(graph::TextureGraph),
    Span(Box<BackgroundSpanJob>),
}
impl Graph {
    fn encode_next(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
    ) -> bool {
        match self {
            Self::Texture(g) => g.encode_next(queue, encoder),
            Self::Span(g) => g.encode_next(device, encoder),
        }
    }
    fn output(&self) -> Arc<RealtimeSrOutput> {
        match self {
            Self::Texture(g) => g.output.clone(),
            Self::Span(g) => g.output.clone(),
        }
    }
    fn bytes(&self) -> usize {
        match self {
            Self::Texture(g) => g.bytes,
            Self::Span(g) => g.bytes,
        }
    }
}

struct Active {
    request: RefineRequest,
    graph: Graph,
    stage: usize,
    previous_output: Option<Arc<RealtimeSrOutput>>,
}

fn output_bytes(request: &RefineRequest) -> Option<usize> {
    if !(1..=2).contains(&request.stack_passes) || request.source_size.contains(&0) {
        return None;
    }
    request.source_size[0]
        .checked_mul(request.source_size[1])?
        .checked_mul(4usize.checked_pow(request.stack_passes as u32)?)?
        .checked_mul(4)
}

/// Reserve all active final outputs before starting any member of a spread.
/// That prevents a completed first page from starving the second page forever.
fn graph_allowance(snapshot: &RefineSnapshot, request: &RefineRequest) -> Option<usize> {
    reserved_allowance(
        snapshot.budget_bytes,
        request.key,
        snapshot
            .requests
            .iter()
            .map(|item| {
                Some((
                    item.key,
                    item.source_key,
                    item.source_byte_size,
                    output_bytes(item)?,
                ))
            })
            .collect::<Option<Vec<_>>>()?,
    )
}

fn reserved_allowance(
    budget: usize,
    key: u64,
    footprints: Vec<(u64, u64, usize, usize)>,
) -> Option<usize> {
    let mut inputs = HashSet::new();
    let mut reserved = 0usize;
    for (output_key, input_key, input_bytes, output_bytes) in footprints {
        if inputs.insert(input_key) {
            reserved = reserved.checked_add(input_bytes)?;
        }
        if output_key != key {
            reserved = reserved.checked_add(output_bytes)?;
        }
    }
    budget.checked_sub(reserved)
}

fn can_start(shared: &Shared, generation: u64, now: Instant) -> bool {
    !shared.stopped.load(Ordering::Acquire)
        && shared.generation.load(Ordering::Acquire) == generation
        && now.saturating_duration_since(shared.epoch).as_micros()
            >= shared.quiet_micros.load(Ordering::Acquire) as u128
}

fn make_graph(
    device: &wgpu::Device,
    request: &RefineRequest,
    source: &wgpu::TextureView,
    size: [usize; 2],
    allowance: usize,
    programs: &mut HashMap<WgpuUpscaleMethod, programs::Program>,
) -> Result<Graph, &'static str> {
    if !programs::supported(request.method) {
        return Err("unsupported_method");
    }
    if size.contains(&0)
        || size
            .iter()
            .any(|&value| value > device.limits().max_texture_dimension_2d as usize / 2)
    {
        return Err("texture_limit");
    }
    if request.method == WgpuUpscaleMethod::WgslSrLabSpanX2 {
        return BackgroundSpanJob::new(device, source, size, allowance)
            .map(|g| Graph::Span(Box::new(g)));
    }
    let bytes = programs::graph_bytes(request.method, size).ok_or("size_overflow")?;
    if bytes > allowance {
        return Err("memory_budget");
    }
    if let std::collections::hash_map::Entry::Vacant(entry) = programs.entry(request.method) {
        entry.insert(programs::Program::new(device, request.method).ok_or("unsupported_method")?);
    }
    Ok(Graph::Texture(
        programs
            .get(&request.method)
            .unwrap()
            .graph(device, source, size),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_quiet_gate_and_generation_cancel_are_independent() {
        let epoch = Instant::now();
        let shared = Shared {
            snapshot: Mutex::new(None),
            generation: AtomicU64::new(3),
            quiet_micros: AtomicU64::new(INPUT_QUIET.as_micros() as u64),
            stopped: AtomicBool::new(false),
            failure: AtomicU8::new(0),
            unclaimed_outputs: AtomicUsize::new(0),
            epoch,
        };
        assert!(!can_start(&shared, 3, epoch + Duration::from_millis(149)));
        assert!(can_start(&shared, 3, epoch + INPUT_QUIET));
        shared.generation.store(4, Ordering::Release);
        assert!(!can_start(&shared, 3, epoch + Duration::from_secs(1)));
    }
    #[test]
    fn auto_and_artcnn_are_not_background_manual_methods() {
        assert!(!BackgroundRefiner::supports(WgpuUpscaleMethod::Auto));
        assert!(!BackgroundRefiner::supports(
            WgpuUpscaleMethod::WgslArtcnnC4F16
        ));
        assert!(BackgroundRefiner::supports(WgpuUpscaleMethod::Cunny4x12Nvl));
        assert!(BackgroundRefiner::supports(
            WgpuUpscaleMethod::WgslSrLabSpanX2
        ));
    }
    #[test]
    fn spread_reserves_both_outputs_and_shared_ab_input_once() {
        // A/B share a 100-byte source but have independent 400-byte outputs.
        let footprints = vec![(10, 1, 100, 400), (20, 1, 100, 400)];
        assert_eq!(reserved_allowance(1000, 10, footprints.clone()), Some(500));
        assert_eq!(reserved_allowance(1000, 20, footprints), Some(500));
        // A distinct spread page must pin its own input as well.
        assert_eq!(
            reserved_allowance(1000, 10, vec![(10, 1, 100, 400), (20, 2, 200, 400)]),
            Some(300)
        );
        assert_eq!(
            reserved_allowance(250, 10, vec![(10, 1, 100, 400), (20, 2, 200, 400)]),
            None
        );
    }
}
