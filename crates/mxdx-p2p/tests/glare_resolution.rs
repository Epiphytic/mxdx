#![cfg(not(target_arch = "wasm32"))]
//! Property-based tests for `signaling::glare::resolve`.
//!
//! Three invariants locked (per plan T-42):
//! 1. **Agreement** — both peers computing `resolve` always reach the same
//!    winner. Formally: `resolve(a, b, ca, cb) == resolve(b, a, cb, ca).invert()`.
//! 2. **Totality** — the function is defined for every combination of
//!    arbitrary UTF-8 strings; no panics on any input.
//! 3. **Determinism** — calling twice with identical inputs returns the
//!    same result.

use mxdx_p2p::signaling::glare::{resolve, GlareResult};
use proptest::prelude::*;

proptest! {
    /// Peer-agreement property: whatever we see, our peer sees the
    /// inverted answer — so both sides converge on the same winner.
    ///
    /// Excludes the degenerate case where `a == b && ca == cb` (same
    /// user_id AND same call_id). That represents "the same invite" —
    /// not a glare race — and `resolve` returns a deterministic constant
    /// (`WeWin`) in that case, which cannot satisfy the swap invariant
    /// because swapping the arguments produces byte-identical input.
    /// Phase 5 short-circuits before calling `resolve` with identical
    /// inputs; the function is still total so the call is safe, but the
    /// agreement invariant doesn't apply.
    #[test]
    fn agreement_both_peers_converge(
        a in "[A-Za-z0-9_:@.-]{1,64}",
        b in "[A-Za-z0-9_:@.-]{1,64}",
        ca in "[A-Za-z0-9]{1,32}",
        cb in "[A-Za-z0-9]{1,32}",
    ) {
        prop_assume!(!(a == b && ca == cb));
        let ours = resolve(&a, &b, &ca, &cb);
        let theirs = resolve(&b, &a, &cb, &ca);
        prop_assert_eq!(ours, theirs.invert());
    }

    /// Totality property: no panic on any combination of arbitrary UTF-8
    /// strings, including empty and pathological cases.
    #[test]
    fn total_no_panic_on_arbitrary_strings(
        a in ".{0,200}",
        b in ".{0,200}",
        ca in ".{0,200}",
        cb in ".{0,200}",
    ) {
        // Just run the function — failure is a panic.
        let _ = resolve(&a, &b, &ca, &cb);
    }

    /// Determinism property: repeated calls with the same inputs produce
    /// the same result. Guards against stateful bugs.
    #[test]
    fn deterministic(
        a in "[A-Za-z0-9_:@.-]{1,64}",
        b in "[A-Za-z0-9_:@.-]{1,64}",
        ca in "[A-Za-z0-9]{1,32}",
        cb in "[A-Za-z0-9]{1,32}",
    ) {
        let first = resolve(&a, &b, &ca, &cb);
        let second = resolve(&a, &b, &ca, &cb);
        prop_assert_eq!(first, second);
    }

    /// Equal user_ids: the function still satisfies agreement by falling
    /// back to call_id. Excludes `ca == cb` (same invite, degenerate).
    #[test]
    fn agreement_holds_when_user_ids_tie(
        u in "[A-Za-z0-9_:@.-]{1,64}",
        ca in "[A-Za-z0-9]{1,32}",
        cb in "[A-Za-z0-9]{1,32}",
    ) {
        prop_assume!(ca != cb);
        let ours = resolve(&u, &u, &ca, &cb);
        let theirs = resolve(&u, &u, &cb, &ca);
        prop_assert_eq!(ours, theirs.invert());
    }
}

// ---------------------------------------------------------------------------
// Deterministic spec cases: concrete examples codifying the Matrix-spec rule.
// ---------------------------------------------------------------------------

#[test]
fn spec_case_lower_user_id_wins() {
    // Real Matrix user_ids — lower lexicographically wins.
    assert_eq!(
        resolve("@alice:matrix.org", "@bob:matrix.org", "c1", "c2"),
        GlareResult::WeWin
    );
    assert_eq!(
        resolve("@bob:matrix.org", "@alice:matrix.org", "c2", "c1"),
        GlareResult::TheyWin
    );
}

#[test]
fn spec_case_agreement_sampled() {
    // Spot-check a handful of NON-DEGENERATE glare combinations against
    // the peer-swap invariant. Degenerate `a == b && ca == cb` (same
    // invite) is not a glare race and is out of scope for the agreement
    // property — see the `agreement_both_peers_converge` proptest docstring.
    for (a, b, ca, cb) in [
        ("@alice:ex", "@bob:ex", "call-1", "call-2"),
        ("@zz:ex", "@aa:ex", "x", "y"),
        ("@me:ex", "@me:ex", "alpha", "beta"),
        ("@me:ex", "@me:ex", "beta", "alpha"),
    ] {
        let ours = resolve(a, b, ca, cb);
        let theirs = resolve(b, a, cb, ca);
        assert_eq!(
            ours,
            theirs.invert(),
            "peer swap broke for ({a}, {b}, {ca}, {cb})"
        );
    }
}

#[test]
fn degenerate_identical_inputs_are_deterministic_not_agreement() {
    // Locks documented behavior: when `resolve` is called with the same
    // invite on both sides (a == b && ca == cb), it returns WeWin
    // deterministically. The agreement invariant explicitly excludes this
    // case because swapping byte-identical inputs is a no-op. Phase 5
    // short-circuits before calling resolve with identical inputs.
    let r = resolve("@a:ex", "@a:ex", "same", "same");
    assert_eq!(r, GlareResult::WeWin);
    assert_eq!(r, resolve("@a:ex", "@a:ex", "same", "same"));
}
