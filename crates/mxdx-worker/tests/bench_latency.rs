//! Latency and throughput profiling benchmarks for mxdx E2E operations.
//!
//! These benchmarks measure real-world performance of Matrix operations through
//! encrypted rooms on local Tuwunel instances and (optionally) beta servers.
//!
//! All benchmarks are `#[ignore]` — run with:
//!   cargo test -p mxdx-worker --test bench_latency -- --ignored --nocapture
//!
//! Results are written to `docs/benchmarks/*.json` for historical comparison.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use chrono::Utc;
use mxdx_matrix::{MatrixClient, OwnedRoomId};
use mxdx_test_helpers::tuwunel::TuwunelInstance;
use mxdx_types::events::session::{
    ActiveSessionState, CompletedSessionState, OutputStream, SessionHeartbeat, SessionOutput,
    SessionResult, SessionStart, SessionStatus, SessionTask, SESSION_HEARTBEAT, SESSION_OUTPUT,
    SESSION_RESULT, SESSION_START, SESSION_TASK,
};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Benchmark data types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct BenchmarkReport {
    timestamp: String,
    server: String,
    federated: bool,
    git_sha: String,
    metrics: Vec<Metric>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Metric {
    name: String,
    operation: String,
    latency_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    throughput_ops_per_sec: Option<f64>,
    samples: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    p50_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    p95_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    p99_ms: Option<f64>,
}

// ---------------------------------------------------------------------------
// Measurement helpers
// ---------------------------------------------------------------------------

async fn measure_async<F, R>(f: F) -> (R, f64)
where
    F: std::future::Future<Output = R>,
{
    let start = Instant::now();
    let result = f.await;
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    (result, ms)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64) * p / 100.0).ceil() as usize;
    sorted[idx.saturating_sub(1).min(sorted.len() - 1)]
}

fn compute_stats(mut samples: Vec<f64>) -> (f64, f64, f64, f64, f64, f64) {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let min = samples.first().copied().unwrap_or(0.0);
    let max = samples.last().copied().unwrap_or(0.0);
    let avg = samples.iter().sum::<f64>() / samples.len().max(1) as f64;
    let p50 = percentile(&samples, 50.0);
    let p95 = percentile(&samples, 95.0);
    let p99 = percentile(&samples, 99.0);
    (min, max, avg, p50, p95, p99)
}

fn metric_from_samples(name: &str, operation: &str, samples: Vec<f64>) -> Metric {
    let (min, max, avg, p50, p95, p99) = compute_stats(samples.clone());
    Metric {
        name: name.to_string(),
        operation: operation.to_string(),
        latency_ms: avg,
        throughput_ops_per_sec: None,
        samples: samples.len() as u32,
        min_ms: Some(min),
        max_ms: Some(max),
        p50_ms: Some(p50),
        p95_ms: Some(p95),
        p99_ms: Some(p99),
    }
}

fn metric_single(name: &str, operation: &str, ms: f64) -> Metric {
    Metric {
        name: name.to_string(),
        operation: operation.to_string(),
        latency_ms: ms,
        throughput_ops_per_sec: None,
        samples: 1,
        min_ms: Some(ms),
        max_ms: Some(ms),
        p50_ms: Some(ms),
        p95_ms: Some(ms),
        p99_ms: Some(ms),
    }
}

fn git_sha() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn write_report(report: &BenchmarkReport, prefix: &str) {
    let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let benchmarks_dir = std::path::Path::new(manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("docs")
        .join("benchmarks");
    std::fs::create_dir_all(&benchmarks_dir).expect("Failed to create benchmarks directory");

    let path = benchmarks_dir.join(format!("{prefix}-{timestamp}.json"));
    let json = serde_json::to_string_pretty(report).expect("Failed to serialize report");
    std::fs::write(&path, &json).expect("Failed to write benchmark report");
    eprintln!("Benchmark report written to: {}", path.display());
}

fn make_session_task(uuid: &str, sender_id: &str) -> SessionTask {
    SessionTask {
        uuid: uuid.into(),
        sender_id: sender_id.into(),
        bin: "echo".into(),
        args: vec!["hello".into(), "world".into()],
        env: None,
        cwd: None,
        interactive: false,
        no_room_output: false,
        timeout_seconds: Some(60),
        heartbeat_interval_seconds: 30,
        plan: None,
        required_capabilities: vec![],
        routing_mode: None,
        on_timeout: None,
        on_heartbeat_miss: None,
    }
}

// ---------------------------------------------------------------------------
// Setup helper: register two users, create encrypted room, exchange keys
// ---------------------------------------------------------------------------

async fn setup_encrypted_pair(
    base_url: &str,
) -> (MatrixClient, MatrixClient, OwnedRoomId, f64, f64, f64) {
    // Measure registration + login
    let (client_mc, login_time_1) = measure_async(MatrixClient::register_and_connect(
        base_url,
        "bench-client",
        "pass123",
        "mxdx-test-token",
    ))
    .await;
    let client_mc = client_mc.unwrap();

    let (worker_mc, login_time_2) = measure_async(MatrixClient::register_and_connect(
        base_url,
        "bench-worker",
        "pass123",
        "mxdx-test-token",
    ))
    .await;
    let worker_mc = worker_mc.unwrap();

    let login_time = (login_time_1 + login_time_2) / 2.0;

    // Measure room creation
    let (room_id, room_creation_time) = measure_async(
        client_mc.create_encrypted_room(&[worker_mc.user_id().to_owned()]),
    )
    .await;
    let room_id = room_id.unwrap();

    worker_mc.sync_once().await.unwrap();
    worker_mc.join_room(&room_id).await.unwrap();

    // Set power levels
    let power_levels = serde_json::json!({
        "users": {
            client_mc.user_id().to_string(): 100,
            worker_mc.user_id().to_string(): 50
        },
        "users_default": 0,
        "events_default": 0,
        "state_default": 50,
        "ban": 50, "kick": 50, "invite": 50, "redact": 50
    });
    client_mc
        .send_state_event(&room_id, "m.room.power_levels", "", power_levels)
        .await
        .unwrap();

    // Measure E2EE key exchange (4 sync rounds)
    let key_exchange_start = Instant::now();
    for _ in 0..4 {
        client_mc.sync_once().await.unwrap();
        worker_mc.sync_once().await.unwrap();
    }
    let key_exchange_time = key_exchange_start.elapsed().as_secs_f64() * 1000.0;

    (
        client_mc,
        worker_mc,
        room_id,
        login_time,
        room_creation_time,
        key_exchange_time,
    )
}

// ---------------------------------------------------------------------------
// Test 1: Local (TuwunelInstance) latency benchmark
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "benchmark -- run with --ignored"]
async fn bench_local_latency() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);

    let mut metrics: Vec<Metric> = Vec::new();

    // --- Setup: register users, create room, exchange keys ---
    let (client_mc, worker_mc, room_id, login_time, room_creation_time, key_exchange_time) =
        setup_encrypted_pair(&base_url).await;

    metrics.push(metric_single(
        "login",
        "MatrixClient::register_and_connect",
        login_time,
    ));
    metrics.push(metric_single(
        "room_creation",
        "MatrixClient::create_encrypted_room",
        room_creation_time,
    ));
    metrics.push(metric_single(
        "e2ee_key_exchange",
        "4x sync_once round-trip",
        key_exchange_time,
    ));

    // --- Event send latency (5 samples) ---
    let mut send_times = Vec::new();
    for i in 0..5 {
        let task = make_session_task(
            &format!("bench-send-{i}"),
            &client_mc.user_id().to_string(),
        );
        let payload = serde_json::json!({
            "type": SESSION_TASK,
            "content": serde_json::to_value(&task).unwrap(),
        });
        let (_, ms) = measure_async(client_mc.send_event(&room_id, payload)).await;
        send_times.push(ms);
    }
    metrics.push(metric_from_samples(
        "event_send",
        "send_event (session task)",
        send_times,
    ));

    // --- Event delivery latency (5 samples: send -> sync_and_collect) ---
    let mut delivery_times = Vec::new();
    for i in 0..5 {
        let task = make_session_task(
            &format!("bench-delivery-{i}"),
            &client_mc.user_id().to_string(),
        );
        let payload = serde_json::json!({
            "type": SESSION_TASK,
            "content": serde_json::to_value(&task).unwrap(),
        });
        client_mc
            .send_event(&room_id, payload)
            .await
            .unwrap();

        let uuid_str = format!("bench-delivery-{i}");
        let start = Instant::now();
        // Worker syncs until it sees the event
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            worker_mc.sync_once().await.unwrap();
            let events = worker_mc
                .sync_and_collect_events(&room_id, Duration::from_secs(2))
                .await
                .unwrap();
            let found = events.iter().any(|e| {
                e.get("content")
                    .and_then(|c| c.get("uuid"))
                    .and_then(|u| u.as_str())
                    == Some(&uuid_str)
            });
            if found {
                break;
            }
            if tokio::time::Instant::now() > deadline {
                panic!("Timed out waiting for delivery of {uuid_str}");
            }
        }
        delivery_times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    metrics.push(metric_from_samples(
        "event_delivery",
        "send -> sync_and_collect",
        delivery_times,
    ));

    // --- Threaded event send latency (5 samples) ---
    let root_task = make_session_task("bench-thread-root", &client_mc.user_id().to_string());
    let root_event_id = client_mc
        .send_event(
            &room_id,
            serde_json::json!({
                "type": SESSION_TASK,
                "content": serde_json::to_value(&root_task).unwrap(),
            }),
        )
        .await
        .unwrap();

    let mut threaded_times = Vec::new();
    for i in 0..5 {
        let output = SessionOutput {
            session_uuid: "bench-thread-root".into(),
            worker_id: worker_mc.user_id().to_string(),
            stream: OutputStream::Stdout,
            data: base64::engine::general_purpose::STANDARD
                .encode(format!("output line {i}\n").as_bytes()),
            seq: i,
            timestamp: now_secs(),
        };
        let (_, ms) = measure_async(worker_mc.send_threaded_event(
            &room_id,
            SESSION_OUTPUT,
            &root_event_id,
            serde_json::to_value(&output).unwrap(),
        ))
        .await;
        threaded_times.push(ms);
    }
    metrics.push(metric_from_samples(
        "threaded_event_send",
        "send_threaded_event (session output)",
        threaded_times,
    ));

    // --- State event write/read roundtrip (5 samples) ---
    let mut state_roundtrip_times = Vec::new();
    for i in 0..5 {
        let active_state = ActiveSessionState {
            bin: "echo".into(),
            args: vec!["hello".into()],
            pid: Some(10000 + i as u32),
            start_time: now_secs(),
            client_id: client_mc.user_id().to_string(),
            interactive: false,
            worker_id: worker_mc.user_id().to_string(),
        };
        let state_key = format!("session/bench-state-{i}/active");
        let start = Instant::now();
        worker_mc
            .send_state_event(
                &room_id,
                "org.mxdx.session.active",
                &state_key,
                serde_json::to_value(&active_state).unwrap(),
            )
            .await
            .unwrap();
        // Read it back
        client_mc.sync_once().await.unwrap();
        let read_back = client_mc
            .get_room_state_event(&room_id, "org.mxdx.session.active", &state_key)
            .await
            .unwrap();
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(read_back["pid"], 10000 + i as u64);
        state_roundtrip_times.push(ms);
    }
    metrics.push(metric_from_samples(
        "state_event_roundtrip",
        "send_state_event + sync + get_room_state_event",
        state_roundtrip_times,
    ));

    // --- Full session lifecycle (3 samples) ---
    let mut lifecycle_times = Vec::new();
    for i in 0..3 {
        let uuid = format!("bench-lifecycle-{i}");
        let start = Instant::now();

        // Client sends task
        let task = make_session_task(&uuid, &client_mc.user_id().to_string());
        let task_event_id = client_mc
            .send_event(
                &room_id,
                serde_json::json!({
                    "type": SESSION_TASK,
                    "content": serde_json::to_value(&task).unwrap(),
                }),
            )
            .await
            .unwrap();

        // Worker syncs and posts start
        worker_mc.sync_once().await.unwrap();
        let session_start = SessionStart {
            session_uuid: uuid.clone(),
            worker_id: worker_mc.user_id().to_string(),
            tmux_session: Some(format!("mxdx-{uuid}")),
            pid: Some(20000 + i as u32),
            started_at: now_secs(),
            dm_room_id: None,
        };
        worker_mc
            .send_threaded_event(
                &room_id,
                SESSION_START,
                &task_event_id,
                serde_json::to_value(&session_start).unwrap(),
            )
            .await
            .unwrap();

        // Worker posts output
        let output = SessionOutput {
            session_uuid: uuid.clone(),
            worker_id: worker_mc.user_id().to_string(),
            stream: OutputStream::Stdout,
            data: base64::engine::general_purpose::STANDARD.encode(b"hello world\n"),
            seq: 0,
            timestamp: now_secs(),
        };
        worker_mc
            .send_threaded_event(
                &room_id,
                SESSION_OUTPUT,
                &task_event_id,
                serde_json::to_value(&output).unwrap(),
            )
            .await
            .unwrap();

        // Worker posts heartbeat
        let heartbeat = SessionHeartbeat {
            session_uuid: uuid.clone(),
            worker_id: worker_mc.user_id().to_string(),
            timestamp: now_secs(),
            progress: Some("running".into()),
        };
        worker_mc
            .send_threaded_event(
                &room_id,
                SESSION_HEARTBEAT,
                &task_event_id,
                serde_json::to_value(&heartbeat).unwrap(),
            )
            .await
            .unwrap();

        // Worker posts result
        let result = SessionResult {
            session_uuid: uuid.clone(),
            worker_id: worker_mc.user_id().to_string(),
            status: SessionStatus::Success,
            exit_code: Some(0),
            duration_seconds: 1,
            tail: Some("hello world\n".into()),
        };
        worker_mc
            .send_threaded_event(
                &room_id,
                SESSION_RESULT,
                &task_event_id,
                serde_json::to_value(&result).unwrap(),
            )
            .await
            .unwrap();

        // Worker writes completed state
        let completed_state = CompletedSessionState {
            exit_code: Some(0),
            duration_seconds: 1,
            completion_time: now_secs(),
        };
        worker_mc
            .send_state_event(
                &room_id,
                "org.mxdx.session.completed",
                &format!("session/{uuid}/completed"),
                serde_json::to_value(&completed_state).unwrap(),
            )
            .await
            .unwrap();

        // Client syncs and verifies result
        client_mc.sync_once().await.unwrap();
        let events = client_mc
            .sync_and_collect_events(&room_id, Duration::from_secs(5))
            .await
            .unwrap();
        let found_result = events.iter().any(|e| {
            e.get("content")
                .and_then(|c| c.get("status"))
                .and_then(|s| s.as_str())
                == Some("success")
        });
        assert!(found_result, "Client should see result for {uuid}");

        lifecycle_times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    metrics.push(metric_from_samples(
        "full_session_lifecycle",
        "task -> start -> output -> heartbeat -> result",
        lifecycle_times,
    ));

    // --- Burst throughput: send 20 events as fast as possible ---
    let burst_start = Instant::now();
    let burst_count = 20u32;
    for i in 0..burst_count {
        let task = make_session_task(
            &format!("bench-burst-{i}"),
            &client_mc.user_id().to_string(),
        );
        client_mc
            .send_event(
                &room_id,
                serde_json::json!({
                    "type": SESSION_TASK,
                    "content": serde_json::to_value(&task).unwrap(),
                }),
            )
            .await
            .unwrap();
    }
    let burst_total_ms = burst_start.elapsed().as_secs_f64() * 1000.0;
    let burst_ops_per_sec = burst_count as f64 / (burst_total_ms / 1000.0);
    metrics.push(Metric {
        name: "burst_throughput".to_string(),
        operation: format!("{burst_count} events sequential"),
        latency_ms: burst_total_ms,
        throughput_ops_per_sec: Some(burst_ops_per_sec),
        samples: burst_count,
        min_ms: None,
        max_ms: None,
        p50_ms: None,
        p95_ms: None,
        p99_ms: None,
    });

    // --- Sustained throughput: send events with sync between each ---
    let sustained_start = Instant::now();
    let sustained_count = 10u32;
    for i in 0..sustained_count {
        let task = make_session_task(
            &format!("bench-sustained-{i}"),
            &client_mc.user_id().to_string(),
        );
        client_mc
            .send_event(
                &room_id,
                serde_json::json!({
                    "type": SESSION_TASK,
                    "content": serde_json::to_value(&task).unwrap(),
                }),
            )
            .await
            .unwrap();
        client_mc.sync_once().await.unwrap();
    }
    let sustained_total_ms = sustained_start.elapsed().as_secs_f64() * 1000.0;
    let sustained_ops_per_sec = sustained_count as f64 / (sustained_total_ms / 1000.0);
    metrics.push(Metric {
        name: "sustained_throughput".to_string(),
        operation: format!("{sustained_count} events with sync between sends"),
        latency_ms: sustained_total_ms,
        throughput_ops_per_sec: Some(sustained_ops_per_sec),
        samples: sustained_count,
        min_ms: None,
        max_ms: None,
        p50_ms: None,
        p95_ms: None,
        p99_ms: None,
    });

    // --- Write report ---
    let report = BenchmarkReport {
        timestamp: Utc::now().to_rfc3339(),
        server: "tuwunel-local".to_string(),
        federated: false,
        git_sha: git_sha(),
        metrics,
    };
    write_report(&report, "local");

    // Print summary
    eprintln!("\n=== Local Benchmark Summary ===");
    for m in &report.metrics {
        if let Some(tps) = m.throughput_ops_per_sec {
            eprintln!(
                "  {:<30} {:>10.1}ms  ({:.1} ops/sec, {} samples)",
                m.name, m.latency_ms, tps, m.samples
            );
        } else {
            eprintln!(
                "  {:<30} {:>10.1}ms  (p50={:.1}, p95={:.1}, {} samples)",
                m.name,
                m.latency_ms,
                m.p50_ms.unwrap_or(0.0),
                m.p95_ms.unwrap_or(0.0),
                m.samples
            );
        }
    }

    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 2: Beta server single-server latency benchmark
// ---------------------------------------------------------------------------

struct TestCredentials {
    server_url: String,
    server2_url: Option<String>,
    username1: String,
    password1: String,
    username2: String,
    password2: String,
}

fn load_test_credentials() -> Option<TestCredentials> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?
        .join("test-credentials.toml");

    if !path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&path).ok()?;
    let config: toml::Value = content.parse().ok()?;

    Some(TestCredentials {
        server_url: config["server"]["url"].as_str()?.to_string(),
        server2_url: config
            .get("server2")
            .and_then(|s| s.get("url"))
            .and_then(|u| u.as_str())
            .map(|s| s.to_string()),
        username1: config["account1"]["username"].as_str()?.to_string(),
        password1: config["account1"]["password"].as_str()?.to_string(),
        username2: config["account2"]["username"].as_str()?.to_string(),
        password2: config["account2"]["password"].as_str()?.to_string(),
    })
}

#[tokio::test]
#[ignore = "benchmark -- requires test-credentials.toml"]
async fn bench_beta_single_server_latency() {
    let creds = match load_test_credentials() {
        Some(c) => c,
        None => {
            eprintln!("Skipping: test-credentials.toml not found");
            return;
        }
    };

    let mut metrics: Vec<Metric> = Vec::new();

    // Login both accounts on same server
    let (client_mc, login_time_1) = measure_async(MatrixClient::login_and_connect(
        &creds.server_url,
        &creds.username1,
        &creds.password1,
    ))
    .await;
    let client_mc = client_mc.unwrap();

    let (worker_mc, login_time_2) = measure_async(MatrixClient::login_and_connect(
        &creds.server_url,
        &creds.username2,
        &creds.password2,
    ))
    .await;
    let worker_mc = worker_mc.unwrap();

    let login_time = (login_time_1 + login_time_2) / 2.0;
    metrics.push(metric_single(
        "login",
        "MatrixClient::login_and_connect",
        login_time,
    ));

    // Create encrypted room
    let (room_id, room_creation_time) = measure_async(
        client_mc.create_encrypted_room(&[worker_mc.user_id().to_owned()]),
    )
    .await;
    let room_id = room_id.unwrap();
    metrics.push(metric_single(
        "room_creation",
        "MatrixClient::create_encrypted_room",
        room_creation_time,
    ));

    worker_mc.sync_once().await.unwrap();
    worker_mc.join_room(&room_id).await.unwrap();

    // Power levels
    let power_levels = serde_json::json!({
        "users": {
            client_mc.user_id().to_string(): 100,
            worker_mc.user_id().to_string(): 50
        },
        "users_default": 0, "events_default": 0,
        "state_default": 50, "ban": 50, "kick": 50, "invite": 50, "redact": 50
    });
    client_mc
        .send_state_event(&room_id, "m.room.power_levels", "", power_levels)
        .await
        .unwrap();

    // Key exchange
    let key_exchange_start = Instant::now();
    for _ in 0..4 {
        client_mc.sync_once().await.unwrap();
        worker_mc.sync_once().await.unwrap();
    }
    let key_exchange_time = key_exchange_start.elapsed().as_secs_f64() * 1000.0;
    metrics.push(metric_single(
        "e2ee_key_exchange",
        "4x sync_once round-trip",
        key_exchange_time,
    ));

    // Event send latency (5 samples, with rate-limit delay)
    let mut send_times = Vec::new();
    for i in 0..5 {
        let task = make_session_task(
            &format!("bench-beta-send-{i}"),
            &client_mc.user_id().to_string(),
        );
        let payload = serde_json::json!({
            "type": SESSION_TASK,
            "content": serde_json::to_value(&task).unwrap(),
        });
        let (_, ms) = measure_async(client_mc.send_event(&room_id, payload)).await;
        send_times.push(ms);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    metrics.push(metric_from_samples(
        "event_send",
        "send_event (session task)",
        send_times,
    ));

    // State event roundtrip (3 samples)
    let mut state_times = Vec::new();
    for i in 0..3 {
        let active_state = ActiveSessionState {
            bin: "echo".into(),
            args: vec!["hello".into()],
            pid: Some(30000 + i as u32),
            start_time: now_secs(),
            client_id: client_mc.user_id().to_string(),
            interactive: false,
            worker_id: worker_mc.user_id().to_string(),
        };
        let state_key = format!("session/bench-beta-state-{i}/active");
        let start = Instant::now();
        worker_mc
            .send_state_event(
                &room_id,
                "org.mxdx.session.active",
                &state_key,
                serde_json::to_value(&active_state).unwrap(),
            )
            .await
            .unwrap();
        client_mc.sync_once().await.unwrap();
        let _read_back = client_mc
            .get_room_state_event(&room_id, "org.mxdx.session.active", &state_key)
            .await
            .unwrap();
        state_times.push(start.elapsed().as_secs_f64() * 1000.0);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    metrics.push(metric_from_samples(
        "state_event_roundtrip",
        "send_state_event + sync + get_room_state_event",
        state_times,
    ));

    // Burst throughput (10 events -- lower count for rate-limited server)
    let burst_start = Instant::now();
    let burst_count = 10u32;
    for i in 0..burst_count {
        let task = make_session_task(
            &format!("bench-beta-burst-{i}"),
            &client_mc.user_id().to_string(),
        );
        client_mc
            .send_event(
                &room_id,
                serde_json::json!({
                    "type": SESSION_TASK,
                    "content": serde_json::to_value(&task).unwrap(),
                }),
            )
            .await
            .unwrap();
    }
    let burst_total_ms = burst_start.elapsed().as_secs_f64() * 1000.0;
    let burst_ops_per_sec = burst_count as f64 / (burst_total_ms / 1000.0);
    metrics.push(Metric {
        name: "burst_throughput".to_string(),
        operation: format!("{burst_count} events sequential"),
        latency_ms: burst_total_ms,
        throughput_ops_per_sec: Some(burst_ops_per_sec),
        samples: burst_count,
        min_ms: None,
        max_ms: None,
        p50_ms: None,
        p95_ms: None,
        p99_ms: None,
    });

    let report = BenchmarkReport {
        timestamp: Utc::now().to_rfc3339(),
        server: creds.server_url.clone(),
        federated: false,
        git_sha: git_sha(),
        metrics,
    };
    write_report(&report, "beta-single");

    eprintln!("\n=== Beta Single-Server Benchmark Summary ===");
    for m in &report.metrics {
        if let Some(tps) = m.throughput_ops_per_sec {
            eprintln!(
                "  {:<30} {:>10.1}ms  ({:.1} ops/sec, {} samples)",
                m.name, m.latency_ms, tps, m.samples
            );
        } else {
            eprintln!(
                "  {:<30} {:>10.1}ms  (p50={:.1}, p95={:.1}, {} samples)",
                m.name,
                m.latency_ms,
                m.p50_ms.unwrap_or(0.0),
                m.p95_ms.unwrap_or(0.0),
                m.samples
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test 3: Beta server federated latency benchmark
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "benchmark -- requires test-credentials.toml with server2"]
async fn bench_beta_federated_latency() {
    let creds = match load_test_credentials() {
        Some(c) if c.server2_url.is_some() => c,
        _ => {
            eprintln!("Skipping: test-credentials.toml not found or missing server2");
            return;
        }
    };

    let server2_url = creds.server2_url.as_ref().unwrap();
    let mut metrics: Vec<Metric> = Vec::new();

    // Client on server1, worker on server2
    let (client_mc, login_time_1) = measure_async(MatrixClient::login_and_connect(
        &creds.server_url,
        &creds.username1,
        &creds.password1,
    ))
    .await;
    let client_mc = client_mc.unwrap();

    let (worker_mc, login_time_2) = measure_async(MatrixClient::login_and_connect(
        server2_url,
        &creds.username2,
        &creds.password2,
    ))
    .await;
    let worker_mc = worker_mc.unwrap();

    let login_time = (login_time_1 + login_time_2) / 2.0;
    metrics.push(metric_single(
        "login_federated",
        "MatrixClient::login_and_connect (cross-server)",
        login_time,
    ));

    // Create room on server1, invite user from server2
    let (room_id, room_creation_time) = measure_async(
        client_mc.create_encrypted_room(&[worker_mc.user_id().to_owned()]),
    )
    .await;
    let room_id = room_id.unwrap();
    metrics.push(metric_single(
        "room_creation_federated",
        "create_encrypted_room (cross-server invite)",
        room_creation_time,
    ));

    // Wait for federation invite delivery
    tokio::time::sleep(Duration::from_secs(2)).await;
    worker_mc.sync_once().await.unwrap();
    worker_mc.join_room(&room_id).await.unwrap();

    // Power levels
    let power_levels = serde_json::json!({
        "users": {
            client_mc.user_id().to_string(): 100,
            worker_mc.user_id().to_string(): 50
        },
        "users_default": 0, "events_default": 0,
        "state_default": 50, "ban": 50, "kick": 50, "invite": 50, "redact": 50
    });
    client_mc
        .send_state_event(&room_id, "m.room.power_levels", "", power_levels)
        .await
        .unwrap();

    // Key exchange (federated -- may need more rounds)
    let key_exchange_start = Instant::now();
    for _ in 0..6 {
        client_mc.sync_once().await.unwrap();
        worker_mc.sync_once().await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let key_exchange_time = key_exchange_start.elapsed().as_secs_f64() * 1000.0;
    metrics.push(metric_single(
        "e2ee_key_exchange_federated",
        "6x sync_once round-trip (cross-server)",
        key_exchange_time,
    ));

    // Event send latency (federated, 5 samples)
    let mut send_times = Vec::new();
    for i in 0..5 {
        let task = make_session_task(
            &format!("bench-fed-send-{i}"),
            &client_mc.user_id().to_string(),
        );
        let payload = serde_json::json!({
            "type": SESSION_TASK,
            "content": serde_json::to_value(&task).unwrap(),
        });
        let (_, ms) = measure_async(client_mc.send_event(&room_id, payload)).await;
        send_times.push(ms);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    metrics.push(metric_from_samples(
        "event_send_federated",
        "send_event (session task, cross-server)",
        send_times,
    ));

    // Event delivery latency (federated, 3 samples)
    let mut delivery_times = Vec::new();
    for i in 0..3 {
        let task = make_session_task(
            &format!("bench-fed-delivery-{i}"),
            &client_mc.user_id().to_string(),
        );
        let payload = serde_json::json!({
            "type": SESSION_TASK,
            "content": serde_json::to_value(&task).unwrap(),
        });
        client_mc.send_event(&room_id, payload).await.unwrap();

        let uuid_str = format!("bench-fed-delivery-{i}");
        let start = Instant::now();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        let mut found = false;
        while tokio::time::Instant::now() < deadline {
            worker_mc.sync_once().await.ok();
            let events = worker_mc
                .sync_and_collect_events(&room_id, Duration::from_secs(5))
                .await
                .unwrap_or_default();
            if events.iter().any(|e| {
                e.get("content")
                    .and_then(|c| c.get("uuid"))
                    .and_then(|u| u.as_str())
                    == Some(&uuid_str)
            }) {
                found = true;
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        if found {
            delivery_times.push(start.elapsed().as_secs_f64() * 1000.0);
        } else {
            eprintln!("  WARN: Timed out waiting for federated delivery of {uuid_str}, skipping sample");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    metrics.push(metric_from_samples(
        "event_delivery_federated",
        "send -> sync_and_collect (cross-server)",
        delivery_times,
    ));

    // State event roundtrip (federated, 3 samples)
    let mut state_times = Vec::new();
    for i in 0..3 {
        let active_state = ActiveSessionState {
            bin: "echo".into(),
            args: vec!["hello".into()],
            pid: Some(40000 + i as u32),
            start_time: now_secs(),
            client_id: client_mc.user_id().to_string(),
            interactive: false,
            worker_id: worker_mc.user_id().to_string(),
        };
        let state_key = format!("session/bench-fed-state-{i}/active");
        let start = Instant::now();
        worker_mc
            .send_state_event(
                &room_id,
                "org.mxdx.session.active",
                &state_key,
                serde_json::to_value(&active_state).unwrap(),
            )
            .await
            .unwrap();
        // Federation needs extra sync delay
        tokio::time::sleep(Duration::from_secs(1)).await;
        client_mc.sync_once().await.unwrap();
        let _read_back = client_mc
            .get_room_state_event(&room_id, "org.mxdx.session.active", &state_key)
            .await
            .unwrap();
        state_times.push(start.elapsed().as_secs_f64() * 1000.0);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    metrics.push(metric_from_samples(
        "state_event_roundtrip_federated",
        "send_state_event + sync + get_room_state_event (cross-server)",
        state_times,
    ));

    // Full session lifecycle (federated, 2 samples)
    let mut lifecycle_times = Vec::new();
    for i in 0..2 {
        let uuid = format!("bench-fed-lifecycle-{i}");
        let start = Instant::now();

        let task = make_session_task(&uuid, &client_mc.user_id().to_string());
        let task_event_id = client_mc
            .send_event(
                &room_id,
                serde_json::json!({
                    "type": SESSION_TASK,
                    "content": serde_json::to_value(&task).unwrap(),
                }),
            )
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(1)).await;
        worker_mc.sync_once().await.unwrap();

        let session_start = SessionStart {
            session_uuid: uuid.clone(),
            worker_id: worker_mc.user_id().to_string(),
            tmux_session: Some(format!("mxdx-{uuid}")),
            pid: Some(50000 + i as u32),
            started_at: now_secs(),
            dm_room_id: None,
        };
        worker_mc
            .send_threaded_event(
                &room_id,
                SESSION_START,
                &task_event_id,
                serde_json::to_value(&session_start).unwrap(),
            )
            .await
            .unwrap();

        let result = SessionResult {
            session_uuid: uuid.clone(),
            worker_id: worker_mc.user_id().to_string(),
            status: SessionStatus::Success,
            exit_code: Some(0),
            duration_seconds: 1,
            tail: Some("hello world\n".into()),
        };
        worker_mc
            .send_threaded_event(
                &room_id,
                SESSION_RESULT,
                &task_event_id,
                serde_json::to_value(&result).unwrap(),
            )
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_secs(1)).await;
        client_mc.sync_once().await.unwrap();
        let events = client_mc
            .sync_and_collect_events(&room_id, Duration::from_secs(5))
            .await
            .unwrap();
        let found_result = events.iter().any(|e| {
            e.get("content")
                .and_then(|c| c.get("status"))
                .and_then(|s| s.as_str())
                == Some("success")
        });
        assert!(found_result, "Client should see federated result for {uuid}");

        lifecycle_times.push(start.elapsed().as_secs_f64() * 1000.0);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    metrics.push(metric_from_samples(
        "full_session_lifecycle_federated",
        "task -> start -> result (cross-server)",
        lifecycle_times,
    ));

    let report = BenchmarkReport {
        timestamp: Utc::now().to_rfc3339(),
        server: format!("{} + {}", creds.server_url, server2_url),
        federated: true,
        git_sha: git_sha(),
        metrics,
    };
    write_report(&report, "beta-federated");

    eprintln!("\n=== Beta Federated Benchmark Summary ===");
    for m in &report.metrics {
        if let Some(tps) = m.throughput_ops_per_sec {
            eprintln!(
                "  {:<35} {:>10.1}ms  ({:.1} ops/sec, {} samples)",
                m.name, m.latency_ms, tps, m.samples
            );
        } else {
            eprintln!(
                "  {:<35} {:>10.1}ms  (p50={:.1}, p95={:.1}, {} samples)",
                m.name,
                m.latency_ms,
                m.p50_ms.unwrap_or(0.0),
                m.p95_ms.unwrap_or(0.0),
                m.samples
            );
        }
    }
}

// ===========================================================================
// SSH Baseline Benchmark
// ===========================================================================

/// Measure SSH localhost latency as a baseline comparison for Matrix-based sessions.
///
/// Uses a local ed25519 key at ~/.ssh/id_ed25519_mxdx_test.
/// Generate with: ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_mxdx_test -N ""
/// Authorize with: cat ~/.ssh/id_ed25519_mxdx_test.pub >> ~/.ssh/authorized_keys
#[tokio::test]
#[ignore = "benchmark — requires ~/.ssh/id_ed25519_mxdx_test"]
async fn bench_ssh_baseline() {
    let key_path = std::path::PathBuf::from(std::env::var("HOME").expect("HOME not set"))
        .join(".ssh/id_ed25519_mxdx_test");
    if !key_path.exists() {
        panic!(
            "SSH test key not found at {}. Generate with:\n  \
             ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_mxdx_test -N \"\"\n  \
             cat ~/.ssh/id_ed25519_mxdx_test.pub >> ~/.ssh/authorized_keys",
            key_path.display()
        );
    }
    let key = key_path.to_str().unwrap();

    let ssh_cmd = |args: &[&str]| -> std::process::Command {
        let mut cmd = std::process::Command::new("ssh");
        cmd.args([
            "-i", key,
            "-o", "StrictHostKeyChecking=accept-new",
            "-o", "BatchMode=yes",
            "-o", "LogLevel=ERROR",
            "localhost",
        ]);
        for a in args {
            cmd.arg(a);
        }
        cmd
    };

    let mut metrics = Vec::new();

    // 1. SSH single command (echo hello) — 20 samples
    eprintln!("=== SSH Baseline Benchmark ===");
    eprintln!("[1/6] SSH single command latency (echo hello)...");
    let mut cmd_times = Vec::new();
    for _ in 0..20 {
        let start = Instant::now();
        let output = ssh_cmd(&["echo", "hello"]).output().expect("ssh failed");
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        assert!(output.status.success());
        cmd_times.push(ms);
    }
    metrics.push(metric_from_samples("ssh_command_echo", "ssh localhost echo hello", cmd_times));

    // 2. SSH command with output (ls -la /)
    eprintln!("[2/6] SSH command with output (ls -la /)...");
    let mut ls_times = Vec::new();
    for _ in 0..10 {
        let start = Instant::now();
        let output = ssh_cmd(&["ls", "-la", "/"]).output().expect("ssh failed");
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        assert!(output.status.success());
        ls_times.push(ms);
    }
    metrics.push(metric_from_samples("ssh_command_ls", "ssh localhost ls -la /", ls_times));

    // 3. SSH command with computation (sha256sum)
    eprintln!("[3/6] SSH command with computation...");
    let mut hash_times = Vec::new();
    for _ in 0..10 {
        let start = Instant::now();
        let output = ssh_cmd(&["sha256sum", "/etc/hostname"]).output().expect("ssh failed");
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        assert!(output.status.success());
        hash_times.push(ms);
    }
    metrics.push(metric_from_samples("ssh_command_sha256sum", "ssh localhost sha256sum /etc/hostname", hash_times));

    // 4. SSH burst throughput (20 sequential commands)
    eprintln!("[4/6] SSH burst throughput (20 sequential echo)...");
    let burst_start = Instant::now();
    for i in 0..20 {
        let output = ssh_cmd(&["echo", &format!("burst-{i}")]).output().expect("ssh failed");
        assert!(output.status.success());
    }
    let burst_ms = burst_start.elapsed().as_secs_f64() * 1000.0;
    let burst_ops = 20.0 / (burst_ms / 1000.0);
    metrics.push(Metric {
        name: "ssh_burst_throughput".into(),
        operation: "20 sequential ssh echo commands".into(),
        latency_ms: burst_ms,
        throughput_ops_per_sec: Some(burst_ops),
        samples: 20,
        min_ms: None, max_ms: None, p50_ms: None, p95_ms: None, p99_ms: None,
    });

    // 5. SSH session lifecycle (connect + multi-step script + capture output)
    eprintln!("[5/6] SSH full session lifecycle equivalent...");
    let mut lifecycle_times = Vec::new();
    for i in 0..5 {
        let start = Instant::now();
        let output = ssh_cmd(&["bash", "-c", &format!(
            "echo 'session-{i} started'; sleep 0.1; echo 'processing...'; echo 'session-{i} done'; exit 0"
        )]).output().expect("ssh failed");
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(&format!("session-{i} done")));
        lifecycle_times.push(ms);
    }
    metrics.push(metric_from_samples("ssh_session_lifecycle", "ssh: connect + bash script + capture output", lifecycle_times));

    // 6. SSH with PTY allocation (-t -t)
    eprintln!("[6/6] SSH with PTY allocation...");
    let mut pty_times = Vec::new();
    for _ in 0..10 {
        let start = Instant::now();
        let mut cmd = std::process::Command::new("ssh");
        cmd.args(["-i", key, "-o", "StrictHostKeyChecking=accept-new",
                  "-o", "BatchMode=yes", "-o", "LogLevel=ERROR",
                  "-t", "-t", "localhost", "echo", "pty-test"]);
        let output = cmd.output().expect("ssh -t failed");
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        if output.status.success() {
            pty_times.push(ms);
        }
    }
    if !pty_times.is_empty() {
        metrics.push(metric_from_samples("ssh_pty_command", "ssh -t localhost echo (with PTY)", pty_times));
    }

    // Write report
    let report = BenchmarkReport {
        timestamp: Utc::now().to_rfc3339(),
        server: "localhost-ssh".into(),
        federated: false,
        git_sha: git_sha(),
        metrics: metrics.clone(),
    };
    write_report(&report, "ssh-baseline");

    eprintln!("\n=== SSH Baseline Summary ===");
    for m in &metrics {
        if let Some(tps) = m.throughput_ops_per_sec {
            eprintln!("  {:<30} {:>10.1}ms  ({:.1} ops/sec, {} samples)",
                m.name, m.latency_ms, tps, m.samples);
        } else {
            eprintln!("  {:<30} {:>10.1}ms  (p50={:.1}, p95={:.1}, {} samples)",
                m.name, m.latency_ms, m.p50_ms.unwrap_or(0.0), m.p95_ms.unwrap_or(0.0), m.samples);
        }
    }

    eprintln!("\n=== SSH vs mxdx Comparison ===");
    let ssh_echo = metrics.iter().find(|m| m.name == "ssh_command_echo")
        .map(|m| m.p50_ms.unwrap_or(m.latency_ms)).unwrap_or(0.0);
    let ssh_lifecycle = metrics.iter().find(|m| m.name == "ssh_session_lifecycle")
        .map(|m| m.p50_ms.unwrap_or(m.latency_ms)).unwrap_or(0.0);
    let ssh_burst = metrics.iter().find(|m| m.name == "ssh_burst_throughput")
        .and_then(|m| m.throughput_ops_per_sec).unwrap_or(0.0);

    eprintln!("  Single command:");
    eprintln!("    SSH echo (p50):             {:>8.1}ms", ssh_echo);
    eprintln!("    mxdx local event send:      {:>8}ms  (from local benchmark)", "~18");
    eprintln!("    mxdx local event delivery:  {:>8}ms  (includes sync polling)", "~5100");
    eprintln!("  Session lifecycle:");
    eprintln!("    SSH (script + output):      {:>8.1}ms", ssh_lifecycle);
    eprintln!("    mxdx local (full flow):     {:>8}ms", "~5271");
    eprintln!("    mxdx beta federated:        {:>8}ms", "~8093");
    eprintln!("  Throughput:");
    eprintln!("    SSH burst:                  {:>8.1} ops/sec", ssh_burst);
    eprintln!("    mxdx local burst:           {:>8} ops/sec", "~83");
    eprintln!("    mxdx beta burst:            {:>8} ops/sec", "~10");
}
