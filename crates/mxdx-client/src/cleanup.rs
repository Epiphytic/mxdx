use anyhow::Result;
use serde::Deserialize;

#[derive(Deserialize)]
struct DeviceInfo {
    device_id: String,
    display_name: Option<String>,
    last_seen_ts: Option<u64>,
}

#[derive(Deserialize)]
struct DevicesResponse {
    devices: Vec<DeviceInfo>,
}

/// List all devices for the current user.
async fn list_devices(homeserver: &str, access_token: &str) -> Result<Vec<DeviceInfo>> {
    let client = reqwest::Client::new();
    let url = format!(
        "{}/_matrix/client/v3/devices",
        homeserver.trim_end_matches('/')
    );
    let resp = client.get(&url).bearer_auth(access_token).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("Failed to list devices: {}", resp.status());
    }
    let data: DevicesResponse = resp.json().await?;
    Ok(data.devices)
}

/// Delete a device with UIA password authentication.
async fn delete_device(
    homeserver: &str,
    access_token: &str,
    device_id: &str,
    user_id: &str,
    password: &str,
) -> Result<()> {
    let client = reqwest::Client::new();
    let base = homeserver.trim_end_matches('/');
    let url = format!("{}/_matrix/client/v3/devices/{}", base, device_id);

    // First attempt without auth — may succeed or return 401 with UIA session
    let resp = client.delete(&url).bearer_auth(access_token).send().await?;
    if resp.status().is_success() {
        return Ok(());
    }
    if resp.status().as_u16() != 401 {
        anyhow::bail!("Delete device failed: {}", resp.status());
    }

    // UIA required — extract session
    let uia_info: serde_json::Value = resp.json().await?;
    let session = uia_info["session"].as_str().unwrap_or("");
    let localpart = user_id
        .split(':')
        .next()
        .unwrap_or(user_id)
        .trim_start_matches('@');

    let auth_body = serde_json::json!({
        "auth": {
            "type": "m.login.password",
            "identifier": { "type": "m.id.user", "user": localpart },
            "password": password,
            "session": session,
        }
    });

    let resp2 = client
        .delete(&url)
        .bearer_auth(access_token)
        .json(&auth_body)
        .send()
        .await?;
    if !resp2.status().is_success() {
        anyhow::bail!("UIA delete failed: {}", resp2.status());
    }
    Ok(())
}

/// List joined rooms.
async fn list_joined_rooms(homeserver: &str, access_token: &str) -> Result<Vec<String>> {
    let client = reqwest::Client::new();
    let url = format!(
        "{}/_matrix/client/v3/joined_rooms",
        homeserver.trim_end_matches('/')
    );
    let resp = client.get(&url).bearer_auth(access_token).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("Failed to list rooms: {}", resp.status());
    }
    let data: serde_json::Value = resp.json().await?;
    let rooms = data["joined_rooms"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Ok(rooms)
}

/// Leave and forget a room.
async fn leave_and_forget(homeserver: &str, access_token: &str, room_id: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let base = homeserver.trim_end_matches('/');
    let encoded = urlencoding::encode(room_id);

    let leave_url = format!("{}/_matrix/client/v3/rooms/{}/leave", base, encoded);
    let _ = client
        .post(&leave_url)
        .bearer_auth(access_token)
        .json(&serde_json::json!({}))
        .send()
        .await?;

    let forget_url = format!("{}/_matrix/client/v3/rooms/{}/forget", base, encoded);
    let _ = client
        .post(&forget_url)
        .bearer_auth(access_token)
        .json(&serde_json::json!({}))
        .send()
        .await?;

    Ok(())
}

/// Logout all sessions (nuclear — deletes all devices).
async fn logout_all(homeserver: &str, access_token: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!(
        "{}/_matrix/client/v3/logout/all",
        homeserver.trim_end_matches('/')
    );
    let resp = client
        .post(&url)
        .bearer_auth(access_token)
        .json(&serde_json::json!({}))
        .send()
        .await?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("logout/all failed: {}", body);
    }
    Ok(())
}

/// Run the cleanup command.
pub async fn run_cleanup(
    homeserver: &str,
    access_token: &str,
    current_device_id: &str,
    user_id: &str,
    password: &str,
    targets: &str,
    force: bool,
    delete_all_sessions: bool,
) -> Result<()> {
    let target_list: Vec<&str> = targets.split(',').map(|s| s.trim()).collect();

    if delete_all_sessions {
        if !force {
            eprintln!("WARNING: This will log out ALL sessions and delete ALL devices.");
            eprintln!("You will need to re-login. Use --force to skip this prompt.");
            return Ok(());
        }
        eprintln!("Logging out all sessions...");
        logout_all(homeserver, access_token).await?;
        eprintln!("All sessions logged out. All devices deleted. Re-login required.");
        return Ok(());
    }

    let do_devices = target_list.iter().any(|t| *t == "devices" || *t == "all");
    let do_rooms = target_list.iter().any(|t| *t == "rooms" || *t == "all");

    // Validate targets
    for target in &target_list {
        match *target {
            "devices" | "rooms" | "all" => {}
            other => {
                eprintln!(
                    "Unknown cleanup target: '{}'. Use: devices, rooms, all",
                    other
                );
            }
        }
    }

    if do_devices {
        eprintln!("Cleaning up devices...");
        let devices = list_devices(homeserver, access_token).await?;
        let to_delete: Vec<&DeviceInfo> = devices
            .iter()
            .filter(|d| d.device_id != current_device_id)
            .collect();

        eprintln!(
            "Found {} device(s) to delete (keeping current: {})",
            to_delete.len(),
            current_device_id
        );
        for d in &to_delete {
            let name = d.display_name.as_deref().unwrap_or("(unnamed)");
            let ts = d
                .last_seen_ts
                .map(|t| format!("{}ms", t))
                .unwrap_or_else(|| "never".into());
            eprintln!("  {} — {} (last seen: {})", d.device_id, name, ts);
        }

        if !force && !to_delete.is_empty() {
            eprintln!("Use --force to proceed with deletion.");
        } else {
            let mut deleted = 0;
            let mut errors = 0;
            for d in &to_delete {
                match delete_device(homeserver, access_token, &d.device_id, user_id, password).await
                {
                    Ok(()) => {
                        deleted += 1;
                    }
                    Err(e) => {
                        eprintln!("  Error deleting {}: {}", d.device_id, e);
                        errors += 1;
                    }
                }
            }
            eprintln!("Devices: {} deleted, {} errors", deleted, errors);
        }
    }

    if do_rooms {
        eprintln!("Cleaning up rooms...");
        let rooms = list_joined_rooms(homeserver, access_token).await?;
        eprintln!("Found {} joined room(s)", rooms.len());

        if !force && !rooms.is_empty() {
            for r in &rooms {
                eprintln!("  {}", r);
            }
            eprintln!("Use --force to leave and forget all rooms.");
        } else {
            let mut left = 0;
            let mut errors = 0;
            for room_id in &rooms {
                match leave_and_forget(homeserver, access_token, room_id).await {
                    Ok(()) => {
                        eprintln!("  Left {}", room_id);
                        left += 1;
                    }
                    Err(e) => {
                        eprintln!("  Error on {}: {}", room_id, e);
                        errors += 1;
                    }
                }
            }
            eprintln!("Rooms: {} left+forgotten, {} errors", left, errors);
        }
    }

    Ok(())
}
