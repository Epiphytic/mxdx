# Cleanup Utility Design

**Date:** 2026-03-08
**Status:** Approved

## Goal

Provide a way to clean up stale Matrix state (old devices, old events, abandoned rooms) without deleting and recreating accounts. Available from CLI (client + launcher) and the web console.

## Architecture

### Shared Core: `packages/core/cleanup.js`

Pure functions using `fetch()` against the Matrix REST API directly. No WASM changes needed — uses `accessToken` + `homeserverUrl` from the existing session.

```js
// Each function returns { preview, execute }
// - preview: array of items that will be affected (for display before confirmation)
// - execute(): performs the cleanup, returns results

export async function cleanupDevices({ accessToken, homeserverUrl, currentDeviceId, password, olderThan, onProgress })
export async function cleanupEvents({ accessToken, homeserverUrl, olderThan, onProgress })
export async function cleanupRooms({ accessToken, homeserverUrl, olderThan, onProgress })
```

### Cleanup Targets

| Target | What it does | Matrix API |
|--------|-------------|------------|
| `devices` | Delete all devices except current + any primary | `GET /_matrix/client/v3/devices` → `POST /_matrix/client/v3/delete_devices` |
| `events` | Redact all mxdx-typed events in exec/logs rooms | `GET /rooms/{id}/messages` → `PUT /rooms/{id}/redact/{eventId}/{txnId}` |
| `rooms` | Leave and forget all mxdx rooms (spaces, exec, logs, DMs) | `POST /rooms/{id}/leave` → `POST /rooms/{id}/forget` |

### `--older-than` Filter

All three targets support an optional `--older-than` duration filter. When provided, only items older than the specified duration are affected. Without it, all eligible items are cleaned up.

**Duration format:** `<number><unit>` where unit is `d` (days), `w` (weeks), or `m` (months, 30 days).

Examples: `1d` (1 day), `2w` (2 weeks), `3m` (3 months)

**Parsing:** `parseOlderThan(str)` returns a Unix timestamp (ms) cutoff. Items with `last_seen_ts` / `origin_server_ts` before this cutoff are eligible for cleanup.

| Target | Timestamp field used | Behavior |
|--------|---------------------|----------|
| `devices` | `last_seen_ts` from device list | Only deletes devices not seen since before the cutoff |
| `events` | `origin_server_ts` from message events | Only redacts events sent before the cutoff |
| `rooms` | Most recent `origin_server_ts` across room messages | Only leaves rooms with no activity since before the cutoff |

### Device Cleanup Details

- `GET /_matrix/client/v3/devices` — list all devices
- Filter out current device_id (never delete self)
- If `olderThan` provided, further filter by `last_seen_ts < cutoff`
- `POST /_matrix/client/v3/delete_devices` with body `{ devices: [ids] }`
- Requires UIA (User-Interactive Auth) — password auth flow:
  1. First call returns 401 with session ID
  2. Second call includes `auth: { type: "m.login.password", identifier: { type: "m.id.user", user }, password, session }`

### Event Cleanup Details

- Find all mxdx rooms via `listLauncherSpaces()` (from WASM client)
- For each exec/logs room: `GET /rooms/{id}/messages?dir=b&limit=100`
- Filter for mxdx event types: `org.mxdx.host_telemetry`, `org.mxdx.command`, `org.mxdx.command_result`, `org.mxdx.terminal.*`
- If `olderThan` provided, filter events by `origin_server_ts < cutoff`
- Redact each: `PUT /rooms/{id}/redact/{eventId}/{txnId}` with `{ reason: "mxdx cleanup" }`
- Paginate if more than 100 events

### Room Cleanup Details

- Find all mxdx rooms via `listLauncherSpaces()` + scan for DM rooms with mxdx topics
- If `olderThan` provided, check most recent event timestamp in room; skip rooms with recent activity
- For each room: `POST /rooms/{id}/leave` then `POST /rooms/{id}/forget`
- Order: child rooms first, then spaces (avoid orphaned references)

## Consumers

### CLI (client + launcher)

```
mxdx-client cleanup <targets>
mxdx-launcher cleanup <targets>

targets: comma-separated list of: devices, events, rooms
  --force-cleanup    Skip confirmation prompts
  --older-than <dur> Only clean up items older than duration (e.g. 1d, 2w, 3m)

Examples:
  mxdx-client cleanup devices
  mxdx-client cleanup devices --older-than 2w
  mxdx-client cleanup events,rooms --older-than 1m
  mxdx-launcher cleanup devices,events,rooms --force-cleanup
  mxdx-launcher cleanup devices --older-than 7d --force-cleanup
```

Both use the same `connect()` flow to get a session. Preview is printed to stderr, confirmation is TTY prompt "Are you sure? (y/N)". `--force-cleanup` skips the prompt.

Added to:
- `packages/client/bin/mxdx-client.js` — new `cleanup` commander subcommand
- `packages/launcher/bin/mxdx-launcher.js` — new `cleanup` commander subcommand

### Web Console

- Gear icon added to header nav (next to Dashboard / Logout)
- Routes to settings view (`packages/web-console/src/settings.js`, new file)
- Settings page is a placeholder with one tab: "Server Cleanup"
- Server Cleanup tab:
  - Checkboxes: Devices / Events / Rooms
  - "Older Than" input field (optional, e.g. `2w`, `1m`, `7d`)
  - Password field (required for device cleanup UIA)
  - "Preview Cleanup" button → shows list of affected items
  - "Run Cleanup" button → confirmation modal ("Are you sure? This cannot be undone.") → executes
  - Progress indicator during execution
- Session credentials (`accessToken`, `homeserverUrl`) read from saved session in IndexedDB
- `deviceId` from `client.deviceId()`

## Confirmation Flow

All consumers follow the same pattern:

1. Call `cleanupX()` which returns `{ preview, execute }`
2. Display preview (list of items to be deleted/redacted/left)
3. Ask for confirmation (TTY prompt or modal dialog)
4. If confirmed, call `execute()`
5. Display results

`--force-cleanup` (CLI) skips step 3.

## Error Handling

- 429 rate limits: exponential backoff with max 3 retries per operation
- Partial failures: continue with remaining items, report failures at end
- UIA failures (wrong password): abort device cleanup, report error
- Network errors: abort current target, report what was completed

## Future: Scheduled Cleanup

The launcher's `cleanup` command is designed to be cron-friendly with `--force-cleanup`. Future work could add a `cleanup_schedule` config option to run automatically (e.g., daily device cleanup).
