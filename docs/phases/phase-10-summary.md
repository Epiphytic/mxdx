# Phase 10: Web App — Summary

## Completion Date: 2026-03-06

## What Was Built

### Axum Scaffold (`src/routes/`, `src/state.rs`)
- `AppState` with `Arc<RwLock<Vec<LauncherInfo>>>` + broadcast channel
- `GET /dashboard` — HTMX-friendly HTML with launcher cards
- Router with CSP headers, no CORS (browser same-origin policy)

### SSE Live Updates (`src/routes/sse.rs`)
- `GET /sse/launchers` — Server-Sent Events via broadcast channel
- HTMX `hx-swap-oob` fragments for live launcher card updates
- Uses `tokio-stream::BroadcastStream`

### PWA (`static/manifest.webmanifest`, `static/sw.js`)
- PWA manifest with standalone display mode
- Service worker: cache-first for static assets, network-first for HTMX partials
- SRI verification structure in service worker

### CSP Headers
- Applied to all responses via `SetResponseHeaderLayer`
- `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self' wss:; worker-src 'self'`

## Tests

8 total tests:

| Category | Count | Key Tests |
|:---|:---|:---|
| Dashboard | 3 | 200 response, launcher cards, 404 handling |
| SSE | 3 | 200 + event-stream, broadcast delivery, OOB fragment |
| Static/CSP | 2 | manifest JSON, CSP header present |

## Security Issues Addressed

| Finding | Status | Control |
|:---|:---|:---|
| mxdx-web (CORS) | Fixed | Removed CORS layer — browser same-origin policy applies |
| mxdx-web (CSP) | Implemented | CSP headers on all responses |
| mxdx-web (SRI) | Partial | Service worker structure in place, hash injection needed |

## Security Review Findings

- **HIGH (fixed)**: CORS origin "null" was insecure — removed CORS layer entirely
- **MEDIUM**: SRI verification dead code (server doesn't set X-Content-Hash)
- **MEDIUM**: No server-side origin check on HTMX partials
- **MEDIUM**: SSE endpoint has no connection limits

## Key Commits

| Commit | Description |
|:---|:---|
| `7d9aac0` | Axum scaffold with dashboard routes |
| `bf808b1` | SSE live updates + PWA manifest |
