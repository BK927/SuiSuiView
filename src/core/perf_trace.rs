use std::time::Duration;

#[derive(Debug, Clone, Copy)]
#[cfg_attr(
    not(any(feature = "perf-dev", feature = "perf-diagnostics")),
    allow(dead_code)
)]
pub enum PerfField {
    Bool(&'static str, bool),
    Str(&'static str, &'static str),
    U32(&'static str, u32),
    Usize(&'static str, usize),
}

pub fn record_duration(event: &'static str, duration: Duration, fields: &[PerfField]) {
    imp::record_duration(event, duration, fields);
}

pub fn record_duration_if_at_least(
    event: &'static str,
    duration: Duration,
    threshold: Duration,
    fields: &[PerfField],
) {
    if duration >= threshold {
        record_duration(event, duration, fields);
    }
}

pub fn flush_timeout(timeout: Duration) -> bool {
    imp::flush_timeout(timeout)
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
mod imp {
    use super::PerfField;
    use crossbeam_channel::{bounded, unbounded, Sender};
    use serde_json::{json, Map, Value};
    use std::env;
    use std::fs::{self, OpenOptions};
    use std::io::{BufWriter, Write};
    use std::path::PathBuf;
    use std::sync::OnceLock;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const PERF_LOG_ENV: &str = "SUISUIVIEW_PERF_LOG";

    struct PerfLogger {
        tx: Sender<PerfMessage>,
    }

    struct PerfEvent {
        ts_ms: u128,
        event: &'static str,
        duration: Duration,
        fields: Vec<PerfField>,
    }

    enum PerfMessage {
        Event(PerfEvent),
        Flush(Sender<()>),
    }

    static LOGGER: OnceLock<Option<PerfLogger>> = OnceLock::new();

    pub(super) fn record_duration(event: &'static str, duration: Duration, fields: &[PerfField]) {
        let Some(logger) = logger() else {
            return;
        };
        let event = PerfEvent {
            ts_ms: unix_ms_now(),
            event,
            duration,
            fields: fields.to_vec(),
        };
        let _ = logger.tx.send(PerfMessage::Event(event));
    }

    pub(super) fn flush_timeout(timeout: Duration) -> bool {
        let Some(logger) = logger() else {
            return true;
        };
        let (tx, rx) = bounded(1);
        if logger.tx.send(PerfMessage::Flush(tx)).is_err() {
            return false;
        }
        rx.recv_timeout(timeout).is_ok()
    }

    fn logger() -> Option<&'static PerfLogger> {
        LOGGER.get_or_init(init_logger).as_ref()
    }

    fn init_logger() -> Option<PerfLogger> {
        let path = env::var_os(PERF_LOG_ENV).filter(|value| !value.is_empty())?;
        let path = PathBuf::from(path);
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).ok()?;
        }
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .ok()?;
        let (tx, rx) = unbounded();
        thread::Builder::new()
            .name("suisuiview-perf-writer".to_owned())
            .spawn(move || {
                let mut writer = BufWriter::new(file);
                while let Ok(message) = rx.recv() {
                    match message {
                        PerfMessage::Event(event) => {
                            let _ = writeln!(writer, "{}", event_to_json(event));
                        }
                        PerfMessage::Flush(ack) => {
                            let _ = writer.flush();
                            let _ = ack.send(());
                        }
                    }
                }
            })
            .ok()?;
        Some(PerfLogger { tx })
    }

    fn event_to_json(event: PerfEvent) -> Value {
        let mut object = Map::new();
        object.insert("ts_ms".to_owned(), json!(event.ts_ms));
        object.insert("event".to_owned(), json!(event.event));
        object.insert(
            "duration_ms".to_owned(),
            json!(event.duration.as_secs_f64() * 1000.0),
        );
        object.insert("duration_us".to_owned(), json!(event.duration.as_micros()));
        for field in event.fields {
            let (name, value) = field_to_json(field);
            object.insert(name.to_owned(), value);
        }
        Value::Object(object)
    }

    fn field_to_json(field: PerfField) -> (&'static str, Value) {
        match field {
            PerfField::Bool(name, value) => (name, json!(value)),
            PerfField::Str(name, value) => (name, json!(value)),
            PerfField::U32(name, value) => (name, json!(value)),
            PerfField::Usize(name, value) => (name, json!(value)),
        }
    }

    fn unix_ms_now() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    }
}

#[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
mod imp {
    use super::PerfField;
    use std::time::Duration;

    pub(super) fn record_duration(
        _event: &'static str,
        _duration: Duration,
        _fields: &[PerfField],
    ) {
    }

    pub(super) fn flush_timeout(_timeout: Duration) -> bool {
        true
    }
}
