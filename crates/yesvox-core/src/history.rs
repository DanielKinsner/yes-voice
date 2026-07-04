//! A bounded undo/redo history over value snapshots (#25, G18).
//!
//! Generic over the snapshot type `T` so it is tested without a `Session`. The
//! GUI parameterizes it with a settings snapshot ([`crate::session::SessionSnapshot`])
//! and records one after each settings edit. Semantics:
//!
//! - **coalescing** — consecutive edits carrying the *same* coalesce key (a
//!   control identity) merge into one undo step, so dragging a knob is a single
//!   undoable change rather than one per frame;
//! - **redo truncation** — any new record after an undo drops the redo branch;
//! - **bounded** — at most `cap` undo steps; the oldest is evicted past the cap.

/// A bounded undo/redo stack. `current()` is the present value; [`undo`] and
/// [`redo`] walk the history.
#[derive(Clone, Debug)]
pub struct History<T> {
    past: Vec<T>,
    future: Vec<T>,
    current: T,
    cap: usize,
    last_key: Option<String>,
}

impl<T: Clone> History<T> {
    /// A history seeded with `initial` and room for `cap` undo steps (`cap` is
    /// clamped to at least 1).
    pub fn new(initial: T, cap: usize) -> Self {
        History {
            past: Vec::new(),
            future: Vec::new(),
            current: initial,
            cap: cap.max(1),
            last_key: None,
        }
    }

    /// The present value.
    pub fn current(&self) -> &T {
        &self.current
    }

    /// Whether an undo step is available.
    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    /// Whether a redo step is available.
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// Number of undo steps currently held (post-cap).
    pub fn undo_depth(&self) -> usize {
        self.past.len()
    }

    /// Number of redo steps currently held.
    pub fn redo_depth(&self) -> usize {
        self.future.len()
    }

    /// Record `next` as the new present. If `coalesce` is `Some` and equals the
    /// previous record's key, the change merges into the current undo step (no
    /// new step); otherwise a new undo step is pushed. Either way the redo branch
    /// is dropped and the stack is capped (oldest evicted).
    pub fn record(&mut self, next: T, coalesce: Option<&str>) {
        self.future.clear();
        let merges =
            coalesce.is_some() && coalesce == self.last_key.as_deref() && !self.past.is_empty();
        if merges {
            // Same control, still in the same edit — just advance the present.
            self.current = next;
        } else {
            self.past.push(std::mem::replace(&mut self.current, next));
            if self.past.len() > self.cap {
                self.past.remove(0); // evict the oldest undo step
            }
        }
        self.last_key = coalesce.map(str::to_owned);
    }

    /// Step back one undo entry, returning the restored present (or `None` if at
    /// the bottom). A subsequent [`record`] will not coalesce across the undo.
    pub fn undo(&mut self) -> Option<&T> {
        let prev = self.past.pop()?;
        self.future.push(std::mem::replace(&mut self.current, prev));
        self.last_key = None;
        Some(&self.current)
    }

    /// Step forward one redo entry, returning the restored present (or `None`).
    pub fn redo(&mut self) -> Option<&T> {
        let next = self.future.pop()?;
        self.past.push(std::mem::replace(&mut self.current, next));
        self.last_key = None;
        Some(&self.current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn undo_and_redo_walk_distinct_states() {
        let mut h = History::new(0i32, 100);
        h.record(1, None);
        h.record(2, None);
        assert_eq!(*h.current(), 2);
        assert_eq!(h.undo_depth(), 2);
        assert_eq!(*h.undo().unwrap(), 1);
        assert_eq!(*h.undo().unwrap(), 0);
        assert!(h.undo().is_none(), "at the bottom");
        assert_eq!(*h.redo().unwrap(), 1);
        assert_eq!(*h.redo().unwrap(), 2);
        assert!(h.redo().is_none(), "at the top");
    }

    #[test]
    fn same_key_edits_coalesce_into_one_step() {
        let mut h = History::new(0i32, 100);
        h.record(1, Some("gain"));
        h.record(2, Some("gain"));
        h.record(3, Some("gain"));
        assert_eq!(*h.current(), 3);
        // One undo step spanning the whole drag, back to the pre-drag value.
        assert_eq!(h.undo_depth(), 1);
        assert_eq!(*h.undo().unwrap(), 0);
    }

    #[test]
    fn different_keys_do_not_coalesce() {
        let mut h = History::new(0i32, 100);
        h.record(1, Some("gain"));
        h.record(2, Some("tilt"));
        assert_eq!(h.undo_depth(), 2);
    }

    #[test]
    fn no_coalesce_across_an_undo() {
        let mut h = History::new(0i32, 100);
        h.record(1, Some("gain"));
        h.undo();
        // Re-editing the same control after an undo starts a fresh step.
        h.record(5, Some("gain"));
        assert_eq!(h.undo_depth(), 1);
        assert_eq!(*h.undo().unwrap(), 0);
    }

    #[test]
    fn a_new_edit_after_undo_truncates_redo() {
        let mut h = History::new(0i32, 100);
        h.record(1, None);
        h.record(2, None);
        h.undo(); // current = 1, redo has [2]
        assert!(h.can_redo());
        h.record(9, None); // new branch
        assert!(!h.can_redo(), "redo branch dropped");
        assert_eq!(*h.current(), 9);
    }

    #[test]
    fn cap_evicts_the_oldest_without_panic() {
        let mut h = History::new(0i32, 3);
        for v in 1..=10 {
            h.record(v, None);
        }
        assert_eq!(h.undo_depth(), 3, "capped at 3 undo steps");
        // Undo to the bottom of what remains; the true origin (0) was evicted.
        while h.undo().is_some() {}
        assert_eq!(*h.current(), 7, "oldest surviving state");
    }

    proptest! {
        #[test]
        fn undo_to_bottom_restores_initial_redo_to_top_restores_final(
            ops in prop::collection::vec((any::<i32>(), prop::option::of("[a-c]")), 0..50)
        ) {
            let initial = 12345i32;
            // Cap comfortably above the op count so nothing is evicted.
            let mut h = History::new(initial, 1000);
            for (v, key) in &ops {
                h.record(*v, key.as_deref());
            }
            let final_state = *h.current();
            while h.undo().is_some() {}
            prop_assert_eq!(*h.current(), initial, "undo-to-bottom restores initial");
            while h.redo().is_some() {}
            prop_assert_eq!(*h.current(), final_state, "redo-to-top restores final");
        }
    }
}
