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

### mxdx-core-wasm

| Symbol | Kind | File |
|:---|:---|:---|
| `init` | fn | `crates/mxdx-core-wasm/src/lib.rs` |
| `sdk_version` | fn | `crates/mxdx-core-wasm/src/lib.rs` |
| `LauncherTopology` | struct | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient` | struct | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::register` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::login` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::is_logged_in` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::user_id` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::sync_once` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::invite_user` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::join_room` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::invited_room_ids` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::export_session` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::restore_session` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::bootstrap_cross_signing` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::bootstrap_cross_signing_if_needed` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::device_id` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::verify_user` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::verify_own_identity` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::is_user_verified` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::create_launcher_space` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::find_launcher_space` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::get_or_create_launcher_space` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::list_launcher_spaces` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::send_event` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::send_state_event` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::read_room_events` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::collect_room_events` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::create_dm_room` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::create_room` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::find_room_events` | method | `crates/mxdx-core-wasm/src/lib.rs` |
| `WasmMatrixClient::on_room_event` | method | `crates/mxdx-core-wasm/src/lib.rs` |

### mxdx-fabric

| Symbol | Kind | File |
|:---|:---|:---|
| `CapabilityIndex` | struct | `crates/mxdx-fabric/src/capability_index.rs` |
| `CapabilityIndex::new` | method | `crates/mxdx-fabric/src/capability_index.rs` |
| `CapabilityIndex::capability_room_name` | method | `crates/mxdx-fabric/src/capability_index.rs` |
| `CapabilityIndex::find_room` | method | `crates/mxdx-fabric/src/capability_index.rs` |
| `CapabilityIndex::get_or_create_room` | method | `crates/mxdx-fabric/src/capability_index.rs` |
| `CapabilityIndex::populate_from_server` | method | `crates/mxdx-fabric/src/capability_index.rs` |
| `ClaimRace` | struct | `crates/mxdx-fabric/src/claim.rs` |
| `ClaimRace::new` | method | `crates/mxdx-fabric/src/claim.rs` |
| `ClaimRace::worker_id` | method | `crates/mxdx-fabric/src/claim.rs` |
| `WatchEntry` | struct | `crates/mxdx-fabric/src/coordinator.rs` |
| `CoordinatorBot` | struct | `crates/mxdx-fabric/src/coordinator.rs` |
| `CoordinatorBot::new` | method | `crates/mxdx-fabric/src/coordinator.rs` |
| `CoordinatorBot::run` | method | `crates/mxdx-fabric/src/coordinator.rs` |
| `CoordinatorBot::handle_task_event` | method | `crates/mxdx-fabric/src/coordinator.rs` |
| `CoordinatorBot::watchlist_len` | method | `crates/mxdx-fabric/src/coordinator.rs` |
| `CoordinatorBot::watchlist_contains` | method | `crates/mxdx-fabric/src/coordinator.rs` |
| `CoordinatorBot::capability_index` | method | `crates/mxdx-fabric/src/coordinator.rs` |
| `CoordinatorBot::handle_claim_event` | method | `crates/mxdx-fabric/src/coordinator.rs` |
| `CoordinatorBot::handle_heartbeat_event` | method | `crates/mxdx-fabric/src/coordinator.rs` |
| `CoordinatorBot::handle_result_event` | method | `crates/mxdx-fabric/src/coordinator.rs` |
| `FailureContext` | struct | `crates/mxdx-fabric/src/failure.rs` |
| `apply_policy` | fn | `crates/mxdx-fabric/src/failure.rs` |
| `OutputFormat` | enum | `crates/mxdx-fabric/src/jcode_worker.rs` |
| `JcodeOptions` | struct | `crates/mxdx-fabric/src/jcode_worker.rs` |
| `JcodeOptions::from_payload` | method | `crates/mxdx-fabric/src/jcode_worker.rs` |
| `JcodeOptions::build_args` | method | `crates/mxdx-fabric/src/jcode_worker.rs` |
| `JcodeWorker` | struct | `crates/mxdx-fabric/src/jcode_worker.rs` |
| `JcodeWorker::new` | method | `crates/mxdx-fabric/src/jcode_worker.rs` |
| `JcodeWorker::worker_client` | method | `crates/mxdx-fabric/src/jcode_worker.rs` |
| `JcodeWorker::run_task` | method | `crates/mxdx-fabric/src/jcode_worker.rs` |
| `SenderClient` | struct | `crates/mxdx-fabric/src/sender.rs` |
| `SenderClient::new` | method | `crates/mxdx-fabric/src/sender.rs` |
| `SenderClient::post_task` | method | `crates/mxdx-fabric/src/sender.rs` |
| `SenderClient::wait_for_result` | method | `crates/mxdx-fabric/src/sender.rs` |
| `SenderClient::submit_and_wait` | method | `crates/mxdx-fabric/src/sender.rs` |
| `SenderClient::sender_id` | method | `crates/mxdx-fabric/src/sender.rs` |
| `SenderClient::connect_stream` | method | `crates/mxdx-fabric/src/sender.rs` |
| `EVENT_TASK` | const | `crates/mxdx-fabric/src/worker.rs` |
| `EVENT_CLAIM` | const | `crates/mxdx-fabric/src/worker.rs` |
| `EVENT_HEARTBEAT` | const | `crates/mxdx-fabric/src/worker.rs` |
| `EVENT_RESULT` | const | `crates/mxdx-fabric/src/worker.rs` |
| `EVENT_CAPABILITY` | const | `crates/mxdx-fabric/src/worker.rs` |
| `WorkerClient` | struct | `crates/mxdx-fabric/src/worker.rs` |
| `WorkerClient::new` | method | `crates/mxdx-fabric/src/worker.rs` |
| `WorkerClient::advertise_capabilities` | method | `crates/mxdx-fabric/src/worker.rs` |
| `WorkerClient::watch_and_claim` | method | `crates/mxdx-fabric/src/worker.rs` |
| `WorkerClient::try_claim` | method | `crates/mxdx-fabric/src/worker.rs` |
| `WorkerClient::post_heartbeat` | method | `crates/mxdx-fabric/src/worker.rs` |
| `WorkerClient::post_result` | method | `crates/mxdx-fabric/src/worker.rs` |
| `WorkerClient::post_state_event` | method | `crates/mxdx-fabric/src/worker.rs` |
| `WorkerClient::worker_id` | method | `crates/mxdx-fabric/src/worker.rs` |
| `WorkerClient::homeserver` | method | `crates/mxdx-fabric/src/worker.rs` |

### mxdx-fabric-cli

_No public symbols._

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
| `ExecutorError` | struct | `crates/mxdx-launcher/src/executor.rs` |
| `ValidatedCommand` | struct | `crates/mxdx-launcher/src/executor.rs` |
| `CommandResult` | struct | `crates/mxdx-launcher/src/executor.rs` |
| `execute_command` | fn | `crates/mxdx-launcher/src/executor.rs` |
| `validate_command` | fn | `crates/mxdx-launcher/src/executor.rs` |
| `FailoverState` | enum | `crates/mxdx-launcher/src/multi_hs.rs` |
| `MultiHsLauncher` | struct | `crates/mxdx-launcher/src/multi_hs.rs` |
| `MultiHsLauncher::start` | method | `crates/mxdx-launcher/src/multi_hs.rs` |
| `MultiHsLauncher::primary` | method | `crates/mxdx-launcher/src/multi_hs.rs` |
| `MultiHsLauncher::connected_count` | method | `crates/mxdx-launcher/src/multi_hs.rs` |
| `MultiHsLauncher::clients` | method | `crates/mxdx-launcher/src/multi_hs.rs` |
| `MultiHsLauncher::state` | method | `crates/mxdx-launcher/src/multi_hs.rs` |
| `MultiHsLauncher::primary_port` | method | `crates/mxdx-launcher/src/multi_hs.rs` |
| `MultiHsLauncher::health_check` | method | `crates/mxdx-launcher/src/multi_hs.rs` |
| `MultiHsLauncher::failover` | method | `crates/mxdx-launcher/src/multi_hs.rs` |
| `collect_telemetry` | fn | `crates/mxdx-launcher/src/telemetry/system.rs` |
| `OutputBatcher` | struct | `crates/mxdx-launcher/src/terminal/batcher.rs` |
| `OutputBatcher::new` | method | `crates/mxdx-launcher/src/terminal/batcher.rs` |
| `OutputBatcher::push` | method | `crates/mxdx-launcher/src/terminal/batcher.rs` |
| `OutputBatcher::tick` | method | `crates/mxdx-launcher/src/terminal/batcher.rs` |
| `OutputBatcher::flush` | method | `crates/mxdx-launcher/src/terminal/batcher.rs` |
| `compress_encode` | fn | `crates/mxdx-launcher/src/terminal/compression.rs` |
| `decode_decompress_bounded` | fn | `crates/mxdx-launcher/src/terminal/compression.rs` |
| `list_tmux_sessions` | fn | `crates/mxdx-launcher/src/terminal/recovery.rs` |
| `RecoveryState` | struct | `crates/mxdx-launcher/src/terminal/recovery.rs` |
| `SessionState` | struct | `crates/mxdx-launcher/src/terminal/recovery.rs` |
| `RecoveryState::load` | method | `crates/mxdx-launcher/src/terminal/recovery.rs` |
| `RecoveryState::save` | method | `crates/mxdx-launcher/src/terminal/recovery.rs` |
| `RecoveryState::add_session` | method | `crates/mxdx-launcher/src/terminal/recovery.rs` |
| `RecoveryState::remove_session` | method | `crates/mxdx-launcher/src/terminal/recovery.rs` |
| `RecoveryState::recoverable_sessions` | method | `crates/mxdx-launcher/src/terminal/recovery.rs` |
| `EventRingBuffer` | struct | `crates/mxdx-launcher/src/terminal/ring_buffer.rs` |
| `EventRingBuffer::new` | method | `crates/mxdx-launcher/src/terminal/ring_buffer.rs` |
| `EventRingBuffer::push` | method | `crates/mxdx-launcher/src/terminal/ring_buffer.rs` |
| `EventRingBuffer::get_range` | method | `crates/mxdx-launcher/src/terminal/ring_buffer.rs` |
| `EventRingBuffer::get` | method | `crates/mxdx-launcher/src/terminal/ring_buffer.rs` |
| `EventRingBuffer::len` | method | `crates/mxdx-launcher/src/terminal/ring_buffer.rs` |
| `EventRingBuffer::is_empty` | method | `crates/mxdx-launcher/src/terminal/ring_buffer.rs` |
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

### mxdx-matrix

| Symbol | Kind | File |
|:---|:---|:---|
| `MatrixClient` | struct | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::login_and_connect` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::connect_with_token` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::register_and_connect` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::is_logged_in` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::crypto_enabled` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::user_id` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::create_encrypted_room` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::create_dm` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::join_room` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::invite_user` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::send_event` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::send_state_event` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::sync_once` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::sync_and_collect_events` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::wait_for_key_exchange` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::set_room_creation_delay` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::room_creation_delay` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::set_room_creation_timeout` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClient::inner` | method | `crates/mxdx-matrix/src/client.rs` |
| `MatrixClientError` | enum | `crates/mxdx-matrix/src/error.rs` |
| `Result` | type | `crates/mxdx-matrix/src/error.rs` |
| `LauncherTopology` | struct | `crates/mxdx-matrix/src/rooms.rs` |
| `MatrixClient::create_launcher_space` | method | `crates/mxdx-matrix/src/rooms.rs` |
| `MatrixClient::find_launcher_space` | method | `crates/mxdx-matrix/src/rooms.rs` |
| `MatrixClient::get_or_create_launcher_space` | method | `crates/mxdx-matrix/src/rooms.rs` |
| `MatrixClient::create_terminal_session_dm` | method | `crates/mxdx-matrix/src/rooms.rs` |
| `MatrixClient::tombstone_room` | method | `crates/mxdx-matrix/src/rooms.rs` |
| `MatrixClient::get_room_state_event` | method | `crates/mxdx-matrix/src/rooms.rs` |
| `MatrixClient::get_room_state` | method | `crates/mxdx-matrix/src/rooms.rs` |
| `MatrixClient::create_named_unencrypted_room` | method | `crates/mxdx-matrix/src/rooms.rs` |

### mxdx-policy

| Symbol | Kind | File |
|:---|:---|:---|
| `AppserviceRegistrar` | trait | `crates/mxdx-policy/src/appservice.rs` |
| `TuwunelRegistrar` | struct | `crates/mxdx-policy/src/appservice.rs` |
| `ManualRegistrar` | struct | `crates/mxdx-policy/src/appservice.rs` |
| `AppserviceRegistration` | struct | `crates/mxdx-policy/src/appservice.rs` |
| `Namespaces` | struct | `crates/mxdx-policy/src/appservice.rs` |
| `NamespaceEntry` | struct | `crates/mxdx-policy/src/appservice.rs` |
| `AppserviceRegistration::from_config` | method | `crates/mxdx-policy/src/appservice.rs` |
| `AppserviceRegistration::to_yaml` | method | `crates/mxdx-policy/src/appservice.rs` |
| `register_appservice` | fn | `crates/mxdx-policy/src/appservice.rs` |
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
| `CommandEvent` | struct | `crates/mxdx-types/src/events/command.rs` |
| `CommandAction` | enum | `crates/mxdx-types/src/events/command.rs` |
| `TaskEvent` | struct | `crates/mxdx-types/src/events/fabric.rs` |
| `CapabilityEvent` | struct | `crates/mxdx-types/src/events/fabric.rs` |
| `ClaimEvent` | struct | `crates/mxdx-types/src/events/fabric.rs` |
| `HeartbeatEvent` | struct | `crates/mxdx-types/src/events/fabric.rs` |
| `TaskResultEvent` | struct | `crates/mxdx-types/src/events/fabric.rs` |
| `FailurePolicy` | enum | `crates/mxdx-types/src/events/fabric.rs` |
| `RoutingMode` | enum | `crates/mxdx-types/src/events/fabric.rs` |
| `TaskStatus` | enum | `crates/mxdx-types/src/events/fabric.rs` |
| `LauncherIdentityEvent` | struct | `crates/mxdx-types/src/events/launcher.rs` |
| `OutputEvent` | struct | `crates/mxdx-types/src/events/output.rs` |
| `OutputStream` | enum | `crates/mxdx-types/src/events/output.rs` |
| `ResultEvent` | struct | `crates/mxdx-types/src/events/result.rs` |
| `ResultStatus` | enum | `crates/mxdx-types/src/events/result.rs` |
| `SecretRequestEvent` | struct | `crates/mxdx-types/src/events/secret.rs` |
| `SecretResponseEvent` | struct | `crates/mxdx-types/src/events/secret.rs` |
| `HostTelemetryEvent` | struct | `crates/mxdx-types/src/events/telemetry.rs` |
| `CpuInfo` | struct | `crates/mxdx-types/src/events/telemetry.rs` |
| `MemoryInfo` | struct | `crates/mxdx-types/src/events/telemetry.rs` |
| `DiskInfo` | struct | `crates/mxdx-types/src/events/telemetry.rs` |
| `NetworkInfo` | struct | `crates/mxdx-types/src/events/telemetry.rs` |
| `TerminalDataEvent` | struct | `crates/mxdx-types/src/events/terminal.rs` |
| `TerminalResizeEvent` | struct | `crates/mxdx-types/src/events/terminal.rs` |
| `TerminalSessionRequestEvent` | struct | `crates/mxdx-types/src/events/terminal.rs` |
| `TerminalSessionResponseEvent` | struct | `crates/mxdx-types/src/events/terminal.rs` |
| `TerminalRetransmitEvent` | struct | `crates/mxdx-types/src/events/terminal.rs` |

### mxdx-web

| Symbol | Kind | File |
|:---|:---|:---|
| `routes` | fn | `crates/mxdx-web/src/routes/dashboard.rs` |
| `build_router` | fn | `crates/mxdx-web/src/routes/mod.rs` |
| `routes` | fn | `crates/mxdx-web/src/routes/sse.rs` |
| `routes` | fn | `crates/mxdx-web/src/routes/static_files.rs` |
| `LauncherInfo` | struct | `crates/mxdx-web/src/state.rs` |
| `LauncherStatus` | enum | `crates/mxdx-web/src/state.rs` |
| `AppState` | struct | `crates/mxdx-web/src/state.rs` |
| `AppState::new` | method | `crates/mxdx-web/src/state.rs` |
| `AppState::update_launcher` | method | `crates/mxdx-web/src/state.rs` |
<!-- END GENERATED SYMBOL TABLES -->



