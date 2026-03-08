/**
 * Run a non-interactive command on a launcher and display output.
 * @param {object} client - WasmMatrixClient
 * @param {object} launcher - Launcher info with exec_room_id
 * @param {string} cmdString - Full command string (e.g. "ls -la /tmp")
 * @param {HTMLElement} outputEl - Output panel element
 */
export async function runExecCommand(client, launcher, cmdString, outputEl) {
  const parts = cmdString.split(/\s+/);
  const command = parts[0];
  const args = parts.slice(1);
  const requestId = crypto.randomUUID();

  outputEl.hidden = false;
  outputEl.replaceChildren();

  const statusLine = document.createElement('div');
  statusLine.className = 'stdout';
  statusLine.textContent = `$ ${cmdString}`;
  outputEl.appendChild(statusLine);

  // Send command
  try {
    await client.sendEvent(
      launcher.exec_room_id,
      'org.mxdx.command',
      JSON.stringify({
        request_id: requestId,
        command,
        args,
        cwd: '/tmp',
      }),
    );
  } catch (err) {
    const errLine = document.createElement('div');
    errLine.className = 'stderr';
    errLine.textContent = `Failed to send command: ${err}`;
    outputEl.appendChild(errLine);
    return;
  }

  // Poll for output and result events
  const deadline = Date.now() + 30000;
  let done = false;

  while (!done && Date.now() < deadline) {
    try {
      await client.syncOnce();
      const eventsJson = await client.collectRoomEvents(launcher.exec_room_id, 1);
      const events = JSON.parse(eventsJson);

      for (const event of events) {
        if (event.type === 'org.mxdx.output' && event.content?.request_id === requestId) {
          const data = atob(event.content.data);
          const line = document.createElement('div');
          line.className = event.content.stream === 'stderr' ? 'stderr' : 'stdout';
          line.textContent = data;
          outputEl.appendChild(line);
          outputEl.scrollTop = outputEl.scrollHeight;
        }

        if (event.type === 'org.mxdx.result' && event.content?.request_id === requestId) {
          done = true;
          const exitLine = document.createElement('div');
          exitLine.className = 'exit-code';
          exitLine.textContent = `Exit code: ${event.content.exit_code}`;
          if (event.content.error) {
            exitLine.textContent += ` (${event.content.error})`;
          }
          outputEl.appendChild(exitLine);
        }
      }
    } catch {
      // Sync error, retry
      await new Promise((r) => setTimeout(r, 500));
    }
  }

  if (!done) {
    const timeoutLine = document.createElement('div');
    timeoutLine.className = 'stderr';
    timeoutLine.textContent = 'Timed out waiting for result';
    outputEl.appendChild(timeoutLine);
  }
}
