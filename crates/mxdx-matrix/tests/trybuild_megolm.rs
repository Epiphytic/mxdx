//! Structural E2EE invariant: constructing `Megolm<T>` outside `mxdx-matrix`
//! MUST be a compile error. See ADR `2026-04-15-megolm-bytes-newtype.md`.
//!
//! If this test turns red, some future change weakened the newtype (e.g.,
//! widened visibility, added a `pub fn new`, or exposed the tuple field).
//! Re-seal it — do not relax the test.

#[test]
fn megolm_constructor_compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/trybuild/megolm-constructor-fails.rs");
}
