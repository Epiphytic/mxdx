use anyhow::Result;
use clap::{Parser, Subcommand};
use mxdx_client::config::{ClientArgs, ClientRuntimeConfig};
use mxdx_client::liveness::{check_worker_liveness, LivenessStatus};
use mxdx_client::matrix::{self, ClientRoomOps, IncomingClientEvent};
use mxdx_types::events::session::{
    SessionOutput, SessionResult, SESSION_CANCEL, SESSION_SIGNAL, SESSION_TASK,
};
use mxdx_types::events::telemetry::WORKER_TELEMETRY;
use std::io::Write;

#[derive(Parser)]
#[command(name = "mxdx-client", about = "mxdx client CLI")]
struct Cli {
    /// Matrix homeserver URL or server name
    #[arg(long, env = "MXDX_HOMESERVER", global = true)]
    homeserver: Option<String>,
    /// Matrix username
    #[arg(long, env = "MXDX_USERNAME", global = true)]
    username: Option<String>,
    /// Matrix password
    #[arg(long, env = "MXDX_PASSWORD", global = true)]
    password: Option<String>,
    /// Direct room ID (bypasses space discovery)
    #[arg(long, env = "MXDX_ROOM_ID", global = true)]
    room_id: Option<String>,
    /// Force a fresh device login, skipping session restore
    #[arg(long, global = true, default_value_t = false)]
    force_new_device: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Submit and run a command on a worker
    Run {
        /// Command to execute
        command: String,
        /// Command arguments
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
        /// Detached mode — print UUID and exit
        #[arg(short = 'd', long)]
        detach: bool,
        /// Interactive mode
        #[arg(short = 'i', long)]
        interactive: bool,
        /// Suppress room output
        #[arg(long)]
        no_room_output: bool,
        /// Timeout in seconds
        #[arg(long)]
        timeout: Option<u64>,
        /// Worker room name
        #[arg(long)]
        worker_room: Option<String>,
        /// Skip the worker liveness check before task submission
        #[arg(long)]
        skip_liveness_check: bool,
    },
    /// Alias for run (backward compat)
    Exec {
        /// Command to execute
        command: String,
        /// Command arguments
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
        /// Detached mode — print UUID and exit
        #[arg(short = 'd', long)]
        detach: bool,
        /// Interactive mode
        #[arg(short = 'i', long)]
        interactive: bool,
        /// Suppress room output
        #[arg(long)]
        no_room_output: bool,
        /// Timeout in seconds
        #[arg(long)]
        timeout: Option<u64>,
        /// Worker room name
        #[arg(long)]
        worker_room: Option<String>,
        /// Skip the worker liveness check before task submission
        #[arg(long)]
        skip_liveness_check: bool,
    },
    /// Attach to an active session
    Attach {
        /// Session UUID
        uuid: String,
        /// Force interactive mode
        #[arg(short = 'i', long)]
        interactive: bool,
    },
    /// List sessions
    Ls {
        /// Include completed sessions
        #[arg(long)]
        all: bool,
        /// Worker room name
        #[arg(long)]
        worker_room: Option<String>,
    },
    /// View session logs
    Logs {
        /// Session UUID
        uuid: String,
        /// Follow output in real-time
        #[arg(short = 'f', long)]
        follow: bool,
        /// Worker room name
        #[arg(long)]
        worker_room: Option<String>,
    },
    /// Cancel a session
    Cancel {
        /// Session UUID
        uuid: String,
        /// Send specific signal
        #[arg(long)]
        signal: Option<String>,
        /// Worker room name
        #[arg(long)]
        worker_room: Option<String>,
    },
    /// Trust management
    Trust {
        #[command(subcommand)]
        action: TrustAction,
    },
}

#[derive(Subcommand)]
enum TrustAction {
    /// List trusted devices
    List,
    /// Add a trusted device
    Add {
        #[arg(long)]
        device: String,
    },
    /// Remove a trusted device
    Remove {
        #[arg(long)]
        device: String,
    },
    /// Pull trust list from device
    Pull {
        #[arg(long)]
        from: String,
    },
    /// Show or set trust anchor
    Anchor { user_id: Option<String> },
}

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
    let cli_room_id = cli.room_id.clone();
    let cli_force_new_device = cli.force_new_device;

    match cli.command {
        Commands::Run {
            command,
            args,
            detach,
            interactive,
            no_room_output,
            timeout,
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
            worker_room,
            skip_liveness_check,
        } => {
            let client_args = ClientArgs {
                worker_room: worker_room.clone(),
                coordinator_room: None,
                timeout,
                heartbeat_interval: None,
                interactive,
                no_room_output,
                homeserver: cli.homeserver,
                username: cli.username,
                password: cli.password,
                force_new_device: cli_force_new_device,
            };
            let config = ClientRuntimeConfig::load()?.with_cli_overrides(&client_args);
            let accounts = config.resolve_accounts();
            if accounts.is_empty() {
                anyhow::bail!(
                    "No Matrix accounts configured (use --homeserver/--username/--password or config file)"
                );
            }
            let room_name = resolve_worker_room(&worker_room, &config, cli_room_id.is_some())?;

            // Connect to Matrix with multi-homeserver failover
            let mut mx_room = matrix::connect_multi(
                &accounts,
                &room_name,
                cli_room_id.as_deref(),
                config.force_new_device,
            )
            .await?;

            // Check worker liveness before submitting a task
            if !skip_liveness_check {
                // Sync once to populate room state before reading state events
                mx_room.client().sync_once().await?;

                let telemetry_state = mx_room
                    .read_state_events(
                        mx_room.room_id().as_str(),
                        WORKER_TELEMETRY,
                    )
                    .await;

                let telemetry_json = telemetry_state
                    .ok()
                    .and_then(|events| {
                        events.into_iter().find_map(|(_key, value)| {
                            if value.is_null()
                                || value.as_object().map_or(true, |o| o.is_empty())
                            {
                                None
                            } else {
                                Some(value)
                            }
                        })
                    })
                    .unwrap_or(serde_json::Value::Null);

                match check_worker_liveness(&telemetry_json) {
                    LivenessStatus::Online { capabilities } => {
                        tracing::info!(
                            capabilities = ?capabilities,
                            "worker is online, proceeding with task submission"
                        );
                    }
                    LivenessStatus::NoWorker => {
                        eprintln!("Error: no worker found in room. Is a worker running?");
                        std::process::exit(1);
                    }
                    LivenessStatus::Offline => {
                        eprintln!("Error: worker is offline.");
                        std::process::exit(1);
                    }
                    LivenessStatus::Stale(duration) => {
                        eprintln!(
                            "Error: worker last seen {}s ago (stale).",
                            duration.as_secs()
                        );
                        std::process::exit(1);
                    }
                }
            }

            // Build task with actual user ID
            let sender_id = mx_room.user_id_string();
            let task = mxdx_client::submit::build_task(
                &command,
                &args,
                interactive,
                no_room_output,
                timeout.or(config.client.session.timeout_seconds),
                config.client.session.heartbeat_interval,
                &sender_id,
            );

            let task_uuid = task.uuid.clone();
            let task_content = matrix::serialize_event(&task)?;

            // Submit the task event to the exec room
            let event_id = mx_room
                .post_event_mut(SESSION_TASK, task_content)
                .await?;
            tracing::info!(uuid = %task_uuid, event_id = %event_id, "task submitted");

            if detach {
                println!("{}", task_uuid);
            } else {
                // Tail the session: sync for output and result events
                eprintln!("Session {} submitted, waiting for output...", task_uuid);
                let mut exit_code: Option<i32> = None;

                loop {
                    let events = mx_room.sync_events_mut().await?;
                    for event in events {
                        match event {
                            IncomingClientEvent::SessionOutput {
                                session_uuid,
                                content,
                            } => {
                                if session_uuid != task_uuid {
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
                                session_uuid,
                                content,
                            } => {
                                if session_uuid != task_uuid {
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
                interactive,
                no_room_output: false,
                homeserver: cli.homeserver,
                username: cli.username,
                password: cli.password,
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
                } = e
                {
                    if session_uuid == &uuid {
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
                homeserver: cli.homeserver,
                username: cli.username,
                password: cli.password,
                force_new_device: cli_force_new_device,
            };
            let config = ClientRuntimeConfig::load()?.with_cli_overrides(&client_args);
            let accounts = config.resolve_accounts();
            if accounts.is_empty() {
                anyhow::bail!(
                    "No Matrix accounts configured (use --homeserver/--username/--password or config file)"
                );
            }
            let room_name = resolve_worker_room(&worker_room, &config, cli_room_id.is_some())?;

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

            let completed_state = if all {
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
                homeserver: cli.homeserver,
                username: cli.username,
                password: cli.password,
                force_new_device: cli_force_new_device,
            };
            let config = ClientRuntimeConfig::load()?.with_cli_overrides(&client_args);
            let accounts = config.resolve_accounts();
            if accounts.is_empty() {
                anyhow::bail!(
                    "No Matrix accounts configured (use --homeserver/--username/--password or config file)"
                );
            }
            let room_name = resolve_worker_room(&worker_room, &config, cli_room_id.is_some())?;

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
                } = event
                {
                    if session_uuid == uuid {
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

            if follow {
                eprintln!("(follow mode: watching for new output...)");
                loop {
                    let events = mx_room.sync_events_mut().await?;
                    for event in events {
                        match event {
                            IncomingClientEvent::SessionOutput {
                                session_uuid,
                                content,
                            } => {
                                if session_uuid != uuid {
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
                                if session_uuid == uuid {
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
                homeserver: cli.homeserver,
                username: cli.username,
                password: cli.password,
                force_new_device: cli_force_new_device,
            };
            let config = ClientRuntimeConfig::load()?.with_cli_overrides(&client_args);
            let accounts = config.resolve_accounts();
            if accounts.is_empty() {
                anyhow::bail!(
                    "No Matrix accounts configured (use --homeserver/--username/--password or config file)"
                );
            }
            let room_name = resolve_worker_room(&worker_room, &config, cli_room_id.is_some())?;

            let mut mx_room = matrix::connect_multi(
                &accounts,
                &room_name,
                cli_room_id.as_deref(),
                config.force_new_device,
            )
            .await?;

            if let Some(sig) = signal {
                let event = mxdx_client::cancel::build_signal(&uuid, &sig);
                let content = matrix::serialize_event(&event)?;
                let event_id = mx_room
                    .post_event_mut(SESSION_SIGNAL, content)
                    .await?;
                tracing::info!(uuid = %uuid, signal = %sig, event_id = %event_id, "signal sent");
                eprintln!("Signal {} sent to session {}", sig, uuid);
            } else {
                let event = mxdx_client::cancel::build_cancel(&uuid, None, None);
                let content = matrix::serialize_event(&event)?;
                let event_id = mx_room
                    .post_event_mut(SESSION_CANCEL, content)
                    .await?;
                tracing::info!(uuid = %uuid, event_id = %event_id, "cancel sent");
                eprintln!("Cancel sent for session {}", uuid);
            }
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
    }
    Ok(())
}
