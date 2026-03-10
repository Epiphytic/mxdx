import { parseOlderThan, cleanupDevices, cleanupRooms, cleanupEvents } from '../../core/cleanup.js';
import { loadSession } from './session-store.js';

let initialized = false;

export function setupSettings(client) {
  const container = document.getElementById('settings');
  if (initialized) return;
  initialized = true;

  // Title
  const title = document.createElement('h2');
  title.textContent = 'Settings';
  title.className = 'settings-title';
  container.appendChild(title);

  // Wrapper
  const wrapper = document.createElement('div');
  wrapper.className = 'settings-wrapper';
  container.appendChild(wrapper);

  // Tab bar
  const tabBar = document.createElement('div');
  tabBar.className = 'settings-tabs';
  const cleanupTab = document.createElement('button');
  cleanupTab.className = 'settings-tab active';
  cleanupTab.textContent = 'Server Cleanup';
  tabBar.appendChild(cleanupTab);
  const p2pTab = document.createElement('button');
  p2pTab.className = 'settings-tab';
  p2pTab.textContent = 'P2P Transport';
  tabBar.appendChild(p2pTab);
  wrapper.appendChild(tabBar);

  // Cleanup panel
  const panel = document.createElement('div');
  panel.className = 'cleanup-panel';
  wrapper.appendChild(panel);

  // P2P settings panel
  const p2pPanel = document.createElement('div');
  p2pPanel.className = 'cleanup-panel';
  p2pPanel.hidden = true;
  wrapper.appendChild(p2pPanel);

  // Tab switching
  cleanupTab.addEventListener('click', () => {
    cleanupTab.classList.add('active');
    p2pTab.classList.remove('active');
    panel.hidden = false;
    p2pPanel.hidden = true;
  });
  p2pTab.addEventListener('click', () => {
    p2pTab.classList.add('active');
    cleanupTab.classList.remove('active');
    p2pPanel.hidden = false;
    panel.hidden = true;
  });

  // --- P2P Settings ---
  buildP2PSettings(p2pPanel);

  // Checkboxes
  const targets = ['devices', 'events', 'rooms'];
  const checkboxes = {};
  for (const target of targets) {
    const label = document.createElement('label');
    label.className = 'cleanup-option';
    const cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.name = target;
    checkboxes[target] = cb;
    label.appendChild(cb);
    const span = document.createElement('span');
    span.textContent = ` ${target.charAt(0).toUpperCase() + target.slice(1)}`;
    label.appendChild(span);
    panel.appendChild(label);
  }

  // Older Than input
  const olderThanLabel = document.createElement('label');
  olderThanLabel.className = 'cleanup-pw-label';
  olderThanLabel.textContent = 'Older Than (optional)';
  panel.appendChild(olderThanLabel);
  const olderThanInput = document.createElement('input');
  olderThanInput.type = 'text';
  olderThanInput.className = 'cleanup-input';
  olderThanInput.placeholder = 'e.g. 2w, 1m, 7d';
  panel.appendChild(olderThanInput);

  // Password input
  const pwLabel = document.createElement('label');
  pwLabel.className = 'cleanup-pw-label';
  pwLabel.textContent = 'Password (required for device cleanup)';
  panel.appendChild(pwLabel);
  const pwInput = document.createElement('input');
  pwInput.type = 'password';
  pwInput.className = 'cleanup-input';
  pwInput.placeholder = 'Matrix account password';
  panel.appendChild(pwInput);

  // Buttons
  const actions = document.createElement('div');
  actions.className = 'cleanup-actions';
  const previewBtn = document.createElement('button');
  previewBtn.className = 'btn';
  previewBtn.textContent = 'Preview Cleanup';
  actions.appendChild(previewBtn);
  const runBtn = document.createElement('button');
  runBtn.className = 'btn btn-danger';
  runBtn.textContent = 'Run Cleanup';
  runBtn.disabled = true;
  actions.appendChild(runBtn);
  panel.appendChild(actions);

  // Output area
  const output = document.createElement('pre');
  output.className = 'cleanup-output';
  panel.appendChild(output);

  function appendOutput(msg) {
    output.textContent += msg + '\n';
    output.scrollTop = output.scrollHeight;
  }

  function clearOutput() {
    output.textContent = '';
  }

  // State for preview results
  let previewResults = null;

  previewBtn.addEventListener('click', async () => {
    const selected = targets.filter(t => checkboxes[t].checked);
    if (selected.length === 0) {
      clearOutput();
      appendOutput('Select at least one cleanup target.');
      return;
    }

    if (selected.includes('devices') && !pwInput.value) {
      clearOutput();
      appendOutput('Password is required for device cleanup.');
      return;
    }

    let olderThan;
    try {
      olderThan = parseOlderThan(olderThanInput.value || null);
    } catch (err) {
      clearOutput();
      appendOutput(`Error: ${err.message}`);
      return;
    }

    clearOutput();
    previewResults = {};
    previewBtn.disabled = true;
    runBtn.disabled = true;

    try {
      const session = JSON.parse(client.exportSession());
      const accessToken = session.access_token;
      const homeserverUrl = session.homeserver_url;
      const userId = client.userId();
      const currentDeviceId = client.deviceId();

      let launchersJson;
      if (selected.includes('events') || selected.includes('rooms')) {
        launchersJson = await client.listLauncherSpaces();
      }

      for (const target of selected) {
        if (target === 'devices') {
          previewResults.devices = await cleanupDevices({
            accessToken, homeserverUrl, currentDeviceId, userId,
            password: pwInput.value, olderThan, onProgress: appendOutput,
          });
          appendOutput(`\nDevices to delete (${previewResults.devices.preview.length}):`);
          for (const d of previewResults.devices.preview) {
            const ts = d.last_seen_ts ? new Date(d.last_seen_ts).toISOString() : 'unknown';
            appendOutput(`  ${d.device_id} — ${d.display_name} (last seen: ${ts})`);
          }
        } else if (target === 'events') {
          previewResults.events = await cleanupEvents({
            accessToken, homeserverUrl, launchersJson, userId,
            olderThan, onProgress: appendOutput,
          });
          appendOutput(`\nEvents to redact:`);
          for (const r of previewResults.events.preview) {
            appendOutput(`  ${r.type} room for ${r.launcher_id}: ${r.event_count} event(s)`);
          }
        } else if (target === 'rooms') {
          previewResults.rooms = await cleanupRooms({
            accessToken, homeserverUrl, launchersJson, olderThan, onProgress: appendOutput,
          });
          appendOutput(`\nRooms to leave+forget (${previewResults.rooms.preview.length}):`);
          for (const r of previewResults.rooms.preview) {
            appendOutput(`  ${r.type} — ${r.launcher_id} (${r.room_id})`);
          }
        }
      }

      const totalItems = Object.values(previewResults).reduce((sum, r) => {
        return sum + (r.preview ? (Array.isArray(r.preview) ? r.preview.length : 0) : 0);
      }, 0);

      if (totalItems > 0) {
        runBtn.disabled = false;
      } else {
        appendOutput('\nNothing to clean up.');
      }
    } catch (err) {
      appendOutput(`\nError: ${err.message}`);
    } finally {
      previewBtn.disabled = false;
    }
  });

  runBtn.addEventListener('click', () => {
    if (!previewResults) return;
    showConfirmModal(() => executeCleanup());
  });

  async function executeCleanup() {
    runBtn.disabled = true;
    previewBtn.disabled = true;
    appendOutput('\n--- Executing cleanup ---');

    for (const [target, result] of Object.entries(previewResults)) {
      try {
        const outcome = await result.execute();
        appendOutput(`${target}: ${JSON.stringify(outcome)}`);
      } catch (err) {
        appendOutput(`Error executing ${target}: ${err.message}`);
      }
    }

    appendOutput('\nCleanup complete.');
    previewResults = null;
    previewBtn.disabled = false;
  }

  function buildP2PSettings(container) {
    // Description
    const desc = document.createElement('p');
    desc.style.cssText = 'color: var(--text-muted); font-size: 0.8125rem; margin-bottom: 1rem;';
    desc.textContent = 'WebRTC P2P data channels bypass homeserver latency for interactive terminal sessions. Terminal data is always Megolm-encrypted on the P2P path.';
    container.appendChild(desc);

    // P2P Enabled checkbox
    const enabledLabel = document.createElement('label');
    enabledLabel.className = 'cleanup-option';
    const enabledCb = document.createElement('input');
    enabledCb.type = 'checkbox';
    enabledCb.checked = localStorage.getItem('mxdx-p2p-enabled') !== 'false';
    enabledLabel.appendChild(enabledCb);
    const enabledSpan = document.createElement('span');
    enabledSpan.textContent = ' Enable P2P transport';
    enabledLabel.appendChild(enabledSpan);
    container.appendChild(enabledLabel);

    enabledCb.addEventListener('change', () => {
      localStorage.setItem('mxdx-p2p-enabled', enabledCb.checked ? 'true' : 'false');
    });

    // Batch Ms input
    const batchLabel = document.createElement('label');
    batchLabel.className = 'cleanup-pw-label';
    batchLabel.textContent = 'P2P batch interval (ms) — lower = more responsive, higher = fewer messages';
    container.appendChild(batchLabel);
    const batchInput = document.createElement('input');
    batchInput.type = 'number';
    batchInput.className = 'cleanup-input';
    batchInput.min = '1';
    batchInput.max = '1000';
    batchInput.value = clampP2PValue(localStorage.getItem('mxdx-p2p-batch-ms'), 10, 1, 1000);
    container.appendChild(batchInput);

    batchInput.addEventListener('change', () => {
      const val = clampP2PValue(batchInput.value, 10, 1, 1000);
      batchInput.value = val;
      localStorage.setItem('mxdx-p2p-batch-ms', String(val));
    });

    // Idle timeout input
    const idleLabel = document.createElement('label');
    idleLabel.className = 'cleanup-pw-label';
    idleLabel.textContent = 'P2P idle timeout (seconds) — P2P channel torn down after this much inactivity';
    container.appendChild(idleLabel);
    const idleInput = document.createElement('input');
    idleInput.type = 'number';
    idleInput.className = 'cleanup-input';
    idleInput.min = '30';
    idleInput.max = '3600';
    idleInput.value = clampP2PValue(localStorage.getItem('mxdx-p2p-idle-timeout-s'), 300, 30, 3600);
    container.appendChild(idleInput);

    idleInput.addEventListener('change', () => {
      const val = clampP2PValue(idleInput.value, 300, 30, 3600);
      idleInput.value = val;
      localStorage.setItem('mxdx-p2p-idle-timeout-s', String(val));
    });

    // Status line
    const statusLine = document.createElement('p');
    statusLine.style.cssText = 'color: var(--text-muted); font-size: 0.75rem; margin-top: 1rem;';
    statusLine.textContent = 'Changes take effect on the next terminal session.';
    container.appendChild(statusLine);
  }

  function clampP2PValue(raw, defaultVal, min, max) {
    const n = parseInt(raw, 10);
    if (isNaN(n)) return defaultVal;
    return Math.max(min, Math.min(max, n));
  }

  function showConfirmModal(onConfirm) {
    const overlay = document.createElement('div');
    overlay.className = 'cleanup-modal-overlay';

    const modal = document.createElement('div');
    modal.className = 'cleanup-modal';

    const msg = document.createElement('p');
    msg.textContent = 'Are you sure? This cannot be undone.';
    modal.appendChild(msg);

    const modalActions = document.createElement('div');
    modalActions.className = 'cleanup-modal-actions';

    const cancelBtn = document.createElement('button');
    cancelBtn.className = 'btn';
    cancelBtn.textContent = 'Cancel';
    cancelBtn.addEventListener('click', () => overlay.remove());
    modalActions.appendChild(cancelBtn);

    const confirmBtn = document.createElement('button');
    confirmBtn.className = 'btn btn-danger';
    confirmBtn.textContent = 'Confirm Cleanup';
    confirmBtn.addEventListener('click', () => {
      overlay.remove();
      onConfirm();
    });
    modalActions.appendChild(confirmBtn);

    modal.appendChild(modalActions);
    overlay.appendChild(modal);
    document.body.appendChild(overlay);
  }
}
