use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "mxdx-client", about = "mxdx client CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Execute a command on a worker
    Exec {
        /// Target worker room
        #[arg(long)]
        worker_room: Option<String>,

        /// Coordinator room
        #[arg(long)]
        coordinator_room: Option<String>,

        /// Session timeout in seconds
        #[arg(long)]
        timeout: Option<u64>,

        /// Heartbeat interval in seconds
        #[arg(long)]
        heartbeat_interval: Option<u64>,

        /// Interactive session
        #[arg(long)]
        interactive: bool,

        /// Suppress room output
        #[arg(long)]
        no_room_output: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Exec {
            worker_room,
            coordinator_room,
            timeout,
            heartbeat_interval,
            interactive,
            no_room_output,
        } => {
            let _args = mxdx_client::config::ClientArgs {
                worker_room,
                coordinator_room,
                timeout,
                heartbeat_interval,
                interactive,
                no_room_output,
            };
            tracing::info!("mxdx-client exec stub");
        }
    }

    Ok(())
}
