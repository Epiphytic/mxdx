use std::time::Duration;

/// Result of checking worker liveness via telemetry state event.
#[derive(Debug, Clone, PartialEq)]
pub enum LivenessStatus {
    /// Worker is online with these capabilities.
    Online { capabilities: Vec<String> },
    /// Worker's last heartbeat was too old.
    Stale(Duration),
    /// Worker reported itself as offline.
    Offline,
    /// No telemetry state event found in the room.
    NoWorker,
}

/// Check whether a live worker exists in the given room by reading
/// the `org.mxdx.host_telemetry` state event.
///
/// This is a pure function: the caller reads the state event from Matrix
/// and passes it in as JSON. This makes it testable without a Matrix connection.
///
/// Returns `LivenessStatus` based on the state event's timestamp,
/// heartbeat_interval_ms, and status fields.
pub fn check_worker_liveness(telemetry_json: &serde_json::Value) -> LivenessStatus {
    // Missing or null JSON → no worker
    if telemetry_json.is_null() || telemetry_json.as_object().map_or(true, |o| o.is_empty()) {
        return LivenessStatus::NoWorker;
    }

    // Extract required fields
    let status = match telemetry_json.get("status").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return LivenessStatus::NoWorker,
    };

    let timestamp_str = match telemetry_json.get("timestamp").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return LivenessStatus::NoWorker,
    };

    let heartbeat_interval_ms = telemetry_json
        .get("heartbeat_interval_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(60_000);

    // Check offline status first (regardless of freshness)
    if status == "offline" {
        return LivenessStatus::Offline;
    }

    // Parse timestamp and check freshness
    let event_ts = match parse_iso8601_to_epoch_ms(timestamp_str) {
        Some(ts) => ts,
        None => return LivenessStatus::NoWorker,
    };

    let now_ms = now_epoch_ms();
    let age_ms = now_ms.saturating_sub(event_ts);
    let stale_threshold_ms = 2 * heartbeat_interval_ms;

    if age_ms > stale_threshold_ms {
        return LivenessStatus::Stale(Duration::from_millis(age_ms));
    }

    // Worker is online and within freshness window
    let capabilities = telemetry_json
        .get("capabilities")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    LivenessStatus::Online { capabilities }
}

/// Get current time as milliseconds since Unix epoch.
fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Parse an ISO 8601 timestamp (e.g., "2026-03-31T12:00:00Z" or "2026-03-31T12:00:00.000Z")
/// into milliseconds since Unix epoch.
///
/// Supports the formats emitted by both the Rust worker (`chrono::Utc::now().to_rfc3339()`)
/// and the npm launcher (`new Date().toISOString()`).
fn parse_iso8601_to_epoch_ms(s: &str) -> Option<u64> {
    // Strip trailing 'Z' or '+00:00' timezone suffix
    let s = s.strip_suffix('Z').or_else(|| s.strip_suffix("+00:00"))?;

    // Split on 'T' to get date and time parts
    let (date_part, time_part) = s.split_once('T')?;

    // Parse date: YYYY-MM-DD
    let mut date_parts = date_part.splitn(3, '-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u64 = date_parts.next()?.parse().ok()?;
    let day: u64 = date_parts.next()?.parse().ok()?;

    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    // Parse time: HH:MM:SS or HH:MM:SS.sss
    let (time_main, frac_ms) = if let Some((main, frac)) = time_part.split_once('.') {
        let frac_str = &frac[..frac.len().min(3)];
        let frac_padded = format!("{:0<3}", frac_str);
        let ms: u64 = frac_padded.parse().ok()?;
        (main, ms)
    } else {
        (time_part, 0u64)
    };

    let mut time_parts = time_main.splitn(3, ':');
    let hour: u64 = time_parts.next()?.parse().ok()?;
    let minute: u64 = time_parts.next()?.parse().ok()?;
    let second: u64 = time_parts.next()?.parse().ok()?;

    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    // Convert to epoch using a simplified days-from-epoch calculation
    let days = days_from_civil(year, month as i64, day as i64);
    let epoch_ms =
        (days as u64) * 86_400_000 + hour * 3_600_000 + minute * 60_000 + second * 1_000 + frac_ms;

    Some(epoch_ms)
}

/// Compute days since Unix epoch (1970-01-01) from a civil date.
/// Uses the algorithm from Howard Hinnant's `chrono`-compatible date library.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = month as u64;
    let doy = if m > 2 {
        (153 * (m - 3) + 2) / 5 + (day as u64) - 1
    } else {
        (153 * (m + 9) + 2) / 5 + (day as u64) - 1
    };
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe as i64 - 719468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a telemetry JSON value with given parameters.
    fn make_telemetry(status: &str, timestamp: &str, heartbeat_ms: u64) -> serde_json::Value {
        serde_json::json!({
            "timestamp": timestamp,
            "heartbeat_interval_ms": heartbeat_ms,
            "hostname": "test-worker",
            "platform": "linux",
            "arch": "x86_64",
            "status": status,
            "capabilities": ["docker", "gpu"],
            "tmux_available": true,
            "session_persistence": true,
        })
    }

    /// Helper: produce an ISO 8601 timestamp string for `now - offset`.
    fn timestamp_ago(offset: Duration) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let target_ms = now.as_millis() as u64 - offset.as_millis() as u64;
        epoch_ms_to_iso8601(target_ms)
    }

    /// Convert epoch millis back to ISO 8601 for test helpers.
    fn epoch_ms_to_iso8601(ms: u64) -> String {
        let secs = ms / 1000;
        let millis = ms % 1000;
        let days = secs / 86400;
        let day_secs = secs % 86400;
        let hour = day_secs / 3600;
        let minute = (day_secs % 3600) / 60;
        let second = day_secs % 60;

        // Civil date from days since epoch (inverse of days_from_civil)
        let z = days as i64 + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = (z - era * 146097) as u64;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };

        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            y, m, d, hour, minute, second, millis
        )
    }

    #[test]
    fn online_worker_with_recent_timestamp() {
        let ts = timestamp_ago(Duration::from_secs(10));
        let json = make_telemetry("online", &ts, 60_000);
        let result = check_worker_liveness(&json);
        assert_eq!(
            result,
            LivenessStatus::Online {
                capabilities: vec!["docker".into(), "gpu".into()]
            }
        );
    }

    #[test]
    fn offline_worker_returns_offline() {
        let ts = timestamp_ago(Duration::from_secs(5));
        let json = make_telemetry("offline", &ts, 60_000);
        let result = check_worker_liveness(&json);
        assert_eq!(result, LivenessStatus::Offline);
    }

    #[test]
    fn stale_worker_with_old_timestamp() {
        // heartbeat_interval_ms = 60_000, so stale threshold = 120_000ms = 120s
        // Timestamp 200 seconds ago should be stale
        let ts = timestamp_ago(Duration::from_secs(200));
        let json = make_telemetry("online", &ts, 60_000);
        let result = check_worker_liveness(&json);
        match result {
            LivenessStatus::Stale(duration) => {
                // Should be approximately 200 seconds (allow some test execution time)
                assert!(duration.as_secs() >= 198 && duration.as_secs() <= 210);
            }
            other => panic!("expected Stale, got {:?}", other),
        }
    }

    #[test]
    fn missing_json_returns_no_worker() {
        let json = serde_json::Value::Null;
        assert_eq!(check_worker_liveness(&json), LivenessStatus::NoWorker);
    }

    #[test]
    fn empty_object_returns_no_worker() {
        let json = serde_json::json!({});
        assert_eq!(check_worker_liveness(&json), LivenessStatus::NoWorker);
    }

    #[test]
    fn missing_status_field_returns_no_worker() {
        let json = serde_json::json!({
            "timestamp": "2026-03-31T12:00:00Z",
            "heartbeat_interval_ms": 60000,
        });
        assert_eq!(check_worker_liveness(&json), LivenessStatus::NoWorker);
    }

    #[test]
    fn missing_timestamp_field_returns_no_worker() {
        let json = serde_json::json!({
            "status": "online",
            "heartbeat_interval_ms": 60000,
        });
        assert_eq!(check_worker_liveness(&json), LivenessStatus::NoWorker);
    }

    #[test]
    fn invalid_timestamp_returns_no_worker() {
        let json = serde_json::json!({
            "timestamp": "not-a-timestamp",
            "heartbeat_interval_ms": 60000,
            "status": "online",
        });
        assert_eq!(check_worker_liveness(&json), LivenessStatus::NoWorker);
    }

    #[test]
    fn online_worker_with_no_capabilities() {
        let ts = timestamp_ago(Duration::from_secs(5));
        let mut json = make_telemetry("online", &ts, 60_000);
        json.as_object_mut().unwrap().remove("capabilities");
        let result = check_worker_liveness(&json);
        assert_eq!(
            result,
            LivenessStatus::Online {
                capabilities: vec![]
            }
        );
    }

    #[test]
    fn default_heartbeat_interval_when_missing() {
        // When heartbeat_interval_ms is missing, defaults to 60000ms
        // 200s ago with 60s interval (stale threshold 120s) → Stale
        let ts = timestamp_ago(Duration::from_secs(200));
        let json = serde_json::json!({
            "timestamp": ts,
            "status": "online",
        });
        let result = check_worker_liveness(&json);
        assert!(matches!(result, LivenessStatus::Stale(_)));
    }

    #[test]
    fn offline_status_regardless_of_freshness() {
        // Even with a very recent timestamp, offline is offline
        let ts = timestamp_ago(Duration::from_secs(1));
        let json = make_telemetry("offline", &ts, 60_000);
        assert_eq!(check_worker_liveness(&json), LivenessStatus::Offline);
    }

    #[test]
    fn boundary_exactly_at_stale_threshold() {
        // heartbeat = 30_000ms, stale threshold = 60_000ms = 60s
        // Timestamp exactly 60s ago should NOT be stale (age == threshold, not >)
        // But due to test execution time we use 59s to be safe
        let ts = timestamp_ago(Duration::from_secs(59));
        let json = make_telemetry("online", &ts, 30_000);
        let result = check_worker_liveness(&json);
        assert_eq!(
            result,
            LivenessStatus::Online {
                capabilities: vec!["docker".into(), "gpu".into()]
            }
        );
    }

    #[test]
    fn npm_launcher_timestamp_format() {
        // npm launcher uses new Date().toISOString() → "2026-03-31T10:00:00.000Z"
        let ts = timestamp_ago(Duration::from_secs(5));
        let json = make_telemetry("online", &ts, 60_000);
        let result = check_worker_liveness(&json);
        assert!(matches!(result, LivenessStatus::Online { .. }));
    }

    #[test]
    fn parse_iso8601_basic_cases() {
        // 2020-01-01T00:00:00Z = epoch 1577836800000
        assert_eq!(
            parse_iso8601_to_epoch_ms("2020-01-01T00:00:00Z"),
            Some(1577836800000)
        );
        // 1970-01-01T00:00:00Z = epoch 0
        assert_eq!(
            parse_iso8601_to_epoch_ms("1970-01-01T00:00:00Z"),
            Some(0)
        );
        // With milliseconds
        assert_eq!(
            parse_iso8601_to_epoch_ms("2020-01-01T00:00:00.500Z"),
            Some(1577836800500)
        );
    }

    #[test]
    fn parse_iso8601_with_plus_zero_offset() {
        assert_eq!(
            parse_iso8601_to_epoch_ms("2020-01-01T00:00:00+00:00"),
            Some(1577836800000)
        );
    }
}
