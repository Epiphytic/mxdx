# mxdx

Matrix-native fleet management with interactive browser terminals, end-to-end encryption, and P2P transport.

## Features

- **End-to-end encrypted** command execution and terminal sessions via Matrix Megolm
- **P2P transport** over WebRTC data channels with AES-256-GCM encryption (0.17ms avg RTT)
- **Interactive browser terminals** powered by xterm.js with zlib compression
- **Multi-homeserver support** -- works with any Matrix homeserver (Tuwunel, matrix.org, etc.)
- **Adaptive batching** -- 5ms for P2P, 200ms for Matrix rate-limit safety
- **Cross-platform** -- Rust WASM core shared between Node.js agents and browser clients

## Architecture

```
                        Matrix Homeserver
                       (Tuwunel / matrix.org)
                              |
              E2EE signaling + fallback transport
                              |
         +--------------------+--------------------+
         |                    |                     |
    +---------+         +---------+          +------------+
    | Launcher|---P2P---| Client  |          | Web Console|
    | (agent) | WebRTC  |  (CLI)  |          |   (SPA)    |
    +---------+         +---------+          +------------+
         |                                        |
      PTY/exec                               xterm.js
```

## Quick Start

See the [Quickstart Guide](docs/quickstart.md) for full setup and deployment instructions.

```sh
npm install
wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm
wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/web-console/wasm
```

## Components

| Package | Path | Description |
| :--- | :--- | :--- |
| `@mxdx/core` | `packages/core` | WASM bindings (Rust matrix-sdk via wasm-pack), TerminalSocket, BatchedSender, P2P transport, credentials |
| Launcher | `packages/launcher` | Node.js agent running on managed hosts; creates PTY sessions, handles commands |
| Client | `packages/client` | CLI for fleet management: exec commands, shell sessions, telemetry |
| Web Console | `packages/web-console` | Vite SPA with xterm.js -- login, dashboard, terminal, and exec views |
| E2E Tests | `packages/e2e-tests` | End-to-end tests with TuwunelInstance helper and performance benchmarks |
| Core WASM | `crates/mxdx-core-wasm` | Rust crate compiled to WASM; wraps matrix-sdk for both Node.js and browser targets |

## Security Model

All communications are end-to-end encrypted. Encryption is never bypassed under any circumstances.

- **Matrix transport**: Megolm group encryption for rooms, MSC4362 encrypted state events
- **P2P transport**: AES-256-GCM over WebRTC data channels with DTLS
- **Signaling**: WebRTC offer/answer/candidates sent via encrypted Matrix events (m.call.invite, m.call.answer, m.call.candidates)
- **Credentials**: Stored encrypted at rest; OS keychain integration where available
- **Room topology**: Two E2EE rooms per launcher (exec + logs); DM rooms for interactive terminal sessions

## Performance

| Metric | P2P (WebRTC) | Matrix (Local) | Matrix (Public) |
| :--- | :--- | :--- | :--- |
| Terminal RTT | ~0.17ms | ~25ms | ~1100ms |
| Batching interval | 5ms | 200ms | 200ms |
| Transport | Direct / TURN relay | Homeserver | Homeserver (rate-limited) |

Private Tuwunel deployments provide low-latency fleet management. Public homeservers (matrix.org) work but are rate-limited to approximately 1 event/sec.

## Documentation

- [Quickstart Guide](docs/quickstart.md)
- [Architecture Overview](docs/mxdx-architecture.md)
- [Tuwunel Setup Guide](docs/guides/tuwunel-setup.md) -- build your own private server
- [Public Server Interop Guide](docs/guides/public-server-interop.md) -- using matrix.org with latency caveats
- [Management Console Design](docs/mxdx-management-console.md)
- [Development Methodology](docs/mxdx-development-methodology.md)
- [Performance Benchmarks](packages/e2e-tests/results/perf-terminal.html) -- P2P vs Matrix latency results
- [ADRs](docs/adr/)
- [Security Reviews](docs/reviews/security/)

## License

MIT
