# ADR 0006: OpenClaw Fabric Callback Plugin

**Date:** 2026-03-22
**Status:** Accepted

## Context

When a task is posted to the mxdx-fabric coordinator via `fabric post`, the result needs to route back to the originating session or thread. Two related gaps exist:

1. **No threading.** The current worker posts heartbeat and result events as plain room messages. There is no `in_reply_to`/`relates_to` support in `mxdx-matrix`, so output cannot be threaded onto the task event. Threading is the right model: the task event is the anchor, all worker output (progress chunks + final result) threads onto it, and the thread is durable in Matrix history.

2. **No callback routing.** Even with threading, OpenClaw has no mechanism to know which Discord thread or session to relay results to. The routing info must come from the task event itself (authored by OpenClaw), not from the worker.

## Decision

### Part 1: Matrix Threading in the Worker

Add `send_threaded_event(room_id, in_reply_to_event_id, payload)` to `mxdx-matrix`. This sends a Matrix message with `m.relates_to` set to thread onto the given event ID.

The worker uses this for all output:

- **Heartbeat chunks** — each batch of stdout is threaded onto the task event (replacing plain room heartbeat events)
- **Final result** — the completion event (`org.mxdx.fabric.result`) is also threaded onto the task event

The task event becomes the durable anchor for the full execution transcript. Anyone watching the thread sees the complete run history, regardless of when they connect.

Workers receive the task event ID when they claim a task (it's the Matrix event ID of the `org.mxdx.fabric.task` event in the coordinator room). This is already available via the Matrix sync event envelope.

### Part 2: Trust Model for Callbacks

OpenClaw embeds `_callback` in the `org.mxdx.fabric.task` event at post time. This event is authored by OpenClaw's sender identity and is immutable in Matrix once posted.

On result, the plugin correlates `task_uuid` → `coordinator_event_id` from its own outbox, fetches the original task event, and reads `_callback` from it. The worker's output is trusted only for content — never for routing.

Workers MUST ignore unknown payload fields. Workers MUST NOT copy, modify, or echo `_callback` into any output event.

### Part 3: Outbox State Event

Each sender identity maintains a single Matrix state event in its private outbox room:

- **Event type:** `org.mxdx.fabric.outbox`
- **State key:** `""` (singleton per room, overwritten on each change)
- **Content:**
```json
{
  "updated_at": 1774166300123,
  "entries": {
    "{task_uuid}": {
      "coordinator_event_id": "$abc123:ca1-beta.mxdx.dev",
      "callback": {
        "channel": "discord",
        "thread_id": "1485151340614254673",
        "reply_to_message_id": "1485370710770712657"
      },
      "posted_at": 1774166000000,
      "timeout_secs": 1800
    }
  }
}
```

`updated_at` is a **client-authored millisecond timestamp**, not the server's event timestamp. This is the conflict resolution key.

### Part 4: Private Outbox Room

Each sender identity gets a dedicated private room:

- **Name:** deterministic — `openclaw-fabric-{sha256(user_id)[:12]}`
- **Access:** invite-only, only the sender identity is a member
- **Creation:** plugin creates it on first run if absent; joins it on subsequent starts

The deterministic name means the plugin can locate its room after restart without any external state.

### Part 5: Multi-Identity Write Protocol

When the outbox changes (entry added or removed):

1. Compute new state with a fresh `updated_at = Date.now()` (milliseconds)
2. Write state event to identity-1's outbox room, await server ack
3. Call `Date.now()` again for identity-2 — guaranteed distinct millisecond — write, await ack
4. Continue for further identities

### Part 6: Conflict Resolution

On startup (or any time multiple identity rooms are readable):

1. Read `org.mxdx.fabric.outbox` state event from all identity rooms
2. Compare `updated_at` fields (client-authored)
3. Take the entry set from the event with the highest `updated_at` as truth
4. Rewrite the canonical state to any rooms that were behind

### Part 7: Expiry

When writing or reading the outbox, remove entries where:

```
now_ms > posted_at + max(7 * 24 * 60 * 60 * 1000, timeout_secs * 5 * 1000)
```

That is: 7 days or 5× the task timeout, whichever is longer. Also remove any entry whose original coordinator event no longer exists (404 or not found on fetch).

### Part 8: Startup Sequence

1. Locate or create private outbox room for each configured sender identity
2. Read outbox state from all identity rooms; apply conflict resolution
3. Prune expired entries; remove entries whose coordinator events are gone (404)
4. Rewrite state to all identity rooms if anything was pruned
5. Subscribe to coordinator room for thread activity on known task event IDs
6. Backfill: fetch the Matrix thread for each outbox entry since `posted_at`; process any result events that arrived while offline

### Part 9: Result Handling

On thread activity on a known task event ID:

1. Check if the new threaded event is `org.mxdx.fabric.result` (final result marker)
2. If so: fetch original `org.mxdx.fabric.task` event by `coordinator_event_id`
3. Read `_callback` from the original task event
4. Route result to the specified channel/thread via OpenClaw's message API
5. Remove entry from outbox, rewrite state to all identity rooms

Intermediate heartbeat/progress thread events may optionally be relayed to the callback channel as live updates (future feature; not required for initial implementation).

### Part 10: Arbitrary Payload Fields

`fabric post` gains a `--payload-json` flag accepting arbitrary JSON merged into the task payload. The `_callback` field is one such field. Workers treat unknown payload fields as opaque and ignore them.

## Implementation Checklist

- [ ] `mxdx-matrix`: add `send_threaded_event(room_id, in_reply_to_event_id, payload)`
- [ ] `mxdx-fabric/worker`: use `send_threaded_event` for heartbeats and result (pass task event ID through from claim)
- [ ] `mxdx-fabric/worker`: remove plain-room heartbeat fallback once threading is in
- [ ] `mxdx-types`: ensure `TaskEvent` carries the Matrix event ID through to the worker (currently available via sync envelope — may need explicit plumbing)
- [ ] `mxdx-fabric/cli`: add `--payload-json` flag to `fabric post`
- [ ] OpenClaw plugin: outbox room create/join on startup
- [ ] OpenClaw plugin: state event read/write with conflict resolution
- [ ] OpenClaw plugin: subscribe to coordinator room, watch for thread activity on outbox entries
- [ ] OpenClaw plugin: backfill on startup
- [ ] OpenClaw plugin: route result to callback channel

## Consequences

**Positive:**
- The task event thread is the complete durable record of a task run — progress + result, readable at any time from Matrix history
- Trust boundary is at post time: routing info always comes from our own authored event
- Matrix is the persistence layer: no SQLite, no external state, survives restarts naturally
- Multi-identity redundancy: any surviving identity room is sufficient to recover pending callbacks
- Self-healing: expired and invalid entries pruned on every read/write cycle
- Conflict resolution is deterministic: highest `updated_at` wins
- Offline resilience: backfill on startup catches results that arrived while plugin was down

**Negative:**
- `mxdx-matrix` needs threading support before workers can use it (currently only has plain `send_event`)
- Workers need the task event ID plumbed through from claim to execution (currently available in the sync envelope but not explicitly carried)
- Fetching the original task event on each result adds one Matrix API call per completion
- If all identity outbox rooms are lost, pending callbacks are unrecoverable (acceptable: tasks are time-bounded)

## Out of Scope

- Live progress relay to Discord (heartbeat thread events → Discord messages) — design is ready, deferred to follow-on
- Cross-machine callback routing — callback routes to the posting OpenClaw instance only
- Callback authentication beyond task event authorship trust

## Related

- ADR-0005: Worker Capability Advertisement via Matrix State Events
- mxdx-fabric coordinator: `org.mxdx.fabric.task`, `org.mxdx.fabric.heartbeat`, `org.mxdx.fabric.result` event schemas
