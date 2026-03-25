# ADR 0005: Worker Capability Advertisement via Matrix State Events

**Date:** 2026-03-21
**Status:** Accepted

## Context

Currently the mxdx-fabric worker advertises capabilities as a simple CSV string (`rust,linux,bash`) when claiming tasks. This has several problems:

1. **No schema visibility.** There is no way for a client (human or LLM) to know what payload fields a worker accepts, what arguments a tool supports, or what version of a tool is running.
2. **No health signal.** A worker may have a missing binary or misconfigured environment. Clients discover this only after a task fails.
3. **Global capability tags.** Capability strings are not host-scoped. Different hosts may run different binaries with different flags, hardware, or configurations. There is no way to target a specific host's worker.

## Decision

Workers publish a structured capability advertisement as a Matrix state event on startup and periodically (or on change). The state event is host-scoped: `state_key` is the worker's Matrix user ID, so coordinators can distinguish capabilities per-worker.

The capability schema uses MCP `inputSchema` format (JSON Schema) for tool definitions. This is the same format used for LLM tool calling — no translation layer is needed when an LLM queries capabilities to construct a task payload.

### State Event Schema

```yaml
type: org.mxdx.fabric.capability
state_key: @bel-worker:ca1-beta.mxdx.dev   # worker Matrix user ID — host-scoped

content:
  worker_id: "@bel-worker:ca1-beta.mxdx.dev"
  host: "belthanior"                         # hostname
  tools:
    - name: "process"                        # generic process executor
      version: "0.1.0"                       # ProcessWorker version
      description: "Generic process executor — runs any binary specified by the client"
      healthy: true                           # false if e.g. binary not found on PATH
      inputSchema:                            # MCP-compatible JSON Schema
        type: object
        properties:
          bin:
            type: string
            description: "Binary to execute (e.g. jcode, cargo, bash)"
          args:
            type: array
            items:
              type: string
            description: "Command-line arguments passed to the binary"
          env:
            type: object
            additionalProperties:
              type: string
            description: "Environment variables to set for the process"
          cwd:
            type: string
            description: "Absolute working directory path (no .. components)"
        required:
          - bin
```

### New CLI Subcommand

`fabric capabilities [worker-id]` — reads room state and prints tool schemas for the specified worker, or all workers if no argument is given. This replaces the need for a persistent MCP server: the CLI is invoked ad-hoc, and schema only enters context when needed.

### Why MCP inputSchema Format

- LLMs already reason about JSON Schema natively — it is what tool calling uses.
- No translation layer needed when an LLM queries capabilities to construct a task payload.
- The schema could be wrapped in an actual MCP server later with no changes.

### Why NOT an MCP Server

- An MCP server puts the full capability schema always in context, whether relevant or not.
- Skill + CLI is lazy: the skill description triggers only when relevant, and `fabric capabilities` pulls schema on-demand.
- Less infrastructure to keep alive and fail at the wrong moment.

### Why Host-Scoped (Not Global Capability Tags)

- Different hosts may have different binaries available with different versions and flags.
- Different hosts have different configurations, available tools, and hardware.
- Host-specific targeting by worker Matrix ID is already possible — this makes it explicit.
- The coordinator uses `state_key` to scope per-worker; it can route to a specific worker ID or to any worker advertising a matching tool name.

## Consequences

**Positive:**
- Clients (human and LLM) can discover exact tool schemas before constructing task payloads
- Health status surfaces broken workers before tasks are dispatched
- Per-host capability scoping enables targeted task routing
- Any binary can be executed through the same generic worker — capability advertisement becomes part of the worker contract

**Negative:**
- Workers gain startup logic to publish the capability state event and must re-publish on changes
- `fabric capabilities` CLI subcommand must be implemented
- Schema maintenance is now the worker's responsibility — stale schemas are possible if a worker crashes without clearing state

**Migration:**
- Skill documentation updated to reference `fabric capabilities` for discovering current input schema instead of hardcoding payload fields
- Existing CSV-based capability tags can coexist during transition; the state event is additive

## Related

- ADR 0004: Dashboard Scaling and Session Preservation
- MCP tool calling specification: `inputSchema` is standard JSON Schema
- Matrix spec: state events with `state_key` for per-entity scoping
