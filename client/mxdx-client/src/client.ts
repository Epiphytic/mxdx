import type { HostTelemetryEvent } from "./types/index.js";
import {
  discoverLaunchers,
  getLauncherStatus,
  type DiscoveredLauncher,
} from "./discovery.js";

export interface MatrixClient {
  fetchJson<T>(path: string, init?: RequestInit): Promise<T>;
}

export interface TerminalSessionHandle {
  sessionId: string;
  roomId: string | null;
}

export interface TerminalSocket {
  readonly sessionId: string;
  readonly roomId: string;
}

export class MxdxClient {
  private homeserver: string | null = null;
  private accessToken: string | null = null;

  get connected(): boolean {
    return this.homeserver !== null && this.accessToken !== null;
  }

  connect(homeserver: string, accessToken: string): void {
    if (!homeserver || !accessToken) {
      throw new Error("homeserver and accessToken are required");
    }
    this.homeserver = homeserver.replace(/\/+$/, "");
    this.accessToken = accessToken;
  }

  async fetchJson<T>(path: string, init?: RequestInit): Promise<T> {
    if (!this.homeserver || !this.accessToken) {
      throw new Error("Not connected. Call connect() first.");
    }
    const url = `${this.homeserver}${path}`;
    const headers: Record<string, string> = {
      Authorization: `Bearer ${this.accessToken}`,
      "Content-Type": "application/json",
      ...((init?.headers as Record<string, string>) ?? {}),
    };
    const response = await fetch(url, { ...init, headers });
    if (!response.ok) {
      throw new Error(`Matrix API error: ${response.status} ${response.statusText}`);
    }
    return (await response.json()) as T;
  }

  async listLaunchers(spaceRoomId: string): Promise<DiscoveredLauncher[]> {
    return discoverLaunchers(this, spaceRoomId);
  }

  async getLauncherStatus(
    roomId: string,
    launcherId: string,
  ): Promise<HostTelemetryEvent | null> {
    return getLauncherStatus(this, roomId, launcherId);
  }

  async createTerminalSession(
    launcherId: string,
    roomId: string,
    command: string,
    cols: number,
    rows: number,
  ): Promise<TerminalSessionHandle> {
    const uuid = crypto.randomUUID();
    const content = { uuid, command, env: { TERM: "xterm-256color" }, cols, rows };

    const txnId = `mxdx-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    await this.fetchJson(
      `/_matrix/client/v3/rooms/${encodeURIComponent(roomId)}/send/org.mxdx.terminal.session_request/${encodeURIComponent(txnId)}`,
      { method: "PUT", body: JSON.stringify(content) },
    );

    return { sessionId: uuid, roomId: null };
  }

  async attachTerminalSession(
    _sessionId: string,
    _roomId: string,
  ): Promise<TerminalSocket> {
    throw new Error("Not implemented: TerminalSocket will be added in Task 7.3");
  }
}
