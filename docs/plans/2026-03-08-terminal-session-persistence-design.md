# Terminal Session Persistence Design

## Problem

Interactive terminal sessions die when the user navigates away from the web console. The PtyBridge uses `script(1)` which has no persistence — the shell process is tied to the bridge's lifetime. Users expect to start a command, navigate away, come back, and find their session intact with history preserved.

## Approach: tmux + Matrix History Replay

Use tmux as the session persistence layer and Matrix DM room history as the scrollback replay mechanism.

- **tmux** keeps the shell alive across disconnects. Sessions survive user navigation, browser closure, and even launcher restarts (tmux is a separate process).
- **Matrix room history** stores all `org.mxdx.terminal.data` events. On reconnect, replay recent events into xterm.js for scrollback restoration.
- **Graceful fallback** to `script(1)` when tmux is unavailable or disabled.

## PtyBridge Changes

### tmux via script(1)

The existing `script(1)` pattern (piped stdio) is preserved. tmux is spawned inside it:

```
1. tmux new-session -d -s mxdx-{id} -x 80 -y 24
2. script -q /dev/null -c "tmux attach -t mxdx-{id}"
```

- Input: write to script's stdin -> tmux -> shell
- Output: read from script's stdout -> terminal data events
- Disconnect: kill script bridge; tmux session stays alive
- Reconnect: spawn new `script + tmux attach` to same session
- Resize: `tmux resize-window -t mxdx-{id} -x cols -y rows`

### Fallback mode

When tmux is unavailable or disabled, PtyBridge uses `script -q /dev/null -c bash` (current behavior). Sessions are non-persistent — disconnect kills the shell.

### API changes

- Constructor: accepts `sessionName` option for reconnecting to existing tmux sessions
- `detach()`: kills the script bridge but leaves tmux session alive
- Static `list()`: returns active mxdx tmux session names
- `resize()`: uses `tmux resize-window` when tmux-backed, SIGWINCH otherwise
- `persistent` getter: true when tmux-backed

## Launcher Config

New `use_tmux` setting in `launcher.toml`:

```toml
[launcher]
use_tmux = "auto"  # "auto" | "always" | "never"
```

- `auto` (default): detect tmux at startup, use if available
- `always`: require tmux, fail fast if missing
- `never`: disable tmux, use script(1) only

## Telemetry

Telemetry posted to exec room includes session persistence info:

```json
{
  "hostname": "...",
  "tmux_available": true,
  "tmux_version": "3.4",
  "session_persistence": true
}
```

Browser reads `session_persistence` to decide whether to warn on navigate-away.

## Session Registry

In-memory Map on the launcher:

```
sessionId -> { tmuxName, dmRoomId, sender, persistent, createdAt }
```

- Entries created on `interactive` action
- Cleaned up when tmux session exits or script process dies
- Survives browser disconnect (tmux keeps running)

## Protocol Changes

### New action: `list_sessions`

```json
// Request (org.mxdx.command):
{ "action": "list_sessions", "request_id": "..." }

// Response (org.mxdx.terminal.sessions):
{
  "request_id": "...",
  "sessions": [
    { "session_id": "abc", "room_id": "!dm:mx.org", "persistent": true, "created_at": "..." }
  ]
}
```

### New action: `reconnect`

```json
// Request (org.mxdx.command):
{ "action": "reconnect", "session_id": "abc", "request_id": "...", "cols": 80, "rows": 24 }

// Response (org.mxdx.terminal.session):
{ "request_id": "...", "status": "reconnected", "room_id": "!dm:mx.org" }
// or: { "request_id": "...", "status": "expired" }
```

### Updated `interactive` response

```json
{
  "request_id": "...",
  "status": "started",
  "room_id": "!dm:mx.org",
  "session_id": "abc",
  "persistent": true
}
```

## Browser Changes

### Dashboard

- On render, send `list_sessions` to each launcher
- Show active sessions with "Reconnect" buttons alongside "New Terminal"
- Non-persistent sessions labeled accordingly

### Terminal view — reconnect flow

1. Join existing DM room (already a member)
2. Fetch recent `org.mxdx.terminal.data` events from room history
3. Replay into xterm.js sorted by seq for scrollback
4. Create TerminalSocket on DM room, go live

### Navigate-away warning

- If `session_persistence: false` and terminal is active: warn via `beforeunload`
- If `session_persistence: true`: no warning

### Session memory

- Save `{ sessionId, dmRoomId, launcherExecRoomId }` in sessionStorage
- On page reload: auto-attempt reconnect before showing dashboard
