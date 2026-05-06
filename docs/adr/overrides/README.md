# Wire-Format-Parity Gate Override Policy

**ADR:** docs/adr/2026-04-29-rust-npm-binary-parity.md (requirement 8a)
**Status:** Active
**Date:** 2026-04-30

## Purpose

The `wire-format-parity` gate is a required check on `main`. This document specifies when and how the gate may be overridden, and how overrides are logged for audit.

## Override Conditions

An override is only permitted when ALL of the following are true:

1. **Explicit written justification** in the PR body, naming the failing combination by ID (e.g., "t3a is red due to issue mxdx-XXXX — unrelated to this PR's changes"). The justification must explain why the failure is orthogonal to the PR's changes.
2. **Project-owner approval** — the project owner must explicitly approve the PR (not just any reviewer).
3. **Linked tracking issue** — the PR body must link a beads or GitHub issue with a specific deadline for restoring the gate to green.

## Security Fix Processing

Security fixes to non-parity code MUST be processable within **24 hours** even when a parity gate combination is red.

For a security fix PR when the gate is red:
- The PR author documents the failing combination in the PR body per the standard justification format.
- Project owner may approve immediately without waiting for the parity issue to be resolved.
- The tracking issue for the parity failure must already exist (or be created as part of the override).

## Override Log

Every override invocation MUST be logged in this directory as a dated file:

```
docs/adr/overrides/override-YYYY-MM-DD-<pr-number>.md
```

Each override log file MUST contain:
- Date
- PR number
- Which combination(s) were red
- Justification (verbatim from the PR body)
- Deadline for restoring the gate to green
- Link to the tracking issue

## Override Log Template

```markdown
# Gate Override Log

**Date:** YYYY-MM-DD
**PR:** #<number>
**Approved by:** <project owner>

## Failing Combination(s)

- `t3a`: <one-line reason>

## Justification

<copy justification from PR body>

## Tracking Issue

<link to beads or GitHub issue>

## Deadline

<date by which the gate must be green again>
```

## Flaky Quarantine

Flaky combinations are handled separately per `wire-format-parity-gate-policy.md`. A quarantined combination does NOT require an override log — it is excluded from the blocking gate by the quarantine annotation until the quarantine expires.
