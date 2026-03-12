# Security Review: Terminal Subsystem (Phase 7)

**Date:** 2026-03-05
**Reviewer:** Claude Sonnet 4.6 (mxdx-1jv)
**Codebase snapshot:** `coder/terminal-session` @ 3280609
**Files reviewed:**
- `crates/mxdx-launcher/src/terminal/session.rs`
- `crates/mxdx-launcher/src/terminal/compression.rs`
- `crates/mxdx-launcher/src/lib.rs`
- `crates/mxdx-types/src/events/command.rs`
- `crates/mxdx-types/src/events/terminal.rs`

---

## Summary

| Control | Status | Notes |
|---|---|---|
| PTY bridge — no shell interpolation (exec path) | ✅ PASS | All tmux args passed as argv slices |
| Command allowlist enforcement | ✅ PASS | Checked before spawn; validate() enforces absolute path, cwd allowlist, arg metacharacters |
| Path traversal (`..`) blocked | ✅ PASS | `validate_cwd()` rejects `..` in any path component |
| Arg injection blocked | ✅ PASS | `validate_args()` rejects shell metacharacters, null bytes, git -c, docker --config |
| Zlib bomb protection | ✅ PASS | `decode_decompress_bounded` enforces 1MB ceiling (mxdx-ccx) |
| Session room `history_visibility=joined` | ⚠️ GAP | Not set in `create_session()` — late joiners may read pre-join history |
| Session room power levels | ⚠️ GAP | Not configured — any room member can send `org.mxdx.terminal.data` |
| Tmux command shell execution | 🔴 FINDING | `tmux new-session <command>` passes command to shell — shell injection risk |
| Session ID validation | ⚠️ GAP | `session_id` not validated; special chars in `-s` flag could cause tmux issues |
| Rate limiting | ⚠️ GAP | No rate limit on input from Matrix events |
| Input payload size check | ⚠️ GAP | `send_input()` doesn't bound payload size before forwarding to tmux |

---

## Findings

### 🔴 FINDING 1: tmux new-session shell executes the command string (HIGH)

**Location:** `session.rs:38-54` — `TmuxSession::create()`

```rust
tokio::process::Command::new("tmux")
    .args([
        "new-session", "-d", "-s", session_name,
        "-x", &cols.to_string(), "-y", &rows.to_string(),
        command,   // ← THIS
    ])
```

`tmux new-session` passes its final argument to the default shell (`/bin/sh -c <command>`). Even though `command` is passed to tmux's argv without shell interpolation, **tmux itself re-execs it through `/bin/sh`**. A command string like `bash; rm -rf /tmp/secret` would execute both `bash` and the rm.

**Origin of `command`:** `TerminalSessionRequestEvent.command` — comes from a Matrix room event. A user with room membership can send an arbitrary `command` string.

**Required fix:** Either:
1. Pass command as an explicit list (use `tmux new-session ... -- /bin/bash` and send the actual command via `send-keys` after creation), OR
2. Validate `command` against the same allowlist and `validate_cmd()`/`validate_args()` checks applied to `CommandEvent` before calling `create_session()`. Reject if it contains shell metacharacters.

**Recommended:** Option 1 (launch a fixed shell in the tmux session; deliver the command via `send-keys` after creation). This eliminates the shell-exec attack surface entirely.

---

### ⚠️ GAP 2: Session room `history_visibility` not set (MEDIUM)

**Location:** `session.rs:242-257` — `create_session()`

The `create_session()` method calls `client.join_room(room_id)` and begins streaming `org.mxdx.terminal.data` events. It does not set `history_visibility=joined` on the session room.

If `history_visibility=shared` (Matrix default), late-joining users — including those who joined after the terminal session ended — can read all past terminal output. This violates the isolation requirement stated in `docs/plans/mxdx-management-console.md` §5.

**Required fix:** After `join_room`, set `history_visibility=joined` via the Matrix CS API (`PUT /rooms/{roomId}/state/m.room.history_visibility`).

---

### ⚠️ GAP 3: Session room power levels not configured (MEDIUM)

**Location:** `session.rs:242-257` — `create_session()`

No power levels are set in the session room. Any room member with default power level (0) can send `org.mxdx.terminal.data` events, bypassing the launcher as the authoritative output source.

**Required fix:** After joining the session room, set power levels so only the launcher bot user (PL 100) can send `org.mxdx.terminal.data` and `org.mxdx.terminal.retransmit` events. Clients should have PL 0 and can only send `org.mxdx.terminal.data` (input direction) — or a separate event type for client→launcher input should be used to avoid confusion.

---

### ⚠️ GAP 4: Session ID not validated (LOW)

**Location:** `session.rs:38` — `TmuxSession::create(session_name, ...)`

`session_name` is passed directly to tmux's `-s` flag without validation. Tmux session names with characters like `:` or `.` have special meaning in tmux addressing. A session name containing `.` could shadow another pane address.

**Required fix:** Validate `session_id` to `[a-zA-Z0-9_-]+` before use. Reject otherwise.

---

### ⚠️ GAP 5: No rate limiting on terminal input (LOW)

**Location:** `session.rs:270-276` — `send_input()`

There is no rate limit on how frequently Matrix events can drive `send_input()`. A malicious client could flood the tmux session with rapid input events, causing resource exhaustion.

**Required fix:** Implement per-session token-bucket rate limiting on `send_input()`. Suggested: max 100 events/second per session.

---

### ⚠️ GAP 6: Input payload size not bounded before tmux (LOW)

**Location:** `session.rs:270-276` — `send_input()`

`data` received from a Matrix `org.mxdx.terminal.data` event is passed to `send_input()` without a size check. A large input payload could cause `tmux send-keys` to consume excessive memory or time.

Note: `decode_decompress_bounded()` in `mxdx-types` enforces 1MB on decompressed output, but this operates on the _output_ path (PTY → Matrix). The _input_ path (Matrix → tmux) has no equivalent check.

**Required fix:** Reject `data` payloads exceeding a configurable `max_input_bytes` (suggested: 64KB) in `send_input()`.

---

## Items Passing Review

### ✅ PTY dumb-pipe (exec path)

`tokio::process::Command::new("tmux").args([...])` — all arguments passed as separate vector elements, never interpolated into a shell string. This is the correct pattern.

### ✅ Command allowlist (Launcher::handle_command)

`lib.rs:77` checks `allowed_commands.contains(&cmd_event.cmd)` before spawning any process. The allowlist is set at launcher startup and is not runtime-configurable.

### ✅ Path traversal blocked

`validate_cwd()` in `command.rs:73-103` rejects:
- Any `cwd` containing `..`
- Relative paths (must start with `/`)
- Paths not matching the `DEFAULT_CWD_ALLOWLIST`

### ✅ Argument injection blocked

`validate_args()` in `command.rs:104+` rejects:
- Null bytes in any argument
- Shell metacharacters (`;`, `|`, `&`, `` ` ``, `$`)
- `git -c` config injection
- `docker --config` / `docker compose -f` file overrides

### ✅ Zlib bomb protection

`decode_decompress_bounded()` in `mxdx-types/src/events/terminal.rs` enforces a hard 1MB ceiling on decompressed output, preventing zlib bomb attacks (mxdx-ccx).

### ✅ Base64 decoding before decompression

The correct order (base64 → zlib) is maintained. No path decompresses without first decoding, preventing malformed zlib streams from reaching the decompressor directly.

---

## Required Actions Before Phase 7 Merge

| Priority | Action | Issue |
|---|---|---|
| 🔴 HIGH | Fix tmux shell execution: either validate command or use fixed-shell + send-keys | New issue required |
| 🟡 MEDIUM | Set `history_visibility=joined` in `create_session()` | New issue required |
| 🟡 MEDIUM | Set power levels in session room | New issue required |
| 🟢 LOW | Validate session_id format | Can be added to coder task |
| 🟢 LOW | Rate limit `send_input()` | mxdx-bfc (rate limiting epic) |
| 🟢 LOW | Bound `send_input()` payload size | Can be added to coder task |

The HIGH finding (tmux shell execution) is a blocker for merge. The MEDIUM findings should be resolved before production deployment but do not block the Phase 7 development branch.
