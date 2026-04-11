use anyhow::Result;
use clap::Parser;
use mxdx_coordinator::config::{CoordinatorArgs, CoordinatorRuntimeConfig};

#[derive(Parser)]
#[command(name = "mxdx-coordinator", about = "mxdx coordinator — routes tasks to workers")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Start the coordinator
    Start {
        /// Coordinator room ID or alias
        #[arg(long)]
        room: Option<String>,

        /// Prefix for capability-based worker rooms
        #[arg(long)]
        capability_room_prefix: Option<String>,

        /// Default action on task timeout (escalate, abandon)
        #[arg(long)]
        default_on_timeout: Option<String>,

        /// Default action on heartbeat miss (escalate, abandon)
        #[arg(long)]
        default_on_heartbeat_miss: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Start {
            room,
            capability_room_prefix,
            default_on_timeout,
            default_on_heartbeat_miss,
        } => {
            let args = CoordinatorArgs {
                room,
                capability_room_prefix,
                default_on_timeout,
                default_on_heartbeat_miss,
            };
            let config = CoordinatorRuntimeConfig::load()?.with_cli_overrides(&args);
            mxdx_coordinator::run_coordinator(config).await?;
        }
    }

    Ok(())
}
