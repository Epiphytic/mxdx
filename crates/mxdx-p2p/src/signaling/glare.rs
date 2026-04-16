//! Deterministic glare resolution for concurrent `m.call.invite`.
//!
//! When both peers issue an invite for the same logical call within the
//! network window, the Matrix VoIP spec's resolution rule — lower
//! lexicographic `user_id` wins — ensures both sides converge on the same
//! winner without further negotiation. See storm §3.5 and
//! `packages/core/p2p-signaling.js:114-116` for the npm equivalent.
//!
//! This module is a single pure function that returns [`GlareResult::WeWin`]
//! or [`GlareResult::TheyWin`]. It has no I/O, no async, no panics, and no
//! internal state. Property-based tests in
//! `crates/mxdx-p2p/tests/glare_resolution.rs` lock:
//!
//! - Agreement: both peers computing `resolve(ours, theirs, ...)` always
//!   reach the same winner.
//! - Totality: the function is defined for every combination of
//!   `(str, str, str, str)` — no panics on arbitrary inputs including
//!   non-UTF8 byte sequences (the input is `&str` which is always UTF-8,
//!   but we exercise the full `str` range in proptest).
//! - Determinism: calling twice with the same inputs returns the same
//!   result.
//! - Tie-break consistency: when user_ids are equal, the call_id tie-break
//!   also satisfies the peer-agreement property.
//!
//! # Side-channel analysis
//!
//! `resolve` compares only public values — `user_id` and `call_id` are
//! sent over the wire unencrypted (well, Megolm-encrypted for the room
//! but visible to any joined device, including the peer). There is no
//! secret involved; a timing side-channel on `str::cmp` would leak only
//! information already available on the wire. The `match` arms each
//! produce a single `GlareResult` constant, so branch-prediction-based
//! side channels, even if they existed, would leak the public winner —
//! which the peer trivially computes itself.

/// The result of a glare resolution from the perspective of the local peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlareResult {
    /// Our invite wins — continue establishing our call, send
    /// `m.call.select_answer` once we receive the remote answer.
    WeWin,
    /// Their invite wins — hang up our outbound call and answer theirs.
    TheyWin,
}

/// Resolve a glare race per the Matrix VoIP spec.
///
/// Lower lexicographic `user_id` wins. Tie-break on `call_id` (also lower
/// wins) — this case is exercisable only when both peers share the same
/// Matrix user_id (e.g. launcher and browser tabs of the same account);
/// the function is still total over arbitrary string inputs to satisfy
/// the property-test totality invariant.
///
/// This function is pure: no I/O, no async, no state, no panics.
pub fn resolve(
    our_user_id: &str,
    their_user_id: &str,
    our_call_id: &str,
    their_call_id: &str,
) -> GlareResult {
    use std::cmp::Ordering;
    match our_user_id.cmp(their_user_id) {
        Ordering::Less => GlareResult::WeWin,
        Ordering::Greater => GlareResult::TheyWin,
        Ordering::Equal => match our_call_id.cmp(their_call_id) {
            Ordering::Less => GlareResult::WeWin,
            Ordering::Greater => GlareResult::TheyWin,
            // Equal user_id AND call_id means the same invite, not a
            // glare race. Treat it deterministically as "we win" so the
            // caller can move on without a state machine wedge — Phase 5
            // will short-circuit before ever calling resolve with identical
            // inputs, but the function must be total.
            Ordering::Equal => GlareResult::WeWin,
        },
    }
}

impl GlareResult {
    /// Invert the result. Used by the property tests to express the
    /// peer-agreement invariant (`resolve(a, b) == resolve(b, a).invert()`)
    /// without branching — also a useful building block for the Phase 5
    /// state machine when it needs to reason about "what would the peer
    /// have computed."
    pub fn invert(self) -> Self {
        match self {
            GlareResult::WeWin => GlareResult::TheyWin,
            GlareResult::TheyWin => GlareResult::WeWin,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_user_id_wins() {
        assert_eq!(
            resolve("@alice:ex", "@bob:ex", "c1", "c2"),
            GlareResult::WeWin
        );
        assert_eq!(
            resolve("@bob:ex", "@alice:ex", "c2", "c1"),
            GlareResult::TheyWin
        );
    }

    #[test]
    fn same_user_id_falls_back_to_call_id() {
        // Launcher + browser tab of the same user — tie-break on call_id.
        assert_eq!(
            resolve("@me:ex", "@me:ex", "aaa", "bbb"),
            GlareResult::WeWin
        );
        assert_eq!(
            resolve("@me:ex", "@me:ex", "ccc", "bbb"),
            GlareResult::TheyWin
        );
    }

    #[test]
    fn identical_inputs_are_deterministic() {
        // Same user_id AND call_id — not a glare race but function must
        // be total. Always returns WeWin deterministically.
        assert_eq!(
            resolve("@me:ex", "@me:ex", "same", "same"),
            GlareResult::WeWin
        );
        // Double-check determinism by calling twice.
        assert_eq!(
            resolve("@me:ex", "@me:ex", "same", "same"),
            resolve("@me:ex", "@me:ex", "same", "same")
        );
    }

    #[test]
    fn matches_npm_reference_behavior() {
        // npm reference: P2PSignaling.resolveGlare(remote) returns
        // 'win' if localUserId < remoteUserId else 'lose'.
        // packages/core/p2p-signaling.js:115.
        assert_eq!(
            resolve("@alice:ex", "@bob:ex", "c1", "c1"),
            GlareResult::WeWin
        );
        assert_eq!(
            resolve("@bob:ex", "@alice:ex", "c1", "c1"),
            GlareResult::TheyWin
        );
        // Edge: equal user_ids are outside npm's reference contract (npm
        // treats "not less than" as "lose"). Our implementation is more
        // explicit via the call_id tie-break — captures the intent for
        // the mxdx same-user-two-devices scenario.
    }
}
