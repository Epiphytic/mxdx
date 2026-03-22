# ADR 0007: Fabric Callback Routing via Matrix Outbox State

**Date:** 2026-03-22
**Status:** Accepted
**Supersedes:** ADR-0006 (OpenClaw Fabric Callback Plugin)

## Context

ADR-0006 proposed routing fabric task results back to OpenClaw via a SQLite-backed polling loop, with `_callback` routing info echoed from the worker's result event. This design has two fundamental problems:

1. **Wrong trust boundary.** Routing info sourced from the worker's result event means a compromised or misbehaving worker can redirect callbacks anywhere. The authoritative routing info must come from the task event *we* posted.
2. **Wrong persistence layer.** SQLite is an external dependency that doesn't survive cleanly across restarts, doesn't replicate across identities, and creates a state management burden outside the protocol.

The correct design uses Matrix itself as both the trust anchor and the persistence layer.

## Decision

### Trust Model

OpenClaw embeds `_callback` in the `org.mxdx.fabric.task` event at post time. This event is authored by OpenClaw's sender identity and lives in the coordinator room permanently.

On result, the plugin correlates `task_uuid` → `coordinator_event_id` (which it stored itself), fetches the original task event, and reads `_callback` from it. The worker's result event is trusted only for task output (status, stdout, duration) — never for routing.

A worker cannot redirect a callback. It doesn't touch routing info.

### Outbox State Event

Each sender identity maintains a single Matrix state event in its private outbox room:

- **Event type:** `org.mxdx.fabric.outbox`
- **State key:** `""` (singleton per room)
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

### Private Outbox Room

Each sender identity gets a dedicated private room:

- **Name:** deterministic from identity — `openclaw-fabric-{sha256(user_id)[:12]}`
- **Access:** invite-only, only the sender identity is a member
- **Creation:** plugin creates it on first run if it doesn't exist; joins it on subsequent starts

The room name is deterministic so the plugin can locate it after a restart without storing external state.

### Multi-Identity Write Protocol

When the outbox changes (entry added or removed):

1. Compute new state (with fresh `updated_at` millisecond timestamp)
2. Write state event to identity-1's outbox room, await server ack
3. Write state event to identity-2's outbox room (new `updated_at` call — guaranteed different ms)
4. Continue for further identities

No artificial delay needed: each `Date.now()` call produces a distinct millisecond timestamp.

### Conflict Resolution

On startup (or any time multiple identity rooms are readable):

1. Read `org.mxdx.fabric.outbox` state event from all identity rooms
2. Compare `updated_at` fields (client-authored)
3. Take the entry set from the event with the highest `updated_at` as truth
4. Reconcile: rewrite the canonical state to any rooms that were behind

### Expiry

When writing or reading the outbox, remove entries where:

```
now_ms > posted_at + max(7 * 24 * 60 * 60 * 1000, timeout_secs * 5 * 1000)
```

That is: 7 days or 5× the task timeout, whichever is longer. If the original coordinator event no longer exists (fetch returns 404 or event not found), also remove that entry.

### Startup Sequence

1. Locate or create private outbox room for each configured sender identity
2. Read outbox state from all identity rooms; apply conflict resolution
3. Prune expired entries; remove entries whose coordinator events are gone (404)
4. Rewrite state to all identity rooms if anything was pruned
5. Subscribe to coordinator room for `org.mxdx.fabric.result` events
6. Backfill: scan coordinator room history since `min(posted_at)` across all outbox entries for any results that arrived while offline; process matches

### Result Handling

On `org.mxdx.fabric.result` event in coordinator room:

1. Extract `task_uuid`
2. Lookup in in-memory outbox (populated from state on startup)
3. If found: fetch original `org.mxdx.fabric.task` event by `coordinator_event_id`
4. Read `_callback` from the original task event
5. Route result to the specified channel/thread via OpenClaw's message API
6. Remove entry from outbox, rewrite state to all identity rooms

### Arbitrary Payload Fields

`fabric post` gains a `--payload-json` flag accepting arbitrary JSON merged into the task payload. The `_callback` field is one such field. Workers treat unknown payload fields as opaque and ignore them.

## Consequences

**Positive:**
- Trust boundary is at post time: routing info is always from our own authored event
- Matrix is the persistence layer: no SQLite, no external state, survives restarts naturally
- Multi-identity redundancy: outbox is replicated across all sender identities; any one surviving is sufficient
- Self-healing: expired and invalid entries are pruned on every read/write cycle
- Conflict resolution is deterministic: highest client timestamp wins
- Offline resilience: backfill on startup catches results that arrived while the plugin was down

**Negative:**
- Plugin requires Matrix read access to the coordinator room's history (for backfill)
- Fetching the original task event on each result adds one Matrix API call per completion
- If all identity rooms are lost, pending callbacks are unrecoverable (acceptable: tasks are time-bounded anyway)

## Payload Schema Addition (mxdx)

`org.mxdx.fabric.task` event content gains an optional `_callback` field:

```json
{
  "task_uuid": "...",
  "prompt": "...",
  "capabilities": ["rust"],
  "cwd": "/path/to/repo",
  "timeout_secs": 1800,
  "_callback": {
    "channel": "discord",
    "thread_id": "...",
    "reply_to_message_id": "..."
  }
}
```

Workers MUST ignore unknown fields. Workers MUST NOT copy, modify, or echo `_callback` or any other unknown field into the result event.

## Out of Scope

- Cross-machine callback routing (callback routes to the posting OpenClaw instance only)
- Streaming/heartbeat callbacks (result event only; heartbeat events are separate)
- Callback authentication beyond the task event authorship trust model

## Related

- ADR-0006: Superseded by this document
- ADR-0005: Worker Capability Advertisement via Matrix State Events
- mxdx-fabric coordinator: `org.mxdx.fabric.task` and `org.mxdx.fabric.result` event schemas
