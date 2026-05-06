import { TerminalDataEvent, processTerminalInput } from '@mxdx/core';
// Rust equivalent: crates/mxdx-core-wasm/src/lib.rs::process_terminal_input

/**
 * Multiplexes terminal I/O across sessions sharing a single DM room.
 *
 * Rust equivalent: none — PTY I/O multiplexing is OS-bound via node-pty
 * (see ADR docs/adr/2026-04-29-rust-npm-binary-parity.md Pillar 3 OS-bound
 * wrapper table). Session-routing logic that does NOT touch PTY I/O lives
 * in `crates/mxdx-core-wasm/src/lib.rs::WasmSessionManager`; the transport
 * connection lifecycle lives in
 * `crates/mxdx-core-wasm/src/lib.rs::SessionTransportManager`. This file
 * fans incoming `org.mxdx.terminal.data` events out to the right `PtyBridge`
 * (a node-pty wrapper) and forwards local PTY output to per-session
 * `WasmBatchedSender` instances — both of those endpoints are inherently
 * Node/native-bound.
 */
export class SessionMux {
  #transport; #roomId; #launcherUserId; #sessions = new Map(); #senders = new Map();
  #running = false; #log;

  constructor(transport, roomId, launcherUserId, log) {
    this.#transport = transport; this.#roomId = roomId;
    this.#launcherUserId = launcherUserId; this.#log = log;
  }

  addSession(sessionId, pty) {
    this.#sessions.set(sessionId, { pty });
    if (!this.#running) this.#start();
  }

  registerSender(sessionId, sender) { this.#senders.set(sessionId, sender); }

  removeSession(sessionId) {
    this.#sessions.delete(sessionId); this.#senders.delete(sessionId);
    if (this.#sessions.size === 0) this.#running = false;
  }

  setBatchMs(ms) { for (const s of this.#senders.values()) s.batchMs = ms; }

  get sessionCount() { return this.#sessions.size; }

  #start() {
    this.#running = true;
    this.#poll().catch((err) => this.#log.warn('SessionMux poll error', { room_id: this.#roomId, error: err.message }));
  }

  async #poll() {
    this.#pollResize();
    while (this.#running && this.#sessions.size > 0) {
      try {
        const dataJson = await this.#transport.onRoomEvent(this.#roomId, 'org.mxdx.terminal.data', 1);
        if (dataJson != null) {
          const event = JSON.parse(dataJson);
          const content = event.content || event;
          const sender = event.sender;
          const sessionId = content.session_id;
          if (sender !== this.#launcherUserId && sessionId) {
            const session = this.#sessions.get(sessionId);
            if (session) this.#processInput(content, session.pty);
          }
        }
      } catch { await new Promise((r) => setTimeout(r, 1000)); }
    }
  }

  async #pollResize() {
    while (this.#running && this.#sessions.size > 0) {
      try {
        const resizeJson = await this.#transport.onRoomEvent(this.#roomId, 'org.mxdx.terminal.resize', 2);
        if (resizeJson != null) {
          const event = JSON.parse(resizeJson);
          const content = event.content || event;
          const sessionId = content.session_id;
          if (sessionId) {
            const session = this.#sessions.get(sessionId);
            if (session && content.cols && content.rows) session.pty.resize(content.cols, content.rows);
          }
        }
      } catch { await new Promise((r) => setTimeout(r, 1000)); }
    }
  }

  #processInput(content, pty) {
    const parsed = TerminalDataEvent.safeParse(content);
    if (!parsed.success) return;
    const { data, encoding } = parsed.data;
    try {
      // Rust equivalent: crates/mxdx-core-wasm/src/lib.rs::process_terminal_input
      const bytes = processTerminalInput(data, encoding || 'base64');
      pty.write(bytes);
    } catch { /* zlib bomb protection — WASM throws on oversized decompression */ }
  }
}
