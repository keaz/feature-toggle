# B09: Heartbeat task leaks on every stream reconnect

| Field | Value |
|---|---|
| Type | Bug (resource leak) |
| Severity | Medium |
| Status | Confirmed (code traced) |
| Crate | `feature-edge-server` |
| Behavior change | No |

## Problem

`run_stream_task` (`feature-edge-server/src/grpc_client/stream.rs:120`) spawns a heartbeat task on every connect (`:142`). The task loops forever and ignores send errors:

```rust
// feature-edge-server/src/grpc_client/stream.rs:46
fn spawn_heartbeat(tx: tokio::sync::mpsc::Sender<pb::StreamRequest>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            // ...
            let _ = tx.send(pb::StreamRequest { /* Heartbeat */ }).await;
        }
    });
}
```

When the stream ends, the receiver `rx` (owned by the streaming call) is dropped, and `tx.send` returns `Err`. The loop keeps running.

## How it fails

The backend restarts or the network flaps N times. N heartbeat tasks remain, each waking every 30 s forever, and each holds a `Sender` clone. Memory and wakeups grow without bound over the process lifetime.

## Fix

Exit the loop when the receiver is gone:

```rust
if tx.send(pb::StreamRequest { /* ... */ }).await.is_err() {
    break;
}
```

Optional: also return the `JoinHandle` and abort it when the stream loop iteration ends. With the `break`, that is not required.

## Tests

`#[tokio::test(start_paused = true)]`: create a channel, call `spawn_heartbeat(tx)`, drop `rx`, advance time by 31 s, and assert that the task finished. To make it observable, have `spawn_heartbeat` return the `JoinHandle` and assert `handle.is_finished()`.

## Acceptance criteria

- The heartbeat task ends after the stream receiver is dropped.
- `cargo test -p feature-edge-server` passes.
