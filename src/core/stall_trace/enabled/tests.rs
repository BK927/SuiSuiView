use super::*;

struct Lines(Sender<String>);

impl Write for Lines {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .send(String::from_utf8(bytes.to_vec()).unwrap())
            .unwrap();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn ongoing_stall_is_written_before_scope_finishes() {
    let (tx, rx) = bounded(QUEUE_CAPACITY);
    let logger = Arc::new(Logger::new(tx));
    let (index, token) = logger.begin(Stage::OpenPath).unwrap();
    let (lines_tx, lines_rx) = crossbeam_channel::unbounded();
    let worker_logger = logger.clone();
    let worker = thread::spawn(move || write_session(&worker_logger, &rx, Lines(lines_tx)));
    let start: Value =
        serde_json::from_str(&lines_rx.recv_timeout(Duration::from_secs(1)).unwrap()).unwrap();
    assert_eq!(start["event"], "session_start");
    let sample: Value =
        serde_json::from_str(&lines_rx.recv_timeout(Duration::from_secs(4)).unwrap()).unwrap();
    assert_eq!(sample["event"], "in_progress");
    assert_eq!(sample["stage"], "open_path");
    assert_eq!(sample["thread"], "ui");
    assert!(sample["duration_ms"].as_f64().unwrap() >= 1000.0);
    assert_eq!(logger.slots[index].load(Ordering::Relaxed), token);
    logger.finish(index, token);
    logger.stopping.store(true, Ordering::Relaxed);
    worker.join().unwrap().unwrap();
    let rest: Vec<Value> = lines_rx
        .try_iter()
        .map(|line| serde_json::from_str(&line).unwrap())
        .collect();
    assert!(rest.iter().any(|line| line["event"] == "slow"));
    assert_eq!(rest.last().unwrap()["event"], "session_end");
}

#[test]
fn saturation_cannot_leave_a_false_in_progress_scope() {
    let (tx, _rx) = bounded(1);
    let mut logger = Logger::new(tx);
    logger.origin = Instant::now() - Duration::from_secs(2);
    let token = Stage::ReadPage as u64;
    logger.slots[0].store(token, Ordering::Relaxed);
    logger.tx.try_send(Completed { token, end_us: 0 }).unwrap();
    logger.finish(0, token);
    assert_eq!(logger.slots[0].load(Ordering::Relaxed), 0);
    assert_eq!(logger.dropped.load(Ordering::Relaxed), 1);
    for slot in &logger.slots {
        slot.store(token, Ordering::Relaxed);
    }
    assert!(logger.begin(Stage::OpenPath).is_none());
    assert_eq!(logger.dropped.load(Ordering::Relaxed), 2);
}

#[test]
fn short_scopes_do_not_queue_records_and_background_role_is_fixed() {
    let (tx, rx) = bounded(1);
    let logger = Arc::new(Logger::new(tx));
    let other_logger = logger.clone();
    let token = thread::spawn(move || {
        let (index, token) = other_logger.begin(Stage::ReadPage).unwrap();
        other_logger.finish(index, token);
        token
    })
    .join()
    .unwrap();
    assert!(rx.is_empty());
    assert_ne!(token & THREAD_BIT, 0);
    let row = timing("slow", token, (token >> 8) + SLOW_US);
    let keys: Vec<_> = row
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["duration_ms", "event", "stage", "start_ms", "thread"]
    );
    assert_eq!(row["thread"], "background");
    assert_eq!(row["duration_ms"], 100.0);
}

#[test]
fn output_stops_at_size_cap_with_an_explicit_marker() {
    let mut output = Output {
        writer: Vec::new(),
        bytes: 0,
    };
    let row = timing("slow", Stage::ReadPage as u64, SLOW_US);
    while output.record(row.clone()).unwrap() {}
    assert!(output.writer.len() <= MAX_BYTES);
    assert!(output
        .writer
        .ends_with(b"{\"event\":\"log_limit_reached\"}\n"));
}

#[test]
fn failed_destination_does_not_retry_or_accumulate_events() {
    struct Failed;
    impl Write for Failed {
        fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "synthetic failure",
            ))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let (tx, rx) = bounded(QUEUE_CAPACITY);
    assert!(write_session(&Logger::new(tx), &rx, Failed).is_err());
}

#[test]
#[ignore = "release-only diagnostic overhead measurement"]
fn measure_scope_overhead() {
    use std::hint::black_box;
    let (tx, _rx) = bounded(QUEUE_CAPACITY);
    let logger = Logger::new(tx);
    let started = Instant::now();
    for _ in 0..100_000 {
        let (index, token) = logger.begin(black_box(Stage::UpdateFrame)).unwrap();
        logger.finish(index, token);
    }
    println!(
        "100000 enabled scopes: {:.3} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let started = Instant::now();
    for _ in 0..100_000 {
        black_box(scope(black_box(Stage::UpdateFrame)));
    }
    println!(
        "100000 uninitialized scopes: {:.3} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
}
