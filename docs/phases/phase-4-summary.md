# Phase 4: Matrix Client Facade — Summary

## Completion Date: 2026-03-06

## What Was Built

### MatrixClient Facade (`crates/mxdx-matrix`)

A wrapper around matrix-sdk 0.16 that provides the only authorized interface to the Matrix protocol. No other crate may import matrix-sdk directly.

**Core Client (`src/client.rs`):**
- `register_and_connect(homeserver_url, username, password)` — registers via REST API, builds matrix-sdk Client with sqlite store for E2EE
- `is_logged_in()`, `crypto_enabled()`, `user_id()` — session introspection
- `create_encrypted_room(invite)` — Megolm-encrypted room with invites
- `create_dm(user_id)` — encrypted direct message room
- `join_room(room_id)` — join by room ID
- `send_event(room_id, payload)` — send custom events (e.g. org.mxdx.command)
- `send_state_event(room_id, event_type, state_key, content)` — send state events
- `sync_once()` — single sync cycle
- `sync_and_collect_events(room_id, timeout)` — sync with automatic E2EE decryption via Room::messages()
- `get_room_state(room_id, event_type)` — fetch state events via REST API
- `inner()` — escape hatch for advanced matrix-sdk usage

**Room Topology (`src/rooms.rs`):**
- `create_launcher_space(launcher_id)` — m.space with exec/status/logs child rooms linked via m.space.child
- `create_terminal_session_dm(user_id)` — encrypted DM with history_visibility=joined (mxdx-aew)
- `tombstone_room(room_id, replacement_room_id)` — m.room.tombstone state event

**Error Handling (`src/error.rs`):**
- `MatrixClientError` enum: Sdk, Http, Registration, RoomNotFound, Other
- From impls for matrix_sdk::Error, HttpError, ClientBuildError, anyhow::Error

### Integration Tests

5 tests running against real Tuwunel homeserver:

| Test | File | Validates |
|:---|:---|:---|
| `client_connects_and_initializes_crypto` | connect.rs | Login + E2EE ed25519 key |
| `two_clients_exchange_encrypted_event` | connect.rs | Full Megolm round-trip with org.mxdx.command |
| `create_launcher_space_creates_space_with_child_rooms` | rooms.rs | Space topology with m.space.child links |
| `terminal_dm_has_joined_history_visibility_from_creation` | rooms.rs | mxdx-aew security: history_visibility=joined |
| `tombstone_room_marks_room_replaced` | rooms.rs | m.room.tombstone state event |

### CI

- `cargo test -p mxdx-matrix` added to integration job (requires tuwunel)

## Security Issues Addressed

- **mxdx-aew**: Terminal DM rooms created with `history_visibility=joined` in initial state, preventing late-joining users from seeing prior messages. Verified by integration test.

## Key Commits

| Commit | Description |
|:---|:---|
| `d3f2c83` | Failing integration tests (TDD) |
| `9f619f8` | MatrixClient facade implementation |
| `4145c9f` | Remove federation #[ignore] |
| `38da9fb` | Room topology helpers |

## Dependencies

- matrix-sdk 0.16 (workspace)
- ruma 0.14 (workspace, resolves to ruma-events 0.32)
- reqwest (for registration and state API)
- tempfile (sqlite store directory)
