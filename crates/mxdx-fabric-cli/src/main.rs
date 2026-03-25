use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use chrono::{NaiveDate, Utc};
use clap::{Parser, Subcommand};
use mxdx_fabric::{coordinator::CoordinatorBot, process_worker::ProcessWorker, worker::WorkerClient};
use mxdx_matrix::MatrixClient;
use mxdx_types::events::capability::CapabilityAdvertisement;
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

        #[arg(long)]
        task_uuid: Option<String>,

        #[arg(
            long,
            value_name = "JSON",
            help = "Arbitrary JSON object merged into task payload"
        )]
        payload_json: Option<String>,
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
    /// Show full logs/output for a task by UUID.
    Logs {
        #[arg(value_name = "TASK_UUID")]
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
    /// Show capability advertisements from workers in the coordinator room.
    Capabilities {
        /// Optional worker ID to filter (exact match on state_key)
        #[arg(value_name = "WORKER_ID")]
        worker_id: Option<String>,
    },
    /// Show past task history from the coordinator room timeline.
    History {
        /// Max number of tasks to show (default: 20)
        #[arg(long, default_value = "20")]
        limit: usize,

        /// Filter by status: "done", "failed", "success", "timeout", or "any" (default: "any")
        #[arg(long, default_value = "any")]
        status: String,

        /// Show tasks from a specific date (YYYY-MM-DD, UTC)
        #[arg(long)]
        date: Option<String>,

        /// Relative time filter: "1h", "1d", "1w", "1m" (last hour/day/week/month)
        #[arg(long)]
        since: Option<String>,

        /// Start of date range (YYYY-MM-DD, inclusive)
        #[arg(long)]
        from_date: Option<String>,

        /// End of date range (YYYY-MM-DD, inclusive)
        #[arg(long)]
        to_date: Option<String>,
    },
    /// Run the worker daemon: claims and executes tasks as a generic process executor.
    Worker {
        /// Capabilities this worker advertises (CSV)
        #[arg(long, default_value = "rust,linux,bash")]
        capabilities: String,

        /// Binaries available on this worker host (CSV, for capability advertisement)
        #[arg(long, default_value = "jcode")]
        available_bins: String,

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
            task_uuid,
            payload_json,
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
                task_uuid,
                payload_json,
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
        Commands::Logs { task_uuid, room } => {
            let room_id = resolve(&room, &cli.coordinator_room, "room")
                .or_else(|_| resolve(&None, &config.coordinator_room, "room"))?;
            cmd_logs(&homeserver, &token, &room_id, &task_uuid).await
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
            let device_id = coordinator_device_id.unwrap_or_else(|| "scfvjFQUVO".to_string());
            cmd_coordinator(&homeserver, &token, &coordinator_room, &user_id, &device_id).await
        }
        Commands::Capabilities { worker_id } => {
            let coordinator_room = resolve(
                &cli.coordinator_room,
                &config.coordinator_room,
                "coordinator-room",
            )?;
            cmd_capabilities(&homeserver, &token, &coordinator_room, worker_id.as_deref()).await
        }
        Commands::History {
            limit,
            status,
            date,
            since,
            from_date,
            to_date,
        } => {
            let coordinator_room = resolve(
                &cli.coordinator_room,
                &config.coordinator_room,
                "coordinator-room",
            )?;
            cmd_history(
                &homeserver,
                &token,
                &coordinator_room,
                limit,
                &status,
                date.as_deref(),
                since.as_deref(),
                from_date.as_deref(),
                to_date.as_deref(),
            )
            .await
        }
        Commands::Worker {
            capabilities,
            available_bins,
            worker_user_id,
            worker_device_id,
        } => {
            let coordinator_room = resolve(
                &cli.coordinator_room,
                &config.coordinator_room,
                "coordinator-room",
            )?;
            let user_id =
                worker_user_id.unwrap_or_else(|| "@bel-worker:ca1-beta.mxdx.dev".to_string());
            let device_id = worker_device_id.unwrap_or_else(|| "vDLnPCG2CQ".to_string());
            let bins: Vec<String> = available_bins
                .split(',')
                .map(|s| s.trim().to_string())
                .collect();
            cmd_worker(
                &homeserver,
                &token,
                &coordinator_room,
                &capabilities,
                &bins,
                &user_id,
                &device_id,
            )
            .await
        }
    }
}

async fn matrix_get_room_state(
    http: &reqwest::Client,
    homeserver: &str,
    token: &str,
    room_id: &str,
) -> Result<Vec<serde_json::Value>> {
    let url = format!(
        "{}/_matrix/client/v3/rooms/{}/state",
        homeserver,
        urlencoding::encode(room_id),
    );

    let resp = http
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .context("failed to GET room state")?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("Matrix GET room state failed: {body}");
    }

    let events: Vec<serde_json::Value> = resp.json().await.context("failed to parse room state")?;
    Ok(events)
}

fn format_capabilities(advertisements: &[(String, CapabilityAdvertisement)]) {
    for (state_key, ad) in advertisements {
        println!("Worker: {} (host: {})", ad.worker_id, ad.host);
        if state_key != &ad.worker_id {
            println!("  state_key: {state_key}");
        }
        for tool in &ad.tools {
            let version = tool.version.as_deref().unwrap_or("unknown");
            let health = if tool.healthy { "healthy" } else { "unhealthy" };
            println!("  Tool: {} v{} [{}]", tool.name, version, health);

            let mut names: Vec<&String> = tool.input_schema.properties.keys().collect();
            names.sort();

            for name in names {
                let prop = &tool.input_schema.properties[name];
                let required = if tool.input_schema.required.contains(name) {
                    "(required)"
                } else {
                    ""
                };
                println!(
                    "    {:<16} {:<8} {:>10}  {}",
                    name, prop.r#type, required, prop.description,
                );
            }
        }
    }
}

async fn cmd_capabilities(
    homeserver: &str,
    token: &str,
    coordinator_room: &str,
    worker_id_filter: Option<&str>,
) -> Result<()> {
    let http = reqwest::Client::new();

    let all_state = matrix_get_room_state(&http, homeserver, token, coordinator_room).await?;

    let mut advertisements: Vec<(String, CapabilityAdvertisement)> = Vec::new();

    for event in &all_state {
        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if event_type != "org.mxdx.fabric.capability" {
            continue;
        }

        let state_key = event
            .get("state_key")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();

        if let Some(filter) = worker_id_filter {
            if state_key != filter {
                continue;
            }
        }

        let content = match event.get("content") {
            Some(c) => c,
            None => continue,
        };

        match serde_json::from_value::<CapabilityAdvertisement>(content.clone()) {
            Ok(ad) => advertisements.push((state_key, ad)),
            Err(_) => {
                eprintln!("Warning: could not parse capability for state_key={state_key}");
            }
        }
    }

    if advertisements.is_empty() {
        if let Some(filter) = worker_id_filter {
            eprintln!("No capability advertisement found for worker: {filter}");
        } else {
            eprintln!("No capability advertisements found in room {coordinator_room}");
        }
        std::process::exit(1);
    }

    advertisements.sort_by(|a, b| a.0.cmp(&b.0));
    format_capabilities(&advertisements);

    Ok(())
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
    available_bins: &[String],
    user_id: &str,
    device_id: &str,
) -> Result<()> {
    let caps: Vec<String> = capabilities_csv
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();

    println!(
        "Worker running on {homeserver}, room {coordinator_room}, capabilities: {:?}, bins: {:?}",
        caps, available_bins
    );

    let matrix_client = MatrixClient::connect_with_token(homeserver, token, user_id, device_id)
        .await
        .context("failed to connect to Matrix with token")?;

    let matrix_client = Arc::new(matrix_client);

    let room_id: mxdx_matrix::OwnedRoomId = coordinator_room
        .try_into()
        .context("invalid coordinator room ID")?;

    let worker_client =
        WorkerClient::new(matrix_client, user_id.to_string(), homeserver.to_string());

    // Advertise capabilities (legacy CSV)
    worker_client
        .advertise_capabilities(&caps, &room_id)
        .await
        .context("failed to advertise capabilities")?;

    let process_worker = ProcessWorker::new(worker_client);

    // Publish structured capability advertisement (ADR-0005)
    let bin_refs: Vec<&str> = available_bins.iter().map(|s| s.as_str()).collect();
    if let Err(e) = process_worker
        .publish_capability_advertisement(&bin_refs, &room_id)
        .await
    {
        eprintln!("Warning: failed to publish capability advertisement: {e:#}");
    }

    // Spawn periodic capability advertisement refresh (every 60s)
    let refresh_room_id = room_id.clone();
    let refresh_worker_id = user_id.to_string();
    let refresh_homeserver = homeserver.to_string();
    let refresh_bins: Vec<String> = available_bins.to_vec();
    let refresh_matrix = process_worker.worker_client().matrix_client_arc();
    tokio::spawn(async move {
        let refresh_worker_client =
            WorkerClient::new(refresh_matrix, refresh_worker_id, refresh_homeserver);
        let refresh_worker = ProcessWorker::new(refresh_worker_client);
        let bin_refs: Vec<&str> = refresh_bins.iter().map(|s| s.as_str()).collect();
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if let Err(e) = refresh_worker
                .publish_capability_advertisement(&bin_refs, &refresh_room_id)
                .await
            {
                eprintln!("Warning: periodic capability refresh failed: {e:#}");
            }
        }
    });

    // Watch coordinator room for tasks
    loop {
        match process_worker
            .worker_client()
            .watch_and_claim(&room_id, &caps)
            .await
        {
            Ok(Some((task, task_event_id))) => {
                println!("Claimed task {}", task.uuid);
                if let Err(e) = process_worker.run_task(task, &room_id, task_event_id).await {
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
) -> Result<String> {
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

    let body: serde_json::Value = resp.json().await.context("failed to parse send response")?;
    let event_id = body
        .get("event_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(event_id)
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

#[allow(clippy::too_many_arguments)]
fn merge_payload(base: serde_json::Value, extra_json: Option<&str>) -> Result<serde_json::Value> {
    let extra = match extra_json {
        Some(s) => {
            let parsed: serde_json::Value =
                serde_json::from_str(s).context("--payload-json is not valid JSON")?;
            match parsed {
                serde_json::Value::Object(map) => map,
                _ => bail!("--payload-json must be a JSON object"),
            }
        }
        None => return Ok(base),
    };

    let mut merged = match base {
        serde_json::Value::Object(map) => map,
        other => {
            let mut map = serde_json::Map::new();
            if !other.is_null() {
                map.insert("_base".to_string(), other);
            }
            map
        }
    };

    let explicit_keys: std::collections::HashSet<String> = merged.keys().cloned().collect();

    for (key, value) in extra {
        if !explicit_keys.contains(&key) {
            merged.insert(key, value);
        }
    }

    Ok(serde_json::Value::Object(merged))
}

fn format_post_output(task_uuid: &str, event_id: &str) -> String {
    format!("task_uuid: {task_uuid}\nevent_id: {event_id}")
}

#[allow(clippy::too_many_arguments)]
async fn cmd_post(
    homeserver: &str,
    token: &str,
    coordinator_room: &str,
    capabilities: &str,
    prompt: &str,
    timeout: u64,
    p2p_stream: bool,
    task_uuid_override: Option<String>,
    payload_json: Option<String>,
) -> Result<()> {
    let http = reqwest::Client::new();

    let task_uuid = task_uuid_override.unwrap_or_else(|| Uuid::new_v4().to_string());
    let caps: Vec<String> = capabilities
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();

    let base_payload = serde_json::json!({"prompt": prompt});
    let payload = merge_payload(base_payload, payload_json.as_deref())?;

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
        payload,
        plan: Some(prompt.to_string()),
    };

    let task_json = serde_json::to_value(&task)?;

    eprintln!("Posting task {} to {}", task_uuid, coordinator_room);
    let event_id = matrix_send_event(
        &http,
        homeserver,
        token,
        coordinator_room,
        "org.mxdx.fabric.task",
        &task_json,
    )
    .await?;

    println!("{}", format_post_output(&task_uuid, &event_id));
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

async fn cmd_logs(homeserver: &str, token: &str, room_id: &str, task_uuid: &str) -> Result<()> {
    let http = reqwest::Client::new();

    // Page backwards through the timeline, collecting up to 500 events
    let mut all_events: Vec<serde_json::Value> = Vec::new();
    let mut pagination_token: Option<String> = None;
    let max_events: usize = 500;

    loop {
        let (events, next_token) = matrix_get_messages(
            &http,
            homeserver,
            token,
            room_id,
            100,
            pagination_token.as_deref(),
        )
        .await?;

        if events.is_empty() {
            break;
        }

        all_events.extend(events);

        if all_events.len() >= max_events {
            break;
        }

        match next_token {
            Some(t) => pagination_token = Some(t),
            None => break,
        }
    }

    // Heartbeats (full event objects, so we can read origin_server_ts)
    let mut heartbeats = find_heartbeat_events(&all_events, task_uuid);
    // Sort chronologically (oldest first) by origin_server_ts
    heartbeats.sort_by_key(|hb| {
        hb.get("origin_server_ts")
            .and_then(|t| t.as_i64())
            .unwrap_or(0)
    });

    if !heartbeats.is_empty() {
        println!("=== Heartbeats ===");
        for hb in &heartbeats {
            let ts_ms = hb
                .get("origin_server_ts")
                .and_then(|t| t.as_i64())
                .unwrap_or(0);
            let dt = chrono::DateTime::from_timestamp_millis(ts_ms)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let progress = hb
                .get("content")
                .and_then(|c| c.get("progress"))
                .and_then(|p| p.as_str())
                .unwrap_or("(no progress)");
            println!("[{dt}] {progress}");
        }
        println!();
    }

    // Result
    let result = find_result_events(&all_events, task_uuid);

    match result {
        Some(content) => {
            println!("=== Result ===");
            if let Some(output) = content.get("output") {
                if let Some(s) = output.as_str() {
                    println!("{s}");
                } else {
                    println!("{}", serde_json::to_string_pretty(output)?);
                }
            } else if let Some(result_val) = content.get("result") {
                if let Some(s) = result_val.as_str() {
                    println!("{s}");
                } else {
                    println!("{}", serde_json::to_string_pretty(result_val)?);
                }
            } else {
                println!("{}", serde_json::to_string_pretty(&content)?);
            }
        }
        None => {
            println!("Task still running or no result found.");
        }
    }

    Ok(())
}

/// Parsed info about a task event for history display.
#[derive(Debug, Clone)]
struct HistoryTask {
    timestamp_ms: i64,
    uuid: String,
    capabilities: Vec<String>,
    prompt: String,
    status: String,
    duration_seconds: Option<u64>,
}

/// Format a single history task as one line of output.
fn format_history_line(task: &HistoryTask) -> String {
    let dt = chrono::DateTime::from_timestamp_millis(task.timestamp_ms)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let short_uuid = if task.uuid.len() >= 7 {
        &task.uuid[..7]
    } else {
        &task.uuid
    };

    let caps = if task.capabilities.is_empty() {
        "-".to_string()
    } else {
        task.capabilities.join(",")
    };

    let prompt_display = if task.prompt.len() > 60 {
        format!("\"{}...\"", &task.prompt[..57])
    } else {
        format!("\"{}\"", task.prompt)
    };

    let duration = task
        .duration_seconds
        .map(|d| format!("  ({}s)", d))
        .unwrap_or_default();

    format!(
        "{}  {}  {}  {}  {}{}",
        dt, task.status, short_uuid, caps, prompt_display, duration,
    )
}

/// Parse a relative time period like "1h", "1d", "1w", "1m" into a duration.
fn parse_since_period(period: &str) -> Result<chrono::Duration> {
    let period = period.trim();
    if period.len() < 2 {
        bail!("Invalid --since value: {period}. Use format like 1h, 1d, 1w, 1m");
    }
    let (num_str, unit) = period.split_at(period.len() - 1);
    let num: i64 = num_str
        .parse()
        .with_context(|| format!("Invalid number in --since: {num_str}"))?;

    match unit {
        "h" => Ok(chrono::Duration::hours(num)),
        "d" => Ok(chrono::Duration::days(num)),
        "w" => Ok(chrono::Duration::weeks(num)),
        "m" => Ok(chrono::Duration::days(num * 30)),
        _ => bail!("Invalid --since unit: {unit}. Use h, d, w, or m"),
    }
}

/// Compute the time window (start_ms, end_ms) for filtering based on CLI options.
fn compute_time_window(
    date: Option<&str>,
    since: Option<&str>,
    from_date: Option<&str>,
    to_date: Option<&str>,
) -> Result<(Option<i64>, Option<i64>)> {
    if let Some(date_str) = date {
        let d = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
            .with_context(|| format!("Invalid --date: {date_str}. Use YYYY-MM-DD"))?;
        let start = d
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        let end = d
            .and_hms_opt(23, 59, 59)
            .unwrap()
            .and_utc()
            .timestamp_millis()
            + 999;
        return Ok((Some(start), Some(end)));
    }

    if let Some(period) = since {
        let duration = parse_since_period(period)?;
        let cutoff = Utc::now() - duration;
        return Ok((Some(cutoff.timestamp_millis()), None));
    }

    let start = if let Some(from) = from_date {
        let d = NaiveDate::parse_from_str(from, "%Y-%m-%d")
            .with_context(|| format!("Invalid --from-date: {from}. Use YYYY-MM-DD"))?;
        Some(
            d.and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp_millis(),
        )
    } else {
        None
    };

    let end = if let Some(to) = to_date {
        let d = NaiveDate::parse_from_str(to, "%Y-%m-%d")
            .with_context(|| format!("Invalid --to-date: {to}. Use YYYY-MM-DD"))?;
        Some(
            d.and_hms_opt(23, 59, 59)
                .unwrap()
                .and_utc()
                .timestamp_millis()
                + 999,
        )
    } else {
        None
    };

    Ok((start, end))
}

/// Check whether a task status matches the CLI --status filter.
fn status_matches_filter(task_status: &str, filter: &str) -> bool {
    match filter {
        "any" => true,
        "done" => matches!(
            task_status,
            "success" | "failed" | "timeout" | "cancelled"
        ),
        "success" => task_status == "success",
        "failed" => task_status == "failed",
        "timeout" => task_status == "timeout",
        _ => task_status == filter,
    }
}

/// Extract the prompt/plan text from a task event's content.
fn extract_prompt(content: &serde_json::Value) -> String {
    // Try plan field first, then payload.prompt
    if let Some(plan) = content.get("plan").and_then(|p| p.as_str()) {
        if !plan.is_empty() {
            return plan.to_string();
        }
    }
    if let Some(prompt) = content
        .get("payload")
        .and_then(|p| p.get("prompt"))
        .and_then(|p| p.as_str())
    {
        return prompt.to_string();
    }
    "(no prompt)".to_string()
}

#[allow(clippy::too_many_arguments)]
async fn cmd_history(
    homeserver: &str,
    token: &str,
    coordinator_room: &str,
    limit: usize,
    status_filter: &str,
    date: Option<&str>,
    since: Option<&str>,
    from_date: Option<&str>,
    to_date: Option<&str>,
) -> Result<()> {
    let http = reqwest::Client::new();
    let (time_start, time_end) = compute_time_window(date, since, from_date, to_date)?;

    // Collect task events and result events by paging backwards through the timeline
    let mut task_events: Vec<serde_json::Value> = Vec::new();
    let mut result_map: HashMap<String, serde_json::Value> = HashMap::new();
    let mut pagination_token: Option<String> = None;
    let mut total_events_seen: usize = 0;
    let max_events: usize = 500;

    loop {
        let (events, next_token) = matrix_get_messages(
            &http,
            homeserver,
            token,
            coordinator_room,
            100,
            pagination_token.as_deref(),
        )
        .await?;

        if events.is_empty() {
            break;
        }

        total_events_seen += events.len();

        for event in &events {
            let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");

            match event_type {
                "org.mxdx.fabric.task" => {
                    task_events.push(event.clone());
                }
                "org.mxdx.fabric.result" => {
                    if let Some(content) = event.get("content") {
                        if let Some(task_uuid) =
                            content.get("task_uuid").and_then(|u| u.as_str())
                        {
                            // Keep the first result we see (most recent, since we page backwards)
                            result_map
                                .entry(task_uuid.to_string())
                                .or_insert_with(|| event.clone());
                        }
                    }
                }
                _ => {}
            }
        }

        // Check if we should stop paging
        if total_events_seen >= max_events {
            break;
        }

        match next_token {
            Some(t) => pagination_token = Some(t),
            None => break,
        }
    }

    // Build HistoryTask entries
    let mut history_tasks: Vec<HistoryTask> = Vec::new();

    for event in &task_events {
        let content = match event.get("content") {
            Some(c) => c,
            None => continue,
        };

        let uuid = content
            .get("uuid")
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string();

        if uuid.is_empty() {
            continue;
        }

        let timestamp_ms = event
            .get("origin_server_ts")
            .and_then(|t| t.as_i64())
            .unwrap_or(0);

        // Apply time window filter
        if let Some(start) = time_start {
            if timestamp_ms < start {
                continue;
            }
        }
        if let Some(end) = time_end {
            if timestamp_ms > end {
                continue;
            }
        }

        let capabilities: Vec<String> = content
            .get("required_capabilities")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        let prompt = extract_prompt(content);

        // Look up result
        let (status, duration_seconds) = if let Some(result_event) = result_map.get(&uuid) {
            let result_content = result_event.get("content").unwrap_or(content);
            let status = result_content
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("unknown")
                .to_string();

            let duration = result_content
                .get("duration_seconds")
                .and_then(|d| d.as_u64());

            // If no explicit duration_seconds, compute from timestamps
            let duration = duration.or_else(|| {
                let result_ts = result_event
                    .get("origin_server_ts")
                    .and_then(|t| t.as_i64())
                    .unwrap_or(0);
                if result_ts > timestamp_ms {
                    Some(((result_ts - timestamp_ms) / 1000) as u64)
                } else {
                    None
                }
            });

            (status, duration)
        } else {
            ("pending".to_string(), None)
        };

        // Apply status filter
        if !status_matches_filter(&status, status_filter) {
            continue;
        }

        history_tasks.push(HistoryTask {
            timestamp_ms,
            uuid,
            capabilities,
            prompt,
            status,
            duration_seconds,
        });
    }

    // Sort reverse-chronological (most recent first)
    history_tasks.sort_by(|a, b| b.timestamp_ms.cmp(&a.timestamp_ms));

    // Truncate to limit
    history_tasks.truncate(limit);

    if history_tasks.is_empty() {
        println!("No tasks found.");
        return Ok(());
    }

    for task in &history_tasks {
        println!("{}", format_history_line(task));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_payload_without_extra_returns_base() {
        let base = serde_json::json!({"prompt": "do stuff"});
        let result = merge_payload(base.clone(), None).unwrap();
        assert_eq!(result, base);
    }

    #[test]
    fn merge_payload_adds_extra_fields() {
        let base = serde_json::json!({"prompt": "do stuff"});
        let extra = r#"{"cwd": "/tmp", "model": "gpt-4"}"#;
        let result = merge_payload(base, Some(extra)).unwrap();
        assert_eq!(result["prompt"], "do stuff");
        assert_eq!(result["cwd"], "/tmp");
        assert_eq!(result["model"], "gpt-4");
    }

    #[test]
    fn merge_payload_named_flag_wins_on_conflict() {
        let base = serde_json::json!({"prompt": "explicit-prompt", "cwd": "/explicit"});
        let extra = r#"{"prompt": "overridden", "cwd": "/overridden", "model": "gpt-4"}"#;
        let result = merge_payload(base, Some(extra)).unwrap();
        assert_eq!(result["prompt"], "explicit-prompt");
        assert_eq!(result["cwd"], "/explicit");
        assert_eq!(result["model"], "gpt-4");
    }

    #[test]
    fn merge_payload_rejects_non_object_json() {
        let base = serde_json::json!({"prompt": "do stuff"});
        let extra = r#""just a string""#;
        let result = merge_payload(base, Some(extra));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must be a JSON object"));
    }

    #[test]
    fn merge_payload_rejects_invalid_json() {
        let base = serde_json::json!({"prompt": "do stuff"});
        let extra = r#"not valid json at all"#;
        let result = merge_payload(base, Some(extra));
        assert!(result.is_err());
    }

    #[test]
    fn merge_payload_empty_object() {
        let base = serde_json::json!({"prompt": "do stuff"});
        let extra = r#"{}"#;
        let result = merge_payload(base, Some(extra)).unwrap();
        assert_eq!(result["prompt"], "do stuff");
        assert_eq!(result.as_object().unwrap().len(), 1);
    }

    #[test]
    fn format_history_line_basic() {
        // 2026-03-24T04:10:19Z = 1774339819000 ms
        let task = HistoryTask {
            timestamp_ms: 1774339819000,
            uuid: "e79b93f1-2345-6789-abcd-ef0123456789".to_string(),
            capabilities: vec!["rust".to_string(), "linux".to_string()],
            prompt: "summarize the repo structure".to_string(),
            status: "success".to_string(),
            duration_seconds: Some(120),
        };
        let line = format_history_line(&task);
        assert!(line.contains("success"));
        assert!(line.contains("e79b93f"));
        assert!(line.contains("rust,linux"));
        assert!(line.contains("\"summarize the repo structure\""));
        assert!(line.contains("(120s)"));
        // Verify timestamp format
        assert!(line.starts_with("2026-03-24T"));
    }

    #[test]
    fn format_history_line_no_duration() {
        let task = HistoryTask {
            timestamp_ms: 1774339819000,
            uuid: "abcdefg-1234".to_string(),
            capabilities: vec!["rust".to_string()],
            prompt: "do something".to_string(),
            status: "pending".to_string(),
            duration_seconds: None,
        };
        let line = format_history_line(&task);
        assert!(line.contains("pending"));
        assert!(line.contains("abcdefg"));
        assert!(!line.contains("("));
    }

    #[test]
    fn format_history_line_truncates_long_prompt() {
        let long_prompt = "a".repeat(80);
        let task = HistoryTask {
            timestamp_ms: 1774339819000,
            uuid: "1234567890".to_string(),
            capabilities: vec![],
            prompt: long_prompt,
            status: "failed".to_string(),
            duration_seconds: Some(10),
        };
        let line = format_history_line(&task);
        // Should truncate to 57 chars + "..."
        assert!(line.contains("...\""));
        // Capabilities should show "-" when empty
        assert!(line.contains("  -  "));
    }

    #[test]
    fn format_history_line_prompt_exactly_60_chars() {
        let prompt = "b".repeat(60);
        let task = HistoryTask {
            timestamp_ms: 1774339819000,
            uuid: "1234567890".to_string(),
            capabilities: vec!["linux".to_string()],
            prompt: prompt.clone(),
            status: "success".to_string(),
            duration_seconds: Some(5),
        };
        let line = format_history_line(&task);
        // Exactly 60 chars should NOT be truncated
        assert!(line.contains(&format!("\"{}\"", prompt)));
        assert!(!line.contains("..."));
    }

    #[test]
    fn status_matches_filter_any() {
        assert!(status_matches_filter("success", "any"));
        assert!(status_matches_filter("failed", "any"));
        assert!(status_matches_filter("pending", "any"));
        assert!(status_matches_filter("timeout", "any"));
        assert!(status_matches_filter("cancelled", "any"));
        assert!(status_matches_filter("unknown", "any"));
    }

    #[test]
    fn status_matches_filter_done() {
        assert!(status_matches_filter("success", "done"));
        assert!(status_matches_filter("failed", "done"));
        assert!(status_matches_filter("timeout", "done"));
        assert!(status_matches_filter("cancelled", "done"));
        assert!(!status_matches_filter("pending", "done"));
        assert!(!status_matches_filter("unknown", "done"));
    }

    #[test]
    fn status_matches_filter_specific() {
        assert!(status_matches_filter("success", "success"));
        assert!(!status_matches_filter("failed", "success"));
        assert!(status_matches_filter("failed", "failed"));
        assert!(!status_matches_filter("success", "failed"));
        assert!(status_matches_filter("timeout", "timeout"));
    }

    #[test]
    fn parse_since_period_valid() {
        let h = parse_since_period("1h").unwrap();
        assert_eq!(h.num_hours(), 1);
        let d = parse_since_period("7d").unwrap();
        assert_eq!(d.num_days(), 7);
        let w = parse_since_period("2w").unwrap();
        assert_eq!(w.num_weeks(), 2);
        let m = parse_since_period("1m").unwrap();
        assert_eq!(m.num_days(), 30);
    }

    #[test]
    fn parse_since_period_invalid() {
        assert!(parse_since_period("x").is_err());
        assert!(parse_since_period("1x").is_err());
        assert!(parse_since_period("").is_err());
    }

    #[test]
    fn compute_time_window_date() {
        let (start, end) = compute_time_window(Some("2026-03-24"), None, None, None).unwrap();
        assert!(start.is_some());
        assert!(end.is_some());
        // start should be midnight UTC
        let start_dt =
            chrono::DateTime::from_timestamp_millis(start.unwrap()).unwrap();
        assert_eq!(start_dt.format("%Y-%m-%d").to_string(), "2026-03-24");
    }

    #[test]
    fn compute_time_window_none() {
        let (start, end) = compute_time_window(None, None, None, None).unwrap();
        assert!(start.is_none());
        assert!(end.is_none());
    }

    #[test]
    fn extract_prompt_from_plan() {
        let content = serde_json::json!({
            "plan": "write some code",
            "payload": {"prompt": "fallback prompt"}
        });
        assert_eq!(extract_prompt(&content), "write some code");
    }

    #[test]
    fn extract_prompt_from_payload() {
        let content = serde_json::json!({
            "payload": {"prompt": "the actual prompt"}
        });
        assert_eq!(extract_prompt(&content), "the actual prompt");
    }

    #[test]
    fn extract_prompt_missing() {
        let content = serde_json::json!({"uuid": "abc"});
        assert_eq!(extract_prompt(&content), "(no prompt)");
    }

    #[test]
    fn format_post_output_contains_both_ids() {
        let output = format_post_output(
            "550e8400-e29b-41d4-a716-446655440000",
            "$abc123:ca1-beta.mxdx.dev",
        );
        assert!(output.contains("task_uuid: 550e8400-e29b-41d4-a716-446655440000"));
        assert!(output.contains("event_id: $abc123:ca1-beta.mxdx.dev"));

        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("task_uuid: "));
        assert!(lines[1].starts_with("event_id: "));
    }
}
