//! Count an intermediate once for its lifetime, independently of cache and
//! draw-state references. This estimates owned texture payload, not driver RAM.
use std::sync::atomic::{AtomicUsize, Ordering};

static INTERMEDIATE_BYTES: AtomicUsize = AtomicUsize::new(0);

pub(super) fn intermediate_bytes() -> usize {
    INTERMEDIATE_BYTES.load(Ordering::Relaxed)
}

pub(super) struct TextureAllocation {
    counter: &'static AtomicUsize,
    bytes: usize,
}

impl TextureAllocation {
    pub(super) fn new(bytes: usize) -> Self {
        Self::with_counter(&INTERMEDIATE_BYTES, bytes)
    }

    fn with_counter(counter: &'static AtomicUsize, bytes: usize) -> Self {
        counter.fetch_add(bytes, Ordering::Relaxed);
        Self { counter, bytes }
    }
}

impl Drop for TextureAllocation {
    fn drop(&mut self) {
        self.counter.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn shared_draw_states_count_one_allocation_until_last_owner_drops() {
        static BYTES: AtomicUsize = AtomicUsize::new(0);
        let pool = Arc::new(TextureAllocation::with_counter(&BYTES, 4096));
        let first_draw = pool.clone();
        let second_draw = pool.clone();
        assert_eq!(BYTES.load(Ordering::Relaxed), 4096);
        drop(pool);
        drop(first_draw);
        assert_eq!(BYTES.load(Ordering::Relaxed), 4096);
        drop(second_draw);
        assert_eq!(BYTES.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn replacement_and_retired_pinned_allocation_are_both_counted() {
        static BYTES: AtomicUsize = AtomicUsize::new(0);
        let old_draw = TextureAllocation::with_counter(&BYTES, 4096);
        let replacement = TextureAllocation::with_counter(&BYTES, 8192);
        assert_eq!(BYTES.load(Ordering::Relaxed), 12288);
        drop(old_draw);
        assert_eq!(BYTES.load(Ordering::Relaxed), 8192);
        drop(replacement);
        assert_eq!(BYTES.load(Ordering::Relaxed), 0);
    }
}
