# B04: Stream snapshot hangs above 64 features and loses updates made during the snapshot

| Field | Value |
|---|---|
| Type | Bug (availability, correctness) |
| Severity | Critical (hang), High (lost updates) |
| Status | Fixed on branch fix/b04-stream-snapshot-deadlock |
| Crate | `feature-toggle-backend` |
| Behavior change | Small: see "Behavior impact" |
| Related | [B03](B03-stream-updates-cross-team.md), [P02](P02-snapshot-n-plus-one-queries.md) |

## Problem

`stream_updates` in `feature-toggle-backend/src/grpc/mod.rs` does this in order:

1. Creates the outgoing channel: `mpsc::channel::<Result<pb::FeatureUpdate, Status>>(64)` (`:1245`).
2. Reads the snapshot features from the DB (`:1266-1298`).
3. Sends every snapshot feature with `out_tx.send(..).await` (`:1304-1316`). Each send also runs extra queries in `map_db_feature_to_full`.
4. Only then subscribes to the broadcast: `let mut rx = self.updates_tx.subscribe();` (`:1321`).
5. Spawns the forwarding task and finally returns `Response::new(ReceiverStream::new(out_rx))` (`:1427`).

### Defect 1: hang (deadlock) above 64 snapshot features

Nobody reads `out_rx` until the handler returns at step 5. The channel holds 64 messages. When the snapshot has more than 64 features, the 65th `send().await` waits forever:
- The handler never returns.
- The edge's 10 s request timeout (`feature-edge-server/src/grpc_client.rs:188`) fires.
- The edge reconnects and hits the same hang.

Any team with more than 64 flags (or a keys-filtered snapshot that matches more than 64 rows, see B01) never gets a working stream.

### Defect 2: updates made during the snapshot are lost

The subscription filter is registered early (`:1248-1252`), but that only adds a map entry. The broadcast receiver is created at step 4. Any update broadcast between the DB read in step 2 and step 4 is lost. There are no versions or replay.

## How it fails

- Team with 100 flags: the edge stream never connects. It loops through timeout and reconnect, and live updates never arrive.
- Team with 10 flags: an operator triggers a kill switch while the edge is reconnecting. The snapshot already holds the old state, and the update is broadcast before `subscribe()`. The edge keeps serving the flag as enabled until some later change touches it.

## Fix

1. Call `let mut rx = self.updates_tx.subscribe();` right after registering the subscription (about `:1253`), before reading the snapshot.
2. Move the snapshot read and send into the spawned task, before the forwarding loop, so the handler returns `Response` immediately and tonic starts draining `out_rx`.
   - Clone what the task needs (repositories are `Arc`/boxed; check `Send + 'static`). If `map_db_feature_to_full` borrows `&self`, extract the needed repository handles or wrap the service state in `Arc`.
   - On a snapshot DB error, send `Err(Status::internal(..))` on `out_tx` and end the task. Today the error is returned as the RPC status.
3. Replaying an Upsert that the snapshot already contains is harmless: Upserts carry full state and arrive in broadcast order.

## Behavior impact

- An edge may receive a duplicate Upsert right after the snapshot. This is harmless and idempotent on the edge.
- A very slow snapshot with more than 128 queued broadcast updates triggers the existing "lagged" path, and the edge does a full resync. That is correct.
- Snapshot DB failures arrive as a stream error item instead of a failed RPC status. The edge already treats both as "reconnect".

## Tests

In `feature-toggle-backend/tests/grpc_tests.rs`:
- **Hang:** make the mocked `get_features` return 100 features. Assert that the `stream_updates` call returns within 5 s and that 100 Snapshot messages arrive. This fails (times out) today.
- **Lost update:** inside the mock `expect_get_features().returning(..)` closure, call `tx_clone.send(Upsert { key: "x" })`. `broadcast::send` is synchronous, so this works there. Assert the stream later yields that Upsert. This fails today.

## Acceptance criteria

- Snapshots of any size complete.
- No update broadcast after the subscription starts is lost.
- `cargo test -p feature-toggle-backend --test grpc_tests` passes.
