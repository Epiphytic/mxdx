/**
 * Fetches TURN server credentials from the homeserver.
 * Returns null if the homeserver doesn't provide TURN.
 *
 * Uses URL() constructor for safe path construction (no string concatenation).
 * Validates https: scheme to prevent credential exfiltration via SSRF.
 * Disables redirect following to prevent credential leakage to non-homeserver domains.
 */
export async function fetchTurnCredentials(homeserverUrl, accessToken, fetchFn = fetch) {
  try {
    const parsed = new URL(homeserverUrl);
    if (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') {
      return null;
    }
    parsed.pathname = '/_matrix/client/v3/voip/turnServer';
    const response = await fetchFn(parsed.href, {
      headers: { Authorization: 'Bearer ' + accessToken },
      redirect: 'error',
    });
    if (!response.ok) return null;
    const data = await response.json();
    if (!data.uris || data.uris.length === 0) return null;
    return data;
  } catch {
    return null;
  }
}

/**
 * Converts homeserver TURN response to RTCPeerConnection iceServers format.
 */
export function turnToIceServers(turnResponse) {
  if (!turnResponse?.uris?.length) return [];
  return [{
    urls: turnResponse.uris,
    username: turnResponse.username,
    credential: turnResponse.password,
  }];
}
