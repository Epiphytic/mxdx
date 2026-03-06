# Phase 6: Terminal — Interactive Sessions — Summary

## Completion Date: 2026-03-06

## What Was Built

### PTY/tmux Integration (`src/terminal/tmux.rs`)
- `TmuxSession`: async wrapper around tmux CLI
- `create`, `send_input` (literal mode `-l`), `capture_pane`, `capture_pane_until`, `resize`, `kill`
- Session name validation: `[a-zA-Z0-9_-]+` — rejects path traversal (mxdx-8bm)
- `Drop` impl for cleanup on panic/early exit
- Uses `Command::new("tmux").args([...])` — no shell interpolation

### Adaptive Compression (`src/terminal/compression.rs`)
- `compress_encode`: raw+base64 for <32 bytes, zlib+base64 for >=32 bytes
- `decode_decompress_bounded`: streaming 8KB chunk decompression with byte counting
- Zlib bomb protection: bails immediately when `total > max_bytes` (mxdx-ccx)

### Output Batcher (`src/terminal/batcher.rs`)
- Accumulates PTY output, flushes at size threshold or time interval
- Prevents flooding Matrix rooms with per-byte events

### Ring Buffer (`src/terminal/ring_buffer.rs`)
- `EventRingBuffer<T>`: O(1) seq lookup, capacity-bounded with eviction
- u64 seq counter — tested with near `u64::MAX` values (mxdx-seq)

### Terminal Session (`src/terminal/session.rs`)
- `TerminalSession`: ties TmuxSession + EventRingBuffer + compression together
- `handle_input`, `capture_output`, `resize`, `kill`

### Crash Recovery (`src/terminal/recovery.rs`)
- `RecoveryState`: JSON persistence of session-to-room mappings
- `list_tmux_sessions`: discovers existing tmux sessions on restart
- `recoverable_sessions`: matches saved state to live tmux sessions

## Tests

35 lib tests (16 new for Phase 6) + 3 E2E integration tests:

| Category | Count | Key Tests |
|:---|:---|:---|
| tmux integration | 3 | create+capture, name validation, resize |
| Compression | 5 | raw/zlib paths, boundary, zlib bomb rejection, fast-fail timing |
| Ring buffer | 4 | basic ops, eviction, u64::MAX seq, get_since |
| Batcher | 2 | size flush, time flush |
| Recovery | 4 | save/load, empty default, recoverable matching, list sessions |
| E2E terminal | 3 | DM creation (mxdx-aew), input/output bridge, u64 seq counter |

## Security Issues Addressed

| Finding | Status | Control |
|:---|:---|:---|
| mxdx-aew (history_visibility) | Implemented + tested | `HistoryVisibility::Joined` in `create_terminal_session_dm` |
| mxdx-ccx (zlib bomb) | Implemented + tested | Streaming 8KB chunk decompression with byte limit |
| mxdx-seq (u64 seq) | Implemented + tested | `EventRingBuffer` uses u64, tested near u64::MAX |
| mxdx-8bm (PTY injection) | Implemented + tested | tmux `-l` literal mode, session name regex, no `sh -c` |

## Security Review Findings

- **Medium**: terminal command not validated against allowlist in session creation path
- **Low**: recovery state file not permission-checked (room IDs not secrets, but leaks topology)
- **Low**: tmux `send-keys -l` interprets Enter — correct behavior for terminal bridging

## CI Updates

- `cargo test -p mxdx-launcher --lib` job covers all terminal unit tests
- `cargo test -p mxdx-launcher --test e2e_terminal_session` added to integration job (requires tuwunel + tmux)

## Key Commits

| Commit | Description |
|:---|:---|
| `ab9917e` | tmux integration and adaptive compression |
| `e7940ad` | batcher, ring buffer, session lifecycle |
| `36ea8e7` | crash recovery with tmux session matching |
