# ADR 0006: OpenClaw Fabric Callback Plugin

**Date:** 2026-03-22
**Status:** Superseded by ADR-0007

## Context

When a task is posted to the mxdx-fabric coordinator via `fabric post`, the result event (`org.mxdx.fabric.result`) is delivered to the Matrix coordinator room and received by the OpenClaw Matrix plugin. However, there is no mechanism to route that result back to the originating session or thread. The caller either blocks (synchronous CLI) or polls manually. There is no push-based wakeup.

This means:
1. **No async completion.** Agent sessions that post fabric tasks must block or spin-wait for results, consuming context window and preventing concurrent work.
2. **No multi-task orchestration.** Posting N tasks and receiving N callbacks as they complete is impossible without manual correlation.
3. **No cross-channel routing.** A task posted from a Discord thread has no way to route the result back to that specific thread.

## Decision

Build an OpenClaw plugin (`openclaw-fabric-plugin`) that intercepts fabric task posts, persists callback state, polls for results, and routes completions back to the originating session/thread.

### Callback Envelope

The task payload includes a `_callback` field, opaque to fabric and workers, echoed back verbatim in the result event:

```json
{
  "_callback": {
    "channel": "discord",
    "thread_id": "1485151340614254673",
    "reply_to_message_id": "...",
    "session_key": "..."
  }
}
```

Workers echo `_callback` unchanged in the result event. The plugin reads it to route the reply.

### Plugin Responsibilities

1. **Intercepts fabric task posts** — provides a tool that posts tasks to fabric AND registers a callback in a local SQLite DB before returning the task UUID to the caller.
2. **Persists callback state** — DB table:
   ```sql
   CREATE TABLE pending_callbacks (
       task_uuid     TEXT PRIMARY KEY,
       callback_json TEXT NOT NULL,
       posted_at     INTEGER NOT NULL,
       timeout_secs  INTEGER NOT NULL,
       last_polled_at INTEGER
   );
   ```
3. **Polls the coordinator room** — while any pending callbacks exist, polls the Matrix coordinator room every N seconds. Looks for `org.mxdx.fabric.result` events matching pending task UUIDs.
4. **Routes results back** — on result (success, failure, or timeout): reads `callback_json`, calls back into the originating OpenClaw session/thread with the result summary.
5. **Cleans up** — removes the callback record after routing.

### Plugin Interface

Two interaction modes:

**Mode A: Tool/command (preferred)** — plugin exposes a `fabric_post` tool to the agent:

```
fabric_post(prompt, capabilities, cwd, model, timeout, callback)
```

Returns task UUID immediately. Plugin handles polling and callback asynchronously.

**Mode B: CLI hook** — `fabric-run` calls `openclaw fabric register-callback --task-uuid X --callback-json Y` after posting, then exits. Plugin polls and routes independently.

Mode A is preferred: it keeps all fabric interaction inside OpenClaw with no external script dependency.

### Poll Strategy

- Poll interval: `max(5s, timeout_secs / 360)`, capped at 60s
- On poll: GET coordinator room timeline since `last_polled_at`, filter for `org.mxdx.fabric.result` events
- On match: route result, delete from DB
- On timeout (`posted_at + timeout_secs + 30s` elapsed): route timeout notification, delete from DB
- Polling loop sleeps when DB is empty, wakes on insert (via DB trigger or simple interval)

## Consequences

**Positive:**
- `fabric post` CLI becomes fire-and-forget for external scripts; the plugin owns the result loop
- Agent sessions get push-based task completion without blocking their context
- Multi-task workflows become possible: post N tasks, get N callbacks as they complete
- Callback envelope is opaque to fabric/workers, requiring no changes to existing worker code

**Negative:**
- Plugin requires Matrix credentials (reuses existing OpenClaw Matrix config), SQLite, and network access to the coordinator room
- Polling adds background load proportional to the number of pending callbacks
- Callback routing depends on the originating channel being available (e.g. Discord thread still exists)

## Out of Scope

- **Per-task reply rooms** — coordinator room + task UUID correlation is sufficient
- **Streaming output callbacks** — heartbeat events can be used for progress; result callback is for completion only
- **Multi-machine routing** — callback routes to the local OpenClaw instance only; cross-machine is a future concern

## Related

- ADR 0005: Worker Capability Advertisement via Matrix State Events
- ADR 0004: Dashboard Scaling and Session Preservation
- mxdx-fabric coordinator: `org.mxdx.fabric.result` event schema
