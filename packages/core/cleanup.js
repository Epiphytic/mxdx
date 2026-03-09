/**
 * Matrix REST API cleanup utilities.
 * Uses fetch() directly — works in both Node.js 22+ and browser.
 */

/**
 * Parse an --older-than duration string into a cutoff timestamp (ms).
 * Supports: Nh (hours), Nd (days), Nw (weeks), Nm (months = 30 days).
 * Returns null if input is falsy.
 * @param {string} [str] - e.g. "1h", "2w", "1m", "7d"
 * @returns {number|null} Unix timestamp cutoff in ms, or null
 */
export function parseOlderThan(str) {
  if (!str) return null;
  const match = str.trim().match(/^(\d+)([hdwm])$/i);
  if (!match) throw new Error(`Invalid --older-than format: "${str}". Use Nh, Nd, Nw, or Nm (e.g. 1h, 7d, 2w, 1m).`);
  const n = parseInt(match[1], 10);
  const unit = match[2].toLowerCase();
  const msPerHour = 3600000;
  const multipliers = { h: msPerHour, d: 24 * msPerHour, w: 7 * 24 * msPerHour, m: 30 * 24 * msPerHour };
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

async function getLastEventTimestamp(homeserverUrl, accessToken, roomId) {
  const params = new URLSearchParams({ dir: 'b', limit: '1' });
  const resp = await matrixFetch(
    homeserverUrl,
    `/_matrix/client/v3/rooms/${encodeURIComponent(roomId)}/messages?${params}`,
    accessToken,
  );
  if (!resp.ok) return null;
  const data = await resp.json();
  if (data.chunk && data.chunk.length > 0) {
    return data.chunk[0].origin_server_ts || null;
  }
  return null;
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

/**
 * Clean up mxdx rooms. Leaves and forgets all mxdx-related rooms
 * (spaces, exec, logs rooms identified by topic prefix).
 *
 * @param {object} opts
 * @param {string} opts.accessToken
 * @param {string} opts.homeserverUrl
 * @param {string} opts.launchersJson - JSON from client.listLauncherSpaces()
 * @param {number} [opts.olderThan] - Cutoff timestamp (ms). Only clean rooms with no activity since before this.
 * @param {function} [opts.onProgress]
 * @returns {{ preview: Array<{room_id, type, launcher_id}>, execute: function }}
 */
export async function cleanupRooms({
  accessToken, homeserverUrl, launchersJson, olderThan, onProgress = () => {},
}) {
  const launchers = JSON.parse(launchersJson);

  // Collect all rooms: exec, logs, space — in that order (children first)
  let rooms = [];
  for (const l of launchers) {
    rooms.push({ room_id: l.exec_room_id, type: 'exec', launcher_id: l.launcher_id });
    rooms.push({ room_id: l.logs_room_id, type: 'logs', launcher_id: l.launcher_id });
    rooms.push({ room_id: l.space_id, type: 'space', launcher_id: l.launcher_id });
  }

  // If olderThan set, check most recent event in each room and skip active rooms
  if (olderThan) {
    const filtered = [];
    for (const room of rooms) {
      const lastTs = await getLastEventTimestamp(homeserverUrl, accessToken, room.room_id);
      if (lastTs === null || lastTs < olderThan) {
        filtered.push(room);
      } else {
        onProgress(`Skipping ${room.type} room for ${room.launcher_id} (recent activity)`);
      }
    }
    rooms = filtered;
  }

  return {
    preview: rooms,
    execute: async () => {
      let left = 0;
      let errors = 0;

      for (const room of rooms) {
        try {
          onProgress(`Leaving ${room.type} room for ${room.launcher_id}...`);
          const leaveResp = await matrixFetch(
            homeserverUrl,
            `/_matrix/client/v3/rooms/${encodeURIComponent(room.room_id)}/leave`,
            accessToken,
            { method: 'POST', body: '{}' },
          );
          if (!leaveResp.ok && leaveResp.status !== 403) {
            throw new Error(`leave failed: ${leaveResp.status}`);
          }

          onProgress(`Forgetting ${room.type} room for ${room.launcher_id}...`);
          const forgetResp = await matrixFetch(
            homeserverUrl,
            `/_matrix/client/v3/rooms/${encodeURIComponent(room.room_id)}/forget`,
            accessToken,
            { method: 'POST', body: '{}' },
          );
          if (!forgetResp.ok && forgetResp.status !== 403) {
            throw new Error(`forget failed: ${forgetResp.status}`);
          }

          left++;
        } catch (err) {
          onProgress(`Error on ${room.room_id}: ${err.message}`);
          errors++;
        }
      }

      onProgress(`Done: ${left} room(s) left+forgotten, ${errors} error(s)`);
      return { left, errors };
    },
  };
}

/**
 * Clean up mxdx events in exec/logs rooms by redacting them.
 * Redacts org.mxdx.* event types and undecryptable encrypted events.
 *
 * @param {object} opts
 * @param {string} opts.accessToken
 * @param {string} opts.homeserverUrl
 * @param {string} opts.launchersJson - JSON from client.listLauncherSpaces()
 * @param {number} [opts.olderThan] - Cutoff timestamp (ms). Only redact events sent before this.
 * @param {function} [opts.onProgress]
 * @returns {{ preview: Array<{room_id, type, event_count}>, execute: function }}
 */
export async function cleanupEvents({
  accessToken, homeserverUrl, launchersJson, olderThan, onProgress = () => {},
}) {
  const launchers = JSON.parse(launchersJson);

  onProgress('Scanning rooms for mxdx events...');
  const roomSummaries = [];

  for (const l of launchers) {
    for (const [type, roomId] of [['exec', l.exec_room_id], ['logs', l.logs_room_id]]) {
      const events = await fetchMxdxEvents(homeserverUrl, accessToken, roomId, olderThan);
      if (events.length > 0) {
        roomSummaries.push({
          room_id: roomId,
          type,
          launcher_id: l.launcher_id,
          event_count: events.length,
          _events: events,
        });
      }
    }
  }

  return {
    preview: roomSummaries.map(({ _events, ...rest }) => rest),
    execute: async () => {
      let redacted = 0;
      let errors = 0;

      for (const summary of roomSummaries) {
        onProgress(`Redacting ${summary._events.length} event(s) in ${summary.type} room for ${summary.launcher_id}...`);
        for (const event of summary._events) {
          try {
            const txnId = crypto.randomUUID();
            const resp = await matrixFetch(
              homeserverUrl,
              `/_matrix/client/v3/rooms/${encodeURIComponent(summary.room_id)}/redact/${encodeURIComponent(event.event_id)}/${txnId}`,
              accessToken,
              { method: 'PUT', body: JSON.stringify({ reason: 'mxdx cleanup' }) },
            );
            if (!resp.ok) throw new Error(`${resp.status}`);
            redacted++;
          } catch (err) {
            onProgress(`Error redacting ${event.event_id}: ${err.message}`);
            errors++;
          }
        }
      }

      onProgress(`Done: ${redacted} event(s) redacted, ${errors} error(s)`);
      return { redacted, errors };
    },
  };
}

async function fetchMxdxEvents(homeserverUrl, accessToken, roomId, olderThan) {
  const events = [];
  let from = '';

  for (let page = 0; page < 10; page++) {
    const params = new URLSearchParams({ dir: 'b', limit: '100' });
    if (from) params.set('from', from);

    const resp = await matrixFetch(
      homeserverUrl,
      `/_matrix/client/v3/rooms/${encodeURIComponent(roomId)}/messages?${params}`,
      accessToken,
    );
    if (!resp.ok) break;

    const data = await resp.json();
    for (const chunk of (data.chunk || [])) {
      const t = chunk.type || '';
      if (t.startsWith('org.mxdx.') || t === 'm.room.encrypted') {
        if (olderThan && chunk.origin_server_ts && chunk.origin_server_ts >= olderThan) {
          continue;
        }
        events.push({ event_id: chunk.event_id, type: t, origin_server_ts: chunk.origin_server_ts });
      }
    }

    if (!data.end || data.chunk.length === 0) break;
    from = data.end;
  }

  return events;
}
