//! Byte admission follows a result until the UI accepts or discards it, including
//! time spent in the UI's deferred queue. Waiting never hides worker commands.
use super::{WorkerCommand, WorkerEvent};
use crossbeam_channel::{bounded, select, unbounded, Receiver, Sender};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const MAX_PENDING_EVENTS: usize = 64;

#[derive(Default)]
pub(super) struct DeliveryUsage {
    bytes: AtomicUsize,
    events: AtomicUsize,
}

impl DeliveryUsage {
    pub(super) fn bytes(&self) -> usize {
        self.bytes.load(Ordering::Acquire)
    }
}

struct DeliveryReceipt {
    bytes: usize,
    usage: Arc<DeliveryUsage>,
    available: Sender<()>,
}

impl Drop for DeliveryReceipt {
    fn drop(&mut self) {
        self.usage.bytes.fetch_sub(self.bytes, Ordering::AcqRel);
        self.usage.events.fetch_sub(1, Ordering::AcqRel);
        let _ = self.available.try_send(());
    }
}

pub struct PendingWorkerEvent {
    event: WorkerEvent,
    _receipt: DeliveryReceipt,
}

impl PendingWorkerEvent {
    pub fn event(&self) -> &WorkerEvent {
        &self.event
    }

    /// The caller now owns processing/retention; deferred events should keep the
    /// envelope instead, so their pixels remain subject to delivery admission.
    pub fn into_event(self) -> WorkerEvent {
        self.event
    }
}

pub(super) struct EventSender {
    events: Sender<PendingWorkerEvent>,
    available_tx: Sender<()>,
    available_rx: Receiver<()>,
    pub(super) usage: Arc<DeliveryUsage>,
}

pub(super) enum DeliveryOutcome {
    Sent,
    Interrupted(WorkerCommand, WorkerEvent),
    Disconnected,
}

pub(super) fn event_channel() -> (EventSender, Receiver<PendingWorkerEvent>) {
    let (events, receiver) = unbounded();
    let (available_tx, available_rx) = bounded(1);
    (
        EventSender {
            events,
            available_tx,
            available_rx,
            usage: Arc::default(),
        },
        receiver,
    )
}

impl EventSender {
    pub(super) fn send(
        &self,
        event: WorkerEvent,
        budget_bytes: usize,
        commands: &Receiver<WorkerCommand>,
    ) -> DeliveryOutcome {
        let bytes = match &event {
            WorkerEvent::PageReady { page, .. } => page.byte_size,
            WorkerEvent::PageFailed { .. } => 0,
        };
        loop {
            let count = self.usage.events.load(Ordering::Acquire);
            let fits = self.usage.bytes().saturating_add(bytes) <= budget_bytes;
            // A source-pixel page may exceed the soft budget on its own. Admit
            // one such result to an empty queue; never accumulate a second one.
            if count < MAX_PENDING_EVENTS && (fits || count == 0) {
                self.usage.bytes.fetch_add(bytes, Ordering::AcqRel);
                self.usage.events.fetch_add(1, Ordering::AcqRel);
                let envelope = PendingWorkerEvent {
                    event,
                    _receipt: DeliveryReceipt {
                        bytes,
                        usage: self.usage.clone(),
                        available: self.available_tx.clone(),
                    },
                };
                return if self.events.send(envelope).is_ok() {
                    DeliveryOutcome::Sent
                } else {
                    DeliveryOutcome::Disconnected
                };
            }
            select! {
                recv(commands) -> command => return match command {
                    Ok(command) => DeliveryOutcome::Interrupted(command, event),
                    Err(_) => DeliveryOutcome::Disconnected,
                },
                recv(self.available_rx) -> _ => {},
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::source::PageId;
    use crate::core::worker::{DecodeBackend, DecodeOptions, PagePixels, PreparedPage};
    use std::thread;

    fn event() -> WorkerEvent {
        WorkerEvent::PageReady {
            book_id: "synthetic".to_owned(),
            source_instance_id: 1,
            page_id: PageId(0),
            decode: DecodeOptions::default(),
            page: Arc::new(PreparedPage {
                pixels: PagePixels::Rgba(vec![0; 4].into()),
                original_width: 1,
                original_height: 1,
                display_width: 1,
                display_height: 1,
                byte_size: 4,
                target_long_edge: 1024,
                decode_backend: DecodeBackend::ImageCrate,
                notice: None,
            }),
        }
    }

    #[test]
    fn full_or_single_oversize_delivery_still_accepts_shutdown() {
        for budget in [2, 4] {
            let (sender, receiver) = event_channel();
            let usage = sender.usage.clone();
            let (command_tx, command_rx) = unbounded();
            assert!(matches!(
                sender.send(event(), budget, &command_rx),
                DeliveryOutcome::Sent
            ));
            assert_eq!(usage.bytes(), 4);
            // The first envelope remains queued, so the next send cannot admit.
            let handle = thread::spawn(move || sender.send(event(), budget, &command_rx));
            command_tx.send(WorkerCommand::Shutdown).unwrap();
            assert!(matches!(
                handle.join().unwrap(),
                DeliveryOutcome::Interrupted(WorkerCommand::Shutdown, _)
            ));
            let deferred = receiver.try_recv().unwrap();
            assert_eq!(usage.bytes(), 4);
            drop(deferred);
            assert_eq!(usage.bytes(), 0);
        }
    }

    #[test]
    fn accepting_a_deferred_result_wakes_blocked_delivery() {
        let (sender, receiver) = event_channel();
        let (command_tx, command_rx) = unbounded();
        assert!(matches!(
            sender.send(event(), 4, &command_rx),
            DeliveryOutcome::Sent
        ));
        let deferred = receiver.try_recv().unwrap();
        let handle = thread::spawn(move || sender.send(event(), 4, &command_rx));
        drop(deferred.into_event());
        assert!(matches!(handle.join().unwrap(), DeliveryOutcome::Sent));
        assert!(receiver.try_recv().is_ok());
        drop(command_tx);
    }
}
