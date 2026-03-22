use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use mxdx_fabric::{
    coordinator::CoordinatorBot,
    jcode_worker::JcodeWorker,
    worker::WorkerClient,
};
use mxdx_matrix::MatrixClient;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "fabric", about = "mxdx fabric task CLI")]
struct Cli {
    #[arg(long, env = "FABRIC_HOMESERVER", global = true)]
    homeserver: Option<String>,

    #[arg(long, env = "FABRIC_TOKEN", global = true)]
    token: Option<String>,

    #[arg(long, env = "FABRIC_COORDINATOR_ROOM", global = true)]
    coordinator_room: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Post {
        #[arg(long)]
        capabilities: String,

        #[arg(long)]
        prompt: String,

        #[arg(long, default_value = "1800")]
        timeout: u64,

        #[arg(long, default_value = "false")]
        p2p_stream: bool,
    },
    Status {
        #[arg(long)]
        task_uuid: String,

        #[arg(long)]
        room: Option<String>,
    },
    Watch {
        #[arg(long)]
        task_uuid: String,

        #[arg(long)]
        room: Option<String>,
    },
    /// Run the coordinator daemon: routes tasks from the coordinator room to worker rooms.
    Coordinator {
        /// User ID for the coordinator account (e.g. @bel-coordinator:example.com)
        #[arg(long, env = "FABRIC_COORDINATOR_USER_ID")]
        coordinator_user_id: Option<String>,

        /// Device ID for the coordinator access token
        #[arg(long, env = "FABRIC_COORDINATOR_DEVICE_ID")]
        coordinator_device_id: Option<String>,
    },
    /// Run the worker daemon: claims and executes tasks via jcode.
    Worker {
        /// Capabilities this worker advertises (CSV)
        #[arg(long, default_value = "rust,linux,bash")]
        capabilities: String,

        /// Worker user ID (e.g. @bel-worker:example.com)
        #[arg(long, env = "FABRIC_WORKER_USER_ID")]
        worker_user_id: Option<String>,

        /// Device ID for the worker access token
        #[arg(long, env = "FABRIC_WORKER_DEVICE_ID")]
        worker_device_id: Option<String>,
    },
}

#[derive(Debug, Deserialize, Serialize)]
struct Config {
    homeserver: Option<String>,
    token: Option<String>,
    coordinator_room: Option<String>,
}

fn load_config() -> Config {
    let config_path = dirs_path().join("config.toml");
    match std::fs::read_to_string(&config_path) {
        Ok(contents) => toml::from_str(&contents).unwrap_or(Config {
            homeserver: None,
            token: None,
            coordinator_room: None,
        }),
        Err(_) => Config {
            homeserver: None,
            token: None,
            coordinator_room: None,
        },
    }
}

fn dirs_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mxdx-fabric")
}

fn resolve(cli_val: &Option<String>, config_val: &Option<String>, name: &str) -> Result<String> {
    cli_val
        .as_ref()
        .or(config_val.as_ref())
        .cloned()
        .with_context(|| format!("--{name} is required (or set in config.toml)"))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let config = load_config();

    let homeserver = resolve(&cli.homeserver, &config.homeserver, "homeserver")?;
    let token = resolve(&cli.token, &config.token, "token")?;

    match cli.command {
        Commands::Post {
            capabilities,
            prompt,
            timeout,
            p2p_stream,
        } => {
            let coordinator_room = resolve(
                &cli.coordinator_room,
                &config.coordinator_room,
                "coordinator-room",
            )?;
            cmd_post(
                &homeserver,
                &token,
                &coordinator_room,
                &capabilities,
                &prompt,
                timeout,
                p2p_stream,
            )
            .await
        }
        Commands::Status { task_uuid, room } => {
            let room_id = resolve(&room, &cli.coordinator_room, "room")
                .or_else(|_| resolve(&None, &config.coordinator_room, "room"))?;
            cmd_status(&homeserver, &token, &room_id, &task_uuid).await
        }
        Commands::Watch { task_uuid, room } => {
            let room_id = resolve(&room, &cli.coordinator_room, "room")
                .or_else(|_| resolve(&None, &config.coordinator_room, "room"))?;
            cmd_watch(&homeserver, &token, &room_id, &task_uuid).await
        }
        Commands::Coordinator {
            coordinator_user_id,
            coordinator_device_id,
        } => {
            let coordinator_room = resolve(
                &cli.coordinator_room,
                &config.coordinator_room,
                "coordinator-room",
            )?;
            let user_id = coordinator_user_id
                .unwrap_or_else(|| "@bel-coordinator:ca1-beta.mxdx.dev".to_string());
            let device_id = coordinator_device_id
                .unwrap_or_else(|| "scfvjFQUVO".to_string());
            cmd_coordinator(&homeserver, &token, &coordinator_room, &user_id, &device_id).await
        }
        Commands::Worker {
            capabilities,
            worker_user_id,
            worker_device_id,
        } => {
            let coordinator_room = resolve(
                &cli.coordinator_room,
                &config.coordinator_room,
                "coordinator-room",
            )?;
            let user_id = worker_user_id
                .unwrap_or_else(|| "@bel-worker:ca1-beta.mxdx.dev".to_string());
            let device_id = worker_device_id
                .unwrap_or_else(|| "vDLnPCG2CQ".to_string());
            cmd_worker(&homeserver, &token, &coordinator_room, &capabilities, &user_id, &device_id).await
        }
    }
}

async fn cmd_coordinator(
    homeserver: &str,
    token: &str,
    coordinator_room: &str,
    user_id: &str,
    device_id: &str,
) -> Result<()> {
    println!("Coordinator running on {homeserver}, room {coordinator_room}");

    let matrix_client = MatrixClient::connect_with_token(homeserver, token, user_id, device_id)
        .await
        .context("failed to connect to Matrix with token")?;

    let room_id: mxdx_matrix::OwnedRoomId = coordinator_room
        .try_into()
        .context("invalid coordinator room ID")?;

    let mut bot = CoordinatorBot::new(Arc::new(matrix_client), room_id, homeserver.to_string());

    loop {
        if let Err(e) = bot.run().await {
            eprintln!("Coordinator error (restarting in 5s): {e:#}");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

async fn cmd_worker(
    homeserver: &str,
    token: &str,
    coordinator_room: &str,
    capabilities_csv: &str,
    user_id: &str,
    device_id: &str,
) -> Result<()> {
    let caps: Vec<String> = capabilities_csv
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();

    println!(
        "Worker running on {homeserver}, room {coordinator_room}, capabilities: {:?}",
        caps
    );

    let matrix_client = MatrixClient::connect_with_token(homeserver, token, user_id, device_id)
        .await
        .context("failed to connect to Matrix with token")?;

    let matrix_client = Arc::new(matrix_client);

    let room_id: mxdx_matrix::OwnedRoomId = coordinator_room
        .try_into()
        .context("invalid coordinator room ID")?;

    let worker_client = WorkerClient::new(
        matrix_client,
        user_id.to_string(),
        homeserver.to_string(),
    );

    // Advertise capabilities
    worker_client
        .advertise_capabilities(&caps, &room_id)
        .await
        .context("failed to advertise capabilities")?;

    let jcode_worker = JcodeWorker::new(worker_client, None);

    // Watch coordinator room for tasks
    loop {
        match jcode_worker.worker_client().watch_and_claim(&room_id, &caps).await {
            Ok(Some(task)) => {
                println!("Claimed task {}", task.uuid);
                if let Err(e) = jcode_worker.run_task(task, &room_id).await {
                    eprintln!("Task error: {e:#}");
                }
            }
            Ok(None) => {
                // No task available, short sleep before polling again
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(e) => {
                eprintln!("Worker watch error (retrying in 5s): {e:#}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn matrix_send_event(
    http: &reqwest::Client,
    homeserver: &str,
    token: &str,
    room_id: &str,
    event_type: &str,
    content: &serde_json::Value,
) -> Result<()> {
    let txn_id = Uuid::new_v4().to_string();
    let url = format!(
        "{}/_matrix/client/v3/rooms/{}/send/{}/{}",
        homeserver,
        urlencoding::encode(room_id),
        urlencoding::encode(event_type),
        urlencoding::encode(&txn_id),
    );

    let resp = http
        .put(&url)
        .bearer_auth(token)
        .json(content)
        .send()
        .await
        .context("failed to send Matrix event")?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("Matrix PUT failed: {body}");
    }

    Ok(())
}

async fn matrix_get_messages(
    http: &reqwest::Client,
    homeserver: &str,
    token: &str,
    room_id: &str,
    limit: u32,
    from: Option<&str>,
) -> Result<(Vec<serde_json::Value>, Option<String>)> {
    let mut url = format!(
        "{}/_matrix/client/v3/rooms/{}/messages?dir=b&limit={}",
        homeserver,
        urlencoding::encode(room_id),
        limit,
    );
    if let Some(from_token) = from {
        url.push_str(&format!("&from={}", urlencoding::encode(from_token)));
    }

    let resp = http
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .context("failed to GET messages")?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("Matrix GET messages failed: {body}");
    }

    let body: serde_json::Value = resp.json().await?;
    let events = body
        .get("chunk")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    let end = body
        .get("end")
        .and_then(|e| e.as_str())
        .map(|s| s.to_string());

    Ok((events, end))
}

async fn matrix_sync(
    http: &reqwest::Client,
    homeserver: &str,
    token: &str,
    since: Option<&str>,
    timeout_ms: u64,
) -> Result<(serde_json::Value, Option<String>)> {
    let mut url = format!(
        "{}/_matrix/client/v3/sync?timeout={}",
        homeserver, timeout_ms,
    );
    if let Some(since_token) = since {
        url.push_str(&format!("&since={}", urlencoding::encode(since_token)));
    }

    let resp = http
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .context("failed to sync")?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("Matrix sync failed: {body}");
    }

    let body: serde_json::Value = resp.json().await?;
    let next_batch = body
        .get("next_batch")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string());

    Ok((body, next_batch))
}

fn find_result_events(events: &[serde_json::Value], task_uuid: &str) -> Option<serde_json::Value> {
    for event in events {
        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if event_type != "org.mxdx.fabric.result" {
            continue;
        }
        let content = match event.get("content") {
            Some(c) => c,
            None => continue,
        };
        if content.get("task_uuid").and_then(|u| u.as_str()) == Some(task_uuid) {
            return Some(content.clone());
        }
    }
    None
}

fn find_heartbeat_events(events: &[serde_json::Value], task_uuid: &str) -> Vec<serde_json::Value> {
    events
        .iter()
        .filter(|event| {
            let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if event_type != "org.mxdx.fabric.heartbeat" {
                return false;
            }
            event
                .get("content")
                .and_then(|c| c.get("task_uuid"))
                .and_then(|u| u.as_str())
                == Some(task_uuid)
        })
        .cloned()
        .collect()
}

fn find_task_status(events: &[serde_json::Value], task_uuid: &str) -> Option<String> {
    if let Some(result) = find_result_events(events, task_uuid) {
        return result
            .get("status")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());
    }

    for event in events {
        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if event_type == "org.mxdx.fabric.claim"
            && event
                .get("content")
                .and_then(|c| c.get("task_uuid"))
                .and_then(|u| u.as_str())
                == Some(task_uuid)
        {
            return Some("claimed".to_string());
        }
        if event_type == "org.mxdx.fabric.task"
            && event
                .get("content")
                .and_then(|c| c.get("uuid"))
                .and_then(|u| u.as_str())
                == Some(task_uuid)
        {
            return Some("posted".to_string());
        }
    }

    None
}

async fn cmd_post(
    homeserver: &str,
    token: &str,
    coordinator_room: &str,
    capabilities: &str,
    prompt: &str,
    timeout: u64,
    p2p_stream: bool,
) -> Result<()> {
    let http = reqwest::Client::new();

    let task_uuid = Uuid::new_v4().to_string();
    let caps: Vec<String> = capabilities
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();

    let task = mxdx_types::events::fabric::TaskEvent {
        uuid: task_uuid.clone(),
        sender_id: "fabric-cli".to_string(),
        required_capabilities: caps,
        estimated_cycles: None,
        timeout_seconds: timeout,
        heartbeat_interval_seconds: 60,
        on_timeout: mxdx_types::events::fabric::FailurePolicy::Escalate,
        on_heartbeat_miss: mxdx_types::events::fabric::FailurePolicy::Escalate,
        routing_mode: mxdx_types::events::fabric::RoutingMode::Auto,
        p2p_stream,
        payload: serde_json::json!({"prompt": prompt}),
        plan: Some(prompt.to_string()),
    };

    let task_json = serde_json::to_value(&task)?;

    eprintln!("Posting task {} to {}", task_uuid, coordinator_room);
    matrix_send_event(
        &http,
        homeserver,
        token,
        coordinator_room,
        "org.mxdx.fabric.task",
        &task_json,
    )
    .await?;
    eprintln!("Task posted, waiting for result (timeout: {}s)...", timeout);

    let (_sync_body, mut sync_token) = matrix_sync(&http, homeserver, token, None, 1000).await?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);

    while tokio::time::Instant::now() < deadline {
        let (_sync_body, new_token) =
            matrix_sync(&http, homeserver, token, sync_token.as_deref(), 5000).await?;
        sync_token = new_token;

        let (events, _) =
            matrix_get_messages(&http, homeserver, token, coordinator_room, 50, None).await?;

        if let Some(result) = find_result_events(&events, &task_uuid) {
            let status = result
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("unknown");
            let output = serde_json::to_string_pretty(&result)?;
            println!("{output}");

            match status {
                "success" => std::process::exit(0),
                _ => std::process::exit(1),
            }
        }
    }

    eprintln!("Timed out waiting for result after {}s", timeout);
    std::process::exit(1);
}

async fn cmd_status(homeserver: &str, token: &str, room_id: &str, task_uuid: &str) -> Result<()> {
    let http = reqwest::Client::new();

    let (events, _) = matrix_get_messages(&http, homeserver, token, room_id, 100, None).await?;

    match find_task_status(&events, task_uuid) {
        Some(status) => {
            println!(
                "{}",
                serde_json::json!({"task_uuid": task_uuid, "status": status})
            );
            Ok(())
        }
        None => {
            println!(
                "{}",
                serde_json::json!({"task_uuid": task_uuid, "status": "not_found"})
            );
            Ok(())
        }
    }
}

async fn cmd_watch(homeserver: &str, token: &str, room_id: &str, task_uuid: &str) -> Result<()> {
    let http = reqwest::Client::new();

    let (_sync_body, mut sync_token) = matrix_sync(&http, homeserver, token, None, 1000).await?;

    let mut seen_heartbeats: std::collections::HashSet<String> = std::collections::HashSet::new();

    loop {
        let (_sync_body, new_token) =
            matrix_sync(&http, homeserver, token, sync_token.as_deref(), 10000).await?;
        sync_token = new_token;

        let (events, _) = matrix_get_messages(&http, homeserver, token, room_id, 50, None).await?;

        if let Some(result) = find_result_events(&events, task_uuid) {
            let output = serde_json::to_string_pretty(&result)?;
            println!("RESULT: {output}");
            return Ok(());
        }

        let heartbeats = find_heartbeat_events(&events, task_uuid);
        for hb in &heartbeats {
            let key = format!(
                "{}:{}",
                hb.get("content")
                    .and_then(|c| c.get("timestamp"))
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0),
                hb.get("content")
                    .and_then(|c| c.get("progress"))
                    .and_then(|p| p.as_str())
                    .unwrap_or("")
            );
            if seen_heartbeats.insert(key) {
                let progress = hb
                    .get("content")
                    .and_then(|c| c.get("progress"))
                    .and_then(|p| p.as_str())
                    .unwrap_or("(no progress)");
                eprintln!("HEARTBEAT: {progress}");
            }
        }
    }
}
