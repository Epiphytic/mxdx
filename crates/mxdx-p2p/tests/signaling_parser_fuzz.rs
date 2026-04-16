//! Fuzz-target compile-only stub for `signaling::parse::parse_event`.
//!
//! This test binary compiles but is `#[ignore]`d — its job is to ensure
//! the parser surface remains fuzz-ready (no generic lifetimes that
//! escape, no panics on arbitrary bytes, no platform-specific state).
//! For real fuzzing, wire this function into a cargo-fuzz or libfuzzer-sys
//! harness:
//!
//! ```ignore
//! // fuzz/fuzz_targets/signaling_parse.rs
//! #![no_main]
//! use libfuzzer_sys::fuzz_target;
//! use mxdx_p2p::signaling::parse::parse_event;
//! fuzz_target!(|data: &[u8]| {
//!     if let Ok(s) = std::str::from_utf8(data) {
//!         let _ = parse_event(s);
//!     }
//! });
//! ```
//!
//! Phase 4 acceptance per plan T-41: "Fuzz target compiles (doesn't need
//! to actually run in CI)". This file is that proof: the `fuzz_one`
//! function has the fuzz-target signature and calls the public parse
//! entry points; `cargo test -p mxdx-p2p --test signaling_parser_fuzz`
//! compiles it.

use mxdx_p2p::signaling::parse::{parse_content, parse_event, parse_value};
use serde_json::Value;

/// The unit-of-work for a future cargo-fuzz or libfuzzer-sys target.
/// Accepts arbitrary bytes, treats them as UTF-8 when possible, and
/// exercises all three parser entry points. Return value is intentionally
/// dropped — fuzzing cares only about panics / crashes.
pub fn fuzz_one(data: &[u8]) {
    if let Ok(s) = std::str::from_utf8(data) {
        // Top-level string entry.
        let _ = parse_event(s);
        // Value-level entry: attempt to pre-parse, then hand to parse_value.
        if let Ok(v) = serde_json::from_str::<Value>(s) {
            let _ = parse_value(&v);
            // Content-level entry: try every recognized type plus one
            // unrecognized sentinel, to cover every match arm.
            for ty in [
                "m.call.invite",
                "m.call.answer",
                "m.call.candidates",
                "m.call.hangup",
                "m.call.select_answer",
                "m.call.never_seen_before",
                "m.room.message",
            ] {
                let _ = parse_content(ty, &v);
            }
        }
    }
}

#[test]
#[ignore = "compile-only stub; wire into cargo-fuzz for real fuzzing"]
fn fuzz_target_compiles() {
    // Call fuzz_one with a few hand-picked inputs so this test actually
    // executes the entry point when run explicitly. Still #[ignore]d so
    // default CI doesn't run it (fuzzing belongs in a dedicated job).
    fuzz_one(b"");
    fuzz_one(b"{\"type\":\"m.call.invite\",\"content\":{}}");
    fuzz_one(b"{\"type\":\"m.call.reject\",\"content\":{}}");
    fuzz_one(&[0xff, 0xfe, 0xfd]);
    fuzz_one(b"null");
}
