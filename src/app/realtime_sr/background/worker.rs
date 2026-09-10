//! The sole background GPU submitter; shared state is updated without UI waits.
use super::*;

pub(super) fn run_worker(
    device: wgpu::Device,
    queue: wgpu::Queue,
    ctx: egui::Context,
    shared: Arc<Shared>,
    wakes: Receiver<()>,
    events: Sender<RefineEvent>,
) {
    let mut snapshot: Option<RefineSnapshot> = None;
    let mut active: Option<Active> = None;
    let mut programs = HashMap::new();
    let mut completed: HashMap<u64, Weak<RealtimeSrOutput>> = HashMap::new();
    let mut skipped = HashSet::new();
    let mut pending_event = None;
    while !shared.stopped.load(Ordering::Acquire) {
        let incoming = shared
            .snapshot
            .lock()
            .ok()
            .and_then(|mut pending| pending.take());
        if let Some(next) = incoming {
            if snapshot.as_ref().is_none_or(|old| {
                old.generation != next.generation || old.budget_bytes != next.budget_bytes
            }) {
                skipped.clear();
            }
            let keys: HashSet<_> = next.requests.iter().map(|r| r.key).collect();
            completed.retain(|key, value| keys.contains(key) && value.strong_count() != 0);
            if active.as_ref().is_some_and(|job| {
                !keys.contains(&job.request.key)
                    || snapshot
                        .as_ref()
                        .is_some_and(|old| old.generation != next.generation)
            }) {
                active = None;
            }
            if keys.is_empty() {
                active = None;
                programs.clear();
            }
            snapshot = Some(next);
        }
        if let Some(event) = pending_event.take() {
            match events.try_send(event) {
                Ok(()) => ctx.request_repaint(),
                Err(crossbeam_channel::TrySendError::Full(event)) => {
                    pending_event = Some(event);
                    let _ = wakes.recv_timeout(Duration::from_millis(5));
                    continue;
                }
                Err(crossbeam_channel::TrySendError::Disconnected(_)) => break,
            }
        }
        let Some(current) = snapshot.as_ref() else {
            if wakes.recv().is_err() {
                break;
            }
            continue;
        };
        // Publish one complete result, then let the UI adopt/drop it and send
        // its next budget snapshot before allocating another full graph. In
        // particular, stale queued outputs cannot escape the memory budget.
        if shared.unclaimed_outputs.load(Ordering::Acquire) != 0 {
            if wakes.recv().is_err() {
                break;
            }
            continue;
        }
        // Source-ready callbacks also need progress when no refinement batch
        // has started yet. Poll is nonblocking and stays off the UI thread.
        if current
            .requests
            .iter()
            .any(|request| !request.source_ready.load(Ordering::Acquire))
            && device.poll(wgpu::PollType::Poll).is_err()
        {
            shared.failure.store(3, Ordering::Release);
            ctx.request_repaint();
            return;
        }
        if shared.generation.load(Ordering::Acquire) != current.generation {
            active = None;
        }
        if !can_start(&shared, current.generation, Instant::now()) {
            let _ = wakes.recv_timeout(Duration::from_millis(10));
            continue;
        }
        if active.is_none() {
            let request = current.requests.iter().find(|r| {
                !skipped.contains(&r.key)
                    && !completed.contains_key(&r.key)
                    && r.source_ready.load(Ordering::Acquire)
            });
            let Some(request) = request.cloned() else {
                if current
                    .requests
                    .iter()
                    .any(|request| !request.source_ready.load(Ordering::Acquire))
                {
                    let _ = wakes.recv_timeout(Duration::from_millis(10));
                } else if wakes.recv().is_err() {
                    break;
                }
                continue;
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                output_bytes(&request).ok_or("invalid_size")?;
                let allowance = graph_allowance(current, &request).ok_or("memory_budget")?;
                // A stacked graph's peak includes its full 2x input retained
                // while the 4x model runs. Reject before allocating stage one.
                if request.stack_passes == 2 && request.method != WgpuUpscaleMethod::WgslSrLabSpanX2
                {
                    let next = [
                        request.source_size[0]
                            .checked_mul(2)
                            .ok_or("size_overflow")?,
                        request.source_size[1]
                            .checked_mul(2)
                            .ok_or("size_overflow")?,
                    ];
                    let peak = programs::graph_bytes(request.method, next)
                        .and_then(|bytes| {
                            bytes.checked_add(next[0].checked_mul(next[1])?.checked_mul(4)?)
                        })
                        .ok_or("size_overflow")?;
                    if peak > allowance {
                        return Err("memory_budget");
                    }
                }
                make_graph(
                    &device,
                    &request,
                    &request.source_view,
                    request.source_size,
                    allowance,
                    &mut programs,
                )
            }))
            .unwrap_or(Err("gpu_prepare_failed"));
            match result {
                Ok(graph) => {
                    active = Some(Active {
                        request,
                        graph,
                        stage: 1,
                        previous_output: None,
                    })
                }
                Err(reason) => {
                    skipped.insert(request.key);
                    pending_event = Some(RefineEvent::Skipped {
                        generation: current.generation,
                        key: request.key,
                        reason,
                    });
                    continue;
                }
            }
        }
        // Compilation/allocation above can take time. Recheck input/generation
        // immediately before putting any work onto the shared display queue.
        if !can_start(&shared, current.generation, Instant::now()) {
            continue;
        }
        let Some(job) = active.as_mut() else {
            continue;
        };
        let previous_bytes = job
            .previous_output
            .as_ref()
            .map_or(0, |output| output.byte_size);
        if graph_allowance(current, &job.request)
            .is_none_or(|allowance| job.graph.bytes().saturating_add(previous_bytes) > allowance)
        {
            let key = job.request.key;
            active = None;
            skipped.insert(key);
            pending_event = Some(RefineEvent::Skipped {
                generation: current.generation,
                key,
                reason: "memory_budget",
            });
            continue;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("suisuiview-refine-batch"),
        });
        let finished = job.graph.encode_next(&device, &queue, &mut encoder);
        if !can_start(&shared, current.generation, Instant::now()) {
            // No commands from this batch were submitted. Discard its advanced
            // cursor; the next quiet request starts with a fresh complete graph.
            active = None;
            continue;
        }
        queue.submit(Some(encoder.finish()));
        let (done_tx, done_rx) = bounded(1);
        queue.on_submitted_work_done(move || {
            let _ = done_tx.try_send(());
        });
        // Only this worker polls. Even cancellation retains all batch resources
        // until the outstanding submission retires before dropping/reusing them.
        loop {
            if device.poll(wgpu::PollType::Poll).is_err() {
                shared.failure.store(3, Ordering::Release);
                ctx.request_repaint();
                return;
            }
            match done_rx.recv_timeout(Duration::from_millis(2)) {
                Ok(()) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            }
        }
        if !finished {
            continue;
        }
        let job = active.take().unwrap();
        if shared.generation.load(Ordering::Acquire) != current.generation {
            continue;
        }
        let output = job.graph.output();
        let request = job.request;
        let next_stage = job.stage + 1;
        drop(job.graph);
        drop(job.previous_output);
        if next_stage <= request.stack_passes {
            let allowance = graph_allowance(current, &request)
                .and_then(|allowance| allowance.checked_sub(output.byte_size));
            match allowance.ok_or("memory_budget").and_then(|allowance| {
                make_graph(
                    &device,
                    &request,
                    &output.view,
                    output.size,
                    allowance,
                    &mut programs,
                )
            }) {
                Ok(graph) => {
                    active = Some(Active {
                        request,
                        graph,
                        stage: next_stage,
                        previous_output: Some(output),
                    })
                }
                Err(reason) => {
                    skipped.insert(request.key);
                    pending_event = Some(RefineEvent::Skipped {
                        generation: current.generation,
                        key: request.key,
                        reason,
                    });
                }
            }
        } else {
            completed.insert(request.key, Arc::downgrade(&output));
            shared.unclaimed_outputs.fetch_add(1, Ordering::AcqRel);
            pending_event = Some(RefineEvent::Ready {
                generation: current.generation,
                key: request.key,
                output,
            });
        }
    }
}
