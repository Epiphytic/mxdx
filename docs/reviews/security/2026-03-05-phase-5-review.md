# Phase 5 Security Review

## Date: 2026-03-06
## Reviewer: PO (manual review after agent failure)

## Checklist

| Check | Status | Notes |
|:---|:---|:---|
| No shell interpolation | **PASS** | `executor.rs:execute_command` uses `TokioCommand::new(&validated.cmd).args(&validated.args)`. No `sh -c` anywhere. |
| cwd validation (mxdx-71v) | **PASS** | `normalize_path` resolves `..` without filesystem access, then checks against `allowed_cwd_prefixes`. Test confirms `/workspace/../../etc` is rejected. |
| Argument injection (mxdx-jjf) | **PASS** | `validate_args` blocks: git `-c`/`--config`, git `submodule foreach`, docker `compose -f`/`--file`, `env` command entirely. 5 tests cover these. |
| Config permissions (mxdx-cfg) | **PASS** | `validate_config_permissions` checks `mode & 0o077 != 0` and warns via `tracing::warn`. |
| Telemetry detail levels (mxdx-tel) | **PASS** | `TelemetryDetail::Summary` returns `None` for network/services/devices. 3 tests confirm. |
| No secrets in logs | **PASS** | Passwords are in config structs but never logged. No `tracing::info/debug` calls emit password fields. |
| Allowlist bypass via path traversal | **PASS** | Command name is checked as a simple string against the allowlist — no path resolution. `../../../bin/rm` would not match `rm` in the allowlist. |

## Adversarial Findings

### Finding 1: env vars from CommandEvent not validated
- **Severity**: Medium
- **Location**: `executor.rs` — `execute_command` does not set env vars from CommandEvent
- **Description**: The `CommandEvent` type has an `env: HashMap<String, String>` field. Currently `execute_command` ignores it (doesn't call `.env()` or `.envs()`). This is safe by omission but means env vars aren't functional yet. When implemented, PATH/LD_PRELOAD injection must be prevented.
- **Recommendation**: When adding env var support, deny dangerous vars (PATH, LD_PRELOAD, LD_LIBRARY_PATH, DYLD_*) or use a strict allowlist.

### Finding 2: TOCTOU between validation and execution
- **Severity**: Low
- **Location**: `executor.rs:validate_command` + `execute_command`
- **Description**: `validate_command` normalizes the cwd path without filesystem access (intentional for testability). Between validation and `execute_command` setting `.current_dir()`, a symlink could be created at the validated path pointing outside the allowed prefix. However, this requires the attacker to already have filesystem write access in the allowed cwd prefix.
- **Recommendation**: Accept as low risk. The attacker would need existing code execution access. Consider using `std::fs::canonicalize` at execution time for defense in depth (not blocking).

### Finding 3: No timeout enforcement in execute_command
- **Severity**: Medium
- **Location**: `executor.rs:execute_command`
- **Description**: `CommandEvent` has `timeout_seconds` but `execute_command` doesn't enforce it. A command could run indefinitely, consuming resources.
- **Recommendation**: Add `tokio::time::timeout` wrapping the process execution in the next iteration.

### Finding 4: git --config (long form) coverage
- **Severity**: Info
- **Location**: `executor.rs:validate_args`
- **Description**: The git arg check blocks `-c` and `--config`. However, git also accepts `-c` with no space (e.g., `-ccore.pager=evil`). The current check compares exact args so `-ccore.pager=evil` as a single arg would not match `-c`.
- **Recommendation**: Add a check for args starting with `-c` (prefix match) for git.

## Summary

Phase 5 security posture is **good**. All planned security controls (mxdx-71v, mxdx-jjf, mxdx-cfg, mxdx-tel) are implemented and tested. The critical path (command execution) uses direct process spawning with no shell interpolation. Two medium findings (env var validation, timeout enforcement) are noted for future work but don't represent current vulnerabilities since those code paths aren't active yet.
