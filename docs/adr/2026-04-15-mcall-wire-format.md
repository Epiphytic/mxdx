# ADR 2026-04-15: Adopt standard Matrix VoIP `m.call.*` wire format

**Status:** Accepted
**Date:** 2026-04-15
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
