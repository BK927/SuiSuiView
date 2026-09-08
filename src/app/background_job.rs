//! A latest-request slot for blocking reads. One running job and one replacement
//! are retained; cancellation never joins a thread waiting on a disk.
use crossbeam_channel::{bounded, Receiver, TryRecvError};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

pub(in crate::app) struct Cancellation {
    current: Arc<AtomicU64>,
    generation: u64,
}

impl Cancellation {
    pub(in crate::app) fn cancelled(&self) -> bool {
        self.current.load(Ordering::Relaxed) != self.generation
    }
}

type Work<T> = Box<dyn FnOnce() -> T + Send>;

#[cfg(test)]
mod tests;

pub(in crate::app) struct BackgroundJob<T> {
    generation: u64,
    current: Arc<AtomicU64>,
    running: Option<(u64, Receiver<T>)>,
    queued: Option<(egui::Context, Work<T>)>,
    error: Option<String>,
}

impl<T> Default for BackgroundJob<T> {
    fn default() -> Self {
        Self {
            generation: 0,
            current: Arc::new(AtomicU64::new(0)),
            running: None,
            queued: None,
            error: None,
        }
    }
}

impl<T> std::fmt::Debug for BackgroundJob<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundJob").finish_non_exhaustive()
    }
}

impl<T: Send + 'static> BackgroundJob<T> {
    pub(in crate::app) fn request(
        &mut self,
        ctx: &egui::Context,
        work: impl FnOnce() -> T + Send + 'static,
    ) {
        self.request_cancellable(ctx, move |_| work());
    }

    pub(in crate::app) fn request_cancellable(
        &mut self,
        ctx: &egui::Context,
        work: impl FnOnce(Cancellation) -> T + Send + 'static,
    ) {
        self.cancel();
        let cancellation = Cancellation {
            current: self.current.clone(),
            generation: self.generation,
        };
        self.queued = Some((ctx.clone(), Box::new(move || work(cancellation))));
        self.start_queued();
    }

    pub(in crate::app) fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.current.store(self.generation, Ordering::Relaxed);
        self.queued = None;
        self.error = None;
    }

    pub(in crate::app) fn pending(&self) -> bool {
        self.queued.is_some()
            || self
                .running
                .as_ref()
                .is_some_and(|(generation, _)| *generation == self.generation)
    }

    pub(in crate::app) fn poll(&mut self) -> Option<Result<T, String>> {
        if let Some(error) = self.error.take() {
            return Some(Err(error));
        }
        let (generation, rx) = self.running.as_ref()?;
        let result = match rx.try_recv() {
            Ok(value) => Ok(value),
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                Err("Background read stopped unexpectedly".to_owned())
            }
        };
        let current = *generation == self.generation;
        self.running = None;
        self.start_queued();
        if current {
            Some(result)
        } else {
            // Starting the replacement may fail. Deliver that error now even
            // if the cancelled job's completion was the last scheduled repaint.
            self.error.take().map(Err)
        }
    }

    fn start_queued(&mut self) {
        if self.running.is_some() {
            return;
        }
        let Some((ctx, work)) = self.queued.take() else {
            return;
        };
        let (tx, rx) = bounded(1);
        match std::thread::Builder::new()
            .name("suisuiview-background-read".to_owned())
            .spawn(move || {
                // Repaint even if the job panics, so a disconnected receiver clears
                // the loading state instead of leaving an endless spinner.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
                if let Ok(value) = result {
                    let _ = tx.send(value);
                }
                drop(tx);
                ctx.request_repaint();
            }) {
            Ok(_) => self.running = Some((self.generation, rx)),
            Err(error) => self.error = Some(format!("Could not start background read: {error}")),
        }
    }
}

impl<T> Drop for BackgroundJob<T> {
    fn drop(&mut self) {
        self.current.fetch_add(1, Ordering::Relaxed);
    }
}
