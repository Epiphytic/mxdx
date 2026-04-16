//! AES-256-GCM defense-in-depth layer for the P2P data channel.
//!
//! `SealedKey` is a sealed newtype with a `pub(in crate::crypto)` constructor;
//! the only way to transport it to a peer is via `signaling::events::build_invite`,
//! which embeds it in a Megolm-encrypted `m.call.invite`. See ADR
//! `2026-04-15-megolm-bytes-newtype.md`.
//!
//! Implemented in Phase 1 (T-11).
