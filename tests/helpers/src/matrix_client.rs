/// Lightweight test Matrix client for registration and basic API calls.
#[derive(Debug, Clone)]
pub struct TestMatrixClient {
    pub user_id: String,
    pub access_token: String,
    pub device_id: String,
    pub homeserver_url: String,
}
