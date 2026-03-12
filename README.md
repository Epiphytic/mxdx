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

## Install

```sh
npm install -g @mxdx/cli
```

This installs the `mxdx` command (and `mx` alias) with all components: launcher, client, and web console.

Or run directly without installing:

```sh
npx -y @mxdx/cli launcher --help
npx -y @mxdx/cli client --help
```

Individual packages can also be installed standalone:

```sh
npx -y @mxdx/launcher start --servers http://localhost:8008
npx -y @mxdx/client exec my-launcher echo hello
npx -y @mxdx/web-console
```

## Quick Start

**1. Start a launcher on a managed host:**

```sh
mx launcher start \
  --servers http://localhost:8008 \
  --registration-token my-secret-token \
  --admin-user @admin:localhost \
  --allowed-commands echo,ls,date,uname \
  --allowed-cwd /tmp,/home
```

**2. Run a command from the client:**

```sh
mx client exec my-launcher echo hello
```

**3. Open an interactive shell:**

```sh
mx client shell my-launcher
```

**4. Launch the web console:**

```sh
mx web-console
```

Open [http://localhost:5173](http://localhost:5173) in your browser.

See the [Quickstart Guide](docs/quickstart.md) for full setup instructions including homeserver configuration.

## Packages

| Package | npm | Description |
| :--- | :--- | :--- |
| `@mxdx/cli` | [![npm](https://img.shields.io/npm/v/@mxdx/cli)](https://www.npmjs.com/package/@mxdx/cli) | Meta-package with `mxdx` and `mx` CLI aliases |
| `@mxdx/core` | [![npm](https://img.shields.io/npm/v/@mxdx/core)](https://www.npmjs.com/package/@mxdx/core) | WASM bindings, TerminalSocket, BatchedSender, P2P transport, credentials |
| `@mxdx/launcher` | [![npm](https://img.shields.io/npm/v/@mxdx/launcher)](https://www.npmjs.com/package/@mxdx/launcher) | Node.js agent running on managed hosts; creates PTY sessions, handles commands |
| `@mxdx/client` | [![npm](https://img.shields.io/npm/v/@mxdx/client)](https://www.npmjs.com/package/@mxdx/client) | CLI for fleet management: exec commands, shell sessions, telemetry |
| `@mxdx/web-console` | [![npm](https://img.shields.io/npm/v/@mxdx/web-console)](https://www.npmjs.com/package/@mxdx/web-console) | Browser console with xterm.js -- login, dashboard, terminal, and exec views |

Rust crates are also published on [crates.io](https://crates.io/search?q=mxdx) for developers building on the mxdx platform.

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
