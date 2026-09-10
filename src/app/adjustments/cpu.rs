use crate::app::{PageCacheKey, SuiSuiViewApp, TextureCacheKey, TextureEntry};
use crate::core::effects::{apply_effects_to_owned_image, ImageFilter, ViewEffects};
use crate::core::worker::PagePixels;
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const MAX_PENDING: usize = 8;
const MAX_WORK_BYTES: usize = 256 * 1024 * 1024;
type RequestId = (u64, PageCacheKey);

struct Input {
    book: u64,
    key: TextureCacheKey,
    pixels: PagePixels,
    size: [usize; 2],
}

impl Input {
    fn reservation_bytes(&self) -> Option<usize> {
        let rgba = self.size[0].checked_mul(self.size[1])?.checked_mul(4)?;
        // Spatial filters retain the expansion, transformed input and output.
        // Tone/transform-only processing needs at most two RGBA buffers.
        let buffers = if self.key.effects.filter == ImageFilter::None {
            2
        } else {
            3
        };
        self.pixels
            .byte_len()
            .checked_add(rgba.checked_mul(buffers)?)
    }
}

/// Keep accounting alive through the completed channel, including stale results.
struct Reservation {
    bytes: usize,
    used: Arc<AtomicUsize>,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Desired {
    key: TextureCacheKey,
    generation: u64,
}

struct Job {
    input: Input,
    generation: u64,
    reservation: Reservation,
}

struct Completed {
    book: u64,
    desired: Desired,
    image: egui::ColorImage,
    _reservation: Reservation,
}

#[derive(Default)]
struct Queue {
    pending: Vec<Job>,
    wanted: HashMap<RequestId, Desired>,
    generation: u64,
}

impl Queue {
    fn is_current(&self, book: u64, desired: Desired) -> bool {
        self.wanted.get(&(book, desired.key.page)) == Some(&desired)
    }

    fn remove(&mut self, book: u64, desired: Desired) {
        if self.is_current(book, desired) {
            self.wanted.remove(&(book, desired.key.page));
        }
    }

    fn retain(&mut self, book: u64, keys: &[TextureCacheKey]) {
        self.wanted
            .retain(|(old_book, _), wanted| *old_book == book && keys.contains(&wanted.key));
        self.pending.retain(|job| {
            self.wanted.get(&(job.input.book, job.input.key.page))
                == Some(&Desired {
                    key: job.input.key,
                    generation: job.generation,
                })
        });
    }
}

#[derive(Debug)]
enum RequestError {
    PageTooLarge,
    Unavailable(String),
}

/// A latest-value mailbox per visible page; dragging a control never queues every value.
pub(super) struct CpuAdjustmentWorker {
    queue: Arc<Mutex<Queue>>,
    reserved: Arc<AtomicUsize>,
    wake: Sender<()>,
    completed: Receiver<Completed>,
}

struct WakeOnExit(egui::Context);

impl Drop for WakeOnExit {
    fn drop(&mut self) {
        self.0.request_repaint();
    }
}

impl CpuAdjustmentWorker {
    fn new(ctx: egui::Context) -> Result<Self, String> {
        let queue = Arc::new(Mutex::new(Queue::default()));
        let worker_queue = queue.clone();
        let (wake, wakes) = bounded(1);
        let (tx, completed) = bounded(2);
        std::thread::Builder::new()
            .name("suisuiview-cpu-adjustment".into())
            .spawn(move || {
                let _wake_on_exit = WakeOnExit(ctx.clone());
                run_worker(&worker_queue, &wakes, &tx, &ctx);
            })
            .map_err(|error| format!("CPU adjustment worker could not start: {error}"))?;
        Ok(Self {
            queue,
            reserved: Arc::new(AtomicUsize::new(0)),
            wake,
            completed,
        })
    }

    /// False means temporary pressure from other jobs, not a permanent failure.
    fn request(&mut self, input: Input, budget: usize) -> Result<bool, RequestError> {
        let bytes = input
            .reservation_bytes()
            .ok_or(RequestError::PageTooLarge)?;
        if bytes > budget {
            return Err(RequestError::PageTooLarge);
        }
        let mut queue = self.queue.lock().map_err(|_| {
            RequestError::Unavailable("CPU adjustment queue is unavailable".to_owned())
        })?;
        let id = (input.book, input.key.page);
        if queue
            .wanted
            .get(&id)
            .is_some_and(|wanted| wanted.key == input.key)
        {
            return Ok(true);
        }
        queue.wanted.remove(&id);
        queue
            .pending
            .retain(|old| (old.input.book, old.input.key.page) != id);
        if queue.pending.len() == MAX_PENDING {
            let old = queue.pending.remove(0);
            queue.remove(
                old.input.book,
                Desired {
                    key: old.input.key,
                    generation: old.generation,
                },
            );
        }
        let used = self.reserved.load(Ordering::Acquire);
        if bytes > budget.saturating_sub(used) {
            return Ok(false);
        }
        self.reserved.fetch_add(bytes, Ordering::AcqRel);
        let reservation = Reservation {
            bytes,
            used: self.reserved.clone(),
        };
        queue.generation = queue.generation.wrapping_add(1);
        let generation = queue.generation;
        queue.wanted.insert(
            id,
            Desired {
                key: input.key,
                generation,
            },
        );
        queue.pending.push(Job {
            input,
            generation,
            reservation,
        });
        drop(queue);
        match self.wake.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => Ok(true),
            Err(TrySendError::Disconnected(())) => Err(RequestError::Unavailable(
                "CPU adjustment worker stopped".to_owned(),
            )),
        }
    }

    fn retain(&self, book: u64, keys: &[TextureCacheKey]) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.retain(book, keys);
        }
        // A full completion channel can leave the worker waiting. Wake it only
        // if it may need to discard its now-stale completed image.
        if self.completed.is_full() {
            let _ = self.wake.try_send(());
        }
    }
}

fn run_worker(
    queue: &Mutex<Queue>,
    wakes: &Receiver<()>,
    tx: &Sender<Completed>,
    ctx: &egui::Context,
) {
    while wakes.recv().is_ok() {
        loop {
            let job = {
                let Ok(mut queue) = queue.lock() else { return };
                queue.pending.pop()
            };
            let Some(job) = job else { break };
            let desired = Desired {
                key: job.input.key,
                generation: job.generation,
            };
            if !queue
                .lock()
                .is_ok_and(|queue| queue.is_current(job.input.book, desired))
            {
                continue;
            }
            let image = apply_effects_to_owned_image(
                job.input.pixels.to_color_image(job.input.size),
                job.input.key.effects,
            );
            let completed = Completed {
                book: job.input.book,
                desired,
                image,
                _reservation: job.reservation,
            };
            // Release the pinned source before handing over its complete result;
            // the conservative reservation stays until upload or rejection.
            drop(job.input);
            if !publish(queue, wakes, tx, ctx, completed) {
                return;
            }
        }
    }
}

fn publish(
    queue: &Mutex<Queue>,
    wakes: &Receiver<()>,
    tx: &Sender<Completed>,
    ctx: &egui::Context,
    mut completed: Completed,
) -> bool {
    loop {
        {
            let Ok(queue) = queue.lock() else {
                return false;
            };
            if !queue.is_current(completed.book, completed.desired) {
                return true;
            }
            match tx.try_send(completed) {
                Ok(()) => {
                    ctx.request_repaint();
                    return true;
                }
                Err(TrySendError::Disconnected(_)) => return false,
                Err(TrySendError::Full(result)) => completed = result,
            }
        }
        // The UI wakes us when it drains or cancels; no completion polling loop.
        if wakes.recv().is_err() {
            return false;
        }
    }
}

impl SuiSuiViewApp {
    pub(in crate::app) fn request_cpu_adjustment(
        &mut self,
        key: TextureCacheKey,
        pixels: PagePixels,
        size: [usize; 2],
    ) -> Result<(), String> {
        if let Some(error) = &self.adjustments.cpu_error {
            return Err(error.clone());
        }
        let input = Input {
            book: self.gpu_paint_book_key(),
            key,
            pixels,
            size,
        };
        let budget = self.texture_cache_budget_bytes().min(MAX_WORK_BYTES);
        if input.reservation_bytes().is_none_or(|bytes| bytes > budget) {
            return Err(self.i18n().text("adjust.cpu_memory_limit"));
        }
        if self.adjustments.cpu.is_none() {
            match CpuAdjustmentWorker::new(self.egui_ctx.clone()) {
                Ok(worker) => self.adjustments.cpu = Some(worker),
                Err(error) => {
                    self.adjustments.cpu_error = Some(error.clone());
                    return Err(error);
                }
            }
        }
        match self
            .adjustments
            .cpu
            .as_mut()
            .unwrap()
            .request(input, budget)
        {
            Ok(true) => Ok(()),
            Ok(false) => {
                // The old running operation is allowed to finish. Retry only
                // while this page is still requesting a correction.
                self.egui_ctx
                    .request_repaint_after(std::time::Duration::from_millis(25));
                Ok(())
            }
            Err(RequestError::PageTooLarge) => Err(self.i18n().text("adjust.cpu_memory_limit")),
            Err(RequestError::Unavailable(error)) => {
                self.adjustments.cpu = None;
                self.adjustments.cpu_error = Some(error.clone());
                Err(error)
            }
        }
    }

    pub(in crate::app) fn prune_cpu_adjustments(&mut self) {
        let Some(worker) = self.adjustments.cpu.as_ref() else {
            return;
        };
        let book = self.gpu_paint_book_key();
        let effects = self.display_effects();
        if self.source.is_none()
            || self.can_paint_wgsl_effects()
            || effects == ViewEffects::default()
        {
            worker.retain(book, &[]);
            return;
        }
        let budget = self.texture_cache_budget_bytes().min(MAX_WORK_BYTES);
        let mut keys = Vec::new();
        let mut add_page = |index, target| {
            if let Some(page) = self
                .page_key_at(index, target)
                .and_then(|key| self.best_page_key(key))
            {
                let key = TextureCacheKey {
                    page,
                    effects,
                    sampling: self.texture_sampling_for_page_key(page),
                };
                if self.decoded_pages.peek(&page).is_some_and(|decoded| {
                    Input {
                        book,
                        key,
                        pixels: decoded.pixels.clone(),
                        size: decoded.image_size(),
                    }
                    .reservation_bytes()
                    .is_some_and(|bytes| bytes <= budget)
                }) {
                    keys.push(key);
                }
            }
        };
        for index in self.visible_page_indices() {
            add_page(index, self.target_long_edge);
        }
        if let Some(transition) = &self.transition {
            for &index in &transition.from_indices {
                add_page(index, transition.target_long_edge);
            }
        }
        worker.retain(book, &keys);
    }

    pub(in crate::app) fn drain_cpu_adjustments(&mut self, ctx: &egui::Context) {
        self.prune_cpu_adjustments();
        let Some(worker) = self.adjustments.cpu.as_mut() else {
            return;
        };
        let mut uploaded = false;
        let mut stopped = false;
        loop {
            let result = match worker.completed.try_recv() {
                Ok(result) => result,
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    stopped = true;
                    break;
                }
            };
            let _ = worker.wake.try_send(());
            let valid = worker.queue.lock().is_ok_and(|mut queue| {
                let valid = queue.is_current(result.book, result.desired);
                queue.remove(result.book, result.desired);
                valid
            });
            if !valid {
                continue;
            }
            let key = result.desired.key;
            let byte_size = result.image.pixels.len().saturating_mul(4);
            let texture = ctx.load_texture(
                "adjusted-page",
                result.image,
                crate::app::texture_options_for_sampling(key.sampling),
            );
            self.textures.put(key, TextureEntry { texture, byte_size });
            uploaded = true;
            break;
        }
        if !worker.completed.is_empty() {
            ctx.request_repaint();
        }
        if stopped {
            self.adjustments.cpu = None;
            self.adjustments.cpu_error = Some("CPU adjustment worker stopped".to_owned());
            ctx.request_repaint();
        }
        if uploaded {
            self.prune_texture_cache();
        }
    }
}

#[cfg(test)]
#[path = "cpu_tests.rs"]
mod tests;
