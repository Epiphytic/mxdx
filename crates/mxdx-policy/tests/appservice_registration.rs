use mxdx_policy::appservice::{register_appservice, AppserviceRegistration};
use mxdx_policy::config::PolicyConfig;
use mxdx_test_helpers::tuwunel::TuwunelInstance;

/// Helper to build a PolicyConfig for a running Tuwunel instance.
fn test_config(instance: &TuwunelInstance) -> PolicyConfig {
    let scheme = if instance.tls { "https" } else { "http" };
    PolicyConfig {
        homeserver_url: format!("{scheme}://127.0.0.1:{}", instance.port),
        as_token: "test_as_token_12345".to_string(),
        hs_token: "test_hs_token_12345".to_string(),
        server_name: instance.server_name.clone(),
        sender_localpart: "mxdx-policy".to_string(),
        user_prefix: "agent-".to_string(),
        appservice_port: 0, // Not used in these tests
    }
}

#[tokio::test]
async fn appservice_registers_with_tuwunel() {
    let instance = TuwunelInstance::start().await.unwrap();

    // First registered user becomes admin
    let admin = instance.register_user("admin", "adminpass").await.unwrap();

    let config = test_config(&instance);
    let registration = AppserviceRegistration::from_config(&config);

    // Register the appservice via admin room
    register_appservice(&config.homeserver_url, &admin.access_token, &registration)
        .await
        .expect("Appservice registration should succeed");

    // Verify the appservice is registered by trying to register it again —
    // Tuwunel should respond (the command completes without error either way,
    // but we verify the first registration didn't fail).
}

#[tokio::test]
async fn agent_namespace_is_exclusive() {
    let instance = TuwunelInstance::start().await.unwrap();

    // First registered user becomes admin
    let admin = instance.register_user("admin", "adminpass").await.unwrap();

    let config = test_config(&instance);
    let registration = AppserviceRegistration::from_config(&config);

    // Register the appservice to claim @agent-* namespace
    register_appservice(&config.homeserver_url, &admin.access_token, &registration)
        .await
        .expect("Appservice registration should succeed");

    // Now try to register a user matching @agent-test:<server_name> via
    // the normal registration API. This should be forbidden because the
    // appservice has exclusive claim on the namespace.
    let http_client = reqwest::Client::new();
    let reg_url = format!("{}/_matrix/client/v3/register", config.homeserver_url);
    let body = serde_json::json!({
        "username": "agent-test",
        "password": "testpass",
        "auth": {
            "type": "m.login.registration_token",
            "token": "mxdx-test-token"
        }
    });

    let resp = http_client
        .post(&reg_url)
        .json(&body)
        .send()
        .await
        .expect("Registration request should send");

    let status = resp.status();
    let resp_body: serde_json::Value = resp.json().await.unwrap_or_default();

    // The homeserver should reject registration of users in the exclusive namespace.
    // Tuwunel returns M_EXCLUSIVE for appservice-claimed namespaces.
    let errcode = resp_body["errcode"].as_str().unwrap_or_default();

    assert!(
        !status.is_success(),
        "Registration of @agent-test should fail when namespace is exclusively claimed. \
         Got status {} with body: {}",
        status,
        resp_body
    );
    assert!(
        errcode == "M_EXCLUSIVE" || errcode == "M_FORBIDDEN",
        "Expected M_EXCLUSIVE or M_FORBIDDEN, got errcode={} body={}",
        errcode,
        resp_body
    );
}

#[tokio::test]
async fn non_agent_user_can_still_register() {
    let instance = TuwunelInstance::start().await.unwrap();

    // First registered user becomes admin
    let admin = instance.register_user("admin", "adminpass").await.unwrap();

    let config = test_config(&instance);
    let registration = AppserviceRegistration::from_config(&config);

    // Register the appservice
    register_appservice(&config.homeserver_url, &admin.access_token, &registration)
        .await
        .expect("Appservice registration should succeed");

    // A user outside the agent- namespace should still register fine
    let regular_user = instance.register_user("regularuser", "pass").await;
    assert!(
        regular_user.is_ok(),
        "Regular user registration should succeed: {:?}",
        regular_user.err()
    );
}
