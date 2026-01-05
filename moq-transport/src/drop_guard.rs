//! Global drop depth tracking to debug stack overflow issues.
//!
//! This module provides a thread-local counter to track drop depth across
//! all custom Drop implementations. This helps identify infinite recursion
//! in drop chains.

use std::cell::Cell;

thread_local! {
    /// Global drop depth counter - shared across all drop implementations
    static GLOBAL_DROP_DEPTH: Cell<usize> = const { Cell::new(0) };

    /// Maximum depth seen (for logging)
    static MAX_DROP_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Maximum allowed drop depth before we skip drop logic
pub const MAX_DROP_DEPTH_LIMIT: usize = 50;

/// Enter a drop context, returns the current depth (before incrementing).
/// If depth exceeds limit, returns None indicating drop should be skipped.
pub fn enter_drop(location: &str) -> Option<usize> {
    GLOBAL_DROP_DEPTH.with(|d| {
        let current = d.get();
        let new_depth = current + 1;
        d.set(new_depth);

        // Track max depth for debugging
        MAX_DROP_DEPTH.with(|max| {
            if new_depth > max.get() {
                max.set(new_depth);
                if new_depth > 10 && new_depth % 10 == 0 {
                    log::warn!("drop_guard: new max depth {} at {}", new_depth, location);
                }
            }
        });

        if new_depth > MAX_DROP_DEPTH_LIMIT {
            log::error!(
                "drop_guard: DEPTH EXCEEDED at {} (depth={}), skipping drop logic",
                location,
                new_depth
            );
            None
        } else {
            if new_depth > 20 {
                log::warn!("drop_guard: deep drop at {} (depth={})", location, new_depth);
            }
            Some(current)
        }
    })
}

/// Exit a drop context - should be called at end of drop
pub fn exit_drop(saved_depth: usize) {
    GLOBAL_DROP_DEPTH.with(|d| {
        d.set(saved_depth);
    });
}

/// Get current drop depth without modifying
pub fn current_depth() -> usize {
    GLOBAL_DROP_DEPTH.with(|d| d.get())
}

/// Reset depth (useful for tests)
pub fn reset_depth() {
    GLOBAL_DROP_DEPTH.with(|d| d.set(0));
    MAX_DROP_DEPTH.with(|d| d.set(0));
}
