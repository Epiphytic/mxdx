# Phase 5: Launcher v1 — Non-Interactive Sessions — Summary

## Completion Date: 2026-03-06

## What Was Built

### Launcher Config (`src/config.rs`)
- TOML-based configuration: global, homeservers, capabilities, telemetry sections
- `CapabilityMode::Allowlist` / `Denylist` with command and cwd prefix restrictions
- `TelemetryDetail::Full` / `Summary` for privacy-aware telemetry
- Custom deserializer rejects empty `launcher_id` (fail-fast)
- `validate_config_permissions` warns on insecure file permissions (mxdx-cfg)

### Command Executor (`src/executor.rs`)
- `validate_command`: allowlist check, cwd normalization + prefix validation, argument injection checks
- `execute_command`: async process execution with separate stdout/stderr piping via `tokio::select!`
- Uses `Command::new(cmd).args(args)` — no shell interpolation
- Returns `CommandResult` with exit code, stdout/stderr lines, seq counter

### Telemetry (`src/telemetry/`)
- `collect_telemetry(detail_level)` using `sysinfo::System`
- Full mode: CPU, memory, disk, network stats
- Summary mode: hostname, OS, arch, uptime, load average, basic CPU/memory only

### Identity (`src/identity.rs`)
- Stub module for future Matrix registration/login lifecycle

## Tests

19 total tests:

| Category | Count | Key Tests |
|:---|:---|:---|
| Config | 3 | valid parse, empty id rejection, telemetry levels |
| Security (executor) | 9 | allowlist, cwd traversal, git -c, docker -f, env injection |
| Telemetry | 3 | summary excludes detail, full includes all, basic info |
| E2E integration | 4 | echo capture, stdout/stderr separation, seq ordering, Matrix round-trip |

## Security Issues Addressed

| Finding | Status | Control |
|:---|:---|:---|
| mxdx-71v (cwd validation) | Implemented + tested | `normalize_path` + prefix check |
| mxdx-jjf (argument injection) | Implemented + tested | Per-command deny patterns |
| mxdx-cfg (config permissions) | Implemented | Unix permission check on startup |
| mxdx-tel (telemetry levels) | Implemented + tested | Summary/Full detail control |

## Security Review Findings

- **Medium**: env vars from CommandEvent not yet wired (safe by omission, needs validation when added)
- **Medium**: No timeout enforcement yet (CommandEvent.timeout_seconds ignored)
- **Low**: TOCTOU between cwd validation and execution (requires existing fs access)
- **Info**: git `-ckey=val` (no space) variant not caught

## CI Updates

- `cargo test -p mxdx-launcher --lib` added as new CI job
- `cargo test -p mxdx-launcher --test e2e_command` added to integration job (requires tuwunel)

## Key Commits

| Commit | Description |
|:---|:---|
| `4871ad4` | Config types with TOML parsing |
| `876ae07` | Command validation + telemetry |
| `12c5215` | E2E command execution |
