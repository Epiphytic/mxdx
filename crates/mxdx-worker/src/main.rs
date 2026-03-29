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

        /// Matrix homeserver URL
        #[arg(long, env = "MXDX_HOMESERVER")]
        homeserver: Option<String>,

        /// Matrix username
        #[arg(long, env = "MXDX_USERNAME")]
        username: Option<String>,

        /// Matrix password
        #[arg(long, env = "MXDX_PASSWORD")]
        password: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Start {
            trust_anchor,
            history_retention,
            cross_signing_mode,
            room_name,
            homeserver,
            username,
            password,
        } => {
            let args = WorkerArgs {
                trust_anchor,
                history_retention,
                cross_signing_mode,
                room_name,
                homeserver,
                username,
                password,
            };
            let config = WorkerRuntimeConfig::load()?.with_cli_overrides(&args);
            mxdx_worker::run_worker(config).await?;
        }
    }

    Ok(())
}
