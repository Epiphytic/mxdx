# Building and Running a Private mxdx Service with Tuwunel

## What is Tuwunel?

Tuwunel is a Matrix homeserver, forked from Conduit. It is lightweight, fast, and ships as a single binary. It is the ideal choice for private mxdx deployments: no rate limits, low latency, and full control over the server environment.

## Installing Tuwunel

**From source:**

```bash
cargo install tuwunel
```

Or build directly from the repository: https://github.com/girlbossceo/tuwunel

**Pre-built binaries:**

Download from the [Tuwunel releases](https://github.com/girlbossceo/tuwunel/releases) page.

**System package:**

If installed via your distribution's package manager, the binary is typically at `/usr/sbin/tuwunel`.

## Configuration

Create a minimal `tuwunel.toml` for mxdx:

```toml
[global]
server_name = "mxdx.example.com"
database_path = "/var/lib/tuwunel/db"
address = ["0.0.0.0"]
port = 8448
allow_registration = true
registration_token = "your-secret-token"
log = "info"
new_user_displayname_suffix = ""
```

### Key Settings Explained

- **server_name** -- The domain users will appear as `@user:server_name`. Choose this carefully; it cannot be changed after federation begins.
- **registration_token** -- Required for account creation. Keep this secret; anyone with the token can create accounts on your server.
- **database_path** -- Where Tuwunel stores its database. Ensure the directory exists and is writable.
- **For local dev/testing** -- Use the `.localhost` TLD (RFC 6761). No certificates or `/etc/hosts` entries are needed.
- **For production** -- Use a real domain with TLS. Place a reverse proxy (nginx or Caddy) in front of Tuwunel to terminate TLS.

## Network Architecture

```
[Launcher Host]  <--- Matrix E2EE ---> [Tuwunel Server] <--- Matrix E2EE ---> [Web Console / CLI Client]
     |                                                                              |
     +------------------------ P2P (WebRTC, AES-256-GCM) --------------------------+
```

All Matrix traffic is end-to-end encrypted. The Tuwunel server relays ciphertext but cannot read message contents. P2P connections (for interactive terminals) bypass the server entirely via WebRTC with AES-256-GCM encryption.

## Setting Up Accounts

1. **Create a launcher account** -- One per managed host. The launcher will auto-register on first run if given a registration token.
2. **Create an admin account** -- This is the fleet operator who manages launchers from the web console or CLI.
3. **Cross-sign verify** -- Required for E2EE trust. Both accounts must verify each other's devices before encrypted communication works.

## Running mxdx with Tuwunel

**Start Tuwunel:**

```bash
tuwunel
```

Tuwunel reads `tuwunel.toml` from the current directory by default.

**Start the launcher:**

```bash
mxdx-launcher --servers http://your-server:8448 --registration-token your-token
```

- On first run, the launcher auto-registers an account and creates the room topology (a Matrix space containing an exec room and a logs room, both E2EE with encrypted state events).
- On subsequent runs, the launcher restores its session from the OS keychain. No re-registration or re-login is needed.

**Connect with the web console or CLI client:**

Log in with the admin account to discover and manage all launchers visible to that user.

## Performance Characteristics (Private Server)

| Metric | Value |
| :--- | :--- |
| Matrix event round-trip time | ~25ms (local Tuwunel) |
| P2P terminal round-trip time | ~0.17ms (loopback) |
| Rate limits | None |

A private Tuwunel instance has no rate limits, making it suitable for interactive terminal sessions even without P2P fallback. On public servers like matrix.org, rate limiting (~1 event/sec) makes P2P essential for real-time terminal use.

## Security Considerations

- **Keep the registration_token secret.** Anyone with it can create accounts on your server.
- **Use TLS in production.** Place Tuwunel behind a reverse proxy (nginx/Caddy) with a valid certificate.
- **Server operator cannot read E2EE content.** Tuwunel stores encrypted room state; decryption keys exist only on client devices.
- **P2P data never touches the server.** WebRTC connections run directly between launcher and client, encrypted with AES-256-GCM.
- **Sensitive credentials are stored in the OS keychain**, not on disk in plaintext.

## Scaling

- Each launcher creates its own Matrix space with exec + logs rooms.
- Multiple launchers can share a single Tuwunel instance.
- The web console automatically discovers all launchers visible to the logged-in user.
- For large fleets, Tuwunel's lightweight footprint (single binary, embedded database) keeps resource usage low even with many rooms.
