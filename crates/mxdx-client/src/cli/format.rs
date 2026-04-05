use crate::protocol::methods::*;

/// Format a daemon.status result for terminal display.
pub fn format_status(status: &DaemonStatusResult) -> String {
    let mut out = String::new();
    out.push_str(&format!("Profile: {}\n", status.profile));
    out.push_str(&format!("Uptime: {}s\n", status.uptime_seconds));
    out.push_str(&format!("Matrix: {}\n", status.matrix_status));
    out.push_str(&format!("Active sessions: {}\n", status.active_sessions));
    out.push_str(&format!("Connected clients: {}\n", status.connected_clients));
    if !status.transports.is_empty() {
        out.push_str("Transports:\n");
        for t in &status.transports {
            out.push_str(&format!("  {} @ {}\n", t.r#type, t.address));
        }
    }
    if !status.accounts.is_empty() {
        out.push_str("Accounts:\n");
        for a in &status.accounts {
            out.push_str(&format!("  {}\n", a));
        }
    }
    out
}
