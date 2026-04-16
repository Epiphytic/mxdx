// Negative test: constructing `Megolm<T>` from outside `mxdx-matrix` must
// fail to compile. The tuple field is `pub(crate)`, so the tuple-struct
// constructor `Megolm(...)` is not callable from external crates.
//
// See ADR `2026-04-15-megolm-bytes-newtype.md`. If this file starts
// compiling, the E2EE invariant has been weakened — re-seal the newtype.

use mxdx_matrix::Megolm;

fn main() {
    let _leaked: Megolm<Vec<u8>> = Megolm(vec![0u8; 8]);
}
