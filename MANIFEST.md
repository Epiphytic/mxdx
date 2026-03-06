# Module & Agent Manifest

## Crates

| Crate | Path | Purpose |
|:---|:---|:---|
| mxdx-types | crates/mxdx-types | Shared event schema types |
| mxdx-matrix | crates/mxdx-matrix | matrix-sdk facade |
| mxdx-policy | crates/mxdx-policy | Policy Agent appservice binary |
| mxdx-secrets | crates/mxdx-secrets | Secrets Coordinator binary |
| mxdx-launcher | crates/mxdx-launcher | Launcher binary |
| mxdx-web | crates/mxdx-web | Web app (Axum, HTMX) |

## npm Packages

| Package | Path | Purpose |
|:---|:---|:---|
| @mxdx/client | client/mxdx-client | Browser Matrix client with E2EE |
| @mxdx/web-ui | client/mxdx-web-ui | HTMX dashboard + xterm.js terminal |

## External Facades

| Facade | Crate | Wraps |
|:---|:---|:---|
| MatrixClient | mxdx-matrix | matrix-sdk — never call matrix-sdk directly |
| CryptoClient | client/mxdx-client/src/crypto.ts | matrix-sdk-crypto-wasm |

<!-- BEGIN GENERATED SYMBOL TABLES -->

### mxdx-launcher

| Symbol | Kind | File |
|:---|:---|:---|
| `LauncherConfig` | struct | `crates/mxdx-launcher/src/config.rs` |
| `GlobalConfig` | struct | `crates/mxdx-launcher/src/config.rs` |
| `HomeserverConfig` | struct | `crates/mxdx-launcher/src/config.rs` |
| `CapabilitiesConfig` | struct | `crates/mxdx-launcher/src/config.rs` |
| `CapabilityMode` | enum | `crates/mxdx-launcher/src/config.rs` |
| `TelemetryConfig` | struct | `crates/mxdx-launcher/src/config.rs` |
| `TelemetryDetail` | enum | `crates/mxdx-launcher/src/config.rs` |
| `validate_config_permissions` | fn | `crates/mxdx-launcher/src/config.rs` |
| `TerminalSession` | struct | `crates/mxdx-launcher/src/terminal/session.rs` |
| `TerminalSession::create` | method | `crates/mxdx-launcher/src/terminal/session.rs` |
| `TerminalSession::handle_input` | method | `crates/mxdx-launcher/src/terminal/session.rs` |
| `TerminalSession::capture_output` | method | `crates/mxdx-launcher/src/terminal/session.rs` |
| `TerminalSession::resize` | method | `crates/mxdx-launcher/src/terminal/session.rs` |
| `TerminalSession::kill` | method | `crates/mxdx-launcher/src/terminal/session.rs` |
| `TmuxSession` | struct | `crates/mxdx-launcher/src/terminal/tmux.rs` |
| `TmuxSession::create` | method | `crates/mxdx-launcher/src/terminal/tmux.rs` |
| `TmuxSession::send_input` | method | `crates/mxdx-launcher/src/terminal/tmux.rs` |
| `TmuxSession::capture_pane` | method | `crates/mxdx-launcher/src/terminal/tmux.rs` |
| `TmuxSession::capture_pane_until` | method | `crates/mxdx-launcher/src/terminal/tmux.rs` |
| `TmuxSession::resize` | method | `crates/mxdx-launcher/src/terminal/tmux.rs` |
| `TmuxSession::kill` | method | `crates/mxdx-launcher/src/terminal/tmux.rs` |
| `OutputBatcher` | struct | `crates/mxdx-launcher/src/terminal/batcher.rs` |
| `OutputBatcher::new` | method | `crates/mxdx-launcher/src/terminal/batcher.rs` |
| `OutputBatcher::push` | method | `crates/mxdx-launcher/src/terminal/batcher.rs` |
| `OutputBatcher::tick` | method | `crates/mxdx-launcher/src/terminal/batcher.rs` |
| `OutputBatcher::flush` | method | `crates/mxdx-launcher/src/terminal/batcher.rs` |
| `list_tmux_sessions` | fn | `crates/mxdx-launcher/src/terminal/recovery.rs` |
| `RecoveryState` | struct | `crates/mxdx-launcher/src/terminal/recovery.rs` |
| `SessionState` | struct | `crates/mxdx-launcher/src/terminal/recovery.rs` |
| `RecoveryState::load` | method | `crates/mxdx-launcher/src/terminal/recovery.rs` |
| `RecoveryState::save` | method | `crates/mxdx-launcher/src/terminal/recovery.rs` |
| `RecoveryState::add_session` | method | `crates/mxdx-launcher/src/terminal/recovery.rs` |
| `RecoveryState::remove_session` | method | `crates/mxdx-launcher/src/terminal/recovery.rs` |
| `RecoveryState::recoverable_sessions` | method | `crates/mxdx-launcher/src/terminal/recovery.rs` |
| `compress_encode` | fn | `crates/mxdx-launcher/src/terminal/compression.rs` |
| `decode_decompress_bounded` | fn | `crates/mxdx-launcher/src/terminal/compression.rs` |
| `EventRingBuffer` | struct | `crates/mxdx-launcher/src/terminal/ring_buffer.rs` |
| `EventRingBuffer::new` | method | `crates/mxdx-launcher/src/terminal/ring_buffer.rs` |
| `EventRingBuffer::push` | method | `crates/mxdx-launcher/src/terminal/ring_buffer.rs` |
| `EventRingBuffer::get_range` | method | `crates/mxdx-launcher/src/terminal/ring_buffer.rs` |
| `EventRingBuffer::get` | method | `crates/mxdx-launcher/src/terminal/ring_buffer.rs` |
| `EventRingBuffer::len` | method | `crates/mxdx-launcher/src/terminal/ring_buffer.rs` |
| `EventRingBuffer::is_empty` | method | `crates/mxdx-launcher/src/terminal/ring_buffer.rs` |
| `collect_telemetry` | fn | `crates/mxdx-launcher/src/telemetry/system.rs` |
| `ExecutorError` | struct | `crates/mxdx-launcher/src/executor.rs` |
| `ValidatedCommand` | struct | `crates/mxdx-launcher/src/executor.rs` |
| `CommandResult` | struct | `crates/mxdx-launcher/src/executor.rs` |
| `execute_command` | fn | `crates/mxdx-launcher/src/executor.rs` |
| `validate_command` | fn | `crates/mxdx-launcher/src/executor.rs` |

### mxdx-matrix

| Symbol | Kind | File |
|:---|:---|:---|
| `MatrixClient` | struct | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::register_and_connect` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::is_logged_in` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::crypto_enabled` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::user_id` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::create_encrypted_room` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::create_dm` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::join_room` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::send_event` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::send_state_event` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::sync_once` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::sync_and_collect_events` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::inner` | method | `crates/mxdx-matrix/src/client.rs` |
| `LauncherTopology` | struct | `crates/mxdx-matrix/src/rooms.rs` |
| `MatrixClient::create_launcher_space` | method | `crates/mxdx-matrix/src/rooms.rs` |
| `MatrixClient::create_terminal_session_dm` | method | `crates/mxdx-matrix/src/rooms.rs` |
| `MatrixClient::tombstone_room` | method | `crates/mxdx-matrix/src/rooms.rs` |
| `MatrixClient::get_room_state` | method | `crates/mxdx-matrix/src/rooms.rs` |
| `MatrixClientError` | enum | `crates/mxdx-matrix/src/error.rs` |
| `Result` | type | `crates/mxdx-matrix/src/error.rs` |

### mxdx-policy

| Symbol | Kind | File |
|:---|:---|:---|
| `PolicyConfig` | struct | `crates/mxdx-policy/src/config.rs` |
| `PolicyConfig::appservice_url` | method | `crates/mxdx-policy/src/config.rs` |
| `PolicyConfig::user_namespace_regex` | method | `crates/mxdx-policy/src/config.rs` |
| `PolicyEngine` | struct | `crates/mxdx-policy/src/policy.rs` |
| `PolicyEngine::new` | method | `crates/mxdx-policy/src/policy.rs` |
| `PolicyEngine::with_capacity_and_ttl` | method | `crates/mxdx-policy/src/policy.rs` |
| `PolicyEngine::authorize_user` | method | `crates/mxdx-policy/src/policy.rs` |
| `PolicyEngine::revoke_user` | method | `crates/mxdx-policy/src/policy.rs` |
| `PolicyEngine::check_replay` | method | `crates/mxdx-policy/src/policy.rs` |
| `PolicyEngine::mark_seen` | method | `crates/mxdx-policy/src/policy.rs` |
| `PolicyEngine::is_authorized` | method | `crates/mxdx-policy/src/policy.rs` |
| `PolicyEngine::evaluate` | method | `crates/mxdx-policy/src/policy.rs` |
| `PolicyRejection` | enum | `crates/mxdx-policy/src/policy.rs` |
| `AppserviceRegistration` | struct | `crates/mxdx-policy/src/appservice.rs` |
| `Namespaces` | struct | `crates/mxdx-policy/src/appservice.rs` |
| `NamespaceEntry` | struct | `crates/mxdx-policy/src/appservice.rs` |
| `AppserviceRegistration::from_config` | method | `crates/mxdx-policy/src/appservice.rs` |
| `AppserviceRegistration::to_yaml` | method | `crates/mxdx-policy/src/appservice.rs` |
| `register_appservice` | fn | `crates/mxdx-policy/src/appservice.rs` |

### mxdx-secrets

| Symbol | Kind | File |
|:---|:---|:---|
| `SecretCoordinator` | struct | `crates/mxdx-secrets/src/coordinator.rs` |
| `SecretCoordinator::new` | method | `crates/mxdx-secrets/src/coordinator.rs` |
| `SecretCoordinator::handle_secret_request` | method | `crates/mxdx-secrets/src/coordinator.rs` |
| `decrypt_with_identity` | fn | `crates/mxdx-secrets/src/coordinator.rs` |
| `SecretStore` | struct | `crates/mxdx-secrets/src/store.rs` |
| `SecretStore::new` | method | `crates/mxdx-secrets/src/store.rs` |
| `SecretStore::new_with_test_key` | method | `crates/mxdx-secrets/src/store.rs` |
| `SecretStore::add` | method | `crates/mxdx-secrets/src/store.rs` |
| `SecretStore::get` | method | `crates/mxdx-secrets/src/store.rs` |
| `SecretStore::serialize` | method | `crates/mxdx-secrets/src/store.rs` |
| `SecretStore::deserialize` | method | `crates/mxdx-secrets/src/store.rs` |
| `SecretStore::key` | method | `crates/mxdx-secrets/src/store.rs` |

### mxdx-types

| Symbol | Kind | File |
|:---|:---|:---|
| `ResultEvent` | struct | `crates/mxdx-types/src/events/result.rs` |
| `ResultStatus` | enum | `crates/mxdx-types/src/events/result.rs` |
| `LauncherIdentityEvent` | struct | `crates/mxdx-types/src/events/launcher.rs` |
| `CommandEvent` | struct | `crates/mxdx-types/src/events/command.rs` |
| `CommandAction` | enum | `crates/mxdx-types/src/events/command.rs` |
| `SecretRequestEvent` | struct | `crates/mxdx-types/src/events/secret.rs` |
| `SecretResponseEvent` | struct | `crates/mxdx-types/src/events/secret.rs` |
| `TerminalDataEvent` | struct | `crates/mxdx-types/src/events/terminal.rs` |
| `TerminalResizeEvent` | struct | `crates/mxdx-types/src/events/terminal.rs` |
| `TerminalSessionRequestEvent` | struct | `crates/mxdx-types/src/events/terminal.rs` |
| `TerminalSessionResponseEvent` | struct | `crates/mxdx-types/src/events/terminal.rs` |
| `TerminalRetransmitEvent` | struct | `crates/mxdx-types/src/events/terminal.rs` |
| `HostTelemetryEvent` | struct | `crates/mxdx-types/src/events/telemetry.rs` |
| `CpuInfo` | struct | `crates/mxdx-types/src/events/telemetry.rs` |
| `MemoryInfo` | struct | `crates/mxdx-types/src/events/telemetry.rs` |
| `DiskInfo` | struct | `crates/mxdx-types/src/events/telemetry.rs` |
| `NetworkInfo` | struct | `crates/mxdx-types/src/events/telemetry.rs` |
| `OutputEvent` | struct | `crates/mxdx-types/src/events/output.rs` |
| `OutputStream` | enum | `crates/mxdx-types/src/events/output.rs` |

### mxdx-web

_No public symbols._
<!-- END GENERATED SYMBOL TABLES -->



