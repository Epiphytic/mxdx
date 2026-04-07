use anyhow::Result;
use clap::Parser;
use mxdx_client::cli::{Cli, Commands, DaemonAction, TrustAction};
use mxdx_client::config::{ClientArgs, ClientRuntimeConfig};
use mxdx_client::liveness;
use mxdx_client::matrix::{self, ClientRoomOps, IncomingClientEvent};
use mxdx_types::events::session::{
    SessionOutput, SessionResult, SESSION_CANCEL, SESSION_SIGNAL, SESSION_TASK,
};
use mxdx_types::events::telemetry::WORKER_TELEMETRY;
use std::io::Write;

/// Resolve the worker room name from CLI arg or config.
/// When a direct room_id is provided, the worker room name is optional (used as a label only).
fn resolve_worker_room(
    cli_room: &Option<String>,
    config: &ClientRuntimeConfig,
    has_room_id: bool,
) -> Result<String> {
    cli_room
        .clone()
        .or_else(|| config.client.default_worker_room.clone())
        .or_else(|| if has_room_id { Some("direct".to_string()) } else { None })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No worker room specified. Use --worker-room or set default_worker_room in client.toml"
            )
        })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match &cli.command {
        // Internal daemon mode
        Commands::InternalDaemon { profile, detach: _ } => {
            let config = ClientRuntimeConfig::load()?;
            mxdx_client::daemon::run_daemon(config, profile).await
        }
        // Daemon management commands
        Commands::Daemon { action } => {
            match action {
                DaemonAction::Status => {
                    let stream = mxdx_client::cli::connect::connect_or_spawn(&cli.profile).await?;
                    let (reader, mut writer) = stream.into_split();
                    let mut reader = tokio::io::BufReader::new(reader);
                    let result = mxdx_client::cli::connect::send_request(
                        &mut reader, &mut writer, "daemon.status", None, 1,
                    ).await?;
                    let status: mxdx_client::protocol::methods::DaemonStatusResult =
                        serde_json::from_value(result)?;
                    print!("{}", mxdx_client::cli::format::format_status(&status));
                    Ok(())
                }
                DaemonAction::Stop { all: _ } => {
                    eprintln!("Stop not yet fully implemented");
                    let stream = mxdx_client::cli::connect::connect_or_spawn(&cli.profile).await?;
                    let (reader, mut writer) = stream.into_split();
                    let mut reader = tokio::io::BufReader::new(reader);
                    mxdx_client::cli::connect::send_request(
                        &mut reader, &mut writer, "daemon.shutdown", None, 1,
                    ).await?;
                    eprintln!("Shutdown signal sent");
                    Ok(())
                }
                DaemonAction::Start { detach: _, enable_websocket: _, ws_port: _ } => {
                    let config = ClientRuntimeConfig::load()?;
                    mxdx_client::daemon::run_daemon(config, &cli.profile).await
                }
                DaemonAction::Mcp => {
                    let handler = std::sync::Arc::new(
                        mxdx_client::daemon::handler::Handler::new(&cli.profile)
                    );
                    mxdx_client::daemon::transport::mcp::serve_stdio(handler).await
                }
            }
        }
        Commands::Diagnose { pretty, decrypt } => {
            let input = mxdx_client::diagnose::DiagnoseInput {
                profile: cli.profile.clone(),
                homeserver: cli.homeserver.clone(),
                username: cli.username.clone(),
                password: cli.password.clone(),
                pretty: *pretty,
                decrypt: *decrypt,
            };
            mxdx_client::diagnose::run_diagnose(
                mxdx_client::diagnose::DiagnoseBinary::Client,
                input,
            )
            .await
        }
        // All other commands: daemon mode (default) or direct mode (--no-daemon)
        _ => {
            if cli.no_daemon {
                run_direct(&cli).await
            } else {
                run_via_daemon(&cli).await
            }
        }
    }
}

/// Forward command through the daemon via Unix socket IPC.
async fn run_via_daemon(cli: &Cli) -> Result<()> {
    use mxdx_client::cli::connect::{connect_or_spawn, send_request};
    use mxdx_client::protocol::methods::SessionRunResult;

    let stream = connect_or_spawn(&cli.profile).await?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(reader);

    match &cli.command {
        Commands::Run { command, args, detach, interactive, no_room_output, timeout, cwd, worker_room, skip_liveness_check: _ }
        | Commands::Exec { command, args, detach, interactive, no_room_output, timeout, cwd, worker_room, skip_liveness_check: _ } => {
            let params = serde_json::json!({
                "bin": command,
                "args": args,
                "detach": detach,
                "interactive": interactive,
                "no_room_output": no_room_output,
                "timeout_seconds": timeout,
                "cwd": cwd,
                "worker_room": worker_room,
            });
            let result = send_request(&mut reader, &mut writer, "session.run", Some(params), 1).await?;
            let run_result: SessionRunResult = serde_json::from_value(result)?;

            if *detach {
                println!("{}", run_result.uuid);
                return Ok(());
            }

            // Non-detach: tail session output via daemon notifications.
            // The daemon spawns a background sync loop that sends
            // session.output and session.result notifications.
            eprintln!("Session {} submitted, waiting for output...", run_result.uuid);
            let mut exit_code: Option<i32> = None;

            loop {
                use tokio::io::AsyncBufReadExt;
                let mut line = String::new();
                let n = reader.read_line(&mut line).await?;
                if n == 0 {
                    break; // daemon disconnected
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let value: serde_json::Value = match serde_json::from_str(trimmed) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                // Notifications have "method" but no "id"
                if let Some(method) = value.get("method").and_then(|m| m.as_str()) {
                    match method {
                        "session.output" => {
                            if let Some(data) = value.pointer("/params/data").and_then(|d| d.as_str()) {
                                print!("{}", data);
                                std::io::stdout().flush().ok();
                            }
                        }
                        "session.result" => {
                            exit_code = value.pointer("/params/exit_code")
                                .and_then(|c| c.as_i64())
                                .map(|c| c as i32);
                            if let Some(status) = value.pointer("/params/status").and_then(|s| s.as_str()) {
                                eprintln!("{}", status);
                            }
                            if let Some(tail) = value.pointer("/params/tail").and_then(|t| t.as_str()) {
                                if !tail.is_empty() {
                                    eprintln!("  reason: {}", tail);
                                }
                            }
                            break;
                        }
                        _ => {}
                    }
                }
                // Response objects (with "id") are already handled by send_request
            }

            std::process::exit(exit_code.unwrap_or(1));
        }

        Commands::Cancel { uuid, signal, worker_room } => {
            let params = serde_json::json!({
                "uuid": uuid,
                "signal": signal,
                "worker_room": worker_room,
            });
            send_request(&mut reader, &mut writer, "session.cancel", Some(params), 1).await?;
            eprintln!("Cancel sent for session {}", uuid);
        }

        Commands::Ls { all, worker_room } => {
            let params = serde_json::json!({
                "all": all,
                "worker_room": worker_room,
            });
            let result = send_request(&mut reader, &mut writer, "session.ls", Some(params), 1).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }

        Commands::Logs { uuid, follow, worker_room } => {
            let params = serde_json::json!({
                "uuid": uuid,
                "follow": follow,
                "worker_room": worker_room,
            });
            let result = send_request(&mut reader, &mut writer, "session.logs", Some(params), 1).await?;
            if let Some(lines) = result.get("lines").and_then(|l| l.as_array()) {
                for line in lines {
                    if let Some(s) = line.as_str() {
                        println!("{}", s);
                    }
                }
            }
        }

        // Commands not yet forwarded via daemon -- fall back to direct
        _ => {
            return run_direct(cli).await;
        }
    }
    Ok(())
}

/// Run command directly (no daemon). This is the existing behavior preserved verbatim.
async fn run_direct(cli: &Cli) -> Result<()> {
    let cli_room_id = cli.room_id.clone();
    let cli_force_new_device = cli.force_new_device;

    match &cli.command {
        Commands::Run {
            command,
            args,
            detach,
            interactive,
            no_room_output,
            timeout,
            cwd,
            worker_room,
            skip_liveness_check,
        }
        | Commands::Exec {
            command,
            args,
            detach,
            interactive,
            no_room_output,
            timeout,
            cwd,
            worker_room,
            skip_liveness_check,
        } => {
            let client_args = ClientArgs {
                worker_room: worker_room.clone(),
                coordinator_room: None,
                timeout: *timeout,
                heartbeat_interval: None,
                interactive: *interactive,
                no_room_output: *no_room_output,
                homeserver: cli.homeserver.clone(),
                username: cli.username.clone(),
                password: cli.password.clone(),
                force_new_device: cli_force_new_device,
            };
            let config = ClientRuntimeConfig::load()?.with_cli_overrides(&client_args);
            let accounts = config.resolve_accounts();
            if accounts.is_empty() {
                anyhow::bail!(
                    "No Matrix accounts configured (use --homeserver/--username/--password or config file)"
                );
            }
            let room_name = resolve_worker_room(worker_room, &config, cli_room_id.is_some())?;

            // Connect to Matrix with multi-homeserver failover
            let mut mx_room = matrix::connect_multi(
                &accounts,
                &room_name,
                cli_room_id.as_deref(),
                config.force_new_device,
            )
            .await?;

            // Check worker liveness and capability before submitting a task
            if !skip_liveness_check {
                // Sync once to populate room state before reading state events
                mx_room.client().sync_once().await?;

                let telemetry_events = mx_room
                    .read_state_events(
                        mx_room.room_id().as_str(),
                        WORKER_TELEMETRY,
                    )
                    .await
                    .unwrap_or_default();

                // If no telemetry events at all, treat as no worker
                if telemetry_events.is_empty()
                    || telemetry_events.iter().all(|(_, v)| {
                        v.is_null() || v.as_object().map_or(true, |o| o.is_empty())
                    })
                {
                    eprintln!(
                        "Error: No worker room found for '{}'. Has the worker started and invited this client?",
                        room_name
                    );
                    std::process::exit(10);
                }

                // Check if any worker is online
                let summary = liveness::summarize_worker_liveness(&telemetry_events);
                if summary.online == 0 {
                    if let Some(stale_dur) = summary.stale_details {
                        eprintln!(
                            "Error: No live worker in room '{}' (last seen {}s ago)",
                            room_name,
                            stale_dur.as_secs()
                        );
                    } else if summary.offline > 0 {
                        eprintln!(
                            "Error: No live worker in room '{}' (Worker is offline)",
                            room_name
                        );
                    } else {
                        eprintln!(
                            "Error: No live worker in room '{}' (No telemetry found)",
                            room_name
                        );
                    }
                    std::process::exit(11);
                }

                // Check if any online worker supports the requested command
                let cmd = command;
                if liveness::find_capable_worker(&telemetry_events, cmd).is_none() {
                    eprintln!("Error: No worker supports command '{}'", cmd);
                    std::process::exit(12);
                }

                tracing::info!(
                    online_workers = summary.online,
                    "worker liveness and capability verified, proceeding with task submission"
                );
            }

            // Build task with actual user ID
            let sender_id = mx_room.user_id_string();
            let task = mxdx_client::submit::build_task(
                command,
                args,
                *interactive,
                *no_room_output,
                timeout.or(config.client.session.timeout_seconds),
                config.client.session.heartbeat_interval,
                &sender_id,
                cwd.as_deref(),
            );

            let task_uuid = task.uuid.clone();
            let task_content = matrix::serialize_event(&task)?;

            // Submit the task event to the exec room
            let event_id = mx_room
                .post_event_mut(SESSION_TASK, task_content)
                .await?;
            tracing::info!(uuid = %task_uuid, event_id = %event_id, "task submitted");

            if *detach {
                println!("{}", task_uuid);
            } else {
                // Tail the session: sync for output and result events
                eprintln!("Session {} submitted, waiting for output...", task_uuid);
                let mut exit_code: Option<i32> = None;
                let mut seen_ids = std::collections::HashSet::new();

                loop {
                    let events = mx_room.sync_events_mut().await?;
                    for event in events {
                        match event {
                            IncomingClientEvent::SessionOutput {
                                event_id,
                                session_uuid,
                                content,
                            } => {
                                if session_uuid != task_uuid
                                    || (!event_id.is_empty() && !seen_ids.insert(event_id.clone()))
                                {
                                    continue;
                                }
                                if let Ok(output) =
                                    matrix::deserialize_event::<SessionOutput>(&content)
                                {
                                    if let Ok(text) = mxdx_client::tail::format_output(&output) {
                                        print!("{}", text);
                                        std::io::stdout().flush().ok();
                                    }
                                }
                            }
                            IncomingClientEvent::SessionResult {
                                event_id,
                                session_uuid,
                                content,
                            } => {
                                if session_uuid != task_uuid
                                    || (!event_id.is_empty() && !seen_ids.insert(event_id.clone()))
                                {
                                    continue;
                                }
                                if let Ok(result) =
                                    matrix::deserialize_event::<SessionResult>(&content)
                                {
                                    eprintln!("{}", mxdx_client::tail::format_result(&result));
                                    exit_code = result.exit_code;
                                }
                                break;
                            }
                            _ => {}
                        }
                    }
                    if exit_code.is_some() {
                        break;
                    }
                }

                std::process::exit(exit_code.unwrap_or(1));
            }
        }
        Commands::Attach { uuid, interactive } => {
            tracing::info!(uuid = %uuid, interactive, "attaching to session");
            let client_args = ClientArgs {
                worker_room: None,
                coordinator_room: None,
                timeout: None,
                heartbeat_interval: None,
                interactive: *interactive,
                no_room_output: false,
                homeserver: cli.homeserver.clone(),
                username: cli.username.clone(),
                password: cli.password.clone(),
                force_new_device: cli_force_new_device,
            };
            let config = ClientRuntimeConfig::load()?.with_cli_overrides(&client_args);
            let accounts = config.resolve_accounts();
            if accounts.is_empty() {
                anyhow::bail!(
                    "No Matrix accounts configured (use --homeserver/--username/--password or config file)"
                );
            }
            let room_name = config
                .client
                .default_worker_room
                .clone()
                .or_else(|| if cli_room_id.is_some() { Some("direct".to_string()) } else { None })
                .unwrap_or_else(|| "default".to_string());

            let mut mx_room = matrix::connect_multi(
                &accounts,
                &room_name,
                cli_room_id.as_deref(),
                config.force_new_device,
            )
            .await?;

            // Sync to get room events and find SessionStart for this UUID
            let events = mx_room.sync_events_mut().await?;
            let dm_room_id = events.iter().find_map(|e| {
                if let IncomingClientEvent::SessionStart {
                    session_uuid,
                    content,
                    ..
                } = e
                {
                    if session_uuid == uuid {
                        content
                            .get("dm_room_id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            });

            if let Some(ref dm_id) = dm_room_id {
                eprintln!("Attaching to session {} in DM room {}...", uuid, dm_id);
                eprintln!("Interactive terminal attach not yet fully implemented.");
                eprintln!("Press Ctrl-] to detach.");
                // TODO: Enter terminal raw mode, pipe stdin/stdout via DM room
                // - Read stdin -> send as SESSION_INPUT to DM room
                // - Receive SESSION_OUTPUT from DM room -> write to stdout
                // - Handle SIGWINCH -> send SESSION_RESIZE
                // - Ctrl-] to detach
            } else {
                eprintln!(
                    "No interactive DM room found for session {}. Falling back to thread tail mode.",
                    uuid
                );
                // TODO: Fall back to thread tailing (same as logs --follow)
            }
        }
        Commands::Ls { all, worker_room } => {
            let client_args = ClientArgs {
                worker_room: worker_room.clone(),
                coordinator_room: None,
                timeout: None,
                heartbeat_interval: None,
                interactive: false,
                no_room_output: false,
                homeserver: cli.homeserver.clone(),
                username: cli.username.clone(),
                password: cli.password.clone(),
                force_new_device: cli_force_new_device,
            };
            let config = ClientRuntimeConfig::load()?.with_cli_overrides(&client_args);
            let accounts = config.resolve_accounts();
            if accounts.is_empty() {
                anyhow::bail!(
                    "No Matrix accounts configured (use --homeserver/--username/--password or config file)"
                );
            }
            let room_name = resolve_worker_room(worker_room, &config, cli_room_id.is_some())?;

            let mx_room = matrix::connect_multi(
                &accounts,
                &room_name,
                cli_room_id.as_deref(),
                config.force_new_device,
            )
            .await?;

            // Sync once to populate room state
            mx_room.client().sync_once().await?;

            // Read active session state events
            let active_state = mx_room
                .read_state_events(
                    mx_room.room_id().as_str(),
                    "org.mxdx.session.active",
                )
                .await;

            let completed_state = if *all {
                mx_room
                    .read_state_events(
                        mx_room.room_id().as_str(),
                        "org.mxdx.session.completed",
                    )
                    .await
                    .ok()
            } else {
                None
            };

            let mut entries = Vec::new();

            if let Ok(active_events) = active_state {
                for (_key, value) in &active_events {
                    if let Ok(state) =
                        matrix::deserialize_event::<mxdx_types::events::session::ActiveSessionState>(
                            value,
                        )
                    {
                        // Use a UUID derived from content if available
                        let uuid = value
                            .get("session_uuid")
                            .and_then(|u| u.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        entries.push(mxdx_client::ls::from_active(uuid, &state));
                    }
                }
            }

            if let Some(Ok(completed_events)) = completed_state.map(|v| Ok::<_, anyhow::Error>(v)) {
                for (_key, _value) in &completed_events {
                    // Completed state events require both active + completed state;
                    // for now we only show what we can parse
                    tracing::debug!("completed state event found (display pending)");
                }
            }

            println!("{}", mxdx_client::ls::format_table(&entries));
        }
        Commands::Logs {
            uuid,
            follow,
            worker_room,
        } => {
            let client_args = ClientArgs {
                worker_room: worker_room.clone(),
                coordinator_room: None,
                timeout: None,
                heartbeat_interval: None,
                interactive: false,
                no_room_output: false,
                homeserver: cli.homeserver.clone(),
                username: cli.username.clone(),
                password: cli.password.clone(),
                force_new_device: cli_force_new_device,
            };
            let config = ClientRuntimeConfig::load()?.with_cli_overrides(&client_args);
            let accounts = config.resolve_accounts();
            if accounts.is_empty() {
                anyhow::bail!(
                    "No Matrix accounts configured (use --homeserver/--username/--password or config file)"
                );
            }
            let room_name = resolve_worker_room(worker_room, &config, cli_room_id.is_some())?;

            let mut mx_room = matrix::connect_multi(
                &accounts,
                &room_name,
                cli_room_id.as_deref(),
                config.force_new_device,
            )
            .await?;

            // Collect events and filter for the session's output
            let events = mx_room.sync_events_mut().await?;
            let mut outputs = Vec::new();
            for event in events {
                if let IncomingClientEvent::SessionOutput {
                    session_uuid,
                    content,
                    ..
                } = event
                {
                    if session_uuid == *uuid {
                        if let Ok(output) =
                            matrix::deserialize_event::<SessionOutput>(&content)
                        {
                            outputs.push(output);
                        }
                    }
                }
            }

            let assembled = mxdx_client::logs::reassemble_output_string(outputs)?;
            print!("{}", assembled);
            std::io::stdout().flush().ok();

            if *follow {
                eprintln!("(follow mode: watching for new output...)");
                loop {
                    let events = mx_room.sync_events_mut().await?;
                    for event in events {
                        match event {
                            IncomingClientEvent::SessionOutput {
                                session_uuid,
                                content,
                                ..
                            } => {
                                if session_uuid != *uuid {
                                    continue;
                                }
                                if let Ok(output) =
                                    matrix::deserialize_event::<SessionOutput>(&content)
                                {
                                    if let Ok(text) = mxdx_client::tail::format_output(&output) {
                                        print!("{}", text);
                                        std::io::stdout().flush().ok();
                                    }
                                }
                            }
                            IncomingClientEvent::SessionResult {
                                session_uuid, ..
                            } => {
                                if session_uuid == *uuid {
                                    eprintln!("Session completed.");
                                    return Ok(());
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Commands::Cancel {
            uuid,
            signal,
            worker_room,
        } => {
            let client_args = ClientArgs {
                worker_room: worker_room.clone(),
                coordinator_room: None,
                timeout: None,
                heartbeat_interval: None,
                interactive: false,
                no_room_output: false,
                homeserver: cli.homeserver.clone(),
                username: cli.username.clone(),
                password: cli.password.clone(),
                force_new_device: cli_force_new_device,
            };
            let config = ClientRuntimeConfig::load()?.with_cli_overrides(&client_args);
            let accounts = config.resolve_accounts();
            if accounts.is_empty() {
                anyhow::bail!(
                    "No Matrix accounts configured (use --homeserver/--username/--password or config file)"
                );
            }
            let room_name = resolve_worker_room(worker_room, &config, cli_room_id.is_some())?;

            let mut mx_room = matrix::connect_multi(
                &accounts,
                &room_name,
                cli_room_id.as_deref(),
                config.force_new_device,
            )
            .await?;

            if let Some(sig) = signal {
                let event = mxdx_client::cancel::build_signal(uuid, sig);
                let content = matrix::serialize_event(&event)?;
                let event_id = mx_room
                    .post_event_mut(SESSION_SIGNAL, content)
                    .await?;
                tracing::info!(uuid = %uuid, signal = %sig, event_id = %event_id, "signal sent");
                eprintln!("Signal {} sent to session {}", sig, uuid);
            } else {
                let event = mxdx_client::cancel::build_cancel(uuid, None, None);
                let content = matrix::serialize_event(&event)?;
                let event_id = mx_room
                    .post_event_mut(SESSION_CANCEL, content)
                    .await?;
                tracing::info!(uuid = %uuid, event_id = %event_id, "cancel sent");
                eprintln!("Cancel sent for session {}", uuid);
            }
        }
        Commands::Cleanup {
            targets,
            force,
            delete_all_sessions,
        } => {
            let client_args = ClientArgs {
                worker_room: None,
                coordinator_room: None,
                timeout: None,
                heartbeat_interval: None,
                interactive: false,
                no_room_output: false,
                homeserver: cli.homeserver.clone(),
                username: cli.username.clone(),
                password: cli.password.clone(),
                force_new_device: cli_force_new_device,
            };
            let config = ClientRuntimeConfig::load()?.with_cli_overrides(&client_args);
            let creds = config.require_credentials()?;

            // Login directly via REST API for cleanup (no need for full Matrix SDK connection)
            let homeserver = &creds.homeserver;
            let base = homeserver.trim_end_matches('/');
            let login_resp = reqwest::Client::new()
                .post(format!("{}/_matrix/client/v3/login", base))
                .json(&serde_json::json!({
                    "type": "m.login.password",
                    "identifier": { "type": "m.id.user", "user": &creds.username },
                    "password": &creds.password,
                }))
                .send()
                .await?;

            if !login_resp.status().is_success() {
                anyhow::bail!("Login failed: {}", login_resp.status());
            }
            let login_data: serde_json::Value = login_resp.json().await?;
            let access_token = login_data["access_token"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("No access token in login response"))?;
            let device_id = login_data["device_id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("No device_id in login response"))?;
            let user_id = login_data["user_id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("No user_id in login response"))?;

            mxdx_client::cleanup::run_cleanup(
                homeserver,
                access_token,
                device_id,
                user_id,
                &creds.password,
                targets,
                *force,
                *delete_all_sessions,
            )
            .await?;
        }
        Commands::Trust { action } => match action {
            TrustAction::List => tracing::info!("listing trusted devices"),
            TrustAction::Add { device } => tracing::info!(device = %device, "adding trust"),
            TrustAction::Remove { device } => tracing::info!(device = %device, "removing trust"),
            TrustAction::Pull { from } => tracing::info!(from = %from, "pulling trust list"),
            TrustAction::Anchor { user_id } => {
                if let Some(id) = user_id {
                    tracing::info!(anchor = %id, "setting trust anchor");
                } else {
                    tracing::info!("showing trust anchor");
                }
            }
        },
        // These are handled above in main(), not in run_direct
        Commands::Daemon { .. } | Commands::InternalDaemon { .. } | Commands::Diagnose { .. } => unreachable!(),
    }
    Ok(())
}
