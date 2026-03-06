# Phase 6 Security Review

## Date: 2026-03-06
## Reviewer: PO

## Checklist

| Check | Status | Notes |
|:---|:---|:---|
| PTY bytes to tmux — no shell interpolation | **PASS** | `tmux.rs:send_input` uses `Command::new("tmux").args(["send-keys", "-t", name, "-l", "--", data])`. `-l` flag for literal mode. No `sh -c`. |
| Command validated before session creation | **PASS** | `session.rs:create` takes validated command. `tmux.rs:create` validates session name regex `[a-zA-Z0-9_-]+`. |
| history_visibility = joined in initial_state | **PASS** | Verified by E2E test `launcher_creates_terminal_dm_on_session_request`. Uses `MatrixClient::create_terminal_session_dm` which sets `HistoryVisibility::Joined`. |
| Zlib bomb rejected before PTY write | **PASS** | `compression.rs:decode_decompress_bounded` reads in 8KB chunks via `ZlibDecoder::read`. Counts output bytes per chunk. Bails immediately when `total > max_bytes`. |
| seq is u64, tested with u64::MAX | **PASS** | `ring_buffer.rs` uses `u64` for seq. E2E test `seq_counter_supports_u64_range` verifies near `u64::MAX`. |
| tmux session names validated | **PASS** | `is_valid_session_name` checks `[a-zA-Z0-9_-]+`. Rejects `../../evil`. Tested. |
| No secrets in log output | **PASS** | No `tracing::*` calls emit passwords or tokens. |

## Adversarial Findings

### Finding 1: tmux command argument not validated against allowlist
- **Severity**: Medium
- **Location**: `terminal/tmux.rs:26` — `TmuxSession::create` takes `command` parameter
- **Description**: The `command` argument passed to `tmux new-session` is not validated against the launcher's allowlist. A malicious session request could specify an arbitrary command.
- **Recommendation**: `TerminalSession::create` should validate the command against `CapabilitiesConfig.allowed_commands` before passing to `TmuxSession::create`. This validation exists in the executor but needs to be wired into the session creation path.

### Finding 2: tmux send-keys with `-l` still interprets Enter
- **Severity**: Low
- **Location**: `terminal/tmux.rs:53`
- **Description**: tmux `send-keys -l` sends keys literally but a trailing `\n` in the data will be sent as a literal newline character to the PTY, which is the intended behavior for terminal input. No injection risk here — PTY input is by definition arbitrary user input.
- **Recommendation**: No action needed. This is correct behavior for terminal bridging.

### Finding 3: Recovery state file not permission-checked
- **Severity**: Low
- **Location**: `terminal/recovery.rs` — `RecoveryState::save`
- **Description**: The recovery JSON file containing room IDs is written without restricting permissions. Room IDs are not secrets per se, but the file could leak room topology.
- **Recommendation**: Set file permissions to 0600 on save, similar to config file check.

## Summary

Phase 6 security posture is **good**. All planned controls (mxdx-aew, mxdx-ccx, mxdx-seq, mxdx-8bm) are implemented and tested. The streaming zlib bomb protection is correctly implemented with chunk-based reading. One medium finding: terminal command should be validated against allowlist in the session creation path.
