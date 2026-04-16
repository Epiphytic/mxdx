# ADR 2026-04-16: Enable matrix-sdk `testing` feature in production when it gates an otherwise-stable API

**Status:** Accepted
**Date:** 2026-04-16
**Related:** `docs/adr/2026-04-15-megolm-bytes-newtype.md`, `docs/adr/2026-04-16-ephemeral-key-cross-cert.md`, `docs/adr/2026-04-16-coordinated-rust-npm-releases.md`, beads `mxdx-awe.52`

## Context

Two Phase-4/6 investigations surfaced the same structural issue in matrix-sdk 0.16: APIs that the mxdx security model needs are implemented and public at the `matrix-sdk-crypto` layer but gated at the `matrix-sdk` facade by the crate's `testing` cargo feature:

1. **`OlmMachine::encrypt_room_event_raw`** — reachable only via `matrix_sdk::Client::olm_machine_for_testing()` on stable 0.16. Needed by the Phase 1 Megolm fallback to produce byte-identical ciphertext on both P2P and Matrix transport paths (storm §3.2/§3.3 original intent).
2. **`OlmMachine::sign(&str)`** — also reachable only via `olm_machine_for_testing()`. Needed by the Phase 6 Verifying handshake to sign the transcript with the device's long-term Ed25519 key (storm §3.1 original intent).

Both APIs are fully public and stable on the matrix-rust-sdk `main` branch (verified 2026-04-16 at `crates/matrix-sdk-crypto/src/machine/mod.rs:2817`; `crates/matrix-sdk/src/encryption/mod.rs:864-878`). The `testing` feature exists because the matrix-sdk facade has historically kept the `OlmMachine` accessor `pub(crate)` to preserve the crate's abstraction boundary — it is not a marker for instability of the underlying primitives.

Three options were evaluated for unblocking the mxdx uses:

- **Vendor or fork matrix-sdk.** Rejected — disproportionate maintenance cost.
- **Wait for matrix-sdk 0.17+.** Rejected — blocks the Rust P2P rollout on external release timing. Both uses already shipped with workarounds (semantic equivalence for §3.2/§3.3, ephemeral-key hybrid for §3.1).
- **Enable matrix-sdk's `testing` feature on the mxdx dependency.** The name is misleading ("testing") but the accessor it gates is the only stable way to reach APIs the mxdx security model requires.

## Decision

**Enable matrix-sdk's `testing` cargo feature on the production `mxdx-matrix` dependency, when and only when** the feature gates an otherwise-stable API that mxdx needs. The decision is made per-API, with three preconditions:

1. **Stability check on main.** The API must be fully public and stable on matrix-rust-sdk's `main` branch (verified by URL + commit hash at decision time).
2. **No public equivalent.** No stable-feature accessor exists that achieves the same result.
3. **Bounded transitive cost.** The `testing` feature pulls `wiremock`, `matrix-sdk-test`, and `assert_matches2` into the production dependency graph. These are dev-quality crates but are themselves audited and maintained by the matrix-sdk team. Binary-size and compile-time cost is accepted.

When all three preconditions hold, enabling `testing` in production is preferred over vendor forks, fork-tracking branches, or trust-model workarounds.

## Rationale

- **The feature name is a misnomer, not a signal.** The `testing` feature gates the accessor, not the underlying primitive. `OlmMachine::sign` and `OlmMachine::encrypt_room_event_raw` are the primitives the `matrix-sdk-crypto` team exposes for production use — the facade wrapping is what's gated.
- **Avoids maintenance debt.** Vendoring or forking matrix-sdk creates a recurring fork-tracking obligation (new releases, new security fixes). Enabling an existing feature has zero marginal cost on rebase.
- **Makes upgrade trivial when a proper accessor lands.** Once matrix-sdk ships a non-`testing`-gated accessor, mxdx flips one cargo feature line and the testing dep tree disappears. ADRs that depend on the workaround get a ±1 line addendum.
- **Preserves the cardinal rule.** The APIs in question (`encrypt_room_event_raw`, `sign`) are the correct primitives for E2EE — using them is *more* aligned with the project rule than workarounds that introduce new trust footguns (ephemeral-key publication, plaintext-then-re-encrypt paths).

## Consequences

- `crates/mxdx-matrix/Cargo.toml` adds `features = ["testing", ...]` on its `matrix-sdk` dependency.
- The transitive dev-quality deps (`wiremock`, `matrix-sdk-test`, `assert_matches2`) become part of the mxdx production build. Acceptable.
- Every use of a `testing`-gated API must cite this ADR in a comment adjacent to the call site, naming the API and the specific reason the public alternative does not work.
- Periodic review (each `matrix-sdk` minor-version bump): re-verify the stability precondition on `main` and check whether a non-`testing` accessor has landed. If yes, drop the `testing` feature and delete the workaround.
- The first two uses under this policy:
  - **Phase 1 retrofit** (beads `mxdx-awe.53`): switch `MatrixClient::send_megolm` from `room.send_raw` to `OlmMachine::encrypt_room_event_raw` via `Client::olm_machine_for_testing()`. Restores byte-identical Megolm ciphertext across P2P and Matrix transport paths. Revises addendum in ADR `2026-04-15-megolm-bytes-newtype.md`.
  - **Phase 6 retrofit** (beads `mxdx-awe.52`): switch `MatrixHandshakeSigner` from ephemeral-key publication to direct device-key signing via `OlmMachine::sign()`. Restores storm §3.1 cryptographic-binding property. Revises ADR `2026-04-16-ephemeral-key-cross-cert.md` (status → Superseded-When-Retrofit-Lands).

## Alternatives considered and rejected

- **Vendor matrix-sdk (Option A2).** Rejected — per-release fork-tracking cost dominates the benefit for two single-function accessors.
- **Keep the workarounds in place indefinitely.** Rejected — both workarounds carry real semantic cost (non-byte-identical ciphertext; ephemeral-key trust chain). Acceptable as stop-gaps, not as destinations.
- **Wait for matrix-sdk 0.17+ with a non-`testing` public accessor.** Rejected as a precondition; accepted as an eventual trigger for reverting to the public API. The mxdx Rust P2P rollout cannot wait.
- **Contribute a non-`testing`-gated accessor upstream.** Not rejected — *complementary*. A PR against matrix-rust-sdk that exposes `Client::olm_machine()` publicly (or adds narrower `sign`/`encrypt_room_event_raw` shims on `Client`) is the long-term fix. mxdx-awe.52 and mxdx-awe.53 should reference upstream PR links if/when they are opened.

## Scope clarifications

This ADR does **not** authorize:

- Enabling `testing`-style features on arbitrary third-party crates as a matter of convenience. Each crate's `testing` feature must be individually evaluated against the three preconditions.
- Using `testing`-gated matrix-sdk APIs for non-security-critical work (logging, metrics, UI). Where a production-stable API exists, use it.
- Skipping ADR+review process when enabling the feature for a new use. Each new use is a new decision that must cite this ADR *and* document the API, the public-equivalent check, and the rationale.

## Review triggers

Revisit this decision when any of the following occur:

- matrix-sdk ships a non-`testing` public accessor for `olm_machine()` (target: matrix-sdk 0.17 or later).
- An audited CVE lands in `wiremock`, `matrix-sdk-test`, or `assert_matches2`. Even if exploit path is test-only, production dependence on the crate means mxdx has to patch.
- A second or third `testing`-gated API becomes load-bearing for mxdx — if the surface grows, the vendor-fork trade-off rebalances.
