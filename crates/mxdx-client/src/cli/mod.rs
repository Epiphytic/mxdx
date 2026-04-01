use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "mxdx-client", about = "mxdx client CLI")]
pub struct Cli {
    /// Matrix homeserver URL or server name
    #[arg(long, env = "MXDX_HOMESERVER", global = true)]
    pub homeserver: Option<String>,
    /// Matrix username
    #[arg(long, env = "MXDX_USERNAME", global = true)]
    pub username: Option<String>,
    /// Matrix password
    #[arg(long, env = "MXDX_PASSWORD", global = true)]
    pub password: Option<String>,
    /// Direct room ID (bypasses space discovery)
    #[arg(long, env = "MXDX_ROOM_ID", global = true)]
    pub room_id: Option<String>,
    /// Force a fresh device login, skipping session restore
    #[arg(long, global = true, default_value_t = false)]
    pub force_new_device: bool,
    /// Named profile (default: "default")
    #[arg(long, global = true, default_value = "default")]
    pub profile: String,
    /// Bypass daemon, connect directly
    #[arg(long, global = true, default_value_t = false)]
    pub no_daemon: bool,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
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
    /// Daemon management
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Internal: run as daemon (hidden)
    #[command(name = "_daemon", hide = true)]
    InternalDaemon {
        #[arg(long, default_value = "default")]
        profile: String,
        #[arg(long, default_value_t = false)]
        detach: bool,
    },
}

#[derive(Subcommand)]
pub enum TrustAction {
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

#[derive(Subcommand)]
pub enum DaemonAction {
    /// Start the daemon
    Start {
        #[arg(long, default_value_t = false)]
        detach: bool,
        #[arg(long)]
        enable_websocket: bool,
        #[arg(long)]
        ws_port: Option<u16>,
    },
    /// Stop the daemon
    Stop {
        #[arg(long, default_value_t = false)]
        all: bool,
    },
    /// Show daemon status
    Status,
    /// Run as MCP server (foreground, stdio)
    Mcp,
}

pub mod connect;
pub mod format;
