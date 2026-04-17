# ADR 2026-04-16: Coordinated Rust/npm releases are the default for cross-runtime features

**Status:** Accepted
**Date:** 2026-04-16
**Related:** `docs/adr/2026-04-15-mcall-wire-format.md`, `docs/plans/2026-04-15-rust-p2p-interactive-map.md`

## Context

The mxdx project ships both Rust binaries (`mxdx-client`, `mxdx-worker`) and npm packages (`@mxdx/core`, `@mxdx/launcher`, `@mxdx/client`, `@mxdx/web-console`, `@mxdx/cli`) that speak to each other on the wire (Matrix VoIP events, AES-GCM frames, terminal-protocol messages, session-room state).

Two failure modes are possible when wire formats change:

1. **Silent one-sided changes.** A Rust PR changes a field name or event shape; the npm code still reads/writes the old name; interop tests that exercise both runtimes break in non-obvious ways.
2. **Sequential rollouts.** Rust ships a wire change in release N, npm catches up in release N+1; any mxdx deployment that mixes runtime versions (one peer running the new Rust build, another peer running the old npm build) breaks interop in the window between N and N+1.

During Phase 4 of the Rust P2P interactive sessions port (2026-04-16), a divergence was discovered between the ADR spec (`mxdx_session_key`) and the deployed npm code (`session_key`) for the `m.call.invite` AES key field. The resolution — keep `mxdx_session_key`, update npm to match — surfaced the need for an explicit policy about cross-runtime changes.

## Decision

**For any feature that affects the wire format between Rust and npm runtimes, the release is coordinated across both ecosystems by default.** The single default behavior is:

- The Rust change lands in the same branch/PR as the corresponding npm change (or in paired PRs merged together).
- Both Rust crates and npm packages bump their versions and ship to their registries on the same day.
- Wire-format tests (cross-language vector tests, cross-runtime interop tests) gate the release on both sides passing.

This applies to:

- Matrix event schemas (types, field names, required/optional fields)
- AES-GCM / binary frame formats
- Terminal-protocol message shapes
- Session-room state shapes
- Any other serialization the two runtimes agree on over a network or file

This does NOT apply to:

- Rust-only internals (e.g., `mxdx-worker` internal APIs that never cross a process boundary)
- npm-only internals (e.g., `@mxdx/web-console` UI components that don't emit wire events)
- Changes that are provably backwards-compatible on the wire (e.g., adding a new optional field that old receivers ignore). Additions like this still SHOULD ship coordinated as a matter of hygiene, but are not REQUIRED to.

## Rationale

- **Single source of truth is the goal.** Byte-exact wire compatibility between Rust and npm is the central promise of the cross-runtime architecture (see `docs/adr/2026-04-15-mcall-wire-format.md` and storm spec Q1). Coordinated releases keep that promise mechanical rather than aspirational.
- **Mixed-version deployments are explicitly in-scope.** Tests at `packages/e2e-tests/tests/` exercise Rust↔Rust, npm↔Rust, Rust↔npm, and npm↔npm — eight combinations × two topologies (single-HS + federated). An uncoordinated release would invalidate four of those combinations for as long as versions drift.
- **ADRs should match deployed code, not the other way around.** When an ADR specifies a field name and deployed code uses a different one, the natural resolution is NOT to silently update the ADR to match the code (which weakens ADR discipline) OR to leave them divergent (which guarantees test failures). Coordinated releases are the mechanism that keeps the ADR authoritative AND the code reality.
- **The cost is low for mxdx specifically.** The project already has a single release workflow (`.github/workflows/release.yml`) that publishes to both crates.io (via OIDC) and npm (via OIDC). Coordinated releases are an organizational discipline, not a tooling problem.

## Consequences

- Every PR that touches wire-format code in `crates/mxdx-p2p/src/signaling/events.rs` (or any other wire-facing module) must also touch the corresponding npm file(s) (or explicitly document why no npm change is needed).
- CI adds a `wire-format-parity` check that runs a cross-language round-trip test on any PR touching wire-format code: if Rust emits JSON that npm can't parse, or vice versa, the PR fails. The `cross-vectors` job already partially covers this for `P2PCrypto`; it expands to signaling events in Phase 4 and to other wire formats as they're introduced.
- Release notes call out wire-format changes explicitly in both `CHANGELOG.md` (Rust) and the npm package changelogs.
- A "last compatible version" table is maintained in the project `README.md` documenting which Rust versions speak to which npm versions; coordinated releases keep this table trivial (always "N ↔ N").
- Breaking wire changes without a corresponding npm update are rejected at PR review.

## Alternatives considered and rejected

- **Loose coupling with version negotiation.** Have the wire protocol carry a version field so mixed-version deployments can fall back to a compatible mode. Rejected for mxdx: the project is a closed ecosystem where the Rust and npm packages ship together, not a public protocol where third-party implementers need version negotiation. The complexity isn't worth it.
- **Forever-backwards-compatible wire format.** Never rename fields, never remove fields, always add as optional. Rejected because it leads to cruft-accumulation over time and doesn't help with structural changes (e.g., changing the shape of a nested object). Coordinated releases let you actually clean things up.
- **Leave Rust and npm independently versioned.** Rejected because it guarantees the test matrix will break every time one runtime ships a wire change ahead of the other. Four of eight interop combinations fail in the window between releases.

## Implementation notes

- This is a policy ADR. The only code artifact is the `wire-format-parity` CI gate. Each future wire-format change adds to the parity checks as needed.
- The first test of this policy is the `mxdx_session_key` field-name migration in Phase 4 of the Rust P2P port: both Rust (new emitter) and npm (rename from `session_key` to `mxdx_session_key`) ship in the same coordinated release.
- If a future situation makes a coordinated release genuinely impossible (e.g., npm package registry outage during a critical Rust deploy), document the deviation inline in a `NOTE: coordinated release deviation` comment on the relevant PR and create a follow-up beads task to remediate within the next release cycle.
