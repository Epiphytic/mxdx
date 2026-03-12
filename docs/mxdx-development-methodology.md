# mxdx — Development Methodology

**Date:** 2026-03-04
**Status:** Draft
**References:**
- [Management Console Design](./mxdx-management-console.md)
- [mxdx Architecture](./mxdx-architecture.md)
- [Project Plan](./mxdx-project-plan.md)
- [Agent Roles](https://github.com/Epiphytic/agenticenti/tree/main/prompts/roles)
- [CLAUDE.md Coding Standards](../../../.claude/CLAUDE.md)

---

## 1. Philosophy

Every line of code in mxdx is:

- **Tested against a live Matrix server** — not mocked, not simulated. Public Matrix homeservers and local Tuwunel instances provide real E2EE, real federation, real protocol behavior.
- **Written by a role-specialized agent** — each agent has a bounded scope, clear deliverables, and never touches files outside its responsibility.
- **Reviewed with a critical eye** — code review is not a rubber stamp. Security review is not optional. Both happen before merge.
- **Built test-first** — tests define the expected behavior before implementation begins. If you can't write the test, you don't understand the requirement.

The goal is not speed — it's **confidence**. Every commit should leave the system in a state where you'd be comfortable deploying it.

---

## 2. Test-Driven Development Protocol

### The Cycle

Every feature follows this exact sequence:

```
1. SPECIFY  → Write acceptance criteria (what does "done" look like?)
2. TEST     → Write failing tests that encode the criteria
3. IMPLEMENT → Write the minimum code to make tests pass
4. REVIEW   → Code review + security review
5. VERIFY   → Run full test suite + live Matrix integration tests
6. MERGE    → Only after all gates pass
```

No step can be skipped. No step can be reordered.

### Test Hierarchy

| Layer | Scope | Runner | Matrix? | When |
|:---|:---|:---|:---|:---|
| **Unit** | Single function/module | `cargo test` / `vitest` | No | Every commit |
| **Integration** | Cross-module, event parsing, crypto | `cargo test` / `vitest` | Local Tuwunel | Every commit |
| **E2E Protocol** | Full client ↔ homeserver round-trips | Custom harness | Live Matrix server | Every PR |
| **E2E System** | Launcher + client + terminal + tmux | Custom harness | Live Matrix server | Pre-merge |
| **Security** | Vulnerability scan, dependency audit | `cargo audit` / `npm audit` | N/A | Every PR |

### Live Matrix Testing

Since both the client and launcher can run on the same machine but communicate via the Matrix protocol, **there is no excuse for not testing against real Matrix at every step**.

**Test infrastructure:**

- **Local Tuwunel instance** — Spun up as part of the test harness. Ephemeral, destroyed after test run. Used for integration and E2E tests.
- **Public Matrix homeserver** — For federation tests and real-world protocol compliance. Test accounts on a public homeserver (e.g., `matrix.org` or a dedicated test server).
- **Test accounts** — Dedicated Matrix accounts for test runners. Credentials stored in CI secrets, injected at runtime.

**Test patterns:**

```
// Example: Test that a launcher receives a command over E2EE
test("launcher receives and executes command via Matrix", async () => {
  // ARRANGE: Register test orchestrator + test launcher on local Tuwunel
  const orchestrator = await createTestClient("orchestrator");
  const launcher = await createTestLauncher("launcher", { allowedCommands: ["echo"] });

  // Create execution room, invite both
  const room = await orchestrator.createRoom({ encrypted: true });
  await launcher.joinRoom(room.id);

  // ACT: Send a command
  const cmdId = await orchestrator.sendCommand(room.id, {
    action: "exec",
    cmd: "echo hello",
  });

  // ASSERT: Wait for result event (threaded reply)
  const result = await waitForEvent(room.id, "org.mxdx.result", {
    uuid: cmdId,
    timeout: 10_000,
  });

  expect(result.content.exit_code).toBe(0);
  expect(result.content.status).toBe("exit");
});
```

### Test Requirements by Feature

Every feature PR must include:

1. **Unit tests** for all new functions
2. **Integration tests** for cross-module interactions
3. **At least one E2E test** that exercises the feature over a real Matrix connection
4. **A test for the error path** — what happens when it fails? (network drop, invalid input, policy denial)
5. **A test for the security boundary** — what happens when an unauthorized user tries this?

---

## 3. Agent Team Structure

### Role Definitions

All roles are sourced from the [agenticenti role library](https://github.com/Epiphytic/agenticenti/tree/main/prompts/roles). Each role has strict boundaries on what it can and cannot do.

| Role | Source | Responsibility | Can Write | Cannot Write |
|:---|:---|:---|:---|:---|
| **Team Leader** | `team-leader.md` | Orchestration, task decomposition, team coordination | Task definitions, status updates | Any code, tests, or docs |
| **Planner** | `planner.md` | Analyze requirements, produce implementation plans | `docs/plans/` | Source code, tests |
| **Architect** | `architect.md` | Design systems, write ADRs | `docs/adr/`, `docs/plans/` | Source code, tests |
| **Researcher** | `researcher.md` | Investigate technologies, evaluate dependencies | `docs/research/` | Source code, tests |
| **Coder** | `coder.md` | Write production code | `src/`, `client/`, `web/` | Tests, docs, CI config |
| **Tester** | `tester.md` | Write and maintain tests | `tests/`, `*.test.*`, `*.spec.*` | Source code |
| **Security Reviewer** | `security-reviewer.md` | Audit for vulnerabilities, review crypto | `docs/reviews/security/` | Source code |
| **Reviewer** | `reviewer.md` | Code quality review, pattern adherence | `docs/reviews/` | Source code |
| **DevOps** | `devops.md` | CI/CD, build pipelines, deployment | `.github/workflows/`, `Dockerfile`, `terraform/` | Application source code |
| **Troubleshooter** | `troubleshooter.md` | Diagnose failures, identify root causes | Diagnosis reports | Source code (minimal fixes only) |
| **Integrator** | `integrator.md` | Infrastructure provisioning, IaC | `terraform/`, `install/` | Application source code |
| **Maintainer** | `maintainer.md` | Final integration, merge authority, release | Any file (with review) | N/A — has full access but delegates |

### Team Compositions by Task Complexity

| Complexity | Team Size | Roles | Example |
|:---|:---|:---|:---|
| **Trivial** | 2 | Coder + Tester | Fix a typo in event parsing |
| **Simple** | 3 | Coder + Tester + Reviewer | Add a new field to telemetry event |
| **Standard** | 4-5 | Planner + Coder + Tester + Reviewer + Security Reviewer | Implement terminal resize handling |
| **Complex** | 5-7 | Architect + Planner + Coder + Tester + Security Reviewer + Reviewer | Multi-homeserver failover |
| **Phase-level** | Split into sub-teams | Team Leader + multiple sub-teams of 3-5 | Build the entire launcher |

**Rule:** Never more than 7 agents on a single team. If the task needs more, split into phases.

---

## 4. Workflow: Feature Development

### Step 1: Research (if needed)

**Agent:** Researcher
**Trigger:** Ambiguous requirements, unknown technology, dependency evaluation
**Output:** `docs/research/YYYY-MM-DD-<topic>.md`

The researcher investigates and produces findings. No code is written. The research doc becomes an input for planning.

### Step 2: Plan

**Agent:** Planner (+ Architect for complex features)
**Trigger:** Every feature that touches more than 2 files
**Output:** `docs/plans/YYYY-MM-DD-<feature>.md` (+ `docs/adr/YYYY-MM-DD-NNN-<decision>.md` for architectural decisions)

The plan must include:
- Task dependency graph with parallelization notes
- Files to be created/modified per task
- Verification criteria per task
- Risk assessment

**HITL Gate:** Plan requires explicit user approval before any implementation begins.

### Step 3: Test-First Implementation

**Agents:** Tester (writes tests first) → Coder (implements to pass tests)

The tester reads the plan and writes failing tests that encode the expected behavior. The coder then implements the minimum code to make those tests pass.

**File ownership:** The tester owns `tests/` and test files. The coder owns `src/`. They never touch each other's files. If the coder discovers the test is wrong, they escalate to the team leader — they don't modify the test.

**Live Matrix checkpoint:** At least one E2E test per feature runs against a real Matrix server before the coder considers their work done.

### Step 4: Review

**Agents:** Reviewer (code quality) + Security Reviewer (vulnerabilities)

Both reviews happen in parallel. Both produce written reports:
- `docs/reviews/YYYY-MM-DD-<feature>-code.md`
- `docs/reviews/security/YYYY-MM-DD-<feature>.md`

**Review verdicts:**
- `APPROVE` — Merge-ready
- `REQUEST_CHANGES` — Blocking issues found. Coder addresses them, re-review.
- `NEEDS_DISCUSSION` — Architectural question. Escalate to team leader/architect.

**Confidence threshold:** Only issues scoring >= 75 confidence are reported. This prevents noise.

### Step 5: Verification

**Agent:** Tester (or CI)
**Gate:** ALL of the following must pass:
- Unit tests (cargo test / vitest)
- Integration tests against local Tuwunel
- E2E tests against live Matrix
- Security scan (cargo audit / npm audit)
- No regressions in existing tests

**Immutable testing rule (from CLAUDE.md):** Never disable or alter existing tests to force a build to pass. Tests are only updated if the underlying business logic has intentionally changed.

### Step 6: Merge

**Agent:** Maintainer
**Gate:** Review approval + all verification passing

Atomic commits with conventional messages:
```
feat(launcher): add terminal session spawning via tmux
fix(client): handle sequence gap in terminal data events
refactor(transport): extract filtered sync into reusable module
test(e2e): add live Matrix test for multi-homeserver failover
```

---

## 5. Security-First Development

### Every Feature Gets a Security Review

No exceptions. The security reviewer examines:
- Trust boundaries (where user input enters, where secrets flow)
- OWASP Top 10 applicability
- Crypto usage (are we using the SDK correctly? Are we leaking plaintext?)
- Dependency vulnerabilities (cargo audit / npm audit)
- Policy enforcement (does the Policy Agent actually block unauthorized actions?)

### Security Testing Requirements

For features that touch security boundaries:

1. **Positive test** — Authorized user can perform the action
2. **Negative test** — Unauthorized user is blocked (power level too low, policy denial)
3. **Fail-closed test** — When the Policy Agent is down, the action fails (not succeeds)
4. **Injection test** — Malformed input doesn't bypass validation
5. **Replay test** — Replayed events don't cause duplicate actions

### Secrets Handling (from CLAUDE.md)

- Zero hardcoding — never log, hardcode, or transmit secrets
- All credentials via environment variables or secret managers
- Scrub all log outputs, CLI commands, and PRs for potential tokens
- Test credentials are dedicated test accounts, never production credentials

---

## 6. Code Quality Standards

### From CLAUDE.md

- **Modular & DRY** — Check existing codebase before creating new functions
- **Facade pattern for externals** — All external library/API calls wrapped in local modules
- **Idempotent scripts** — Running a failed script again must not break things or create duplicates
- **Circuit breakers** — Three failures on the same task → halt and escalate
- **Structured logging** — JSON with levels (INFO, WARN, ERROR, DEBUG)
- **Graceful error handling** — Critical errors fail fast; non-critical errors are caught, logged, and bypassed

### Code Review Standards (from reviewer role)

- **Confidence-scored findings** — Only issues >= 75 confidence reported
- **Categorized feedback:** Blocking > Important > Suggestion > Nit
- **Scope discipline** — Review only changed lines; context lines are off-limits unless directly affected
- **Missing code is critical** — Unhandled errors and untested paths are the most important findings

### MANIFEST.md

Per CLAUDE.md, every new reusable module, agent role, or external facade must be recorded in `MANIFEST.md`. This prevents future agents from duplicating existing functionality.

---

## 7. Live Testing Infrastructure

### Local Development

```
┌─ Developer Machine ──────────────────────────┐
│                                              │
│  ┌─ Test Tuwunel (ephemeral) ─────────────┐  │
│  │  SQLite/RocksDB (in /tmp)              │  │
│  │  Listening on localhost:6167            │  │
│  └──────────────────┬─────────────────────┘  │
│                     │                        │
│  ┌─ Test Launcher ──┼──────────────────────┐ │
│  │  @mxdx/client (launcher role)     │ │
│  │  tmux sessions                          │ │
│  └──────────────────┼──────────────────────┘ │
│                     │                        │
│  ┌─ Test Client ────┼──────────────────────┐ │
│  │  mxdx-client (browser or Node)    │ │
│  │  xterm.js (headless for tests)          │ │
│  └─────────────────────────────────────────┘ │
└──────────────────────────────────────────────┘
```

**Setup:** A test helper spins up Tuwunel, registers test accounts, creates rooms, and tears everything down after the test suite. This runs in ~2 seconds (155ms per registration, measured).

### CI Pipeline

```yaml
# .github/workflows/test.yml (conceptual)
jobs:
  unit:
    - cargo test --workspace
    - npm run test:unit

  integration:
    services:
      tuwunel:
        image: ghcr.io/epiphytic/tuwunel:latest
    steps:
      - npm run test:integration  # Uses the Tuwunel service container

  e2e:
    services:
      tuwunel:
        image: ghcr.io/epiphytic/tuwunel:latest
    steps:
      - npm run test:e2e  # Full launcher + client + terminal tests

  security:
    steps:
      - cargo audit
      - npm audit
      - cargo clippy -- -D warnings
```

### Federation Testing

For multi-homeserver features:

```
┌─ Tuwunel A (localhost:6167) ─┐    ┌─ Tuwunel B (localhost:6168) ─┐
│  @launcher:hs-a              │←──→│  @launcher:hs-b              │
│  @tester:hs-a                │    │  @tester:hs-b                │
└──────────────────────────────┘    └──────────────────────────────┘
```

Two Tuwunel instances in CI, federated together. Tests verify:
- Events propagate across homeservers
- Launcher failover from A to B works
- Telemetry state events are visible from both sides
- E2EE works cross-federation

---

## 8. Phase-Level Team Execution

Using the project plan's build order, here's how teams are composed for each phase:

### Phase 1: Core Client (Weeks 1-3)

**Team Leader** coordinates three sub-teams:

| Sub-team | Roles | Deliverable |
|:---|:---|:---|
| **Core WASM** | Researcher → Architect → Coder + Tester | Rust/WASM crypto, identity, protocol |
| **Transport** | Coder + Tester | Filtered sync, send, to-device |
| **Launcher** | Coder + Tester + Security Reviewer | Command exec, streaming, telemetry |

**Integration point:** After all three sub-teams deliver, an E2E test verifies the full flow: launcher registers → receives command → streams output → posts result.

**Review gate:** Reviewer + Security Reviewer examine the full client before v0.1.0 publish.

### Phase 3: Web App + Management Console (Weeks 5-8)

**Team Leader** coordinates:

| Sub-team | Roles | Deliverable |
|:---|:---|:---|
| **Backend** | Architect → Coder + Tester | Axum routes, HTMX templates, SSE |
| **Client Library** | Coder + Tester | `mxdx-client` browser lib, WebSocket API for xterm.js |
| **Terminal** | Coder + Tester + Security Reviewer | PTY bridge, tmux integration, adaptive compression |
| **PWA** | Coder + Tester | Service worker, manifest, offline shell |
| **DevOps** | DevOps | Dockerfile, Cloudflare Workers deploy, CI |

**Integration point:** Full E2E test — browser opens dashboard, connects to launcher, opens terminal, types a command, sees output.

---

## 9. Error Handling & Escalation

### Circuit Breakers (from CLAUDE.md)

If any agent fails the same task three times:
1. Halt immediately
2. Produce a summary of the three failed attempts
3. Escalate to the team leader
4. Team leader decides: reassign, change approach, or escalate to user

### Troubleshooting Protocol (from CLAUDE.md)

When an error occurs:
1. Spawn the **Troubleshooter** agent
2. Troubleshooter diagnoses root cause — **read-only access**, never executes fixes
3. Troubleshooter produces a diagnosis report
4. Team leader assigns the fix to the appropriate Coder

### Escalation Triggers

Any agent should halt and escalate when:
- Requirements are contradictory
- A critical system has no tests
- Multiple valid approaches exist with unclear trade-offs
- A security concern is discovered
- A dependency has known vulnerabilities with no patch

---

## 10. Commit & Review Standards

### Commit Format (from CLAUDE.md)

```
<type>(<scope>): <description>

Types: feat, fix, refactor, test, docs, ci, chore
Scope: client, launcher, web, transport, crypto, terminal, e2e
```

### Atomic Commits

Each commit is:
- A single logical change
- Independently revertible
- Builds and passes tests on its own

### Review Checklist

Before any merge, the Reviewer confirms:

- [ ] All new functions have unit tests
- [ ] At least one E2E test exercises the feature over live Matrix
- [ ] Error paths are tested (not just happy paths)
- [ ] Security boundaries are tested (unauthorized access denied)
- [ ] No secrets, tokens, or credentials in code or logs
- [ ] Existing tests unchanged (unless requirements changed)
- [ ] MANIFEST.md updated if new modules/facades created
- [ ] Conventional commit messages used
- [ ] Code matches existing codebase conventions
- [ ] No TODO comments or partial implementations

---

## 11. Key Principles Summary

| Principle | Source | Enforcement |
|:---|:---|:---|
| Test-first | This methodology | Tester writes failing tests before Coder implements |
| Live Matrix testing | This methodology | E2E tests run against real Tuwunel in CI |
| Role separation | agenticenti roles | Each agent has bounded file access |
| Fail-closed security | mxdx architecture | Policy Agent tests verify fail-closed behavior |
| Confidence-scored reviews | reviewer.md | Only issues >= 75 confidence reported |
| Circuit breakers | CLAUDE.md | 3 failures → halt and escalate |
| Atomic commits | CLAUDE.md | Each commit builds and tests independently |
| Zero hardcoded secrets | CLAUDE.md | Security reviewer audits every PR |
| Facade pattern | CLAUDE.md | All external calls wrapped |
| MANIFEST.md | CLAUDE.md | Updated on every new module/facade |
| Plans before code | CLAUDE.md + planner.md | HITL gate on all plans |
| ADRs for decisions | CLAUDE.md + architect.md | Architectural decisions documented with rationale |
| Immutable tests | CLAUDE.md | Tests never modified to force a pass |
| Structured logging | CLAUDE.md | JSON with levels, no secrets |
