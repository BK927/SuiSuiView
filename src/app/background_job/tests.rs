use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

fn finish<T: Send + 'static>(job: &mut BackgroundJob<T>) -> Result<T, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = job.poll() {
            return result;
        }
        assert!(Instant::now() < deadline, "background job did not finish");
        std::thread::yield_now();
    }
}

#[test]
fn blocked_read_does_not_block_request_or_poll_and_only_latest_replacement_runs() {
    let ctx = egui::Context::default();
    let mut job = BackgroundJob::default();
    let (started_tx, started_rx) = bounded(1);
    let (release_tx, release_rx) = bounded(1);
    job.request(&ctx, move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        0
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let executions = Arc::new(AtomicUsize::new(0));
    for value in 1..=100 {
        let executions = executions.clone();
        job.request(&ctx, move || {
            executions.fetch_add(1, Ordering::SeqCst);
            value
        });
        assert!(job.poll().is_none());
    }
    assert!(job.pending());
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    release_tx.send(()).unwrap();
    assert_eq!(finish(&mut job).unwrap(), 100);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert!(!job.pending());
}

#[test]
fn cancelled_result_cannot_reappear_after_close_or_mutation() {
    let ctx = egui::Context::default();
    let mut job = BackgroundJob::default();
    let (tx, rx) = bounded(1);
    job.request(&ctx, move || {
        rx.recv().unwrap();
        1
    });
    job.request(&ctx, || 2);
    job.cancel();
    assert!(!job.pending());
    tx.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while job.running.is_some() {
        assert!(job.poll().is_none());
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    job.request(&ctx, || 3);
    assert_eq!(finish(&mut job).unwrap(), 3);
}

#[test]
fn failed_worker_ends_loading_and_can_retry() {
    let ctx = egui::Context::default();
    let mut job = BackgroundJob::<usize>::default();
    job.request(&ctx, || panic!("synthetic worker failure"));
    assert!(finish(&mut job).is_err());
    assert!(!job.pending());
    job.request(&ctx, || 4);
    assert_eq!(finish(&mut job).unwrap(), 4);
}

#[test]
fn cancellation_skips_later_io_phases_before_starting_latest_request() {
    let ctx = egui::Context::default();
    let mut job = BackgroundJob::default();
    let (tx, rx) = bounded(1);
    let later_phases = Arc::new(AtomicUsize::new(0));
    let count = later_phases.clone();
    job.request_cancellable(&ctx, move |cancellation| {
        rx.recv().unwrap();
        if cancellation.cancelled() { return 0; }
        count.fetch_add(1, Ordering::SeqCst);
        1
    });
    job.request(&ctx, || 2);
    tx.send(()).unwrap();
    assert_eq!(finish(&mut job).unwrap(), 2);
    assert_eq!(later_phases.load(Ordering::SeqCst), 0);
}
