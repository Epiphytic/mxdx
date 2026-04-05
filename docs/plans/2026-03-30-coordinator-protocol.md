# Coordinator Protocol

**Date:** 2026-03-30
**Status:** Implemented (Phase 7)

## Overview

Coordinators are trusted Matrix users that discover workers, read their state, and assign rooms to them. All communication happens over E2EE Matrix rooms using custom state events.

## 1. Worker Discovery

Coordinators discover workers by reading `org.mxdx.worker.state_room` state events from the exec room. Each worker advertises its state room using a `StateRoomPointer`:

- **Event type:** `org.mxdx.worker.state_room`
- **State key:** `{device_id}`
- **Content:**

```json
{
  "room_id": "!stateroom:example.com",
  "device_id": "ABCDEF123",
  "hostname": "node-01.prod",
  "os_user": "deploy"
}
```

A coordinator reads all state events of this type from the exec room to enumerate all active workers. The `hostname` and `os_user` fields allow coordinators to identify workers without joining their state rooms.

## 2. Joining Worker State Rooms

Once a coordinator has the `room_id` from the `StateRoomPointer`, it can:

1. **Join by room ID** if the state room's join rules allow it (e.g., `knock` or `invite`).
2. **Request an invite** from the worker if the room is invite-only.

The state room is always E2EE. The coordinator must complete key exchange before reading or writing state events.

## 3. Room Assignment

Coordinators assign rooms to workers by writing `org.mxdx.worker.room` state events to the worker's state room using a `CoordinatorRoomAssignment`:

- **Event type:** `org.mxdx.worker.room`
- **State key:** `{room_id}` (the assigned room's ID)
- **Content:**

```json
{
  "room_id": "!exec:example.com",
  "room_name": "prod-exec",
  "assigned_by": "@coordinator:example.com",
  "assigned_at": 1742572800,
  "role": "exec"
}
```

The `role` field indicates the purpose of the assigned room (e.g., `exec`, `logs`, `status`). The worker reads these assignments to know which rooms it should monitor.

## 4. Reading Worker Session State

Coordinators read worker session state from the state room:

- **Event type:** `org.mxdx.worker.session`
- **State key:** `{device_id}/{session_uuid}`
- **Content:** `StateRoomSession` struct with uuid, bin, args, state, timestamps, etc.

Empty content (`{}`) signals a removed/completed session. Coordinators should filter these out when enumerating active sessions.

Additional state available in the worker state room:

| Event Type | State Key | Content | Purpose |
|---|---|---|---|
| `org.mxdx.worker.config` | `""` | `WorkerStateConfig` | Room name, capabilities, trust anchor |
| `org.mxdx.worker.identity` | `""` | `WorkerStateIdentity` | Device ID, hostname, OS user |
| `org.mxdx.worker.topology` | `""` | `StateRoomTopology` | Space, exec, status, logs room IDs |
| `org.mxdx.worker.trusted_client` | `{user_id}` | `TrustedEntity` | Trusted client entries |
| `org.mxdx.worker.trusted_coordinator` | `{user_id}` | `TrustedEntity` | Trusted coordinator entries |

## 5. Trust Model

A coordinator must be in the worker's `trusted_coordinators` list to be granted access to the state room. This list is managed via `org.mxdx.worker.trusted_coordinator` state events.

**Trust establishment flow:**

1. Worker operator adds the coordinator's user ID to the `trusted_coordinators` configuration (or the worker's initial trust anchor does so).
2. Worker writes a `TrustedEntity` state event with the coordinator's user ID as the state key.
3. Coordinator joins the state room (invited by the worker or via knock).
4. Worker validates that the joining user is in the trusted coordinators list before allowing state reads.

**Security constraints:**

- All state room communication is E2EE (encrypted state events via MSC4362).
- The worker validates room creator and encryption status on every access (`validate_state_room`).
- Coordinators cannot write session state -- only workers write sessions.
- Coordinators can only write room assignments (`org.mxdx.worker.room`).
- Room power levels should restrict who can send which event types.

## Data Types

All types are defined in `crates/mxdx-types/src/events/state_room.rs`:

- `StateRoomPointer` -- advertised in exec room for discovery
- `CoordinatorRoomAssignment` -- written by coordinator to assign rooms
- `StateRoomSession` -- written by worker to track sessions
- `WorkerStateConfig` -- worker configuration
- `WorkerStateIdentity` -- worker device/host identity
- `StateRoomTopology` -- room topology pointers
- `TrustedEntity` -- trusted client or coordinator entry
