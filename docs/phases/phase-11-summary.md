# Phase 11: Multi-Homeserver — Summary

## Completion Date: 2026-03-06

## What Was Built

### MultiHsLauncher (`src/multi_hs.rs`)
- Concurrent connect to multiple homeservers via `JoinSet`
- Sync latency measurement for primary selection
- FailoverState machine: Active -> Failing -> Failover -> Unavailable
- `health_check()`: pings primary sync, triggers failover after 3 consecutive failures
- `failover()`: probes all non-primary clients, selects lowest-latency surviving instance
- `primary()`, `connected_count()`, `primary_port()`, `state()` accessors

### Federation CI Job
- Runs on main branch pushes + workflow_dispatch only
- Tests `mxdx-test-helpers` federation with `--include-ignored`
- Tests multi-HS launcher unit tests

## Tests

7 total tests:

| Category | Count | Key Tests |
|:---|:---|:---|
| Multi-HS startup | 3 | single HS connect, dual HS primary selection, empty input |
| Failover | 3 | primary failover, all-down unavailable, health check active |
| Security | 1 | non-launcher can't update identity state event |

## Completion Gate
Launcher fails over from hs_a to hs_b transparently. Commands still flow. Health check detects primary failure within 3 consecutive checks.

## Key Commits

| Commit | Description |
|:---|:---|
| `f95b510` | Multi-HS config + startup with latency selection |
| `85387d7` | Failover with health checks + federation CI |
