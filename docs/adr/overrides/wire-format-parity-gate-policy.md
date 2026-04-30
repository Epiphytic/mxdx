# Wire-Format-Parity Gate Policy

**ADR:** docs/adr/2026-04-29-rust-npm-binary-parity.md (requirement 12a)
**Status:** Active
**Date:** 2026-04-30

## Purpose

This document defines the "green" policy for the `wire-format-parity` CI gate before it is enabled as a required check. It addresses: retry budgets, flaky-quarantine mechanics, P2P advisory ramp, and quarantine time bounds.

## 1. Retry Budget

Each combination (t1a through t4b) MAY be automatically retried by CI up to **2 times** before the combination is marked as failed. If a combination fails on all 3 attempts (initial + 2 retries), it is a hard failure, not a flaky signal.

GitHub Actions implementation: use `retry-action` or a shell loop with `cargo test || cargo test || cargo test` pattern. Do NOT use unlimited retries — a combination that fails consistently is a broken test or broken code, not flakiness.

## 2. Quarantine Mechanism

A combination MAY be flagged as `[flaky-quarantine]` when:
- It has failed intermittently (not on every run) across at least 3 distinct CI runs.
- A tracking issue exists documenting the flakiness and the investigation plan.

### Quarantine annotation

Add the annotation to the combination's test body comment:

```js
// [flaky-quarantine] mxdx-XXXX — <one-line reason> — expires YYYY-MM-DD
```

A quarantined combination is excluded from the blocking gate while the tracking issue is open. It continues to run (for data collection) with `continue-on-error: true`.

### Quarantine time bound

- Maximum quarantine window: **14 days** from the date of quarantine annotation.
- At quarantine expiry, one of the following MUST occur:
  1. The combination is fixed and the quarantine annotation removed, OR
  2. The combination is reclassified as runtime-native via an ADR amendment (docs/adr/2026-04-29-rust-npm-binary-parity.md), OR
  3. The quarantine window is extended by the project owner with a new tracking issue and new expiry date (maximum one extension of 14 days).
- Indefinite quarantine is not permitted.

## 3. P2P Combinations — Advisory Ramp

Combinations t4a and t4b (npm client ↔ npm launcher, P2P transport) MUST begin in **non-blocking advisory mode**:

```yaml
# In wire-format-parity CI job:
- name: Run P2P combinations (advisory)
  run: npx vitest run rust-npm-interop-beta --reporter=verbose
  continue-on-error: true
  env:
    MXDX_PARITY_ADVISORY: t4a,t4b
```

These combinations are promoted to **blocking** only after:
- **10 consecutive green runs** across distinct CI runner environments (not the same runner reused).
- The `docs/adr/2026-04-29-rust-npm-binary-parity.md` section on version skew (`node-datachannel` 0.32 vs `datachannel` 0.16) has been reviewed and the P2P security verification document (`docs/reviews/security/2026-04-29-p2p-cross-runtime-dtls-verification.md`) has been approved by the project owner.

To promote t4a/t4b to blocking: open a PR that removes `continue-on-error: true` from the advisory step, citing the 10-consecutive-green run IDs in the PR description.

## 4. Combination Reference

| ID | Client runtime | Worker runtime | HS topology | Initial mode |
|---|---|---|---|---|
| t1a | rust | rust | same-hs | blocking |
| t1b | rust | rust | federated | blocking |
| t2a | npm | rust | same-hs | blocking |
| t2b | npm | rust | federated | blocking |
| t3a | rust | npm | same-hs | blocking |
| t3b | rust | npm | federated | blocking |
| t4a | npm | npm | same-hs | advisory |
| t4b | npm | npm | federated | advisory |

## 5. Security Fix Exception

Security fixes to non-parity code MUST be processable within **24 hours** even when a parity gate combination is red. See `docs/adr/overrides/README.md` for the emergency override procedure.
