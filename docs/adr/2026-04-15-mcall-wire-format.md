# ADR 2026-04-15: Adopt standard Matrix VoIP `m.call.*` wire format

**Status:** Accepted
**Date:** 2026-04-15
**Implemented in:** Branch `brains/rust-p2p-interactive`, Phase 4 (T-40..T-44)
**Related:** `docs/plans/2026-04-15-rust-p2p-interactive-storm.md`

## Context

Two incompatible wire formats currently exist in the repository for WebRTC signaling:

1. **npm+wasm path** (working, production-tested): uses the standard Matrix VoIP call protocol — `m.call.invite`, `m.call.answer`, `m.call.candidates`, `m.call.hangup`, `m.call.select_answer`. SDP and ICE travel in the event content. The AES-GCM session key for the P2P frame encryption is carried in a custom field `mxdx_session_key` inside `m.call.invite`, which is Megolm-encrypted by the room because session rooms are E2EE (MSC4362). Glare resolution follows the standard Matrix spec (lexicographic user_id).

2. **Rust stubs** (`crates/mxdx-types/src/events/webrtc.rs`, unused on the wire): defines a custom split-signaling schema — `org.mxdx.session.webrtc.offer`/`answer` as thread metadata events + `org.mxdx.webrtc.sdp`/`ice` as to-device messages carrying SDP and ICE.

These two formats cannot interoperate. A Rust peer implementing option 2 cannot talk to the existing npm peer; an npm peer cannot talk to a Rust peer using option 2. Interop matters because:

- Mixed deployments exist during migration (some clients Rust, some npm, some browser)
- The project rule says: "end-to-end tests using the beta server accounts in `test-credentials.toml`" — the beta accounts will be exercised by both runtimes during rollout
- Web-console switchover is delivery step 8 — the native Rust rollout (steps 6-7) must interop with the still-npm web-console

## Decision

**Adopt the npm wire format (option 1) across all Rust code.** The custom `org.mxdx.session.webrtc.*` event schema in `mxdx-types` is deleted in its entirety. The Rust `mxdx-p2p` crate emits and parses standard Matrix VoIP events.

Specifically:

- Event types: `m.call.invite`, `m.call.answer`, `m.call.candidates`, `m.call.hangup`, `m.call.select_answer` (version `"1"`)
- Fields: standard Matrix VoIP fields (`call_id`, `party_id`, `version`, `lifetime`, `offer`/`answer`/`candidates`/`reason`/`selected_party_id`)
- **Extension field** on `m.call.invite`: `mxdx_session_key` (base64 AES-256 key). This field is a documented extension to the Matrix call event; it inherits room E2EE because session rooms are Megolm+MSC4362-encrypted. Non-mxdx Matrix clients will ignore the field.
- TURN provisioning: `GET /_matrix/client/v3/voip/turnServer` (standard Matrix endpoint)
- Glare resolution: standard Matrix rule — lower lexicographic `user_id` wins

## Rationale

- **Interop is non-negotiable.** Q1 of the storm locked this: "Rust ↔ npm must interoperate over P2P" — this is the only wire format that achieves it without rewriting the npm peer.
- **The npm path is working and production-tested.** Changing it introduces risk for zero benefit; changing the unused Rust stubs introduces risk for demonstrable benefit (interop).
- **Matrix VoIP is a standard.** Other Matrix clients that implement voice/video calls (Element, Element Call, SchildiChat, etc.) speak this protocol. Adopting it means the signaling path is not mxdx-specific — only the `mxdx_session_key` extension field is.
- **E2EE is preserved.** The Matrix VoIP events are room events in E2EE session rooms → Megolm-encrypted by the existing pipeline. The embedded `mxdx_session_key` is protected by the same Megolm session. No new send path, no new encryption, no new bypass.
- **The divergent Rust schema had a design rationale** (split thread metadata + to-device SDP for auditability and private SDP), but the benefit does not outweigh the interop cost given the npm peer is already deployed.

## Consequences

- **Delete** `crates/mxdx-types/src/events/webrtc.rs` in its entirety and its `mod webrtc;` declaration in `crates/mxdx-types/src/events/mod.rs`.
- **Delete** references to `WebRtcOffer`/`WebRtcAnswer`/`WebRtcSdp`/`WebRtcIce` in `crates/mxdx-worker/src/webrtc.rs` and anywhere else in the workspace.
- New module `crates/mxdx-p2p/src/signaling/events.rs` defines `CallInvite`, `CallAnswer`, `CallCandidates`, `CallHangup`, `CallSelectAnswer`. Field shapes are documented in the Matrix spec; the `mxdx_session_key` extension field is documented inline.
- The sync filter in `mxdx-matrix` adds `m.call.*` event types to the receive path.
- Cross-runtime interop is possible; the four interop combinations in storm §5.4 become tractable.

## Alternatives considered and rejected

- **Keep the custom `org.mxdx.session.webrtc.*` schema and update npm to match:** rejected — the npm path works today and updating it would require a coordinated release across packages that doubles implementation risk.
- **Dual-protocol support (Rust speaks both):** rejected — adds complexity and does not eliminate the eventual cut-over. The npm peer would still need updating to the mxdx schema, which is the rejected path above.
- **Plain Matrix message events instead of Matrix VoIP events:** rejected — Matrix VoIP gives free TURN provisioning, established glare semantics, and compatibility with existing Matrix infrastructure.

## Addendum (2026-04-16) — Field-name reconciliation: `mxdx_session_key` (not `session_key`)

Phase 4 grooming of the Rust P2P port surfaced a divergence between this ADR and the deployed npm code. The ADR specifies the AES-GCM session-key extension field on `m.call.invite` as **`mxdx_session_key`**. The deployed npm code in `packages/core/p2p-signaling.js:53`, `packages/launcher/src/runtime.js` (offerer + answerer), and `packages/web-console/src/terminal-view.js` (offerer + answerer) uses **`session_key`** (unprefixed). There are zero references to `mxdx_session_key` anywhere in the deployed npm packages.

### Decision (2026-04-16)

**Keep `mxdx_session_key` as the canonical field name.** Update the npm packages to match the Rust implementation in this phase, shipped as a coordinated Rust+npm release per ADR `2026-04-16-coordinated-rust-npm-releases.md`.

### Rationale

- The `mxdx_` prefix carries real value: it namespaces the mxdx extension against other Matrix VoIP implementations and against a hypothetical future standardization of a similar field with a different shape. Dropping the prefix to match npm would be a cosmetic capitulation.
- Coordinated releases are the project's default policy (see the companion ADR). The migration cost — updating five lines across three npm files plus rebuilding dist bundles and regenerating any hard-coded JS test fixtures — is bounded and mechanical.
- The Rust port hasn't shipped yet (branch `brains/rust-p2p-interactive`, `p2p_enabled` defaults to `false` through Phase 9). Flipping npm and Rust atomically in the same release means there is no mixed-version window.
- Existing beta E2E tests (`packages/e2e-tests/tests/p2p-*.test.js`) need minor fixture updates for the field rename — absorbed into the Phase 4 work.

### Implementation placement

Added as a new task **T-44** in Phase 4 of the map plan (`docs/plans/2026-04-15-rust-p2p-interactive-map.md`): npm wire-format migration from `session_key` to `mxdx_session_key`. Scope: `packages/core/p2p-signaling.js`, `packages/launcher/src/runtime.js`, `packages/web-console/src/terminal-view.js`, any JS test fixtures referencing the old field. Acceptance: after T-44 lands, `grep -r 'session_key' packages/ | grep -v 'mxdx_session_key'` returns nothing.

### Second, smaller reconciliation: `lifetime` default

npm's `sendInvite` defaults `lifetime` to `60000` (ms) but every deployed call site explicitly passes `30000`. Storm §4.1 treats 30s as the invite timeout envelope. Both sides adopt `30000` as the default constant. npm change: `p2p-signaling.js:45` `lifetime = 60000` → `lifetime = 30000`. Rust change: `CallInvite::DEFAULT_LIFETIME_MS = 30_000`. Absorbed into T-40 (Rust) and T-44 (npm).

### Third note: `session_uuid` on invite

Storm §3.1 pseudocode shows `session_uuid` as a field on `m.call.invite`, but the deployed npm code does not populate it on invites (it's only on `session.*` events). The Rust `CallInvite` struct keeps `session_uuid: Option<String>` with `#[serde(skip_serializing_if = "Option::is_none")]` — absent on the wire when `None`, matching deployed npm behavior. No npm change needed.
