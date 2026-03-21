use std::time::Duration;

use mxdx_policy::appservice::{register_appservice, AppserviceRegistration};
use mxdx_policy::config::PolicyConfig;
use mxdx_policy::policy::{PolicyEngine, PolicyRejection};
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
        appservice_port: 0,
    }
}

#[tokio::test]
async fn authorized_user_command_reaches_launcher() {
    let mut engine = PolicyEngine::new();
    engine.authorize_user("@admin:example.com");

    // An authorized user submitting a new command should be allowed
    let result = engine.evaluate("$cmd-1", "@admin:example.com", "execute");
    assert!(result.is_ok(), "Authorized user command should be allowed");
}

#[tokio::test]
async fn unauthorized_user_command_is_rejected() {
    let mut engine = PolicyEngine::new();
    engine.authorize_user("@admin:example.com");

    // An unauthorized user should be rejected
    let result = engine.evaluate("$cmd-2", "@rogue:example.com", "execute");
    assert_eq!(
        result,
        Err(PolicyRejection::Unauthorized),
        "Unauthorized user command should be rejected"
    );
}

#[tokio::test]
async fn test_security_policy_agent_down_blocks_all_agent_actions() {
    // Start tuwunel and register the appservice (claiming @agent-* exclusively)
    let instance = TuwunelInstance::start().await.unwrap();
    let admin = instance.register_user("admin", "adminpass").await.unwrap();

    let config = test_config(&instance);
    let registration = AppserviceRegistration::from_config(&config);

    register_appservice(&config.homeserver_url, &admin.access_token, &registration)
        .await
        .expect("Appservice registration should succeed");

    // The appservice is NOT running (no listener on appservice_port).
    // With the namespace exclusively claimed, any attempt to interact with
    // @agent-* users should be rejected by the homeserver.
    let http_client = reqwest::Client::new();

    // Try to register an agent user via normal registration API
    let reg_url = format!("{}/_matrix/client/v3/register", config.homeserver_url);
    let body = serde_json::json!({
        "username": "agent-victim",
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
    let errcode = resp_body["errcode"].as_str().unwrap_or_default();

    // The homeserver should reject this because the namespace is exclusive
    // and the appservice owns it — fail-closed behavior.
    assert!(
        !status.is_success(),
        "Agent user registration should be blocked when appservice owns namespace. \
         Got status {} body: {}",
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

// mxdx-rpl: replay protection
#[tokio::test]
async fn test_security_replayed_event_does_not_double_execute() {
    let mut engine = PolicyEngine::new();
    engine.authorize_user("@operator:example.com");

    let event_id = "$cmd-replay-test-1";
    let user_id = "@operator:example.com";
    let action = "execute";

    // First submission should succeed
    let first = engine.evaluate(event_id, user_id, action);
    assert!(first.is_ok(), "First submission should be processed");

    // Replay the same event — should be rejected
    let replay = engine.evaluate(event_id, user_id, action);
    assert_eq!(
        replay,
        Err(PolicyRejection::Replay),
        "Replayed event must not double-execute"
    );

    // A different event ID should still work
    let different = engine.evaluate("$cmd-replay-test-2", user_id, action);
    assert!(
        different.is_ok(),
        "Different event should still be processed"
    );
}

#[tokio::test]
async fn replay_cache_ttl_expires_entries() {
    // Use a very short TTL so we can test expiry
    let mut engine = PolicyEngine::with_capacity_and_ttl(100, Duration::from_millis(10));
    engine.authorize_user("@operator:example.com");

    let event_id = "$cmd-ttl-test";
    let user_id = "@operator:example.com";

    // First pass
    assert!(engine.evaluate(event_id, user_id, "execute").is_ok());

    // Should be rejected immediately
    assert_eq!(
        engine.evaluate(event_id, user_id, "execute"),
        Err(PolicyRejection::Replay)
    );

    // Wait for TTL to expire
    tokio::time::sleep(Duration::from_millis(20)).await;

    // After TTL, the event should no longer be considered a replay
    assert!(
        !engine.check_replay(event_id),
        "Expired entry should not be a replay"
    );
}
