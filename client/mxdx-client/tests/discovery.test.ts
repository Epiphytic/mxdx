import { describe, it, expect, vi, beforeEach } from "vitest";
import { MxdxClient } from "../src/client.js";
import type { TerminalSessionHandle } from "../src/client.js";

describe("MxdxClient", () => {
  it("exposes expected API surface", () => {
    const client = new MxdxClient();
    expect(typeof client.connect).toBe("function");
    expect(typeof client.listLaunchers).toBe("function");
    expect(typeof client.getLauncherStatus).toBe("function");
    expect(typeof client.createTerminalSession).toBe("function");
    expect(typeof client.attachTerminalSession).toBe("function");
    expect(client.connected).toBe(false);
  });

  it("connect sets connected state", () => {
    const client = new MxdxClient();
    client.connect("https://matrix.example.com", "syt_token");
    expect(client.connected).toBe(true);
  });

  it("connect rejects empty homeserver", () => {
    const client = new MxdxClient();
    expect(() => client.connect("", "token")).toThrow();
  });

  it("connect rejects empty access token", () => {
    const client = new MxdxClient();
    expect(() => client.connect("https://hs.example.com", "")).toThrow();
  });

  it("fetchJson throws when not connected", async () => {
    const client = new MxdxClient();
    await expect(client.fetchJson("/test")).rejects.toThrow("Not connected");
  });
});

describe("discoverLaunchers", () => {
  let client: MxdxClient;

  beforeEach(() => {
    client = new MxdxClient();
    client.connect("https://matrix.example.com", "syt_token");
  });

  it("discovers launchers from space hierarchy", async () => {
    const hierarchyResponse = {
      rooms: [
        { room_id: "!room1:example.com" },
        { room_id: "!room2:example.com" },
      ],
    };

    const room1State = [
      {
        type: "org.mxdx.launcher.identity",
        state_key: "launcher-a",
        content: {
          launcher_id: "launcher-a",
          accounts: ["@launcher-a:example.com"],
          primary: "@launcher-a:example.com",
          capabilities: ["exec", "terminal"],
          version: "0.1.0",
        },
      },
      {
        type: "m.room.name",
        state_key: "",
        content: { name: "Room 1" },
      },
    ];

    const room2State = [
      {
        type: "m.room.name",
        state_key: "",
        content: { name: "Room 2" },
      },
    ];

    const fetchSpy = vi.spyOn(client, "fetchJson");
    fetchSpy.mockImplementation(async (path: string) => {
      if (path.includes("/hierarchy")) return hierarchyResponse;
      if (path.includes("!room1")) return room1State;
      if (path.includes("!room2")) return room2State;
      throw new Error(`Unexpected path: ${path}`);
    });

    const launchers = await client.listLaunchers("!space:example.com");
    expect(launchers).toHaveLength(1);
    expect(launchers[0].roomId).toBe("!room1:example.com");
    expect(launchers[0].identity.launcher_id).toBe("launcher-a");
    expect(launchers[0].identity.capabilities).toContain("terminal");
  });

  it("skips rooms with invalid launcher identity", async () => {
    const hierarchyResponse = {
      rooms: [{ room_id: "!room1:example.com" }],
    };

    const roomState = [
      {
        type: "org.mxdx.launcher.identity",
        state_key: "bad",
        content: { launcher_id: 123 }, // invalid: missing fields, wrong type
      },
    ];

    const fetchSpy = vi.spyOn(client, "fetchJson");
    fetchSpy.mockImplementation(async (path: string) => {
      if (path.includes("/hierarchy")) return hierarchyResponse;
      return roomState;
    });

    const launchers = await client.listLaunchers("!space:example.com");
    expect(launchers).toHaveLength(0);
  });
});

describe("getLauncherStatus", () => {
  let client: MxdxClient;

  beforeEach(() => {
    client = new MxdxClient();
    client.connect("https://matrix.example.com", "syt_token");
  });

  it("returns telemetry for a launcher", async () => {
    const telemetry = {
      timestamp: "2026-03-05T12:00:00Z",
      hostname: "worker-01",
      os: "linux",
      arch: "x86_64",
      uptime_seconds: 86400,
      load_avg: [0.5, 0.3, 0.1],
      cpu: { cores: 8, usage_percent: 45.2 },
      memory: { total_bytes: 16_000_000_000, used_bytes: 8_000_000_000 },
      disk: { total_bytes: 500_000_000_000, used_bytes: 200_000_000_000 },
    };

    const fetchSpy = vi.spyOn(client, "fetchJson");
    fetchSpy.mockResolvedValue(telemetry);

    const status = await client.getLauncherStatus(
      "!room1:example.com",
      "launcher-a",
    );
    expect(status).not.toBeNull();
    expect(status?.hostname).toBe("worker-01");
    expect(status?.cpu.cores).toBe(8);
  });

  it("returns null on fetch error", async () => {
    const fetchSpy = vi.spyOn(client, "fetchJson");
    fetchSpy.mockRejectedValue(new Error("404"));

    const status = await client.getLauncherStatus(
      "!room1:example.com",
      "launcher-a",
    );
    expect(status).toBeNull();
  });
});

describe("createTerminalSession", () => {
  let client: MxdxClient;

  beforeEach(() => {
    client = new MxdxClient();
    client.connect("https://matrix.example.com", "syt_token");
  });

  it("sends session_request event with correct type and content", async () => {
    const fetchSpy = vi.spyOn(client, "fetchJson");
    fetchSpy.mockResolvedValue({ event_id: "$evt1" });

    const handle: TerminalSessionHandle = await client.createTerminalSession(
      "launcher-a",
      "!room1:example.com",
      "/bin/bash",
      80,
      24,
    );

    expect(handle.sessionId).toBeTruthy();
    expect(typeof handle.sessionId).toBe("string");

    expect(fetchSpy).toHaveBeenCalledOnce();
    const [path, init] = fetchSpy.mock.calls[0];
    expect(path).toContain("org.mxdx.terminal.session_request");
    expect(path).toContain("!room1%3Aexample.com");
    expect(init?.method).toBe("PUT");

    const body = JSON.parse(init?.body as string);
    expect(body.command).toBe("/bin/bash");
    expect(body.cols).toBe(80);
    expect(body.rows).toBe(24);
    expect(body.uuid).toBe(handle.sessionId);
  });

  it("attachTerminalSession returns a TerminalSocket", () => {
    const mockMatrixClient = {
      sendEvent: async () => {},
      onRoomEvent: () => () => {},
    };
    const socket = client.attachTerminalSession(mockMatrixClient, "!room:example.com");
    expect(socket).toBeDefined();
    expect(socket.roomId).toBe("!room:example.com");
    expect(socket.connected).toBe(true);
    socket.close();
  });
});
