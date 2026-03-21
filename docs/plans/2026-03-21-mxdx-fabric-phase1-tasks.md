# mxdx-fabric Phase 1 — Task Plan

**Goal:** Types + coordinator skeleton. No failure policy yet, no jcode adapter. Just enough to prove the claim race and routing loop work end-to-end.

**Repo:** `~/workspace/mxdx` (actually `/home/openclaw/.openclaw/workspace/mxdx`)
**New crate:** `crates/mxdx-fabric`
**Design doc:** `docs/plans/2026-03-21-mxdx-fabric-design.md`

---

## Task 1 — New event types in `mxdx-types`

Add to `crates/mxdx-types/src/events/`:
- `fabric.rs` with: `TaskEvent`, `CapabilityEvent`, `ClaimEvent`, `HeartbeatEvent`, `TaskResultEvent`, `FailurePolicy` enum, `RoutingMode` enum, `TaskStatus` enum
- Export from `events/mod.rs`
- Unit tests: JSON round-trip for each type

Commit: `feat(types): add mxdx-fabric event types`

---

## Task 2 — `mxdx-fabric` crate scaffold

Create `crates/mxdx-fabric/` with:
- `Cargo.toml` (deps: mxdx-types, mxdx-matrix, tokio, serde, serde_json, anyhow, uuid, tracing)
- `src/lib.rs` — re-exports
- `src/coordinator.rs` — stub with `CoordinatorBot` struct, `run()` async method (just logs for now)
- `src/worker.rs` — stub with `WorkerClient` struct
- `src/sender.rs` — stub with `SenderClient` struct
- `src/capability_index.rs` — `CapabilityIndex` struct, `find_room()` method
- `src/claim.rs` — `ClaimRace` struct
- Add `mxdx-fabric` to workspace `Cargo.toml` members

Commit: `feat(fabric): scaffold mxdx-fabric crate`

---

## Task 3 — Capability index + room creation

In `capability_index.rs`:
- `CapabilityIndex::new(matrix_client)` 
- `find_room(required_caps: &[String]) -> Option<OwnedRoomId>` — looks up existing room
- `get_or_create_room(required_caps: &[String]) -> Result<OwnedRoomId>` — creates if not found
- Room naming: `#workers.{sorted_caps_joined_by_dot}:homeserver`
- Store capability→room map in memory, populated from Matrix room state events on startup
- Unit tests: room name generation from capability lists

Commit: `feat(fabric): capability index with room creation`

---

## Task 4 — Coordinator routing loop

In `coordinator.rs`:
- `CoordinatorBot::run()` — async event loop watching the coordinator room
- On `TaskEvent`: call `capability_index.get_or_create_room()`, route based on `RoutingMode`
  - `Direct`: invite sender to worker room
  - `Brokered`: post task to worker room on sender's behalf
  - `Auto`: if `timeout_seconds < 30` → direct, else → brokered
- Watchlist: `HashMap<String, WatchEntry>` tracking task UUID → claim state + last heartbeat
- On `ClaimEvent`: update watchlist
- On `HeartbeatEvent`: update last_heartbeat
- On `TaskResultEvent`: remove from watchlist
- Periodic check (every 10s): log warnings for unclaimed/stale tasks (no enforcement yet — v1 backstop is just logging)

Commit: `feat(fabric): coordinator routing loop`

---

## Task 5 — Worker claim race

In `claim.rs` and `worker.rs`:
- `WorkerClient::advertise_capabilities(caps: &[String], room_id: &RoomId)` — posts `CapabilityEvent` state event
- `WorkerClient::watch_and_claim(room_id: &RoomId, my_caps: &[String])` — watches room for `TaskEvent`, attempts claim
- Claim: post `ClaimEvent` as state event with key `task/{uuid}/claim`
- Verify claim: sync, re-read state event, confirm `worker_id == self.worker_id` (else back off)
- `WorkerClient::post_heartbeat(task_uuid, progress)` — posts `HeartbeatEvent`
- `WorkerClient::post_result(task_uuid, status, output)` — posts `TaskResultEvent`

Commit: `feat(fabric): worker capability advertisement and claim race`

---

## Task 6 — Integration test

In `crates/mxdx-fabric/tests/e2e_fabric.rs`:
- Test against local Tuwunel (same pattern as existing `e2e_full_system.rs`)
- Scenario: sender posts task → coordinator routes → worker claims → worker posts heartbeat → worker posts result → sender receives result
- Assert: claim state event is correct, only one worker claims (test with two competing workers)
- Assert: coordinator watchlist is clean after result

Commit: `test(fabric): Phase 1 E2E integration test`

---

## Notes for jcode

- Read `docs/plans/2026-03-21-mxdx-fabric-design.md` before starting each task — it has all the type definitions
- Follow existing code patterns in `crates/mxdx-types/src/events/command.rs` for event type style
- Follow existing test patterns in `crates/mxdx-matrix/tests/` for integration test style
- Commit after each task — don't batch
- Do NOT start on the next task — each jcode run is one task only
