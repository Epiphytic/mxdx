# Using mxdx with Public Matrix Servers

## Overview

mxdx works with any Matrix homeserver that supports E2EE. This guide covers using public servers like matrix.org, with important caveats about performance and rate limits.

### Supported Public Servers
- matrix.org (tested, recommended for public use)
- Any homeserver implementing Matrix Client-Server API with E2EE support

### Setup Steps
1. Create two accounts on matrix.org (or your preferred server)
   - One for the launcher (e.g., @my-launcher:matrix.org)
   - One for the admin/client (e.g., @my-admin:matrix.org)
2. Cross-sign verify both accounts
   - Log into both accounts in Element
   - Verify each other's identity (Security settings > Verify)
   - This establishes E2EE trust between the accounts
3. Configure launcher:
   ```
   node packages/launcher/bin/mxdx-launcher.js \
     --servers matrix.org \
     --username my-launcher \
     --password launcher-password \
     --admin-user @my-admin:matrix.org \
     --allowed-commands echo,ls,date,cat \
     --allowed-cwd /tmp
   ```
4. Use client or web console with the admin account

### Performance Characteristics and Caveats

**Rate Limits (matrix.org)**
- Send rate: ~1-2 events/second sustained
- Burst: ~5 events, then 429 (M_LIMIT_EXCEEDED) with retry_after_ms
- Impact: Without P2P, interactive terminals are too slow for real-time typing

**Latency Comparison**
| Path | Avg RTT | P95 RTT | Notes |
|------|---------|---------|-------|
| P2P (direct) | ~0.17ms | ~0.41ms | Via WebRTC data channel, AES-256-GCM encrypted |
| Matrix (Tuwunel, local) | ~25ms | ~49ms | Full E2EE roundtrip via local homeserver |
| Matrix (matrix.org) | ~1100ms | ~2170ms | E2EE + network latency + server processing |

**Interactive Terminals on Public Servers**
- P2P is essential for usable interactive terminals on public servers
- Without P2P: ~3 second character echo latency (unusable for shell)
- With P2P: ~15ms character echo latency (fully interactive)
- P2P negotiation happens via Matrix signaling (m.call.invite/answer/candidates)
- Once P2P is established, terminal data flows directly between launcher and client

**BatchedSender Adaptive Behavior**
- P2P mode: 5ms batch window (near-instant keystroke delivery)
- Matrix mode: 200ms batch window (aggregates output to stay under rate limits)
- Automatic switch when P2P connects/disconnects

### P2P on Public Servers
- P2P signaling uses Matrix events (m.call.invite, m.call.answer, m.call.candidates)
- WebRTC data channel established after signaling
- Direct connection (host ICE candidates) when both peers are on same network
- TURN relay through Matrix homeserver's TURN credentials when direct connection fails
- matrix.org provides TURN servers; other homeservers may not

### TURN Server Availability
- matrix.org provides TURN servers (turn.matrix.org)
- Credentials are fetched via `/_matrix/client/v3/voip/turnServer`
- If no TURN servers are available, P2P only works with direct connectivity
- Use `--p2p-turn-only true` on the launcher to force relay mode (useful for testing)

### Limitations
- Rate limits make Matrix-only path unsuitable for interactive terminals
- Initial connection takes longer (network latency for signaling)
- TURN relay adds latency compared to direct P2P
- Cross-signing must be done manually via Element before mxdx can verify trust
- Public server operators can see metadata (who talks to whom) but not E2EE content

### Troubleshooting
- "Session rejected": Cross-signing not verified -- verify in Element first
- 429 rate limit errors: Normal on public servers, BatchedSender handles automatically
- P2P not connecting: Check TURN server availability, firewall rules
- High latency: Verify P2P is active (green P2P badge in web console)
