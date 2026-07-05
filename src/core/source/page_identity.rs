use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Stable per-book-session page identity, interned from the page's relative
/// name. Ids are monotonic and never reused within a session, so identity keys
/// remain unambiguous across snapshot refreshes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PageId(pub u32);

struct Inner {
    by_name: HashMap<String, PageId>,
    next: u32,
}

/// Append-only name -> id map shared across the refreshable snapshots of one
/// book session.
pub struct PageIdInterner {
    inner: Mutex<Inner>,
}

impl PageIdInterner {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                by_name: HashMap::new(),
                next: 0,
            }),
        }
    }

    /// Same name -> same id (append-only); unseen name -> next monotonic id.
    pub fn intern(&self, name: &str) -> PageId {
        let mut inner = self.inner.lock().expect("page id interner poisoned");
        if let Some(&id) = inner.by_name.get(name) {
            return id;
        }
        let id = PageId(inner.next);
        inner.next += 1;
        inner.by_name.insert(name.to_owned(), id);
        id
    }
}

impl Default for PageIdInterner {
    fn default() -> Self {
        Self::new()
    }
}

/// Process-global monotonic source instance counter (each constructed source
/// snapshot gets a fresh value; used by the worker epoch to detect swaps).
pub fn next_source_instance_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    // fetch_add returns the previous value, so +1 keeps 0 as the "unset" default.
    COUNTER.fetch_add(1, Ordering::Relaxed) + 1
}

#[cfg(test)]
mod tests;
