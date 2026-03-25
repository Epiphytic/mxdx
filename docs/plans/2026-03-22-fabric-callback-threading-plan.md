# mxdx-fabric Callback & Threading — Implementation Plan

**Goal:** Close the feedback loop. Workers thread all output onto the task event. The OpenClaw plugin tracks pending tasks in a private Matrix outbox, and routes results back to the originating Discord thread or session when they arrive.

**ADR:** `docs/adr/0006-openclaw-fabric-callback-plugin.md`
**Repo (mxdx):** `/home/openclaw/.openclaw/workspace/mxdx`
**Repo (plugin):** `/home/openclaw/.openclaw/extensions/openclaw-fabric-plugin/` (new)

---

## Overview

Four work streams, ordered by dependency:

```
Stream A: mxdx-matrix threading support
    ↓
Stream B: mxdx-fabric worker threading + type cleanup
    ↓
Stream C: mxdx-fabric CLI --payload-json
    ↓
Stream D: OpenClaw plugin (outbox + routing)
```

Streams A, B, C are all in the `mxdx` repo. Stream D is the new plugin repo. Each stream is one jcode task. Do not batch.

---

## Stream A — Matrix Threading Support

**Crate:** `crates/mxdx-matrix`
**Commit:** `feat(matrix): add send_threaded_event for Matrix thread replies`

### What to build

Add `send_threaded_event` to `MatrixClient`:

```rust
pub async fn send_threaded_event(
    &self,
    room_id: &RoomId,
    event_type: &str,
    in_reply_to_event_id: &str,
    content: serde_json::Value,
) -> Result<String>  // returns the new event ID
```

The method sends a Matrix room event with `m.relates_to` set for threading:

```json
{
  "m.relates_to": {
    "rel_type": "m.thread",
    "event_id": "<in_reply_to_event_id>"
  }
}
```

Also update `send_event` to return the new event ID (`String`) — currently returns `()`. The event ID comes from the `event_id` field in the Matrix `/send` response. This return value is needed for tracking task events.

### Tests

- Unit test: `send_threaded_event` payload includes correct `m.relates_to` structure
- Integration test (against local Tuwunel): post a root event, thread a reply onto it, fetch the thread and assert both events present

---

## Stream B — Worker Threading + Type Cleanup

**Crates:** `mxdx-types`, `mxdx-fabric`
**Commit:** `feat(fabric): thread worker output onto task event; remove _callback from wire types`

### Type cleanup (mxdx-types)

Remove `_callback` / `callback` field from both:
- `TaskEvent` — routing is never transmitted to workers
- `TaskResultEvent` — routing is never on the wire

Update all existing tests that reference these fields. The fields go away entirely.

### Task event ID plumbing (mxdx-fabric)

The worker needs the Matrix event ID of the `org.mxdx.fabric.task` event to thread onto it. This event ID is available in the Matrix sync envelope (`event.event_id`) but is not currently passed through to `run_task`.

Changes needed:
- `WorkerClient::watch_and_claim` — when returning a `TaskEvent`, also return the Matrix event ID (`task_event_id: String`)
- `ProcessWorker::run_task(task, room_id, task_event_id)` — accept the event ID
- Pass it through to both `run_task_matrix` and `run_task_p2p`

### Worker output threading

In `run_task_matrix` and `run_task_p2p`:

- Replace `flush_heartbeat` (plain room event) with `flush_threaded_heartbeat` using `send_threaded_event`, threading each batch onto `task_event_id`
- Replace `post_result` with a threaded variant that also threads the result event onto `task_event_id`

Update `WorkerClient`:
- `post_heartbeat` gains optional `in_reply_to: Option<&str>` — if provided, uses `send_threaded_event`; falls back to plain for backwards compat
- `post_result` gains `task_event_id: Option<&str>` — same pattern

Or simply: make threading mandatory now and remove the non-threaded paths. The non-threaded fallback has no known users.

### Tests

- Update `e2e_fabric.rs`: assert that heartbeat and result events are in the thread of the task event (not top-level room events)
- Assert that `TaskEvent` deserialization no longer accepts `_callback` field (or ignores it gracefully — unknown fields should be `deny_unknown_fields`-off, but the field should not be present in the struct)

---

## Stream C — CLI `--payload-json`

**Crate:** `crates/mxdx-fabric` (CLI binary)
**Commit:** `feat(cli): add --payload-json to fabric post for arbitrary payload fields`

### What to build

`fabric post` currently accepts `--prompt`, `--capabilities`, `--cwd`, etc. Add:

```
--payload-json <JSON>    Arbitrary JSON object merged into task payload
```

Merge logic: parse `--payload-json` as a JSON object, merge into the payload map alongside existing fields. Named flags win on conflict (e.g. if `--cwd` and `--payload-json {"cwd": "..."}` both set cwd, the explicit flag wins).

Return the task UUID and the **Matrix event ID** of the posted task event on stdout:

```
task_uuid: 550e8400-e29b-41d4-a716-446655440000
event_id: $abc123:ca1-beta.mxdx.dev
```

The event ID is what the OpenClaw plugin stores in its outbox to anchor the thread.

### Tests

- Unit test: payload merge with and without `--payload-json`
- Unit test: named flag wins over `--payload-json` on conflict
- Snapshot test: output format includes both `task_uuid` and `event_id`

---

## Stream D — OpenClaw Fabric Plugin

**Location:** `/home/openclaw/.openclaw/extensions/openclaw-fabric-plugin/`
**Language:** TypeScript (matches `openclaw-claude-code-plugin` pattern)
**Commit:** `feat: openclaw-fabric-plugin — outbox + callback routing (ADR-0006)`

### Reference

Read `/home/openclaw/.openclaw/extensions/openclaw-claude-code-plugin/` carefully before starting. Match its structure:
- `openclaw.plugin.json` — manifest
- `src/index.ts` — main entry, exports `register(context)`
- `package.json` — deps + build
- `dist/` — compiled output

### Plugin manifest

```json
{
  "id": "openclaw-fabric-plugin",
  "name": "OpenClaw mxdx Fabric Plugin",
  "description": "Routes mxdx-fabric task completions back to originating OpenClaw sessions",
  "version": "0.1.0",
  "configSchema": {
    "type": "object",
    "required": ["homeserver", "token", "coordinatorRoom"],
    "properties": {
      "homeserver":       { "type": "string", "description": "Matrix homeserver URL" },
      "token":            { "type": "string", "description": "Matrix access token" },
      "userId":           { "type": "string", "description": "Matrix user ID for this client" },
      "coordinatorRoom":  { "type": "string", "description": "Matrix coordinator room ID" }
    }
  }
}
```

### Outbox room

On startup, locate or create the private outbox room:
- Alias: `openclaw-fabric-{sha256(userId).slice(0, 12)}`
- If not found: create as a private, invite-only room; no one else is invited
- Join if found

### Outbox state schema

State event type: `org.mxdx.fabric.outbox`, state key: `""`

```typescript
interface OutboxEntry {
  coordinator_event_id: string;  // Matrix event ID of the org.mxdx.fabric.task event
  callback: {
    channel: string;             // e.g. "discord"
    thread_id?: string;
    reply_to_message_id?: string;
  };
  posted_at: number;             // ms timestamp
  timeout_secs: number;
}

interface OutboxState {
  updated_at: number;            // client-authored ms timestamp
  entries: Record<string, OutboxEntry>;  // keyed by task_uuid
}
```

### Startup sequence

1. Locate/create outbox room
2. Read `org.mxdx.fabric.outbox` state event
3. Prune expired entries: `now > posted_at + Math.max(7 * 86400000, timeout_secs * 5000)`
4. Remove entries where fetching `coordinator_event_id` returns 404
5. Rewrite state if anything was pruned
6. Load remaining entries into in-memory map
7. Connect to coordinator room Matrix sync
8. Backfill: for each entry, fetch the Matrix thread on `coordinator_event_id` since `posted_at`; process any `org.mxdx.fabric.result` events found

### Registering a callback

Expose a CLI command `openclaw fabric watch`:

```
openclaw fabric watch \
  --task-uuid <UUID> \
  --event-id <MATRIX_EVENT_ID> \
  --channel discord \
  --thread-id <DISCORD_THREAD_ID> \
  --reply-to <DISCORD_MESSAGE_ID> \
  --timeout <SECS>
```

This adds an entry to the in-memory outbox and writes the state event to the outbox room. Returns immediately.

Intended usage: called by `fabric-run` immediately after `fabric post` outputs the task UUID + event ID.

### Matrix sync loop

Poll the coordinator room using Matrix `/sync` with `timeout=30000`. On each event batch:

1. Filter for events in threads rooted at known `coordinator_event_id` values
2. For each matching thread event: check if `type == "org.mxdx.fabric.result"`
3. On result: call `handleResult(task_uuid, result_content)`

`handleResult`:
1. Look up outbox entry by `task_uuid`
2. Extract status, duration, output summary from result content
3. Format message and send to `callback.channel` + `callback.thread_id` via OpenClaw message API
4. Remove entry from in-memory map
5. Rewrite outbox state event

### Message routing

Use the OpenClaw internal message sender (same pattern as the claude-code plugin's notification sends). Route to `discord` channel, reply to `reply_to_message_id` in `thread_id`.

Format:
- Success: `✅ Fabric task done in {duration}s\n{output_summary}`
- Failed: `❌ Fabric task failed: {error}`
- Timeout: `⏱️ Fabric task timed out after {timeout}s`

Output summary: decode NDJSON (jcode stream format), extract last assistant text. Keep to ~500 chars. Port decoding logic from `/home/openclaw/.local/bin/jcode-decode`.

### Build + install

```bash
npm install
npm run build
# plugin loads from dist/index.js
```

After build: verify `openclaw fabric watch --help` works.

---

## Wiring It Together

After all four streams are built, the end-to-end flow is:

```
1. fabric-run generates task UUID
2. fabric post --payload-json '{"cwd": "..."}' → outputs task_uuid + event_id
3. openclaw fabric watch --task-uuid X --event-id Y --channel discord --thread-id Z ...
4. Worker claims task, runs jcode
5. Worker threads heartbeats + result onto the task event (event_id Y)
6. Plugin sees result in thread → looks up outbox → routes to Discord thread Z
```

---

## Notes for jcode

- Read `docs/adr/0006-openclaw-fabric-callback-plugin.md` before starting
- Each stream is one task — do not build multiple streams in one run
- Commit at the end of each stream with the specified commit message
- Stream D depends on A, B, C being merged — build last
- The existing `openclaw-claude-code-plugin` is the reference implementation for plugin structure
- For Matrix threading: the spec is `m.relates_to` with `rel_type: "m.thread"` — Tuwunel supports this
