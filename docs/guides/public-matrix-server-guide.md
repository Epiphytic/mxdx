# Using mxdx with a Public Matrix Server

## Overview

mxdx is developed and tested against Tuwunel (a Conduit/Conduwuit fork) running locally. However, the core Matrix interactions use standard Matrix Client-Server API calls and should work with any spec-compliant homeserver. This guide covers what works, what doesn't, and how to configure mxdx for public server use.

## Compatible Public Servers

Any homeserver implementing the Matrix Client-Server API v1.1+ should work for core functionality:

| Server | Status | Notes |
|:---|:---|:---|
| **Synapse** (matrix.org) | Supported | Most widely deployed; full spec compliance |
| **Dendrite** | Supported | Lightweight Go implementation |
| **Conduit / Conduwuit / Tuwunel** | Supported | Rust implementation; mxdx's primary test target |

The critical requirement is support for:
- Password login (`m.login.password`)
- E2EE (Olm/Megolm via `/keys/upload`, `/keys/query`, `/keys/claim`)
- Custom event types in room timelines
- Custom state events
- Matrix Spaces (`m.space` room type, `m.space.child` state events)

All of these are part of the stable Matrix spec and supported by all major implementations.

## Account Setup

### For Public Servers (matrix.org, etc.)

1. **Create an account** through the server's registration flow (Element web, etc.)
2. mxdx uses `m.login.password` for authentication — no registration token needed
3. You need at least **two accounts** for the orchestrator/launcher model

### For Self-Hosted Servers

1. Enable registration with a token in server config
2. mxdx can use `m.login.registration_token` for automated account creation
3. First registered user typically becomes server admin (needed for appservice registration)

## Configuration

Set environment variables for mxdx to connect to a public server:

```bash
# Required
export MXDX_PUBLIC_HS_URL="https://matrix-client.matrix.org"
export MXDX_PUBLIC_USERNAME="your-username"
export MXDX_PUBLIC_PASSWORD="your-password"
```

In `launcher.toml`:
```toml
[global]
launcher_id = "my-launcher"
data_dir = "/var/lib/mxdx"

[[homeservers]]
url = "https://matrix-client.matrix.org"
username = "launcher-bot"
password = "secure-password"
```

## What Works Out of the Box

These features use only standard Matrix Client-Server API calls:

| Feature | API Used | Spec Version |
|:---|:---|:---|
| Login | `POST /_matrix/client/v3/login` (m.login.password) | CS API 1.1+ |
| E2EE setup | `/keys/upload`, `/keys/query`, `/keys/claim` (via matrix-sdk) | CS API 1.1+ |
| Create encrypted rooms | `POST /_matrix/client/v3/createRoom` with `m.room.encryption` initial state | CS API 1.1+ |
| Create DMs | `createRoom` with `is_direct: true` | CS API 1.1+ |
| Create Spaces | `createRoom` with `room_type: m.space` in creation_content | CS API 1.11+ |
| Send custom events | `PUT /_matrix/client/v3/rooms/{id}/send/{type}/{txnId}` | CS API 1.1+ |
| Send state events | `PUT /_matrix/client/v3/rooms/{id}/state/{type}/{stateKey}` | CS API 1.1+ |
| Sync | `GET /_matrix/client/v3/sync` | CS API 1.1+ |
| Room messages | `GET /_matrix/client/v3/rooms/{id}/messages` | CS API 1.1+ |
| Room invites | `POST /_matrix/client/v3/rooms/{id}/invite` | CS API 1.1+ |
| Join rooms | `POST /_matrix/client/v3/join/{roomIdOrAlias}` | CS API 1.1+ |
| Space child linking | `m.space.child` state event | CS API 1.2+ |
| Tombstone rooms | `m.room.tombstone` state event | CS API 1.1+ |
| History visibility | `m.room.history_visibility` initial state | CS API 1.1+ |

Custom event types used by mxdx (`org.mxdx.command`, `org.mxdx.output`, `org.mxdx.secret.request`, etc.) are fully supported by the Matrix spec. The spec allows arbitrary event types — there is no whitelist.

## What Requires Server Admin Access

### Appservice Registration

The mxdx policy engine registers as a Matrix Application Service to claim exclusive user namespaces (e.g., `@agent-*`). This requires server-side configuration.

**On Tuwunel:** mxdx sends `!admin appservices register` with YAML to the `#admins` room — this is a Tuwunel-specific admin command.

**On Synapse:** Appservice registration requires editing `homeserver.yaml` to add the appservice YAML file path, then restarting Synapse.

**On public servers:** Appservice registration is **not available**. You cannot register appservices on matrix.org or other shared public servers.

**Impact:** Without appservice registration, the policy engine cannot claim exclusive user namespaces. This means:
- Virtual users (`@agent-*`) cannot be created via the appservice API
- No server-side enforcement preventing others from registering `@agent-*` usernames
- The policy engine still works for authorization and replay protection, but without namespace isolation

### User Registration

`MatrixClient::register_and_connect()` uses `m.login.registration_token` auth, which requires the server to have token-based registration enabled. Public servers like matrix.org do not support this — they use reCAPTCHA or email-based registration flows.

**Impact:** You must create accounts manually (or via the server's supported registration flow) and use `login_username()` directly.

## Limitations vs Self-Hosted

| Feature | Self-Hosted | Public Server |
|:---|:---|:---|
| Automated user registration | Via registration token | Manual account creation required |
| Appservice registration | Via admin room/config | Not available |
| Exclusive user namespaces | Enforced by appservice | Not enforced |
| Federation control | Full (can disable, configure TLS) | Server operator controls |
| Admin API access | Available to server admin | Not available |
| Custom server configuration | Full control | No control |
| Rate limits | Configurable | Server-imposed (matrix.org: ~10 req/sec) |

## Rate Limiting Considerations

Public servers enforce rate limits. matrix.org uses approximately:
- 10 requests per second for most endpoints
- Stricter limits on `/register` and `/login`
- Sync long-polling is exempt

mxdx's sync loop (`sync_once` with 1-second timeout) should be well within limits. However, rapid room creation (e.g., creating a full launcher topology with 4 rooms in sequence) may hit rate limits on busy public servers. Consider adding delays between room creation calls when targeting public servers.

## Running Compatibility Tests

```bash
export MXDX_PUBLIC_HS_URL="https://matrix-client.matrix.org"
export MXDX_PUBLIC_USERNAME="your-test-account"
export MXDX_PUBLIC_PASSWORD="your-password"

cargo test -p mxdx-matrix --test public_server_compat -- --ignored
```

Tests create rooms and clean up after themselves by leaving rooms. On matrix.org, rooms are garbage-collected after all members leave.

## Recommendations

1. **For development and testing:** Use a local Tuwunel instance (automated, fast, full control)
2. **For production with full features:** Self-host Tuwunel, Synapse, or Dendrite
3. **For quick evaluation:** A public server works for everything except appservice registration
4. **For multi-homeserver redundancy:** Mix self-hosted and public servers in the `[[homeservers]]` config array
