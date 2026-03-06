use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

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
    pub launcher_tx: broadcast::Sender<LauncherInfo>,
}

impl AppState {
    pub fn new() -> Self {
        let (launcher_tx, _) = broadcast::channel(64);
        Self {
            launchers: Arc::new(RwLock::new(Vec::new())),
            launcher_tx,
        }
    }

    pub async fn update_launcher(&self, info: LauncherInfo) {
        let mut launchers = self.launchers.write().await;
        if let Some(existing) = launchers.iter_mut().find(|l| l.id == info.id) {
            *existing = info.clone();
        } else {
            launchers.push(info.clone());
        }
        // Ignore send error (no active receivers)
        let _ = self.launcher_tx.send(info);
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
