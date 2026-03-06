use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LauncherInfo {
    pub id: String,
    pub status: LauncherStatus,
    pub cpu_usage_percent: f64,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub hostname: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LauncherStatus {
    Online,
    Offline,
    Error,
}

impl std::fmt::Display for LauncherStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Online => write!(f, "online"),
            Self::Offline => write!(f, "offline"),
            Self::Error => write!(f, "error"),
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub launchers: Arc<RwLock<Vec<LauncherInfo>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            launchers: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
