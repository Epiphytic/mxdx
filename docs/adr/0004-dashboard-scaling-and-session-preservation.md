# ADR 0004: Dashboard Scaling and Session Preservation

**Date:** 2026-03-08
**Status:** Accepted

## Context

The mxdx web console communicates with launchers over Matrix. Every dashboard interaction — reading telemetry, listing sessions, sending commands — is a Matrix event in an E2EE room. Two categories of problems emerged as the system moved from local Tuwunel testing to production use on matrix.org:

1. **Crypto store fragmentation**: The browser lost its device_id and Megolm keys between sessions, creating a new device on every login. This meant old E2EE events were permanently undecryptable, and the launcher couldn't verify the browser as a trusted device.

2. **Rate limiting under load**: The dashboard polled every 10 seconds, sending encrypted events to every launcher on each cycle. With just 2 launchers, this hit matrix.org's 429 rate limits. At the target scale of 100+ launchers, it would be completely unusable.

Both problems stem from treating Matrix like a local RPC bus rather than a federated, rate-limited, cryptographic protocol.

## Decision

### 1. Device and Crypto Store Preservation

**Problem:** `login()` and `restoreSession()` used different IndexedDB store name formats (`mxdx_{username}_{server}` vs `mxdx_{user_id}_{homeserver_url}`), so restoring a session opened a different IndexedDB than the one containing the Megolm keys from the original login.

**Solution:**
- Add a `store_name` field to `WasmMatrixClient` and include it in the exported session JSON. `restoreSession()` reads this field to open the exact same IndexedDB.
- The browser auth flow now always tries `restoreSession()` first, only falling back to `login()` (which creates a new device_id) if no saved session exists.
- `handleLogout()` no longer clears the session from IndexedDB. The session data (device_id, access_token, store_name) is preserved so the next login reuses the same device.
- If `login()` detects a stale crypto store ("account doesn't match" error), it auto-clears the IndexedDB and retries — this is the only path that creates a new device.

**Rationale:** A Matrix device_id is an identity anchor. Megolm session keys are bound to it. Losing the device_id means losing the ability to decrypt any events encrypted for that device. The cost of preserving it (a few KB in IndexedDB) is negligible compared to the cost of losing it (all historical E2EE data becomes inaccessible).

### 2. O(1) Sync Architecture for Dashboard

**Problem:** The original dashboard did N sequential operations per launcher per render cycle:

```
Per launcher (sequential):
  1. collectRoomEvents(exec_room_id, 2s) → calls sync_once() in a loop for 2 seconds
  2. sendEvent('org.mxdx.command', { action: 'list_sessions' }) → encrypted PUT
  3. sync_once() → flush send queue
  4. onRoomEvent('org.mxdx.terminal.sessions', 5s) → polls sync_once() for 5 seconds
```

At 100 launchers: ~700 seconds per render, 100 encrypted event sends per cycle.

**Solution:** Separate reads from writes. Reads use local cache; writes are user-initiated.

```
Per render (constant):
  1. syncOnce() → one server round-trip, updates all rooms
  2. listLauncherSpaces() → reads joined rooms from local cache
  3. Promise.all(readRoomEvents(room) for each launcher) → parallel local cache reads

Per user action (on-demand):
  4. "Refresh Sessions" button → sendEvent + onRoomEvent for ONE launcher
```

**New WASM API:** `readRoomEvents(room_id)` reads from the matrix-sdk's local IndexedDB store without calling `sync_once()`. This makes per-launcher reads a local database operation rather than a network call.

**Scaling characteristics:**

| Metric | Before | After |
|--------|--------|-------|
| Syncs per render | 1 + N×(loop) | 1 |
| Encrypted sends per render | N | 0 |
| Time @ 2 launchers | ~14s | < 1s |
| Time @ 100 launchers | ~700s | < 2s |
| Rate limit risk | High | None on refresh |

**Refresh interval** increased from 10s to 30s. Session listing moved from automatic polling to on-demand per card. Both reduce encrypted event throughput on public homeservers.

### 3. Launcher Session Reuse (Already Correct)

The Node.js launcher (`packages/core/session.js`) already followed the correct pattern: `restoreSession()` first with crypto store loaded from disk via `restoreIndexedDB()`, falling back to `login()` only on failure. No changes needed. Documented here for completeness — the same "never create a new device unless forced" principle applies to all clients.

## Consequences

**Positive:**
- Dashboard renders in constant time regardless of launcher count
- No rate limiting on normal dashboard use (reads are local)
- Device identity and Megolm keys survive across browser sessions, page reloads, and logouts
- Session list fetching is explicit — user sees a button and loading state instead of mysterious delays

**Negative:**
- Telemetry data may be up to 30 seconds stale (refresh interval)
- Session lists require manual refresh per launcher card
- `readRoomEvents` returns whatever the SDK has decrypted locally — events encrypted for a different device still show as missing (this is correct behavior, not a bug)

**Trade-offs accepted:**
- Staleness vs. rate limits: 30s refresh is acceptable for a monitoring dashboard. Users who need real-time data can open a terminal session.
- On-demand sessions vs. automatic: Sending an encrypted command to 100 launchers every 30 seconds is not viable on any federated homeserver. Per-card refresh is the right UX.

## Related

- ADR 0003: WASI Packaging Limitations
- `docs/plans/2026-03-08-terminal-session-persistence-design.md`
- matrix-sdk 0.16 `Room::messages()` reads from local store after sync
- matrix.org rate limits: ~10 requests/second for authenticated endpoints, lower for encrypted sends
