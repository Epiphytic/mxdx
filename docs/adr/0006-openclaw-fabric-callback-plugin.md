# ADR 0006: OpenClaw Fabric Callback Plugin

**Date:** 2026-03-22
**Status:** Accepted

## Context

When a task is posted to the mxdx-fabric coordinator via `fabric post`, the result event (`org.mxdx.fabric.result`) is delivered to the Matrix coordinator room. However, there is no mechanism to route that result back to the originating session or thread. The caller either blocks (synchronous CLI) or polls manually. There is no push-based wakeup.

This means:
1. **No async completion.** Agent sessions that post fabric tasks must block or spin-wait for results, consuming context window and preventing concurrent work.
2. **No multi-task orchestration.** Posting N tasks and receiving N callbacks as they complete is impossible without manual correlation.
3. **No cross-channel routing.** A task posted from a Discord thread has no way to route the result back to that specific thread.

## Decision

Build an OpenClaw plugin (`openclaw-fabric-plugin`) that embeds callback routing info in task posts, persists outbox state in Matrix, and routes completions back to the originating session/thread on result.

### Trust Model

OpenClaw embeds `_callback` in the `org.mxdx.fabric.task` event at post time. This event is authored by OpenClaw's sender identity and lives in the coordinator room permanently.

On result, the plugin correlates `task_uuid` → `coordinator_event_id` (stored in its own outbox), fetches the original task event, and reads `_callback` from it. The worker's result event is trusted only for task output (status, stdout, duration) — **never for routing**.

A worker cannot redirect a callback. Workers MUST ignore unknown payload fields. Workers MUST NOT copy, modify, or echo `_callback` or any other unknown field into the result event.

### Callback Envelope

`_callback` is embedded in the `org.mxdx.fabric.task` event payload at post time:

```json
{
  "task_uuid": "...",
  "prompt": "...",
  "capabilities": ["rust"],
  "cwd": "/path/to/repo",
  "timeout_secs": 1800,
  "_callback": {
    "channel": "discord",
    "thread_id": "1485151340614254673",
    "reply_to_message_id": "..."
  }
}
```

`fabric post` gains a `--payload-json` flag accepting arbitrary JSON merged into the task payload. `_callback` is one such field.

### Outbox State Event

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

### Private Outbox Room

Each sender identity gets a dedicated private room:

- **Name:** deterministic — `openclaw-fabric-{sha256(user_id)[:12]}`
- **Access:** invite-only, only the sender identity is a member
- **Creation:** plugin creates it on first run if absent; joins it on subsequent starts

The deterministic name means the plugin can locate its room after restart without any external state.

### Multi-Identity Write Protocol

When the outbox changes (entry added or removed):

1. Compute new state with a fresh `updated_at = Date.now()` (milliseconds)
2. Write state event to identity-1's outbox room, await server ack
3. Call `Date.now()` again for identity-2 — guaranteed distinct millisecond — write, await ack
4. Continue for further identities

No artificial delay needed: sequential `Date.now()` calls produce distinct timestamps.

### Conflict Resolution

On startup (or any time multiple identity rooms are readable):

1. Read `org.mxdx.fabric.outbox` state event from all identity rooms
2. Compare `updated_at` fields (client-authored)
3. Take the entry set from the event with the highest `updated_at` as truth
4. Rewrite the canonical state to any rooms that were behind

### Expiry

When writing or reading the outbox, remove entries where:

```
now_ms > posted_at + max(7 * 24 * 60 * 60 * 1000, timeout_secs * 5 * 1000)
```

That is: 7 days or 5× the task timeout, whichever is longer. If the original coordinator event no longer exists (404 or not found), also remove that entry.

### Startup Sequence

1. Locate or create private outbox room for each configured sender identity
2. Read outbox state from all identity rooms; apply conflict resolution
3. Prune expired entries; remove entries whose coordinator events are gone (404)
4. Rewrite state to all identity rooms if anything was pruned
5. Subscribe to coordinator room for `org.mxdx.fabric.result` events
6. Backfill: scan coordinator room history since `min(posted_at)` across all outbox entries; process any result events that arrived while offline

### Result Handling

On `org.mxdx.fabric.result` in coordinator room:

1. Extract `task_uuid`
2. Look up in in-memory outbox (loaded from state on startup)
3. If found: fetch original `org.mxdx.fabric.task` event by `coordinator_event_id`
4. Read `_callback` from the original task event
5. Route result to the specified channel/thread via OpenClaw's message API
6. Remove entry from outbox, rewrite state to all identity rooms

## Consequences

**Positive:**
- Trust boundary is at post time: routing info always comes from our own authored event
- Matrix is the persistence layer: no SQLite, no external state, survives restarts naturally
- Multi-identity redundancy: outbox replicated across all sender identities; any one surviving is sufficient
- Self-healing: expired and invalid entries pruned on every read/write cycle
- Conflict resolution is deterministic: highest `updated_at` wins
- Offline resilience: backfill on startup catches results that arrived while plugin was down
- `fabric post --payload-json` enables arbitrary extra fields, extensible beyond `_callback`

**Negative:**
- Plugin requires Matrix read access to coordinator room history (for backfill)
- Fetching the original task event on each result adds one Matrix API call per completion
- If all identity outbox rooms are lost, pending callbacks are unrecoverable (acceptable: tasks are time-bounded)

## Out of Scope

- Cross-machine callback routing (callback routes to the posting OpenClaw instance only)
- Streaming/heartbeat callbacks (result event only; heartbeat events are a separate concern)
- Callback authentication beyond task event authorship trust

## Related

- ADR-0005: Worker Capability Advertisement via Matrix State Events
- mxdx-fabric coordinator: `org.mxdx.fabric.task` and `org.mxdx.fabric.result` event schemas
