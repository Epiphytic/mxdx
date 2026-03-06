# mxdx Rebuild — Lessons Learned

**Date:** 2026-03-06
**Scope:** Full 12-phase rebuild from zero to integration-tested system
**Duration:** Single extended session with agent teams

---

## Architecture & Design

### What Worked

- **Facade pattern for matrix-sdk**: Wrapping matrix-sdk behind `MatrixClient` in a single crate (`mxdx-matrix`) prevented vendor lock-in and kept every other crate from importing SDK internals. When matrix-sdk API changed between versions, fixes were confined to one file.

- **TDD handoff pattern**: Writing tests first, then handing to an implementation agent, caught design issues early. Tests became living documentation of expected behavior.

- **OS-assigned ports (mxdx-ji1)**: Using port 0 everywhere eliminated flaky CI from port conflicts. Zero test failures from port collisions across hundreds of runs.

- **`.localhost` TLD for federation**: RFC 6761 guarantees `.localhost` resolves to loopback. No mkcert, no `/etc/hosts` editing, no cert trust store manipulation. FederatedPair "just works" on any machine.

### What We'd Do Differently

- **Define `TerminalMatrixClient` interface earlier**: The `attachTerminalSession` stub survived until the final audit because the `TerminalSocket` and `MxdxClient` were built by different agents in parallel without agreeing on the integration point. Interface contracts should be locked before parallel implementation begins.

- **Secret coordinator needs sender identity from day one**: The Phase 9 HIGH finding (no sender verification) was flagged in review but deferred. Scope-only authorization without sender identity is fundamentally broken for a secrets system. Security-critical interfaces should block on review findings, not carry them forward.

---

## Testing

### What Worked

- **`TuwunelInstance` helper**: A single helper that starts a real homeserver, manages its lifecycle, and cleans up made integration tests reliable and fast (~2s startup).

- **Security test naming convention (`test_security_*`)**: Prefixing security tests made them greppable, auditable, and runnable as a group in CI. The security report workflow collects them automatically.

- **Separate CI jobs per crate**: Failures are immediately localized. A broken policy test doesn't block launcher development.

### What We'd Do Differently

- **Don't use `#[ignore]` when dependencies are available**: The `full_system_e2e` test was marked `#[ignore]` because it needed tuwunel and tmux — but both were installed in CI and locally. The ignore was cargo-culted from a template. If the dependency exists, run the test.

- **Check sync responses for expected messages**: The `register_appservice` timeout bug happened because the initial sync consumed the confirmation message before the polling loop started. When polling for a server response over Matrix sync, don't discard any sync response — check every one.

---

## Agent Coordination

### What Worked

- **Parallel agent dispatch for independent phases**: Running Phases 7+8+9 simultaneously (different crates, no shared code) cut wall-clock time significantly. Same for Phase 12 tasks.

- **Dedicated security reviewer agent**: Having a separate agent do the final security review with fresh eyes caught the SRI dead code and the CORS origin "null" issue that implementation agents missed.

- **Shutdown idle agents**: Explicitly shutting down agents when their work was done prevented resource waste and context confusion.

### What We'd Do Differently

- **Verify integration points after parallel merges**: When parallel agents implement both sides of an interface (e.g., `TerminalSocket` and `MxdxClient.attachTerminalSession`), run an explicit integration check after merging. The stub survived because each agent validated their own side independently.

- **Don't defer stub cleanup**: Stubs should be tracked as blocking issues, not accepted as "future work." The three stubs found in the final audit (pty.rs, policy main.rs, attachTerminalSession) were all easy to miss individually but indicated gaps in integration.

---

## CI & DevOps

### What Worked

- **Security report as release artifact**: Attaching the security test matrix and audit results to GitHub releases creates an auditable trail without manual effort.

- **Federation tests gated to main + dispatch**: Expensive multi-homeserver tests don't block feature branch CI but still run before release.

### What We'd Do Differently

- **Add `cargo xtask audit-stubs` from the start**: A simple xtask that greps for `todo!()`, `unimplemented!()`, `#[ignore]`, and `throw new Error("Not implemented")` would have caught all three stubs in CI rather than requiring a manual audit at the end.

---

## Security

### What Worked

- **Security finding IDs (mxdx-xxx)**: Assigning unique IDs to each finding in the design review made them trackable across phases, tests, and review documents. Every finding has a clear paper trail from identification to remediation.

- **Double encryption for secrets (mxdx-adr2)**: age x25519 recipient encryption on top of Megolm E2EE means the homeserver never sees secret plaintext, even if Megolm is compromised.

- **Fail-closed policy engine**: The default-deny `PolicyEngine` with explicit `authorize_user()` means a misconfiguration blocks everything rather than allowing everything.

### What We'd Do Differently

- **Enforce security review sign-off before phase closure**: The Phase 9 HIGH finding (no sender identity verification) was documented but not gated. A formal "no unresolved HIGH findings" rule would have forced the fix before moving on.

- **Wire SRI end-to-end or don't ship the code**: The service worker's `verifyIntegrity` function checks an `X-Content-Hash` header that the server never sends. Dead security code is worse than no security code — it creates false confidence. Either implement it fully or remove it.

---

## Process

### What Worked

- **Beads issue tracking with dependencies**: Blocking relationships between issues prevented out-of-order execution. `bd ready` always showed exactly what could be worked on next.

- **Phase summaries as completion gates**: Writing a summary document for each phase forced explicit review of what was built, what was tested, and what was deferred.

- **PO sign-off checkpoints**: Having the product owner review each phase prevented drift from requirements and caught scope issues early.

### What We'd Do Differently

- **Run a full codebase audit mid-project, not just at the end**: The stub audit should have happened after Phase 7 (when all major subsystems existed) rather than after Phase 12. Earlier detection means cheaper fixes.

- **Track "carry-forward" items as blocking issues**: Items deferred from one phase to another (like the Phase 9 sender verification) should be beads issues that block the target phase, not prose in a review document that can be overlooked.
