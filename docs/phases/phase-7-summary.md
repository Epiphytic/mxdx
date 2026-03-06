# Phase 7: Browser Client — Summary

## Completion Date: 2026-03-06

## What Was Built

### CryptoClient (`src/crypto.ts`)
- Facade wrapping `@matrix-org/matrix-sdk-crypto-wasm` OlmMachine
- Singleton WASM initialization, in-memory key store for Node
- `encrypt`, `decrypt`, `outgoingRequests`, identity key accessors

### MxdxClient + Discovery (`src/client.ts`, `src/discovery.ts`)
- `MxdxClient.connect(homeserver, accessToken)` — Matrix CS API client
- `listLaunchers(spaceRoomId)` — reads `org.mxdx.launcher.identity` state events
- `getLauncherStatus(roomId, launcherId)` — reads telemetry state event
- `createTerminalSession` — sends `org.mxdx.terminal.session_request`
- `attachTerminalSession` — stub for TerminalSocket wiring

### TerminalSocket (`src/terminal.ts`)
- xterm.js AttachAddon compatible: `binaryType`, `send()`, `close()`, `onmessage`, `onclose`, `onerror`
- Adaptive compression: zlib for >= 32 bytes, raw base64 otherwise
- Sequence reordering with gap detection
- Retransmit protocol: 500ms gap timeout, `org.mxdx.terminal.retransmit`, 500ms accept timeout
- Exponential backoff reconnect (1s-30s)

## Tests

69 total tests:

| Category | Count | Key Tests |
|:---|:---|:---|
| Crypto | 7 | OlmMachine init, identity keys, distinct instances |
| Discovery | 11 | API surface, mock discovery, session creation |
| TerminalSocket | 17 | xterm.js compat, compression, seq reordering, gap handling, retransmit |
| Schemas | 34 | Existing Zod schema validation |

## Completion Gate
xterm.js AttachAddon binds to TerminalSocket, sends/receives PTY data over Matrix DMs.

## Key Commits

| Commit | Description |
|:---|:---|
| `b6189c5` | CryptoClient facade |
| `44e6342` | MxdxClient + discovery |
| `c2fb113` | TerminalSocket with compression + seq |
| `646a2c0` | Sequence gap handling + retransmit |
