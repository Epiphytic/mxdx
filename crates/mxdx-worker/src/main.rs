use anyhow::Result;
use clap::Parser;
use mxdx_worker::config::{WorkerArgs, WorkerRuntimeConfig};

#[derive(Parser)]
#[command(name = "mxdx-worker", about = "mxdx worker agent")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Start the worker
    Start {
        /// Trust anchor Matrix user ID
        #[arg(long)]
        trust_anchor: Option<String>,

        /// History retention in days
        #[arg(long)]
        history_retention: Option<u64>,

        /// Cross-signing mode (auto or manual)
        #[arg(long)]
        cross_signing_mode: Option<String>,

        /// Room name override
        #[arg(long)]
        room_name: Option<String>,

        /// Use a specific room ID directly (bypasses space creation)
        #[arg(long, env = "MXDX_ROOM_ID")]
        room_id: Option<String>,

        /// Matrix homeserver URL
        #[arg(long, env = "MXDX_HOMESERVER")]
        homeserver: Option<String>,

        /// Matrix username
        #[arg(long, env = "MXDX_USERNAME")]
        username: Option<String>,

        /// Matrix password
        #[arg(long, env = "MXDX_PASSWORD")]
        password: Option<String>,

        /// Force a fresh device login, skipping session restore
        #[arg(long, default_value_t = false)]
        force_new_device: bool,

        /// Maximum concurrent sessions
        #[arg(long)]
        max_sessions: Option<u32>,

        /// Allowed command binaries (can be repeated)
        #[arg(long = "allowed-command")]
        allowed_commands: Vec<String>,

        /// Allowed working directories (prefix match, can be repeated)
        #[arg(long = "allowed-cwd")]
        allowed_cwd: Vec<String>,

        /// Authorized Matrix user IDs (can be repeated)
        #[arg(long = "authorized-user")]
        authorized_users: Vec<String>,
    },
    /// Diagnose runtime state — emits a single JSON report on stdout.
    ///
    /// Safe to run whether or not a worker is active; uses REST and local
    /// file reads only, never takes over the crypto store.
    Diagnose {
        /// Named profile (default: "default")
        #[arg(long, default_value = "default")]
        profile: String,

        /// Matrix homeserver URL
        #[arg(long, env = "MXDX_HOMESERVER")]
        homeserver: Option<String>,

        /// Matrix username
        #[arg(long, env = "MXDX_USERNAME")]
        username: Option<String>,

        /// Matrix password
        #[arg(long, env = "MXDX_PASSWORD")]
        password: Option<String>,

        /// Pretty-print JSON output
        #[arg(long)]
        pretty: bool,

        /// Spawn a temporary matrix-sdk client and decrypt joined-room state
        /// events into the report. Requires homeserver/username/password.
        #[arg(long, default_value_t = false)]
        decrypt: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Start {
            trust_anchor,
            history_retention,
            cross_signing_mode,
            room_name,
            room_id,
            homeserver,
            username,
            password,
            force_new_device,
            max_sessions,
            allowed_commands,
            allowed_cwd,
            authorized_users,
        } => {
            let args = WorkerArgs {
                trust_anchor,
                history_retention,
                cross_signing_mode,
                room_name,
                room_id,
                homeserver,
                username,
                password,
                force_new_device,
                max_sessions,
                allowed_commands,
                allowed_cwd,
                authorized_users,
            };
            let config = WorkerRuntimeConfig::load()?.with_cli_overrides(&args);
            mxdx_worker::run_worker(config).await?;
        }
        Commands::Diagnose {
            profile,
            homeserver,
            username,
            password,
            pretty,
            decrypt,
        } => {
            let input = mxdx_client::diagnose::DiagnoseInput {
                profile,
                homeserver,
                username,
                password,
                pretty,
                decrypt,
            };
            mxdx_client::diagnose::run_diagnose(
                mxdx_client::diagnose::DiagnoseBinary::Worker,
                input,
            )
            .await?;
        }
    }

    Ok(())
}
