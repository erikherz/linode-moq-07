# WebSocket Session Stack Overflow Debug Notes

## Problem Summary

The moq-relay-ietf server crashes with a stack overflow when:
1. iPhone broadcasts video via WebSocket to the relay
2. Safari (PC) views the stream via WebSocket
3. Safari window is closed (WebSocket connection terminates)
4. Server crashes with: `thread 'tokio-runtime-worker' has overflowed its stack`

**Note**: QUIC connections work fine. The issue is specific to WebSocket transport.

## Reproduction Steps

1. Start relay: `moq-relay-ietf --bind 0.0.0.0:443 --tls-cert ... --tls-key ... --announce https://relay.cloudflare.mediaoverquic.com`
2. iPhone connects via WebSocket and starts broadcasting
3. Safari connects via WebSocket and subscribes to the stream
4. Close the Safari browser tab
5. Server crashes within seconds

## Key Observations

- Crash happens on `tokio-runtime-worker` thread, not a custom thread
- Crash occurs AFTER all "failed to serve subgroup" warnings (tasks complete successfully before crash)
- **Global drop guards we added produced NO output** before crash
- This proves the infinite recursion is in **compiler-generated async state machine drops**, not our custom `Drop` implementations

## Architecture Overview

```
Safari WebSocket → WsSession (web-transport-ws) → moq-transport Session → Subscribed
                                                                            ↓
                                                                      serve_subgroups
                                                                            ↓
                                                              spawned tasks per subgroup
                                                                            ↓
                                                                    serve_subgroup
                                                                    (nested loops)
```

When Safari disconnects:
1. WebSocket closes
2. All channels return errors
3. Spawned subgroup tasks fail and log warnings
4. Tasks complete and their futures get dropped
5. **Stack overflow during future drop**

## Things Tried (All Failed)

### 1. Task Isolation with tokio::spawn
- Spawned session components as separate tasks
- Used AbortHandle coordination
- **Result**: Still crashed

### 2. Dedicated Thread with Large Stack (64MB)
- Ran WebSocket session on separate OS thread with 64MB stack
- **Result**: Still crashed - proving it's truly infinite recursion, not just deep stacks

### 3. Recursion Guards in Drop Implementations
- Added thread-local recursion guards to `Subscribed::drop`, `StateDrop::drop`, etc.
- **Result**: Guards never triggered, crash still occurred
- **Insight**: Recursion is not in our custom Drop impls

### 4. Global Drop Depth Tracking
- Created shared `drop_guard` module across all crates
- Instrumented ALL custom Drop implementations
- **Result**: No logs produced before crash
- **Insight**: Recursion is in compiler-generated async code

### 5. Boxing Futures (Box::pin)
- Boxed futures in select! blocks to move state machines to heap
- Converted `while let` loops to explicit `loop` with boxed futures
- Applied to: `serve_subgroups`, `serve_subgroup`, `serve_track`, `serve_datagrams`, `run_streams`, `run_datagrams`
- **Result**: Still crashed

### 6. spawn_blocking for Subgroup Tasks
- Changed from `tokio::spawn` to `tokio::task::spawn_blocking`
- Runs on blocking thread pool with larger stacks
- Uses `Handle::current().block_on()` to run async code
- **Result**: Still crashed

### 7. Removing task.abort() calls
- Let tasks fail naturally instead of aborting
- **Result**: Still crashed

### 8. Increased tokio worker thread stack (8MB)
- Set via `runtime::Builder::thread_stack_size(8 * 1024 * 1024)`
- **Result**: Still crashed (8MB is already quite large)

## Current State of Code

Key files modified:
- `moq-transport/src/drop_guard.rs` - Global drop depth tracking
- `moq-transport/src/session/subscribed.rs` - Boxed futures, spawn_blocking
- `moq-transport/src/session/mod.rs` - Boxed futures in run_streams/run_datagrams
- `moq-transport/src/watch/state.rs` - Drop guards
- `moq-relay-ietf/src/session.rs` - Spawned components as separate tasks
- `internal_crates/web-transport-ws/src/session.rs` - Drop guards on streams

## Hypotheses for Root Cause

### Hypothesis A: Deep Async State Machine in web-transport-ws
The `SessionState::run()` in `web-transport-ws/src/session.rs` has a complex select! with many branches. When dropped, this could create deep drop chains.

### Hypothesis B: mpsc Channel Drop Cascades
When Session drops, multiple mpsc senders/receivers drop. This might trigger wakers that cause cascading drops.

### Hypothesis C: tokio-tungstenite Internal State
The underlying `tokio_tungstenite::WebSocketStream` might have complex internal state that causes deep drops.

### Hypothesis D: Circular Arc References
Multiple `Arc<Mutex<...>>` might create a situation where dropping one triggers complex cleanup in others.

## Next Steps to Try

### 1. Use `stacker` crate
The `stacker` crate can dynamically grow the stack when needed:
```rust
stacker::maybe_grow(32 * 1024, 1024 * 1024, || {
    // code that might need more stack
});
```

### 2. Profile the Drop Chain
Add explicit `log::trace!()` calls at the START of every Drop implementation (before the guard check) to see exact order.

### 3. Simplify web-transport-ws SessionState::run()
Break up the complex select! block into smaller functions to reduce state machine depth.

### 4. Use ManuallyDrop + Explicit Cleanup
```rust
let session = ManuallyDrop::new(session);
// ... use session ...
// Explicitly drop on a thread with large stack
std::thread::Builder::new()
    .stack_size(64 * 1024 * 1024)
    .spawn(move || drop(ManuallyDrop::into_inner(session)))
    .unwrap()
    .join()
    .unwrap();
```

### 5. Get a Stack Trace
Run under gdb/lldb to capture the actual stack trace at crash:
```bash
gdb --args ./target/release/moq-relay-ietf ...
# (gdb) run
# When it crashes:
# (gdb) bt
```

### 6. Try a Simpler WebSocket Implementation
Consider if tokio-tungstenite is the issue - test with a different WebSocket library.

### 7. Limit Concurrent Subgroups
Instead of spawning unlimited subgroup tasks, use a semaphore to limit concurrency. This might reduce the cleanup storm.

### 8. Defer Cleanup
When connection closes, instead of immediately dropping everything, defer to a cleanup task:
```rust
let cleanup = || { /* drop all the things */ };
std::thread::Builder::new()
    .stack_size(128 * 1024 * 1024)
    .spawn(cleanup);
```

## Files to Focus On

1. `internal_crates/web-transport-ws/src/session.rs` - Session and SessionState
2. `moq-transport/src/session/subscribed.rs` - Subscribed and serve_* functions
3. `moq-transport/src/watch/state.rs` - State/StateDrop
4. `moq-relay-ietf/src/session.rs` - Relay Session wrapper

## Useful Commands

```bash
# Build
cargo build --release

# Run with debug logging
RUST_LOG=debug ./target/release/moq-relay-ietf ...

# Run under gdb
gdb --args ./target/release/moq-relay-ietf --bind 0.0.0.0:443 ...

# Check stack size of threads
cat /proc/<pid>/limits | grep stack
```

## Related Issues

This type of issue (stack overflow in async drops) has been seen in other Rust projects:
- https://github.com/tokio-rs/tokio/issues/4017
- https://github.com/rust-lang/rust/issues/97219

The fundamental issue is that Rust's async/await generates state machines that can be very deep when there are nested loops and select! blocks.
