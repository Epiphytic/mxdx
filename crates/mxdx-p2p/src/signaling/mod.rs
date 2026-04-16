//! Matrix VoIP `m.call.*` event de/serialization and glare resolution.
//!
//! See ADR `2026-04-15-mcall-wire-format.md` (and its 2026-04-16 addendum) plus
//! ADR `2026-04-16-coordinated-rust-npm-releases.md`. Implemented in Phase 4:
//! T-40 events ([`events`]), T-41 parser ([`parse`]), T-42 glare resolver
//! ([`glare`]).

pub mod events;
pub mod parse;
