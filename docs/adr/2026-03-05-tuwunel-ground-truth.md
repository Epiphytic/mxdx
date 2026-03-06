# ADR: Tuwunel Ground Truth from Research Spike

**Date:** 2026-03-05
**Status:** Accepted
**Bead:** mxdx-d8h.2

## Context

Tuwunel is a high-performance Matrix homeserver written in Rust, successor to conduwuit (which itself forked from Conduit). Phase 3 of the mxdx rebuild depends on Tuwunel for test infrastructure. This ADR documents ground-truth findings from hands-on installation and experimentation with Tuwunel v1.5.0.

---

## 1. Installation

**Binary name:** `tuwunel` (installed to `/usr/sbin/tuwunel`)

**Version tested:** 1.5.0 (compiled with rustc 1.91.1, released 2026-01-31)

**Package formats available (x86_64 linux):**
- `.deb` (Debian/Ubuntu) -- preferred
- `.rpm` (RHEL/Fedora)
- `.zst` (raw compressed binary)
- `.nix.tar.zst` (Nix)
- Docker image: `jevolk/tuwunel:latest` or `ghcr.io/matrix-construct/tuwunel:latest`
- OCI tarball

**CPU microarchitecture variants:** x86_64-v1 (baseline), x86_64-v2, x86_64-v3. Use v1 for maximum compatibility.

**Build variants:** `all` (all features), `default`, `logging` (with debug log levels), `debuginfo`.

**Download URL pattern:**
```
https://github.com/matrix-construct/tuwunel/releases/download/v1.5.0/v1.5.0-release-all-x86_64-v1-linux-gnu-tuwunel.deb
```

**CI installation (validated):**
```bash
curl -L -o tuwunel.deb \
  "https://github.com/matrix-construct/tuwunel/releases/download/v1.5.0/v1.5.0-release-all-x86_64-v1-linux-gnu-tuwunel.deb"
sudo dpkg -i tuwunel.deb
```

**.deb package details:**
- Only dependency: `ca-certificates`
- Installed size: ~87 MB (single static binary)
- Installs: `/usr/sbin/tuwunel`, `/etc/tuwunel/tuwunel.toml`, `/usr/lib/systemd/system/tuwunel.service`
- Creates `tuwunel` user/group via postinst

**Without sudo (CI/containers):** Extract binary directly from .deb:
```bash
dpkg-deb --fsys-tarfile tuwunel.deb | tar xf - ./usr/sbin/tuwunel
chmod +x ./usr/sbin/tuwunel
```

---

## 2. CLI Flags

```
Usage: tuwunel [OPTIONS]

Options:
  -c, --config <CONFIG>    Path to the config TOML file (optional)
  -O, --option <OPTION>    Override a configuration variable using TOML 'key=value' syntax
      --read-only          Run in a stricter read-only --maintenance mode
      --maintenance        Run in maintenance mode while refusing connections
      --console            Activate admin command console automatically after startup
      --execute <EXECUTE>  Execute console command automatically after startup
  -h, --help               Print help
  -V, --version            Print version
```

**Environment variable override:** All config keys can be set via `TUWUNEL_` prefixed env vars:
- `TUWUNEL_SERVER_NAME`, `TUWUNEL_PORT`, `TUWUNEL_ALLOW_REGISTRATION`, etc.
- Nested keys use double underscore: `TUWUNEL_WELL_KNOWN__SERVER`
- Legacy prefixes `CONDUIT_` and `CONDUWUIT_` are also supported.

**Config via env:** `TUWUNEL_CONFIG=/path/to/config.toml` (used by systemd unit).

---

## 3. Config Format

**Format:** TOML, under `[global]` section. Example config installed to `/etc/tuwunel/tuwunel.toml`.

**Minimal working config (validated):**
```toml
[global]
server_name = "test.localhost"
database_path = "/tmp/tuwunel-test/db"
address = ["127.0.0.1"]
port = 16167
allow_registration = true
registration_token = "testtoken123"
log = "info"
```

**Key configuration options:**

| Key | Default | Description |
|-----|---------|-------------|
| `server_name` | (required) | Matrix server name. Cannot change after first run without DB wipe. |
| `database_path` | `/var/lib/tuwunel` | RocksDB data directory |
| `address` | `["127.0.0.1", "::1"]` | Listen addresses (array) |
| `port` | `8008` | Listen port (or array for multiple) |
| `unix_socket_path` | (unset) | Alternative: UNIX socket (mutually exclusive with address) |
| `allow_registration` | `false` | Enable open registration |
| `registration_token` | (unset) | Token required for registration if enabled |
| `registration_token_file` | (unset) | File containing registration token |
| `log` | `"info"` | tracing EnvFilter directive (release builds only support >= error for trace macros) |
| `log_colors` | `true` | ANSI color output |
| `log_compact` | `false` | Compact log format |
| `allow_federation` | `true` | Enable/disable federation |
| `federation_timeout` | `300` | Federation request timeout (seconds) |
| `allow_public_room_directory_over_federation` | `false` | Public room directory over federation |

**TLS section:**
```toml
[global.tls]
certs = "/path/to/certificate.crt"
key = "/path/to/certificate.key"
dual_protocol = false  # listen on both HTTP and HTTPS
```

**Well-known / delegation:**
```toml
[global.well_known]
client = "https://matrix.example.com"
server = "matrix.example.com:443"
```

---

## 4. Health Check

**Primary health endpoint (validated):**
```
GET /_matrix/client/versions -> HTTP 200
```

Response includes Matrix spec versions r0.0.1 through v1.15 and unstable features.

**Federation version endpoint (validated):**
```
GET /_matrix/federation/v1/version -> HTTP 200
{"server":{"name":"Tuwunel","version":"1.5.0","compiler":"rustc 1.91.1 ..."}}
```

**Startup time (validated):** ~840ms from process start to first HTTP response (cold start with fresh DB on SSD). Database open takes ~590ms. This is fast enough for test infrastructure spin-up.

**Systemd integration:** The service unit uses `Type=notify`, meaning tuwunel sends sd_notify when ready. For non-systemd environments, the server is ready when the TCP port accepts connections.

---

## 5. User Registration

**With registration token (validated):**
```bash
curl -X POST http://127.0.0.1:16167/_matrix/client/v3/register \
  -H "Content-Type: application/json" \
  -d '{
    "username": "testuser",
    "password": "testpass123",
    "auth": {
      "type": "m.login.registration_token",
      "token": "testtoken123"
    }
  }'
```

Response:
```json
{
  "access_token": "NdqfIU469dlCfizdu6L28jtcamfLD0FJ",
  "user_id": "@testuser:test.localhost",
  "device_id": "0yoWzSMoLN"
}
```

**Without token:** Returns UIAA flow requiring `m.login.registration_token`:
```json
{
  "flows": [{"stages": ["m.login.registration_token"]}],
  "session": "<session_id>"
}
```

**First registered user** automatically becomes server admin and is joined to `#admins` room.

**Login flows available:** `m.login.password`, `m.login.token`, `m.login.application_service`, `org.matrix.login.jwt`.

---

## 6. Federation

**Federation is enabled by default** (`allow_federation = true`).

**Requirements for federation between two instances:**
1. Valid DNS name for `server_name`
2. TLS certificate (federation requires HTTPS)
3. Either direct TLS on port 8448 or reverse proxy with `.well-known/matrix/server` delegation
4. Both servers must be able to resolve each other

**For local testing (two instances on localhost):** Federation between two localhost instances requires TLS. Without valid TLS certs and DNS, federation will not work. For CI test infrastructure, consider:
- Using a reverse proxy (Caddy with self-signed certs)
- Or using `[global.tls]` config with generated certs
- `dual_protocol = true` allows both HTTP and HTTPS on same port

**Key federation config:**
```toml
[global]
allow_federation = true
federation_timeout = 300        # seconds
federation_idle_timeout = 25    # seconds
federation_idle_per_host = 1
federation_loopback = false     # allow federation to self (likely a bug if needed)
allow_device_name_federation = false
allow_inbound_profile_lookup_federation_requests = true

[global.well_known]
server = "matrix.example.com:443"
client = "https://matrix.example.com"
```

---

## 7. Appservice

**Registration method:** Via admin room commands (not config file paths like Synapse).

```
!admin appservices register
<paste YAML content>
```

**List registered appservices:**
```
!admin appservices list
```

**Unregister:**
```
!admin appservices unregister <name>
```

**No restart required** after registering an appservice.

**Registration YAML format:** Standard Matrix appservice registration YAML (same as Synapse). The YAML content is pasted directly into the admin room, not referenced by file path.

Example appservice registration YAML:
```yaml
id: my-bridge
url: "http://localhost:9000"
as_token: "secret_as_token"
hs_token: "secret_hs_token"
sender_localpart: "bridge_bot"
namespaces:
  users:
    - exclusive: true
      regex: "@bridge_.*:example.com"
  rooms: []
  aliases: []
```

**Programmatic registration for CI:** Use the `--execute` CLI flag:
```bash
tuwunel -c config.toml --execute "appservices register <yaml_content>"
```

**Config options:**
```toml
appservice_timeout = 35         # request timeout (seconds)
appservice_idle_timeout = 300   # idle connection timeout
dns_passthru_appservices = false
```

---

## 8. Shutdown

**SIGTERM (validated):** Clean graceful shutdown. Process exits within ~1 second.

**Systemd unit configuration:**
```ini
TimeoutStopSec=2m    # max 2 minutes for shutdown
Restart=on-failure
RestartSec=5
```

**Config option:**
```toml
# Grace period for clean shutdown of federation requests (seconds)
# (found in config but default not documented in example)
```

**Behavior observed:** On SIGTERM, tuwunel logs "Shutting down services..." and exits cleanly. The RocksDB database is flushed before exit.

**SIGINT:** Not explicitly tested, but Rust signal handling typically treats SIGINT and SIGTERM equivalently for tokio-based servers.

---

## 9. Quirks vs Synapse/Conduit

1. **Single binary:** Unlike Synapse (Python, many workers), Tuwunel is a single ~87MB static binary. No Python, no workers, no database server needed (embedded RocksDB).

2. **Admin via Matrix:** Admin commands are issued in the `#admins` Matrix room rather than via a REST API or config files. This includes appservice registration.

3. **No separate admin API:** Unlike Synapse's `/_synapse/admin` endpoints, administration is primarily through the admin room. The `--execute` flag allows CLI-based admin commands.

4. **Config is TOML, not YAML:** Synapse uses YAML, Conduit used TOML, Tuwunel continues with TOML.

5. **Legacy compat:** Supports `CONDUIT_` and `CONDUWUIT_` environment variable prefixes for backward compatibility with migrations from those servers.

6. **Server name is permanent:** Cannot be changed after first database creation. Requires full DB wipe to change.

7. **Default port is 8008** (not 8448). The Debian package README states it defaults to 6167 behind a reverse proxy, but the config file default is 8008.

8. **Displayname suffix:** New users get a suffix on their display name (default: heart emoji). Disable with `new_user_displayname_suffix = ""`.

9. **Fast startup:** ~840ms cold start vs Synapse's typical 10-30+ seconds. Suitable for ephemeral test instances.

10. **RocksDB database:** 102 column families created on init. Database version is 17. No external database server needed (vs Synapse which requires PostgreSQL for production).

11. **systemd-notify integration:** Uses `Type=notify` in systemd, so `systemctl start` blocks until the server is actually ready. For non-systemd use, poll the `/versions` endpoint.

12. **Build variants matter:** The `release-all` build includes all features. The `release-default` build may lack some. The `release-logging` build enables debug-level trace macros that are compiled out in release builds.

---

## Decision

Tuwunel v1.5.0 is suitable for mxdx test infrastructure:

- **Fast startup** (~840ms) makes ephemeral test instances viable
- **Single binary with no external dependencies** simplifies CI
- **Token-based registration** allows controlled user creation in tests
- **Standard Matrix API** for health checks and registration
- **Appservice registration** via admin commands (can be automated with `--execute`)
- **Clean shutdown** on SIGTERM

**For CI pipeline integration:**
```bash
# Install
curl -L -o /tmp/tuwunel.deb "$TUWUNEL_DEB_URL"
dpkg-deb --fsys-tarfile /tmp/tuwunel.deb | tar xf - ./usr/sbin/tuwunel

# Run
./usr/sbin/tuwunel -c test-config.toml &
# Wait for ready
until curl -sf http://127.0.0.1:8008/_matrix/client/versions; do sleep 0.1; done

# Register test user
curl -X POST http://127.0.0.1:8008/_matrix/client/v3/register \
  -H "Content-Type: application/json" \
  -d '{"username":"test","password":"test","auth":{"type":"m.login.registration_token","token":"$TOKEN"}}'

# Cleanup
kill %1
```
