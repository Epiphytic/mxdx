# mxdx Quickstart Guide

mxdx is a Matrix-native fleet management system. Launchers run on remote machines and expose terminals and command execution over end-to-end encrypted Matrix rooms. Clients connect via CLI or a web console.

## Prerequisites

- **Node.js v22+**
- A **Matrix homeserver** -- either your own (Tuwunel) or a public server (matrix.org)

## Install

Install the CLI globally:

```sh
npm install -g @mxdx/cli
```

This gives you the `mxdx` command (and `mx` shorthand). Or run without installing:

```sh
npx -y @mxdx/cli --help
```

---

## Option A: Private Server (Tuwunel)

Running your own homeserver gives you full control, no rate limits, and faster interactive sessions.

### 1. Install Tuwunel

Follow the instructions at [https://github.com/girlbossceo/tuwunel](https://github.com/girlbossceo/tuwunel).

### 2. Configure tuwunel.toml

Create a minimal configuration file. The key setting is `registration_token`, which allows mxdx to auto-register accounts:

```toml
[global]
server_name = "localhost"
port = [8008]
database_path = "/var/lib/tuwunel"
allow_registration = true
registration_token = "my-secret-token"
```

Start Tuwunel and confirm it is listening on port 8008.

### 3. Start the launcher

On the machine you want to manage, run:

```sh
mx launcher start \
  --servers http://localhost:8008 \
  --username my-launcher \
  --password secretpass \
  --registration-token my-secret-token \
  --admin-user @admin:localhost \
  --allowed-commands echo,ls,date,uname \
  --allowed-cwd /tmp,/home \
  --log-format text
```

The launcher registers itself on the server, creates its room topology, and begins posting telemetry. The password is stored in your OS keyring after the first run.

Or run without a global install:

```sh
npx -y @mxdx/launcher start --servers http://localhost:8008 ...
```

### 4. Use the CLI client

Run a one-shot command:

```sh
mx client \
  --server http://localhost:8008 \
  --username admin \
  --password adminpass \
  exec my-launcher echo hello
```

Open an interactive shell:

```sh
mx client \
  --server http://localhost:8008 \
  --username admin \
  --password adminpass \
  shell my-launcher
```

Or run standalone:

```sh
npx -y @mxdx/client --server http://localhost:8008 --username admin exec my-launcher echo hello
```

### 5. Use the web console

```sh
mx web-console
```

Open [http://localhost:5173](http://localhost:5173) in your browser. Log in with your admin Matrix credentials.

Or run standalone:

```sh
npx -y @mxdx/web-console
```

---

## Option B: Public Server (matrix.org)

You can run mxdx against any public Matrix homeserver. matrix.org is the largest.

### 1. Create two Matrix accounts

Register two accounts on [https://matrix.org](https://matrix.org) -- one for the launcher and one for the admin user. You can use Element or any other Matrix client to create them.

### 2. Cross-sign verify both accounts

Open both accounts in Element (or another client that supports cross-signing) and verify them. This establishes the trust chain that mxdx relies on for E2EE.

### 3. Start the launcher

```sh
mx launcher start \
  --servers https://matrix.org \
  --username my-launcher \
  --password launcherpass \
  --admin-user @myadmin:matrix.org \
  --allowed-commands echo,ls,date,uname \
  --allowed-cwd /tmp,/home \
  --log-format text
```

### 4. Connect with the client or web console

Use the same client commands from Option A, replacing `--server` with `https://matrix.org` and using your admin credentials.

### Important: rate limits

Public servers enforce rate limits (roughly 1 event/sec on matrix.org). One-shot `exec` commands work fine, but interactive terminal sessions will be sluggish over Matrix transport. For responsive interactive use on public servers, P2P transport is required -- it negotiates a direct WebRTC connection between client and launcher, bypassing the homeserver for terminal I/O.

---

## Launcher CLI Options

| Flag | Description | Default |
| :--- | :--- | :--- |
| `--servers <url,...>` | Comma-separated Matrix server URLs | (required) |
| `--username <name>` | Launcher username | System hostname |
| `--password <pass>` | Password (first run only -- stored in OS keyring) | |
| `--registration-token <tok>` | Auto-register with this token | |
| `--admin-user <mxid,...>` | Admin users to invite at power level 100 | |
| `--allowed-commands <cmd,...>` | Command allowlist for exec | |
| `--allowed-cwd <path,...>` | Allowed working directories | |
| `--config <path>` | Config file path | `~/.config/mxdx-launcher/config.json` |
| `--telemetry <full\|summary>` | Telemetry detail level | `full` |
| `--max-sessions <n>` | Max concurrent terminal sessions | `5` |
| `--log-format <json\|text>` | Log output format | `json` |
| `--use-tmux <mode>` | tmux mode: `auto`, `always`, `never` | `auto` |
| `--batch-ms <ms>` | Terminal output batch window (milliseconds) | `200` |
| `--p2p-enabled <bool>` | Enable P2P transport | `true` |
| `--p2p-batch-ms <ms>` | P2P batch window (milliseconds) | `10` |
| `--p2p-idle-timeout-s <seconds>` | P2P idle timeout (seconds) | `300` |
| `--p2p-advertise-ips <bool>` | Include internal IPs in telemetry | `false` |
| `--p2p-turn-only <bool>` | Force P2P through TURN relay only | `false` |

### Launcher subcommands

| Command | Description |
| :--- | :--- |
| `start` | Start the launcher agent (default) |
| `reload` | Restart with fresh modules (picks up new WASM/libraries) |
| `cleanup <targets>` | Clean up stale Matrix state. Targets: `devices`, `events`, `rooms` |

---

## Client CLI Commands

| Command | Description |
| :--- | :--- |
| `exec <launcher> [cmd...]` | Execute a command on a launcher. Use `--cwd <path>` to set working directory. |
| `shell <launcher> [command]` | Start an interactive terminal session. Defaults to `/bin/bash`. Supports `--cols` and `--rows`. |
| `launchers` | List all discovered launchers and their room topology. |
| `telemetry <launcher>` | Show host telemetry (hostname, platform, CPU, memory, uptime). |
| `verify <user_id>` | Cross-sign verify another user by their Matrix ID. |
| `cleanup <targets>` | Clean up stale Matrix state (devices, events, rooms). Supports `--older-than` and `--force-cleanup`. |

### Client global options

| Flag | Description | Default |
| :--- | :--- | :--- |
| `--server <url>` | Matrix server URL | (required) |
| `--username <name>` | Username | (required) |
| `--password <pass>` | Password (first run only -- stored in OS keyring) | |
| `--registration-token <tok>` | Registration token | |
| `--format <text\|json>` | Output format | `text` |
| `--config <path>` | Config file path | `~/.config/mxdx-client/config.json` |
| `--batch-ms <ms>` | Terminal output batch window (milliseconds) | `200` |
| `--p2p-enabled <bool>` | Enable P2P transport | `true` |
| `--p2p-batch-ms <ms>` | P2P batch window (milliseconds) | `10` |
| `--p2p-idle-timeout-s <seconds>` | P2P idle timeout (seconds) | `300` |

---

## Web Console

The web console is a browser-based UI with xterm.js.

1. **Login** -- Enter your Matrix server URL, username, and password. The console logs in via the WASM Matrix SDK and establishes an E2EE session.

2. **Dashboard** -- After login, the dashboard displays all discovered launchers with their telemetry (hostname, platform, memory, uptime).

3. **Terminal** -- Click a launcher to open an interactive terminal session. Terminal I/O is end-to-end encrypted.

4. **P2P indicator** -- The connection status indicator shows the current transport:
   - Green: P2P connection active (direct WebRTC, low latency)
   - Yellow: Matrix transport (relayed through the homeserver, higher latency)

5. **Exec** -- Run one-shot commands from the dashboard without opening a full terminal session.

---

## Development

To contribute or build from source, see the [Development Methodology](mxdx-development-methodology.md). The source requires Rust (stable), wasm-pack, and Node.js v22+.
