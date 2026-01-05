//! Global drop depth tracking to debug stack overflow issues.

use std::cell::Cell;

thread_local! {
    static GLOBAL_DROP_DEPTH: Cell<usize> = const { Cell::new(0) };
    static MAX_DROP_DEPTH: Cell<usize> = const { Cell::new(0) };
}

pub const MAX_DROP_DEPTH_LIMIT: usize = 50;

pub fn enter_drop(location: &str) -> Option<usize> {
    GLOBAL_DROP_DEPTH.with(|d| {
        let current = d.get();
        let new_depth = current + 1;
        d.set(new_depth);

        MAX_DROP_DEPTH.with(|max| {
            if new_depth > max.get() {
                max.set(new_depth);
                if new_depth > 10 && new_depth % 10 == 0 {
                    log::warn!("ws drop_guard: new max depth {} at {}", new_depth, location);
                }
            }
        });

        if new_depth > MAX_DROP_DEPTH_LIMIT {
            log::error!(
                "ws drop_guard: DEPTH EXCEEDED at {} (depth={}), skipping drop logic",
                location,
                new_depth
            );
            None
        } else {
            if new_depth > 20 {
                log::warn!("ws drop_guard: deep drop at {} (depth={})", location, new_depth);
            }
            Some(current)
        }
    })
}

pub fn exit_drop(saved_depth: usize) {
    GLOBAL_DROP_DEPTH.with(|d| {
        d.set(saved_depth);
    });
}
