use super::*;
use crate::app::TextureSampling;
use crate::core::{source::PageId, worker::DecodeOptions};

fn input(page: u32) -> Input {
    Input {
        book: 1,
        key: TextureCacheKey {
            page: PageCacheKey {
                page_id: PageId(page),
                target_long_edge: 4096,
                decode: DecodeOptions::default(),
            },
            effects: ViewEffects {
                gamma: true,
                ..Default::default()
            },
            sampling: TextureSampling::Linear,
        },
        pixels: PagePixels::Rgba(vec![40, 80, 120, 255].into()),
        size: [1, 1],
    }
}

fn mailbox() -> (CpuAdjustmentWorker, Receiver<()>, Sender<Completed>) {
    let (wake, wakes) = bounded(1);
    let (tx, completed) = bounded(2);
    (
        CpuAdjustmentWorker {
            queue: Arc::new(Mutex::new(Queue::default())),
            reserved: Arc::new(AtomicUsize::new(0)),
            wake,
            completed,
        },
        wakes,
        tx,
    )
}

fn completed(job: Job) -> Completed {
    Completed {
        book: job.input.book,
        desired: Desired {
            key: job.input.key,
            generation: job.generation,
        },
        image: job.input.pixels.to_color_image(job.input.size),
        _reservation: job.reservation,
    }
}

#[test]
fn cpu_adjustment_evicted_requests_are_retryable_and_bounded() {
    let (mut worker, _wakes, _tx) = mailbox();
    for page in 0..100 {
        assert!(worker.request(input(page), 1024).unwrap());
    }
    {
        let queue = worker.queue.lock().unwrap();
        assert_eq!(queue.pending.len(), MAX_PENDING);
        assert_eq!(queue.wanted.len(), MAX_PENDING);
        assert!(!queue.wanted.contains_key(&(1, input(0).key.page)));
    }
    assert_eq!(worker.reserved.load(Ordering::Acquire), 12 * MAX_PENDING);
    assert!(worker.request(input(0), 1024).unwrap());
    assert_eq!(
        worker
            .queue
            .lock()
            .unwrap()
            .pending
            .last()
            .unwrap()
            .input
            .key,
        input(0).key
    );
}

#[test]
fn cpu_adjustment_reset_releases_pending_sources_and_cancels_running_result() {
    let (mut worker, wakes, tx) = mailbox();
    worker.request(input(0), 1024).unwrap();
    let running = worker.queue.lock().unwrap().pending.pop().unwrap();
    worker.request(input(1), 1024).unwrap();
    worker.retain(1, &[]);
    assert!(worker.queue.lock().unwrap().pending.is_empty());
    assert!(worker.queue.lock().unwrap().wanted.is_empty());
    assert_eq!(worker.reserved.load(Ordering::Acquire), 12);
    assert!(publish(
        &worker.queue,
        &wakes,
        &tx,
        &egui::Context::default(),
        completed(running)
    ));
    assert!(worker.completed.is_empty());
    assert_eq!(worker.reserved.load(Ordering::Acquire), 0);
}

#[test]
fn cpu_adjustment_prunes_changed_effects_pages_and_books() {
    let (mut worker, _wakes, _tx) = mailbox();
    worker.request(input(0), 1024).unwrap();
    worker.request(input(1), 1024).unwrap();
    let mut changed = input(0);
    changed.key.effects.invert_colors = true;
    let changed_key = changed.key;
    worker.request(changed, 1024).unwrap();
    worker.retain(1, &[changed_key]);
    let queue = worker.queue.lock().unwrap();
    assert_eq!(queue.pending.len(), 1);
    assert_eq!(queue.wanted.len(), 1);
    assert_eq!(queue.pending[0].input.key, changed_key);
    drop(queue);
    worker.retain(2, &[changed_key]);
    assert!(worker.queue.lock().unwrap().wanted.is_empty());
    assert_eq!(worker.reserved.load(Ordering::Acquire), 0);
}

#[test]
fn cpu_adjustment_same_values_after_reset_do_not_accept_old_generation() {
    let (mut worker, wakes, tx) = mailbox();
    worker.request(input(0), 1024).unwrap();
    let old = worker.queue.lock().unwrap().pending.pop().unwrap();
    worker.retain(1, &[]);
    worker.request(input(0), 1024).unwrap();
    assert!(publish(
        &worker.queue,
        &wakes,
        &tx,
        &egui::Context::default(),
        completed(old)
    ));
    assert!(worker.completed.is_empty());
    assert_eq!(worker.reserved.load(Ordering::Acquire), 12);
}

#[test]
fn cpu_adjustment_reservation_survives_completion_transfer_and_pressure_recovers() {
    let (mut worker, wakes, tx) = mailbox();
    worker.request(input(0), 12).unwrap();
    let job = worker.queue.lock().unwrap().pending.pop().unwrap();
    assert!(publish(
        &worker.queue,
        &wakes,
        &tx,
        &egui::Context::default(),
        completed(job)
    ));
    assert_eq!(worker.reserved.load(Ordering::Acquire), 12);
    assert!(!worker.request(input(1), 12).unwrap());
    assert!(!worker
        .queue
        .lock()
        .unwrap()
        .wanted
        .contains_key(&(1, input(1).key.page)));
    let ready = worker.completed.try_recv().unwrap();
    assert_eq!(worker.reserved.load(Ordering::Acquire), 12);
    drop(ready);
    assert_eq!(worker.reserved.load(Ordering::Acquire), 0);
    assert!(worker.request(input(1), 12).unwrap());
}

#[test]
fn cpu_adjustment_oversized_page_does_not_poison_worker() {
    let (mut worker, _wakes, _tx) = mailbox();
    assert!(matches!(
        worker.request(input(0), 11),
        Err(RequestError::PageTooLarge)
    ));
    assert!(worker.queue.lock().unwrap().wanted.is_empty());
    assert_eq!(worker.reserved.load(Ordering::Acquire), 0);
    assert!(worker.request(input(0), 12).unwrap());
    let mut spatial = input(1);
    spatial.key.effects.filter = ImageFilter::Smooth;
    assert_eq!(spatial.reservation_bytes(), Some(16));
    spatial.size = [usize::MAX, usize::MAX];
    assert_eq!(spatial.reservation_bytes(), None);
}

#[test]
fn cpu_adjustment_worker_checks_cancellation_before_expanding_pixels() {
    let (mut worker, wakes, tx) = mailbox();
    let mut invalid_if_computed = input(0);
    invalid_if_computed.size = [2, 2];
    worker.request(invalid_if_computed, 1024).unwrap();
    // Deliberately retain the old pending allocation to exercise the worker's
    // independent generation check, rather than the UI's normal queue pruning.
    worker.queue.lock().unwrap().wanted.clear();
    drop(worker.wake);
    run_worker(&worker.queue, &wakes, &tx, &egui::Context::default());
    assert!(worker.completed.is_empty());
    assert_eq!(worker.reserved.load(Ordering::Acquire), 0);
}

#[test]
fn cpu_adjustment_completed_channel_is_bounded_and_cancel_wakes_blocked_publish() {
    let (mut worker, wakes, tx) = mailbox();
    let ctx = egui::Context::default();
    for page in 0..2 {
        worker.request(input(page), 1024).unwrap();
        let job = worker.queue.lock().unwrap().pending.pop().unwrap();
        assert!(publish(&worker.queue, &wakes, &tx, &ctx, completed(job)));
    }
    assert!(worker.completed.is_full());
    worker.request(input(2), 1024).unwrap();
    let job = worker.queue.lock().unwrap().pending.pop().unwrap();
    let queue = worker.queue.clone();
    let thread = std::thread::spawn(move || publish(&queue, &wakes, &tx, &ctx, completed(job)));
    worker.retain(1, &[]);
    assert!(thread.join().unwrap());
    assert_eq!(worker.completed.len(), 2);
    while let Ok(ready) = worker.completed.try_recv() {
        drop(ready);
    }
    assert_eq!(worker.reserved.load(Ordering::Acquire), 0);
}

#[test]
fn cpu_adjustment_disconnected_worker_reports_failure() {
    let (mut worker, wakes, _tx) = mailbox();
    drop(wakes);
    assert!(matches!(
        worker.request(input(0), 1024),
        Err(RequestError::Unavailable(_))
    ));
}
