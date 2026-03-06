use axum::extract::State;
use axum::response::Html;
use axum::routing::get;
use axum::Router;

use crate::state::{AppState, LauncherInfo};

pub fn routes() -> Router<AppState> {
    Router::new().route("/dashboard", get(dashboard_handler))
}

async fn dashboard_handler(State(state): State<AppState>) -> Html<String> {
    let launchers = state.launchers.read().await;
    let cards = render_launcher_cards(&launchers);

    Html(format!(
        r#"<div id="dashboard" hx-get="/dashboard" hx-trigger="every 5s" hx-swap="outerHTML">
  <h1>mxdx Dashboard</h1>
  <div class="launcher-grid">{cards}</div>
</div>"#
    ))
}

fn render_launcher_cards(launchers: &[LauncherInfo]) -> String {
    if launchers.is_empty() {
        return "<p>No launchers connected.</p>".to_string();
    }

    launchers
        .iter()
        .map(|l| {
            let mem_pct = if l.memory_total_bytes > 0 {
                (l.memory_used_bytes as f64 / l.memory_total_bytes as f64) * 100.0
            } else {
                0.0
            };

            format!(
                r#"<div class="launcher-card" data-id="{id}">
  <h2>{id}</h2>
  <span class="status status-{status}">{status}</span>
  <p>Host: {hostname}</p>
  <p>CPU: {cpu}%</p>
  <p>Memory: {mem:.1}%</p>
</div>"#,
                id = l.id,
                status = l.status,
                hostname = l.hostname,
                cpu = l.cpu_usage_percent,
                mem = mem_pct,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
