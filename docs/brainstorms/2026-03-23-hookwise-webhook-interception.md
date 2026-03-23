# Brainstorm: Hookwise-Style Webhook/Event Interception System

**Date:** 2026-03-23
**Status:** Brainstorm (pre-design)
**Author:** Jcode Agent

---

## Problem Statement

We want to build a webhook/event interception, inspection, and replay system — similar in spirit to tools like hookwise, webhook.site, hookhaven, and hookreplay — but implemented in Rust/WASM and designed to run within or alongside jcode.

**Assumptions:**
- This sits in the mxdx ecosystem, which already has Matrix-native event routing (mxdx-fabric), E2EE, and a WASM story
- The primary users are developers and AI agents debugging integrations, not end-users clicking through a SaaS dashboard
- "Webhooks" is the starting point, but the real target is broader: any HTTP callback, Matrix event, or inter-agent message that you want to intercept, inspect, modify, and replay
- jcode is the primary interface — this should feel like a tool, not a hosted service

---

## Design Questions

These are the questions we need sharp answers to before writing any code.

---

### 1. What is the interception target — HTTP only, or also Matrix events and agent messages?

Existing webhook tools intercept HTTP POST/PUT requests. But in the mxdx world, the interesting events are Matrix timeline events (fabric tasks, heartbeats, results) and inter-agent messages. Do we build an HTTP-only interceptor, a Matrix event interceptor, or a generalized event interceptor with protocol adapters?

**Why this matters:** An HTTP-only tool is simpler but less useful in an ecosystem where the primary message bus is Matrix. A generalized tool is more powerful but risks being vague. The answer shapes every other decision.

---

### 2. Where does the interceptor sit in the request lifecycle?

Three models:

- **Endpoint mode** — generate a unique URL, point senders at it, capture what arrives (webhook.site model)
- **Proxy/MITM mode** — sit between sender and receiver, forward traffic while recording it (mitmproxy model)
- **Tap mode** — attach to an existing Matrix room or HTTP endpoint as a passive listener, copy events without altering the flow (tcpdump model)

Each has different deployment, trust, and latency implications. Which is primary? Do we need all three?

---

### 3. WASM module or native Rust binary — or both?

Options:

- **Native Rust binary** — runs as a standalone process or a crate in the mxdx workspace. Full access to networking, filesystem, OS keychain. Best performance.
- **WASM module (wasm32-wasip2)** — runs sandboxed, portable, can be loaded by jcode's tool runtime. Limited networking (needs host-provided capabilities). Aligns with the GIRT/jcode tool model.
- **Hybrid** — core interception logic in native Rust (it needs to bind sockets), inspection/replay/query logic as WASM tools that jcode can load.

**Why this matters:** The interception layer *must* bind a socket or tap a Matrix sync stream — WASM can't do that alone in WASI p2. But the query/inspect/replay UX could be a WASM tool that reads from a store. Need to decide the boundary.

---

### 4. What is the storage model for captured events?

Options:

- **In-memory ring buffer** — fast, ephemeral, good for dev. Loses data on restart.
- **SQLite** — durable, queryable, single-file. mxdx already uses SQLite for Matrix crypto state.
- **JSONL append log** — simple, git-friendly, greppable. No indexing.
- **Matrix room as storage** — post captured events into a private Matrix room. Gets E2EE, federation, durability for free. Slow writes.
- **Hybrid** — ring buffer for live inspection, SQLite for durable history, Matrix for E2EE archival.

How long do we keep events? What's the query model (by time? by header? by payload content? by sender)? Is full-text search required?

---

### 5. What does replay actually mean?

"Replay" could mean:

- **Re-send the exact bytes** to the original destination (or a new one)
- **Re-send with modifications** — edit headers, payload, timing before re-sending
- **Re-send to a local dev server** — forward a production webhook capture to localhost
- **Sequence replay** — replay N events in order with original timing gaps (load testing)
- **Conditional replay** — replay only events matching a filter

Which of these is the core use case? Replaying modified events implies an editing UX. Sequence replay implies a timeline model. These are very different features.

---

### 6. How does this expose through jcode?

jcode tools are CLI-first. What's the interface?

```
# Possible CLI shapes:
jcode hook listen --port 8080                    # start interceptor
jcode hook list                                  # show captured events
jcode hook inspect <event-id>                    # show full event details
jcode hook replay <event-id> --to localhost:3000 # replay to local server
jcode hook diff <id-1> <id-2>                    # compare two events
jcode hook filter --header "X-GitHub-Event=push" # filter captured events
jcode hook export <id> --format curl             # export as cURL command
```

Or is it a long-running background process that jcode spawns, with a separate query interface? Does it integrate with the jcode side panel for real-time display?

---

### 7. How does this relate to mxdx-fabric?

mxdx-fabric already routes tasks, heartbeats, and results through Matrix rooms. A "hook" system could:

- **Be orthogonal** — separate tool, doesn't know about fabric
- **Be a fabric observer** — taps into fabric rooms, captures task lifecycle events for debugging
- **Be a fabric middleware** — intercepts task events before they reach workers, allowing inspection/modification/delay (dangerous but powerful for testing)

If it's a fabric observer, it could be the debugging/audit layer that's currently missing. But that's a much tighter coupling.

---

### 8. What is the trust and security model?

Captured webhooks often contain secrets (API keys, tokens, signatures). Questions:

- Are captured events encrypted at rest? (In mxdx, the answer should be yes — age encryption or Matrix E2EE)
- Who can read captured events? Only the interceptor owner? Any jcode session?
- Can the interceptor be used in production, or is it dev-only? Production interception of webhook traffic has serious security implications.
- How do we handle webhook signature verification (e.g., GitHub HMAC signatures)? Do we verify and show pass/fail, or do we strip and re-sign on replay?
- If events are stored in a Matrix room, they're E2EE by default — but the interceptor process has the decryption keys in memory. How do we handle key lifecycle?

---

### 9. How do we handle high-throughput scenarios?

A webhook interceptor in local dev sees maybe 1 req/sec. In CI or staging, it might see 100/sec. In production tap mode, it could see 10k/sec.

- Does the storage model handle high write throughput?
- Does the interceptor add meaningful latency to the forwarding path?
- Is backpressure needed? What happens when the interceptor can't keep up — drop events, buffer, or block the sender?
- Is sampling an option for high-throughput scenarios (capture 1 in N)?

---

### 10. What is the local dev story vs. the CI story vs. the production story?

These are very different deployment models:

- **Local dev:** Developer runs `jcode hook listen`, points Stripe/GitHub webhooks at a tunnel or localhost, inspects and replays manually. Ephemeral, single-user, interactive.
- **CI:** Test suite fires webhooks at the interceptor, asserts on captured payloads, replays fixtures. Needs to be scriptable, headless, fast startup/teardown.
- **Production debugging:** Tap into live traffic between service A and service B, capture events matching a filter, inspect without disrupting the flow. Needs to be safe, auditable, low-overhead.

Do we build one tool that serves all three, or a focused tool for one scenario that can grow?

---

### 11. Do we need a tunnel component, or is that out of scope?

Tools like ngrok, hookhaven, and hookreplay include a tunneling feature (public URL → localhost forwarding). This is useful for local webhook development but is a significant piece of infrastructure (relay server, connection management, DNS).

Options:
- **Out of scope** — use ngrok/cloudflare tunnels separately
- **Leverage mxdx P2P transport** — the existing P2P design (WebRTC data channels, Unix domain sockets) could bridge a public endpoint to localhost through Matrix. No relay server needed, and it's E2EE.
- **Build a minimal relay** — a small Axum service that accepts webhooks and forwards them over WebSocket to the local interceptor

The P2P transport option is interesting because it reuses existing infrastructure and gets encryption for free.

---

### 12. What is the event identity model?

When we capture an event, what identifies it?

- **Auto-generated UUID** per captured event? (simple, unique)
- **Content-addressable hash** of the event payload? (enables dedup, diffing)
- **Matrix event ID** if the source is a Matrix room? (natural, but only works for Matrix events)
- **Composite key** (timestamp + source + hash)? (verbose but unambiguous)

This affects replay (how do you reference an event), storage (how do you index), and diffing (how do you compare).

---

### 13. Should captured events be shareable across team members?

In a team debugging scenario:

- Developer A captures a webhook that reproduces a bug
- Developer A wants to share the exact captured event with Developer B
- Developer B replays it against their local environment

If storage is a Matrix room, sharing is just room membership — E2EE handles key sharing. If storage is local SQLite, sharing means export/import. The Matrix-native path is more elegant but couples tightly to Matrix.

---

### 14. Do we need request/response pairing for proxy mode?

In proxy mode, we see both the incoming request and the upstream response. Do we:

- Store them as a paired unit (request + response)?
- Store them separately and correlate later?
- Only store the request (simpler, but you lose the response)?

Pairing is essential for debugging ("I sent this webhook and got a 500 back — what was in the response body?") but adds complexity to the storage model.

---

### 15. How does this interact with existing observability tools?

The interceptor captures structured event data. Should it:

- Export to OpenTelemetry (traces/spans)?
- Emit structured logs compatible with the mxdx tracing setup?
- Provide a Prometheus endpoint for metrics (events/sec, latency, error rate)?
- Stay standalone and let users pipe output to their own observability stack?

For an AI agent debugging tool, structured logs might be more useful than OTel traces. But for production tap mode, OTel integration would be expected.

---

## Options (Architectural Approaches)

### Option A: HTTP-Only Interceptor (Focused Tool)

**Mechanism:** A standalone Axum service that generates unique webhook URLs, captures incoming HTTP requests to SQLite, and exposes a CLI for query/replay. No Matrix integration. No WASM. Pure Rust binary.

**Optimized for:** Simplicity, fast delivery, familiar model (webhook.site clone you own).

**Drawbacks:** Doesn't leverage any mxdx infrastructure. Can't intercept Matrix events. Storage isn't E2EE. No team sharing without bolt-on features. Essentially building a commodity tool from scratch.

**Fit:** Low fit with mxdx ecosystem. Could be useful standalone but doesn't differentiate.

### Option B: Matrix-Native Event Interceptor (Fabric Observer)

**Mechanism:** A Matrix bot that joins fabric rooms (or any specified rooms), captures all events to a private E2EE room, and exposes them through jcode CLI tools. Replay means re-posting events to a room. HTTP webhook capture is a separate adapter that receives HTTP and posts to a Matrix room, where the observer picks it up.

**Optimized for:** Deep mxdx integration, E2EE by default, team sharing via room membership, durable history via Matrix.

**Drawbacks:** Adds latency for HTTP webhook capture (HTTP → Matrix → observer is two hops). Tightly coupled to Matrix — useless without a homeserver running. Overkill for simple "capture a Stripe webhook" scenarios.

**Fit:** High fit with mxdx. This is the tool you'd want for debugging fabric task flows. But it's a poor standalone webhook debugger.

### Option C: Hybrid — Native Interceptor + WASM Query Tools

**Mechanism:** A native Rust interceptor handles socket binding (HTTP endpoints, Matrix sync taps) and writes to a local SQLite store with age encryption at rest. WASM tools loaded by jcode provide the query/inspect/replay/diff UX. The WASM tools read from the SQLite store via WASI filesystem access. Optional Matrix archival for team sharing and long-term storage.

**Optimized for:** Best of both worlds — fast local capture, portable query tools, optional Matrix integration for sharing/archival.

**Drawbacks:** More moving parts (native process + WASM tools + optional Matrix). Need to define the boundary between native and WASM carefully. SQLite file access from WASM requires host-granted filesystem capabilities.

**Fit:** Good fit. The native interceptor handles what WASM can't (networking), the WASM tools handle what should be portable (querying, formatting, diffing). Matrix integration is opt-in for teams.

### Option D: Fabric Middleware (Interceptor-as-Coordinator-Plugin)

**Mechanism:** Extend the mxdx-fabric coordinator with an interception mode. When enabled, the coordinator captures all task events, heartbeats, and results to a debug room before routing them. Adds "breakpoint" capability — pause a task event, let the developer inspect and optionally modify it, then release it to workers.

**Optimized for:** Deep debugging of fabric task flows. Interactive breakpoints are extremely powerful for testing failure policies, claim races, and coordinator logic.

**Drawbacks:** Only works for fabric events, not arbitrary webhooks. Modifying events in flight is dangerous (could break E2EE signatures, corrupt state). Tight coupling to coordinator internals. Not useful outside mxdx.

**Fit:** Very high fit for fabric debugging specifically. But too narrow as a general webhook tool. Could be a feature of the coordinator rather than a separate tool.

---

## Unknowns

- [ ] What does the WASI p2 networking story look like for 2026? Can WASM modules bind sockets yet, or is host delegation still required?
- [ ] How much latency does Matrix add when used as a capture buffer for HTTP webhooks? Is sub-100ms achievable on a local Tuwunel instance?
- [ ] What's the actual demand split between "debug webhooks from external services" vs. "debug fabric task flows"? This determines whether Option B or Option C is the right starting point.
- [ ] Does jcode's tool loading model support long-running background processes, or only request/response tools? An interceptor is inherently a background daemon.
- [ ] What's the SQLite encryption story for age-encrypted databases? age encrypts files, not database pages. Do we need SQLCipher, or encrypt individual event payloads within plain SQLite?
- [ ] Are there existing Rust crates for webhook signature verification (GitHub HMAC, Stripe signatures, etc.) that we should build on?
- [ ] How does the mxdx P2P transport handle NAT traversal for the tunneling use case? Would it work for routing public webhook traffic to localhost?

---

## Recommendation

**Start with Option C (Hybrid), scoped to HTTP interception first, with fabric observation as the fast-follow.**

Rationale:

1. **HTTP interception is the universal entry point.** Every developer has debugged a webhook. Starting here makes the tool immediately useful without requiring a Matrix homeserver.

2. **The native/WASM split is natural and clean.** The interceptor daemon binds sockets and writes to SQLite — this must be native Rust. The query/inspect/replay tools read from SQLite and format output — these can be WASM tools that jcode loads, making them portable and sandboxed.

3. **Matrix integration is additive, not foundational.** Once HTTP capture works, adding a Matrix room observer is a second adapter that writes to the same SQLite store. The query tools don't change. Team sharing via Matrix room archival is a natural extension.

4. **Fabric middleware (Option D) is a coordinator feature, not a separate tool.** The "breakpoint" concept for fabric tasks is powerful but belongs in the coordinator's debug mode, not in a standalone interceptor. Build it there later.

5. **Tunneling via mxdx P2P transport is the differentiator.** Every other webhook tool needs ngrok or a relay server. If we can route public webhook traffic through Matrix P2P channels (E2EE, no relay server, no third-party tunnel provider), that's a genuine competitive advantage. But it's a Phase 2 feature — get basic capture/replay working first.

**Suggested phasing:**

- **Phase 1:** Native Rust HTTP interceptor + SQLite store + jcode CLI for list/inspect/replay. No WASM, no Matrix. Ship something useful fast.
- **Phase 2:** WASM query tools (inspect, diff, export). Matrix room observer for fabric events. Age encryption at rest.
- **Phase 3:** P2P tunneling via mxdx transport. Webhook signature verification. Team sharing via Matrix rooms.
- **Phase 4:** Fabric middleware/breakpoints in coordinator. OTel export. High-throughput sampling.
