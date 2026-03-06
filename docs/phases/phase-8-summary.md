# Phase 8: Policy Agent — Summary

## Completion Date: 2026-03-06

## What Was Built

### Appservice Registration (`src/appservice.rs`)
- `AppserviceRegistration` with YAML generation
- `register_appservice()` via Tuwunel admin room commands
- Exclusive `@agent-*` namespace — fail-closed when appservice is down

### PolicyConfig (`src/config.rs`)
- Configuration struct for homeserver URL, tokens, namespace
- `user_namespace_regex()` with proper regex escaping

### PolicyEngine (`src/policy.rs`)
- LRU replay protection (mxdx-rpl): capacity 10000, 1-hour TTL
- `check_replay` / `mark_seen` for event deduplication
- `is_authorized` with exact HashSet match
- `evaluate()` combined check returning `PolicyRejection` enum

## Tests

20 total tests (15 unit + 5 integration):

| Category | Count | Key Tests |
|:---|:---|:---|
| Config | 1 | regex escaping |
| Appservice | 2 | YAML format, namespace exclusivity |
| PolicyEngine | 12 | replay detection, TTL expiry, authorization |
| Integration | 5 | Tuwunel registration, M_FORBIDDEN on namespace, replay protection |

## Security Issues Addressed

| Finding | Status | Control |
|:---|:---|:---|
| mxdx-rpl (replay protection) | Implemented + tested | LRU cache with TTL, bounded at 10000 |
| Fail-closed | Verified by test | Exclusive appservice namespace at homeserver level |

## Security Review Findings

- **Low**: LRU eviction theoretical bypass (requires >10K events in TTL window)
- **Low**: Hand-rolled YAML formatter could be vulnerable to config injection
- **Low**: Incomplete URL encoding function
- All checks PASS

## Key Commits

| Commit | Description |
|:---|:---|
| `44e6342` | Appservice registration |
| `c2fb113` | PolicyEngine with replay protection |
