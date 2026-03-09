/**
 * Matrix REST API cleanup utilities.
 * Uses fetch() directly — works in both Node.js 22+ and browser.
 */

/**
 * Parse an --older-than duration string into a cutoff timestamp (ms).
 * Supports: Nd (days), Nw (weeks), Nm (months = 30 days).
 * Returns null if input is falsy.
 * @param {string} [str] - e.g. "2w", "1m", "7d"
 * @returns {number|null} Unix timestamp cutoff in ms, or null
 */
export function parseOlderThan(str) {
  if (!str) return null;
  const match = str.trim().match(/^(\d+)([dwm])$/i);
  if (!match) throw new Error(`Invalid --older-than format: "${str}". Use Nd, Nw, or Nm (e.g. 7d, 2w, 1m).`);
  const n = parseInt(match[1], 10);
  const unit = match[2].toLowerCase();
  const msPerDay = 86400000;
  const multipliers = { d: msPerDay, w: 7 * msPerDay, m: 30 * msPerDay };
  return Date.now() - (n * multipliers[unit]);
}

async function matrixFetch(homeserverUrl, path, accessToken, options = {}, _retries = 0) {
  const url = `${homeserverUrl}${path}`;
  const resp = await fetch(url, {
    ...options,
    headers: {
      'Authorization': `Bearer ${accessToken}`,
      'Content-Type': 'application/json',
      ...options.headers,
    },
  });
  if (resp.status === 429 && _retries < 3) {
    const retryAfter = parseInt(resp.headers.get('Retry-After') || '5', 10);
    await new Promise(r => setTimeout(r, retryAfter * 1000));
    return matrixFetch(homeserverUrl, path, accessToken, options, _retries + 1);
  }
  return resp;
}

/**
 * Clean up old devices. Deletes all devices except the current one.
 * Requires password for UIA (User-Interactive Authentication).
 *
 * @param {object} opts
 * @param {string} opts.accessToken
 * @param {string} opts.homeserverUrl
 * @param {string} opts.currentDeviceId - Will NOT be deleted
 * @param {string} opts.userId - Full Matrix user ID (e.g. @user:matrix.org)
 * @param {string} opts.password - Required for UIA
 * @param {number} [opts.olderThan] - Cutoff timestamp (ms). Only delete devices last seen before this. From parseOlderThan().
 * @param {function} [opts.onProgress] - Called with status messages
 * @returns {{ preview: Array<{device_id, display_name, last_seen_ts}>, execute: function }}
 */
export async function cleanupDevices({
  accessToken, homeserverUrl, currentDeviceId, userId, password, olderThan, onProgress = () => {},
}) {
  onProgress('Fetching device list...');
  const resp = await matrixFetch(homeserverUrl, '/_matrix/client/v3/devices', accessToken);
  if (!resp.ok) throw new Error(`Failed to list devices: ${resp.status}`);
  const { devices } = await resp.json();

  let toDelete = devices.filter(d => d.device_id !== currentDeviceId);
  if (olderThan) {
    toDelete = toDelete.filter(d => d.last_seen_ts && d.last_seen_ts < olderThan);
  }

  return {
    preview: toDelete.map(d => ({
      device_id: d.device_id,
      display_name: d.display_name || '(unnamed)',
      last_seen_ts: d.last_seen_ts,
      last_seen_ip: d.last_seen_ip,
    })),
    execute: async () => {
      if (toDelete.length === 0) {
        onProgress('No devices to delete');
        return { deleted: 0 };
      }

      const deviceIds = toDelete.map(d => d.device_id);
      onProgress(`Deleting ${deviceIds.length} device(s)...`);

      // First request without auth — gets UIA session
      const resp1 = await matrixFetch(
        homeserverUrl, '/_matrix/client/v3/delete_devices', accessToken,
        { method: 'POST', body: JSON.stringify({ devices: deviceIds }) },
      );

      if (resp1.ok) {
        onProgress(`Deleted ${deviceIds.length} device(s)`);
        return { deleted: deviceIds.length };
      }

      if (resp1.status !== 401) {
        throw new Error(`Device deletion failed: ${resp1.status} ${await resp1.text()}`);
      }

      // UIA required — extract session and retry with password
      const uiaInfo = await resp1.json();
      const session = uiaInfo.session;

      const localpart = userId.split(':')[0].replace('@', '');
      const resp2 = await matrixFetch(
        homeserverUrl, '/_matrix/client/v3/delete_devices', accessToken,
        {
          method: 'POST',
          body: JSON.stringify({
            devices: deviceIds,
            auth: {
              type: 'm.login.password',
              identifier: { type: 'm.id.user', user: localpart },
              password,
              session,
            },
          }),
        },
      );

      if (!resp2.ok) {
        const errBody = await resp2.text();
        throw new Error(`Device deletion UIA failed: ${resp2.status} ${errBody}`);
      }

      onProgress(`Deleted ${deviceIds.length} device(s)`);
      return { deleted: deviceIds.length };
    },
  };
}
