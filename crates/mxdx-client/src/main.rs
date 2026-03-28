use anyhow::Result;
use clap::{Parser, Subcommand};
use mxdx_client::config::{ClientArgs, ClientRuntimeConfig};

#[derive(Parser)]
#[command(name = "mxdx-client", about = "mxdx client CLI")]
struct Cli {
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
    },
    /// View session logs
    Logs {
        /// Session UUID
        uuid: String,
        /// Follow output in real-time
        #[arg(short = 'f', long)]
        follow: bool,
    },
    /// Cancel a session
    Cancel {
        /// Session UUID
        uuid: String,
        /// Send specific signal
        #[arg(long)]
        signal: Option<String>,
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            command,
            args,
            detach,
            interactive,
            no_room_output,
            timeout,
            worker_room,
        }
        | Commands::Exec {
            command,
            args,
            detach,
            interactive,
            no_room_output,
            timeout,
            worker_room,
        } => {
            let client_args = ClientArgs {
                worker_room,
                coordinator_room: None,
                timeout,
                heartbeat_interval: None,
                interactive,
                no_room_output,
            };
            let config = ClientRuntimeConfig::load()?.with_cli_overrides(&client_args);

            // Build task
            let task = mxdx_client::submit::build_task(
                &command,
                &args,
                interactive,
                no_room_output,
                timeout.or(config.client.session.timeout_seconds),
                config.client.session.heartbeat_interval,
                "client", // placeholder sender_id
            );

            if detach {
                println!("{}", task.uuid);
                // In detached mode, submit and exit
            } else {
                tracing::info!(uuid = %task.uuid, cmd = %command, "submitting task");
                // Submit task -> tail thread -> exit on result
            }
        }
        Commands::Attach { uuid, interactive } => {
            tracing::info!(uuid = %uuid, interactive, "attaching to session");
        }
        Commands::Ls { all } => {
            tracing::info!(all, "listing sessions");
        }
        Commands::Logs { uuid, follow } => {
            tracing::info!(uuid = %uuid, follow, "viewing logs");
        }
        Commands::Cancel { uuid, signal } => {
            if let Some(sig) = signal {
                let _event = mxdx_client::cancel::build_signal(&uuid, &sig);
                tracing::info!(uuid = %uuid, signal = %sig, "sending signal");
            } else {
                let _event = mxdx_client::cancel::build_cancel(&uuid, None, None);
                tracing::info!(uuid = %uuid, "cancelling session");
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
