use super::Stage;
use crossbeam_channel::{bounded, Receiver, Sender};
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

const SLOT_COUNT: usize = 64;
const QUEUE_CAPACITY: usize = 256;
const SLOW_US: u64 = 100_000;
const BLOCKED_US: u64 = 1_000_000;
const POLL: Duration = Duration::from_millis(100);
const MAX_BYTES: usize = 2 * 1024 * 1024;
const THREAD_BIT: u64 = 128;
static LOGGER: OnceLock<Arc<Logger>> = OnceLock::new();

struct Logger {
    origin: Instant,
    ui_thread: ThreadId,
    active: AtomicBool,
    stopping: AtomicBool,
    // One atomic word publishes a complete scope. Clearing it on Drop cannot
    // get lost behind a full event queue and leave a fictitious ongoing stall.
    slots: [AtomicU64; SLOT_COUNT],
    dropped: AtomicU64,
    tx: Sender<Completed>,
}

#[derive(Clone, Copy)]
struct Completed {
    token: u64,
    end_us: u64,
}

pub struct Session {
    logger: Arc<Logger>,
    done: Receiver<()>,
}

pub struct Scope {
    slot: Option<(&'static Logger, usize, u64)>,
}

/// Initialization runs before loading settings. No destination means no thread,
/// file access, or clock sampling. An existing output file is never overwritten.
pub fn start_session() -> Option<Session> {
    let path = std::env::var_os("SUISUIVIEW_STALL_LOG").filter(|p| !p.is_empty())?;
    let path = PathBuf::from(path);
    if !path.is_absolute() || LOGGER.get().is_some() {
        return None;
    }
    let (tx, rx) = bounded(QUEUE_CAPACITY);
    let logger = Arc::new(Logger::new(tx));
    LOGGER.set(logger.clone()).ok()?;
    let (done_tx, done) = bounded(1);
    let writer_logger = logger.clone();
    if thread::Builder::new()
        .name("suisuiview-stall-writer".into())
        .spawn(move || {
            // Even opening the destination stays off the UI thread.
            if let Ok(file) = OpenOptions::new().write(true).create_new(true).open(path) {
                let _ = write_session(&writer_logger, &rx, file);
            }
            writer_logger.active.store(false, Ordering::Relaxed);
            let _ = done_tx.try_send(());
        })
        .is_err()
    {
        logger.active.store(false, Ordering::Relaxed);
        return None;
    }
    Some(Session { logger, done })
}

#[inline]
pub fn scope(stage: Stage) -> Scope {
    let slot = LOGGER.get().and_then(|logger| {
        if !logger.active.load(Ordering::Relaxed) {
            return None;
        }
        logger
            .begin(stage)
            .map(|(index, token)| (&**logger, index, token))
    });
    Scope { slot }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if let Some((logger, index, token)) = self.slot {
            logger.finish(index, token);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.logger.active.store(false, Ordering::Relaxed);
        self.logger.stopping.store(true, Ordering::Relaxed);
        // A stuck diagnostic disk must not hold application shutdown hostage.
        let _ = self.done.recv_timeout(Duration::from_millis(200));
    }
}

impl Logger {
    fn new(tx: Sender<Completed>) -> Self {
        Self {
            origin: Instant::now(),
            ui_thread: thread::current().id(),
            active: AtomicBool::new(true),
            stopping: AtomicBool::new(false),
            slots: std::array::from_fn(|_| AtomicU64::new(0)),
            dropped: AtomicU64::new(0),
            tx,
        }
    }

    fn now_us(&self) -> u64 {
        self.origin
            .elapsed()
            .as_micros()
            .min((u64::MAX >> 8) as u128) as u64
    }

    fn begin(&self, stage: Stage) -> Option<(usize, u64)> {
        let thread_bit = if thread::current().id() == self.ui_thread {
            0
        } else {
            THREAD_BIT
        };
        let token = (self.now_us() << 8) | thread_bit | stage as u64;
        for (index, slot) in self.slots.iter().enumerate() {
            if slot
                .compare_exchange(0, token, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return Some((index, token));
            }
        }
        self.dropped.fetch_add(1, Ordering::Relaxed);
        None
    }

    fn finish(&self, index: usize, token: u64) {
        let end_us = self.now_us();
        self.slots[index].store(0, Ordering::Relaxed);
        if end_us.saturating_sub(token >> 8) >= SLOW_US
            && self.tx.try_send(Completed { token, end_us }).is_err()
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

struct Output<W> {
    writer: W,
    bytes: usize,
}

impl<W: Write> Output<W> {
    fn record(&mut self, value: Value) -> io::Result<bool> {
        let mut line = value.to_string();
        line.push('\n');
        if self.bytes + line.len() > MAX_BYTES - 128 {
            self.writer
                .write_all(b"{\"event\":\"log_limit_reached\"}\n")?;
            self.writer.flush()?;
            return Ok(false);
        }
        self.writer.write_all(line.as_bytes())?;
        // No user-space buffering: a killed/frozen app retains prior samples.
        // This is not a promise of persistence through power failure.
        self.writer.flush()?;
        self.bytes += line.len();
        Ok(true)
    }
}

fn write_session(logger: &Logger, rx: &Receiver<Completed>, writer: impl Write) -> io::Result<()> {
    let mut output = Output { writer, bytes: 0 };
    output.record(json!({"event":"session_start", "schema":1}))?;
    let mut last_sample = 0;
    let mut dropped = 0;
    loop {
        if let Ok(event) = rx.recv_timeout(POLL) {
            if !output.record(timing("slow", event.token, event.end_us))? {
                return Ok(());
            }
        }
        let now = logger.now_us();
        if now.saturating_sub(last_sample) >= BLOCKED_US {
            for slot in &logger.slots {
                let token = slot.load(Ordering::Relaxed);
                if token != 0
                    && now.saturating_sub(token >> 8) >= BLOCKED_US
                    && !output.record(timing("in_progress", token, now))?
                {
                    return Ok(());
                }
            }
            last_sample = now;
        }
        let current_dropped = logger.dropped.load(Ordering::Relaxed);
        if current_dropped != dropped {
            if !output.record(json!({"event":"samples_dropped", "count":current_dropped}))? {
                return Ok(());
            }
            dropped = current_dropped;
        }
        if logger.stopping.load(Ordering::Relaxed) {
            // Bounded drain even if other workers are still winding down.
            for event in rx.try_iter().take(QUEUE_CAPACITY) {
                if !output.record(timing("slow", event.token, event.end_us))? {
                    return Ok(());
                }
            }
            output.record(json!({"event":"session_end"}))?;
            return Ok(());
        }
    }
}

fn timing(event: &'static str, token: u64, end_us: u64) -> Value {
    json!({
        "event": event,
        "stage": stage_name((token & 127) as u8),
        "thread": if token & THREAD_BIT == 0 { "ui" } else { "background" },
        "start_ms": (token >> 8) / 1000,
        "duration_ms": end_us.saturating_sub(token >> 8) as f64 / 1000.0,
    })
}

fn stage_name(stage: u8) -> &'static str {
    match stage {
        x if x == Stage::StateLoad as u8 => "state_load",
        x if x == Stage::StartupPath as u8 => "startup_path",
        x if x == Stage::WindowEvent as u8 => "window_event",
        x if x == Stage::RedrawGlow as u8 => "redraw_glow",
        x if x == Stage::RedrawWgpu as u8 => "redraw_wgpu",
        x if x == Stage::UpdateFrame as u8 => "update_frame",
        x if x == Stage::OpenPath as u8 => "open_path",
        x if x == Stage::ClassifyPath as u8 => "classify_path",
        x if x == Stage::OpenSource as u8 => "open_source",
        x if x == Stage::PrepareBook as u8 => "prepare_book",
        x if x == Stage::InstallBook as u8 => "install_book",
        x if x == Stage::SiblingNavigation as u8 => "sibling_navigation",
        x if x == Stage::SiblingEntries as u8 => "sibling_entries",
        x if x == Stage::ComparePaths as u8 => "compare_paths",
        x if x == Stage::BookmarkList as u8 => "bookmark_list",
        x if x == Stage::BookmarkJump as u8 => "bookmark_jump",
        x if x == Stage::BookmarkToggle as u8 => "bookmark_toggle",
        x if x == Stage::ReadBookRecord as u8 => "read_book_record",
        x if x == Stage::ScanBookRecords as u8 => "scan_book_records",
        x if x == Stage::CollectBookRecords as u8 => "collect_book_records",
        x if x == Stage::RedirectMetadata as u8 => "redirect_metadata",
        x if x == Stage::WriteState as u8 => "write_state",
        x if x == Stage::Thumbnail as u8 => "thumbnail",
        x if x == Stage::ReadPage as u8 => "read_page",
        x if x == Stage::PreparePage as u8 => "prepare_page",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests;
