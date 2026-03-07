use wasm_bindgen::prelude::*;
use matrix_sdk::{
    config::SyncSettings,
    Client,
};
use std::time::Duration;

#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// Smoke test: returns the matrix-sdk version string to prove it compiled.
#[wasm_bindgen]
pub fn sdk_version() -> String {
    "matrix-sdk-0.16-wasm".to_string()
}

#[wasm_bindgen]
pub struct WasmMatrixClient {
    client: Client,
}

#[wasm_bindgen]
impl WasmMatrixClient {
    /// Register a new user on a homeserver with a registration token.
    #[wasm_bindgen(js_name = "register")]
    pub async fn register(
        homeserver_url: &str,
        username: &str,
        password: &str,
        registration_token: &str,
    ) -> Result<WasmMatrixClient, JsValue> {
        let client = Client::builder()
            .homeserver_url(homeserver_url)
            .build()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        // Register via REST API (matrix-sdk register API is complex, use reqwest directly)
        let reg_url = format!("{homeserver_url}/_matrix/client/v3/register");
        let body = serde_json::json!({
            "username": username,
            "password": password,
            "auth": {
                "type": "m.login.registration_token",
                "token": registration_token
            }
        });

        let http_client = reqwest::Client::new();
        let resp = http_client
            .post(&reg_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| JsValue::from_str(&format!("Registration request failed: {e}")))?;

        if !resp.status().is_success() {
            let err_body = resp.text().await.unwrap_or_else(|_| "unknown error".to_string());
            return Err(JsValue::from_str(&format!("Registration failed: {err_body}")));
        }

        // Login with registered credentials
        client
            .matrix_auth()
            .login_username(username, password)
            .initial_device_display_name("mxdx")
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        // Initial sync to upload device keys
        client
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(5)))
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        Ok(WasmMatrixClient { client })
    }

    /// Login to a Matrix server. server_name can be "matrix.org" or a full URL.
    #[wasm_bindgen(js_name = "login")]
    pub async fn login(
        server_name: &str,
        username: &str,
        password: &str,
    ) -> Result<WasmMatrixClient, JsValue> {
        let builder = Client::builder();
        let client = if server_name.contains("://") {
            builder.homeserver_url(server_name)
        } else {
            builder.server_name_or_homeserver_url(server_name)
        }
        .build()
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

        client
            .matrix_auth()
            .login_username(username, password)
            .initial_device_display_name("mxdx")
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        // Initial sync to upload device keys
        client
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(5)))
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        Ok(WasmMatrixClient { client })
    }

    /// Check if logged in.
    #[wasm_bindgen(js_name = "isLoggedIn")]
    pub fn is_logged_in(&self) -> bool {
        self.client.user_id().is_some()
    }

    /// Get the user ID.
    #[wasm_bindgen(js_name = "userId")]
    pub fn user_id(&self) -> Option<String> {
        self.client.user_id().map(|u| u.to_string())
    }

    /// Perform a single sync cycle.
    #[wasm_bindgen(js_name = "syncOnce")]
    pub async fn sync_once(&self) -> Result<(), JsValue> {
        self.client
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(1)))
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(())
    }
}
