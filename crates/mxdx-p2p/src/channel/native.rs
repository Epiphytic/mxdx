//! Native `WebRtcChannel` implementation. Populated by T-31 (Phase 3) using
//! the `datachannel` crate (FFI to libdatachannel).
//!
//! During T-30 the module is present so that `crates/mxdx-p2p/src/channel/
//! mod.rs` can `pub mod native;` unconditionally on the native target; the
//! type is declared but will not be constructible until T-31 lands the
//! libdatachannel-backed implementation.

/// Placeholder for the native channel type. Constructors and the
/// `WebRtcChannel` impl are added in T-31.
pub struct NativeWebRtcChannel {
    // Intentionally empty during T-30 scaffolding. The T-31 implementation
    // adds the datachannel peer connection handle, the event sender, and
    // the pending-candidate buffer.
    _private: (),
}
