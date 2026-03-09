import { runExecCommand } from './exec-view.js';

let refreshTimer = null;
const cachedSessions = {}; // { exec_room_id: [session, ...] }

export function stopDashboardRefresh() {
  if (refreshTimer) {
    clearInterval(refreshTimer);
    refreshTimer = null;
  }
}

/**
 * Set up the dashboard view.
 * @param {object} client - WasmMatrixClient
 * @param {object} callbacks
 * @param {function} callbacks.onOpenTerminal - Called with launcher info
 */
export function setupDashboard(client, { onOpenTerminal, onReconnect }) {
  stopDashboardRefresh();

  render(client, onOpenTerminal, onReconnect);

  // Auto-refresh every 30s (10s was too aggressive for public homeservers)
  refreshTimer = setInterval(() => {
    render(client, onOpenTerminal, onReconnect);
  }, 30000);
}

async function render(client, onOpenTerminal, onReconnect) {
  const dashboard = document.getElementById('dashboard');

  try {
    // Single sync — all subsequent reads use local cache (O(1) network calls)
    await client.syncOnce();

    // Auto-join any invited rooms (launcher invites client to space/exec/logs)
    try {
      const invited = client.invitedRoomIds();
      if (invited.length > 0) {
        await Promise.all(invited.map(roomId =>
          client.joinRoom(roomId).catch(() => { /* may fail */ }),
        ));
        await client.syncOnce();
      }
    } catch { /* invitedRoomIds may not be available */ }

    // listLauncherSpaces does its own syncOnce internally — TODO: make it cache-only too
    const launchersJson = await client.listLauncherSpaces();
    const launchers = JSON.parse(launchersJson);

    if (launchers.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'no-launchers';
      const p1 = document.createElement('p');
      p1.textContent = 'No launchers discovered.';
      const p2 = document.createElement('p');
      p2.textContent = 'Start a launcher and it will appear here.';
      empty.append(p1, p2);
      dashboard.replaceChildren(empty);
      return;
    }

    // Read telemetry from local cache in parallel — no syncs, no network per launcher
    const telemetryResults = await Promise.all(
      launchers.map(async (launcher) => {
        try {
          const eventsJson = await client.readRoomEvents(launcher.exec_room_id);
          const events = JSON.parse(eventsJson);
          const telemetryEvent = events.find(e => e.type === 'org.mxdx.host_telemetry');
          return telemetryEvent ? telemetryEvent.content : null;
        } catch {
          return null;
        }
      }),
    );

    // Session fetching: only on first load, then on-demand per card
    // (sending an encrypted command to 100 launchers is not viable)
    const launcherData = launchers.map((launcher, i) => ({
      ...launcher,
      telemetry: telemetryResults[i],
      sessions: cachedSessions[launcher.exec_room_id] || [],
    }));

    const grid = document.createElement('div');
    grid.className = 'launcher-grid';

    for (const launcher of launcherData) {
      grid.appendChild(renderCard(launcher, client, onOpenTerminal, onReconnect));
    }

    dashboard.replaceChildren(grid);
  } catch (err) {
    const errDiv = document.createElement('div');
    errDiv.className = 'no-launchers';
    const p = document.createElement('p');
    p.textContent = `Error loading launchers: ${err}`;
    errDiv.appendChild(p);
    dashboard.replaceChildren(errDiv);
  }
}

async function fetchSessions(client, launcher) {
  try {
    const listRequestId = crypto.randomUUID();
    await client.sendEvent(launcher.exec_room_id, 'org.mxdx.command', JSON.stringify({
      action: 'list_sessions',
      request_id: listRequestId,
    }));
    await client.syncOnce();
    const sessionsJson = await client.onRoomEvent(
      launcher.exec_room_id, 'org.mxdx.terminal.sessions', 5,
    );
    if (sessionsJson && sessionsJson !== 'null') {
      const sessionsResponse = JSON.parse(sessionsJson);
      const sessionsContent = sessionsResponse.content || sessionsResponse;
      const sessions = sessionsContent.sessions || [];
      cachedSessions[launcher.exec_room_id] = sessions;
      return sessions;
    }
  } catch { /* sessions not available */ }
  return [];
}

function renderCard(launcher, client, onOpenTerminal, onReconnect) {
  const card = document.createElement('div');
  card.className = 'launcher-card';

  const title = document.createElement('h3');
  title.textContent = launcher.launcher_id;
  card.appendChild(title);

  // Telemetry
  const telDiv = document.createElement('div');
  telDiv.className = 'telemetry';
  const t = launcher.telemetry;
  if (t) {
    appendTelemetryLine(telDiv, 'Hostname', t.hostname || 'unknown');
    appendTelemetryLine(telDiv, 'Platform', `${t.platform || '?'} (${t.arch || '?'})`);
    if (t.cpus != null) appendTelemetryLine(telDiv, 'CPUs', String(t.cpus));
    if (t.total_memory_mb != null) appendTelemetryLine(telDiv, 'Memory', `${t.free_memory_mb || '?'}MB free / ${t.total_memory_mb}MB total`);
    if (t.uptime_secs != null) appendTelemetryLine(telDiv, 'Uptime', `${Math.floor(t.uptime_secs / 3600)}h`);
    if (t.session_persistence != null) {
      appendTelemetryLine(telDiv, 'Session Persistence', t.session_persistence ? 'Yes (tmux)' : 'No');
    }
  } else {
    telDiv.textContent = 'No telemetry data';
  }
  card.appendChild(telDiv);

  // Actions
  const actions = document.createElement('div');
  actions.className = 'actions';

  const termBtn = document.createElement('button');
  termBtn.className = 'btn btn-primary';
  termBtn.textContent = 'Open Terminal';
  termBtn.addEventListener('click', () => {
    if (refreshTimer) {
      clearInterval(refreshTimer);
      refreshTimer = null;
    }
    onOpenTerminal(launcher);
  });
  actions.appendChild(termBtn);
  card.appendChild(actions);

  // Sessions section (populated on-demand)
  const sessionsDiv = document.createElement('div');
  sessionsDiv.className = 'sessions';
  card.appendChild(sessionsDiv);

  function renderSessions(sessions) {
    sessionsDiv.replaceChildren();
    if (sessions.length > 0) {
      const sessionsTitle = document.createElement('h4');
      sessionsTitle.textContent = 'Active Sessions';
      sessionsDiv.appendChild(sessionsTitle);

      for (const session of sessions) {
        const sessionRow = document.createElement('div');
        sessionRow.className = 'session-row';

        const label = document.createElement('span');
        const age = Math.floor((Date.now() - new Date(session.created_at).getTime()) / 60000);
        label.textContent = `${session.session_id} (${age}m ago)${session.persistent ? '' : ' — non-persistent'}`;
        sessionRow.appendChild(label);

        const reconnBtn = document.createElement('button');
        reconnBtn.className = 'btn btn-secondary';
        reconnBtn.textContent = 'Reconnect';
        reconnBtn.addEventListener('click', () => {
          if (refreshTimer) { clearInterval(refreshTimer); refreshTimer = null; }
          onReconnect(launcher, session);
        });
        sessionRow.appendChild(reconnBtn);

        sessionsDiv.appendChild(sessionRow);
      }
    }
  }

  // Show cached sessions if available, add refresh button
  renderSessions(launcher.sessions || []);

  const refreshBtn = document.createElement('button');
  refreshBtn.className = 'btn btn-secondary';
  refreshBtn.textContent = 'Refresh Sessions';
  refreshBtn.addEventListener('click', async () => {
    refreshBtn.disabled = true;
    refreshBtn.textContent = 'Loading...';
    const sessions = await fetchSessions(client, launcher);
    renderSessions(sessions);
    refreshBtn.disabled = false;
    refreshBtn.textContent = 'Refresh Sessions';
  });
  sessionsDiv.appendChild(refreshBtn);

  // Exec input
  const execForm = document.createElement('form');
  execForm.className = 'exec-input';

  const execInput = document.createElement('input');
  execInput.type = 'text';
  execInput.placeholder = 'Run command...';
  execForm.appendChild(execInput);

  const runBtn = document.createElement('button');
  runBtn.type = 'submit';
  runBtn.className = 'btn';
  runBtn.textContent = 'Run';
  execForm.appendChild(runBtn);

  // Exec output panel
  const execOutput = document.createElement('div');
  execOutput.className = 'exec-panel';
  execOutput.hidden = true;

  execForm.addEventListener('submit', (e) => {
    e.preventDefault();
    const cmd = execInput.value.trim();
    if (cmd) {
      runExecCommand(client, launcher, cmd, execOutput);
      execInput.value = '';
    }
  });

  card.appendChild(execForm);
  card.appendChild(execOutput);

  return card;
}

function appendTelemetryLine(container, label, value) {
  const labelSpan = document.createTextNode(`${label}: `);
  const valueSpan = document.createElement('span');
  valueSpan.textContent = value;
  container.append(labelSpan, valueSpan, document.createElement('br'));
}
