# Cleanup Utility Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a `cleanup` command (devices/events/rooms) to the CLI client, launcher, and web console — using direct Matrix REST API calls via `fetch()`.

**Architecture:** Shared core module `packages/core/cleanup.js` with three pure functions that take session credentials and return `{ preview, execute }`. CLI consumers add a `cleanup` commander subcommand. Web console adds a settings page with Server Cleanup tab. All confirmations are preview-first, execute-after-approval.

**Tech Stack:** `fetch()` (Node 22 + browser native), commander.js (CLI), vanilla JS DOM (web console)

---

### Task 1: Shared cleanup core — `cleanupDevices`

**Files:**
- Create: `packages/core/cleanup.js`

**Step 1: Create `packages/core/cleanup.js` with `cleanupDevices`**

```js
/**
 * Matrix REST API cleanup utilities.
 * Uses fetch() directly — works in both Node.js 22+ and browser.
 */

async function matrixFetch(homeserverUrl, path, accessToken, options = {}) {
  const url = `${homeserverUrl}${path}`;
  const resp = await fetch(url, {
    ...options,
    headers: {
      'Authorization': `Bearer ${accessToken}`,
      'Content-Type': 'application/json',
      ...options.headers,
    },
  });
  if (resp.status === 429) {
    const retryAfter = parseInt(resp.headers.get('Retry-After') || '5', 10);
    await new Promise(r => setTimeout(r, retryAfter * 1000));
    return matrixFetch(homeserverUrl, path, accessToken, options);
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
 * @param {function} [opts.onProgress] - Called with status messages
 * @returns {{ preview: Array<{device_id, display_name, last_seen_ts}>, execute: function }}
 */
export async function cleanupDevices({
  accessToken, homeserverUrl, currentDeviceId, userId, password, onProgress = () => {},
}) {
  onProgress('Fetching device list...');
  const resp = await matrixFetch(homeserverUrl, '/_matrix/client/v3/devices', accessToken);
  if (!resp.ok) throw new Error(`Failed to list devices: ${resp.status}`);
  const { devices } = await resp.json();

  const toDelete = devices.filter(d => d.device_id !== currentDeviceId);

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
```

**Step 2: Verify module loads**

```bash
node -e "import('./packages/core/cleanup.js').then(m => console.log(Object.keys(m)))"
```

Expected: `[ 'cleanupDevices' ]`

**Step 3: Commit**

```bash
git add packages/core/cleanup.js
git commit -m "feat: add cleanupDevices to @mxdx/core cleanup module"
```

---

### Task 2: Add `cleanupRooms` to shared core

**Files:**
- Modify: `packages/core/cleanup.js`

**Step 1: Add `cleanupRooms` function**

Append to `packages/core/cleanup.js`:

```js
/**
 * Clean up mxdx rooms. Leaves and forgets all mxdx-related rooms
 * (spaces, exec, logs rooms identified by topic prefix).
 *
 * @param {object} opts
 * @param {string} opts.accessToken
 * @param {string} opts.homeserverUrl
 * @param {string} opts.launchersJson - JSON from client.listLauncherSpaces()
 * @param {function} [opts.onProgress]
 * @returns {{ preview: Array<{room_id, type, launcher_id}>, execute: function }}
 */
export async function cleanupRooms({
  accessToken, homeserverUrl, launchersJson, onProgress = () => {},
}) {
  const launchers = JSON.parse(launchersJson);

  // Collect all rooms: exec, logs, space — in that order (children first)
  const rooms = [];
  for (const l of launchers) {
    rooms.push({ room_id: l.exec_room_id, type: 'exec', launcher_id: l.launcher_id });
    rooms.push({ room_id: l.logs_room_id, type: 'logs', launcher_id: l.launcher_id });
    rooms.push({ room_id: l.space_id, type: 'space', launcher_id: l.launcher_id });
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
```

**Step 2: Commit**

```bash
git add packages/core/cleanup.js
git commit -m "feat: add cleanupRooms to @mxdx/core cleanup module"
```

---

### Task 3: Add `cleanupEvents` to shared core + export from index

**Files:**
- Modify: `packages/core/cleanup.js`
- Modify: `packages/core/index.js`

**Step 1: Add `cleanupEvents` and `fetchMxdxEvents` helper**

Append to `packages/core/cleanup.js`:

```js
/**
 * Clean up mxdx events in exec/logs rooms by redacting them.
 * Redacts org.mxdx.* event types and undecryptable encrypted events.
 *
 * @param {object} opts
 * @param {string} opts.accessToken
 * @param {string} opts.homeserverUrl
 * @param {string} opts.launchersJson - JSON from client.listLauncherSpaces()
 * @param {function} [opts.onProgress]
 * @returns {{ preview: Array<{room_id, type, event_count}>, execute: function }}
 */
export async function cleanupEvents({
  accessToken, homeserverUrl, launchersJson, onProgress = () => {},
}) {
  const launchers = JSON.parse(launchersJson);

  onProgress('Scanning rooms for mxdx events...');
  const roomSummaries = [];

  for (const l of launchers) {
    for (const [type, roomId] of [['exec', l.exec_room_id], ['logs', l.logs_room_id]]) {
      const events = await fetchMxdxEvents(homeserverUrl, accessToken, roomId);
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

async function fetchMxdxEvents(homeserverUrl, accessToken, roomId) {
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
        events.push({ event_id: chunk.event_id, type: t });
      }
    }

    if (!data.end || data.chunk.length === 0) break;
    from = data.end;
  }

  return events;
}
```

**Step 2: Add export to `packages/core/index.js`**

Add this line:

```js
export { cleanupDevices, cleanupRooms, cleanupEvents } from './cleanup.js';
```

**Step 3: Commit**

```bash
git add packages/core/cleanup.js packages/core/index.js
git commit -m "feat: add cleanupEvents and export all cleanup functions from @mxdx/core"
```

---

### Task 4: CLI cleanup subcommand for `mxdx-client`

**Files:**
- Modify: `packages/client/bin/mxdx-client.js`

**Step 1: Add `cleanup` command and `confirmPrompt` helper**

Add cleanup command after the `telemetry` command block (before `program.parse()`). Add `confirmPrompt` helper after the `connect` function. See `docs/plans/2026-03-08-cleanup-utility-design.md` for the full CLI interface spec.

The cleanup command should: parse comma-separated targets, validate them, connect, get session credentials, run preview for each target, display results, prompt for confirmation (unless `--force-cleanup`), then execute.

**Step 2: Verify**

```bash
node packages/client/bin/mxdx-client.js cleanup --help
```

**Step 3: Commit**

```bash
git add packages/client/bin/mxdx-client.js
git commit -m "feat: add cleanup subcommand to mxdx-client CLI"
```

---

### Task 5: CLI cleanup subcommand for `mxdx-launcher`

**Files:**
- Modify: `packages/launcher/bin/mxdx-launcher.js`

**Step 1: Restructure launcher to use commander subcommands**

The launcher currently runs directly on `.parse()`. Restructure: extract config resolution to `resolveConfig(opts)`, add `start` as default command (preserves existing `mxdx-launcher` behavior), add `cleanup` command identical to the client version.

Key: `{ isDefault: true }` on the `start` command ensures `mxdx-launcher` (no subcommand) still starts the agent.

**Step 2: Verify both commands**

```bash
node packages/launcher/bin/mxdx-launcher.js --help
node packages/launcher/bin/mxdx-launcher.js cleanup --help
```

**Step 3: Commit**

```bash
git add packages/launcher/bin/mxdx-launcher.js
git commit -m "feat: add cleanup subcommand to mxdx-launcher CLI"
```

---

### Task 6: Web console — settings page HTML + routing

**Files:**
- Modify: `packages/web-console/index.html`
- Modify: `packages/web-console/src/main.js`

**Step 1: Add settings screen and nav button to `index.html`**

In the `<nav>` (line 13-16), add a Settings button between Dashboard and Logout:

```html
<button id="nav-settings" class="nav-btn">Settings</button>
```

After the terminal div (line 44), add:

```html
<div id="settings" class="screen" hidden></div>
```

**Step 2: Add routing to `main.js`**

- Import `setupSettings` from `./settings.js`
- Add `showSettings()` function (hides all other screens, shows settings, updates nav active state)
- Update `showDashboard()` to also hide settings and clear its nav active state
- Wire `nav-settings` click handler

**Step 3: Commit**

```bash
git add packages/web-console/index.html packages/web-console/src/main.js
git commit -m "feat: add settings page routing and nav to web console"
```

---

### Task 7: Web console — settings.js with Server Cleanup tab

**Files:**
- Create: `packages/web-console/src/settings.js`

**Step 1: Create settings module**

Build the settings page entirely with DOM methods (no innerHTML — security requirement). The page has:
- Title "Settings"
- Tab bar with "Server Cleanup" tab (placeholder for future tabs)
- Three checkboxes: Devices, Events, Rooms
- Password input (for device UIA)
- "Preview Cleanup" button → populates output area with preview
- "Run Cleanup" button (disabled until preview shows items) → shows confirmation modal → executes
- Output area (monospace, scrollable)

The confirmation modal is built with `document.createElement` — no innerHTML.

Import cleanup functions via `@mxdx/core/cleanup.js`. Get session credentials from `loadSession()` (IndexedDB).

**Step 2: Commit**

```bash
git add packages/web-console/src/settings.js
git commit -m "feat: add settings page with Server Cleanup tab"
```

---

### Task 8: Web console — styles for settings + cleanup

**Files:**
- Modify: `packages/web-console/src/style.css`

**Step 1: Append styles**

Add CSS for: `.settings-wrapper`, `.settings-tabs`, `.settings-tab`, `.cleanup-option`, `.cleanup-pw-label`, `.cleanup-input`, `.cleanup-actions`, `.btn-danger`, `.cleanup-output`, `.cleanup-modal-overlay`, `.cleanup-modal`, `.cleanup-modal-actions`.

Follow existing design tokens (`--bg`, `--surface`, `--border`, `--text`, `--text-muted`, `--accent`, `--error`).

**Step 2: Commit**

```bash
git add packages/web-console/src/style.css
git commit -m "feat: add settings and cleanup styles to web console"
```

---

### Task 9: Fix browser import path for cleanup module

**Files:**
- Possibly modify: `packages/core/package.json` (add exports for cleanup.js)
- Possibly modify: `packages/web-console/src/settings.js`

**Step 1: Verify the import works in browser**

Vite resolves workspace packages via monorepo symlinks. Test if `import('@mxdx/core/cleanup.js')` works. If not, check `packages/core/package.json` exports field and add `"./cleanup.js": "./cleanup.js"`.

**Step 2: Commit if needed**

```bash
git add packages/core/package.json packages/web-console/src/settings.js
git commit -m "fix: resolve cleanup module import path for browser"
```

---

### Task 10: End-to-end test — CLI cleanup

**Step 1: Test device cleanup preview**

```bash
node packages/client/bin/mxdx-client.js cleanup devices
```

Expected: Lists devices, prompts, answer N.

**Step 2: Test with --force-cleanup**

```bash
node packages/client/bin/mxdx-client.js cleanup devices --force-cleanup
```

Expected: Deletes old devices.

**Step 3: Verify clean**

```bash
node packages/client/bin/mxdx-client.js cleanup devices
```

Expected: "Nothing to clean up."

---

### Task 11: End-to-end test — web console cleanup

**Step 1:** Navigate to Settings, verify UI renders.
**Step 2:** Check Devices, enter password, click Preview. Verify list.
**Step 3:** Click Run Cleanup, verify modal, confirm, verify execution.

---

### Task 12: Final push

```bash
git push
```
