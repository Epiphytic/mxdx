import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { deflateSync } from "node:zlib";
import { TerminalSocket, type TerminalMatrixClient } from "../src/terminal.js";

function createMockClient(): TerminalMatrixClient & {
  _listeners: Map<string, Array<(content: Record<string, unknown>) => void>>;
  _sentEvents: Array<{ roomId: string; eventType: string; content: Record<string, unknown> }>;
  _simulateEvent: (roomId: string, eventType: string, content: Record<string, unknown>) => void;
} {
  const listeners = new Map<string, Array<(content: Record<string, unknown>) => void>>();
  const sentEvents: Array<{ roomId: string; eventType: string; content: Record<string, unknown> }> = [];

  return {
    _listeners: listeners,
    _sentEvents: sentEvents,
    _simulateEvent(roomId: string, eventType: string, content: Record<string, unknown>) {
      const key = `${roomId}:${eventType}`;
      const cbs = listeners.get(key) ?? [];
      for (const cb of cbs) cb(content);
    },
    async sendEvent(roomId: string, eventType: string, content: Record<string, unknown>) {
      sentEvents.push({ roomId, eventType, content });
    },
    onRoomEvent(roomId: string, eventType: string, callback: (content: Record<string, unknown>) => void) {
      const key = `${roomId}:${eventType}`;
      const cbs = listeners.get(key) ?? [];
      cbs.push(callback);
      listeners.set(key, cbs);
      return () => {
        const arr = listeners.get(key) ?? [];
        const idx = arr.indexOf(callback);
        if (idx >= 0) arr.splice(idx, 1);
      };
    },
  };
}

function base64Encode(data: Uint8Array): string {
  return Buffer.from(data).toString("base64");
}

function base64Decode(str: string): Uint8Array {
  return new Uint8Array(Buffer.from(str, "base64"));
}

describe("TerminalSocket", () => {
  let mockClient: ReturnType<typeof createMockClient>;
  const roomId = "!terminal:localhost";

  beforeEach(() => {
    mockClient = createMockClient();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("is compatible with xterm.js AttachAddon interface", () => {
    const socket = new TerminalSocket(mockClient, roomId);
    expect(socket.binaryType).toBe("arraybuffer");
    expect(typeof socket.send).toBe("function");
    expect(typeof socket.close).toBe("function");
    expect(socket.onmessage).toBeNull();
    expect(socket.onclose).toBeNull();
    expect(socket.onerror).toBeNull();
    socket.close();
  });

  it("send encodes small data as base64 without compression", async () => {
    const socket = new TerminalSocket(mockClient, roomId);
    await socket.send("hi");
    expect(mockClient._sentEvents).toHaveLength(1);

    const evt = mockClient._sentEvents[0];
    expect(evt.eventType).toBe("org.mxdx.terminal.data");
    expect(evt.roomId).toBe(roomId);
    expect(evt.content.encoding).toBe("base64");
    expect(evt.content.seq).toBe(0);

    // Decode and verify
    const decoded = base64Decode(evt.content.data as string);
    expect(new TextDecoder().decode(decoded)).toBe("hi");
    socket.close();
  });

  it("send compresses data >= 32 bytes with zlib", async () => {
    const socket = new TerminalSocket(mockClient, roomId);
    const largeData = "A".repeat(64);
    await socket.send(largeData);

    const evt = mockClient._sentEvents[0];
    expect(evt.content.encoding).toBe("zlib+base64");
    expect(evt.content.seq).toBe(0);

    // Decode and decompress to verify round-trip
    const { inflateSync } = await import("node:zlib");
    const compressed = base64Decode(evt.content.data as string);
    const decompressed = inflateSync(Buffer.from(compressed));
    expect(decompressed.toString()).toBe(largeData);
    socket.close();
  });

  it("send increments sequence numbers", async () => {
    const socket = new TerminalSocket(mockClient, roomId);
    await socket.send("a");
    await socket.send("b");
    await socket.send("c");

    expect(mockClient._sentEvents[0].content.seq).toBe(0);
    expect(mockClient._sentEvents[1].content.seq).toBe(1);
    expect(mockClient._sentEvents[2].content.seq).toBe(2);
    socket.close();
  });

  it("incoming events are delivered via onmessage in order", () => {
    const socket = new TerminalSocket(mockClient, roomId);
    const received: string[] = [];

    socket.onmessage = (event) => {
      const text = new TextDecoder().decode(new Uint8Array(event.data));
      received.push(text);
    };

    // Send events in order
    mockClient._simulateEvent(roomId, "org.mxdx.terminal.data", {
      data: base64Encode(new TextEncoder().encode("hello")),
      encoding: "base64",
      seq: 0,
    });

    mockClient._simulateEvent(roomId, "org.mxdx.terminal.data", {
      data: base64Encode(new TextEncoder().encode("world")),
      encoding: "base64",
      seq: 1,
    });

    expect(received).toEqual(["hello", "world"]);
    socket.close();
  });

  it("incoming events are reordered by seq", () => {
    const socket = new TerminalSocket(mockClient, roomId);
    const received: string[] = [];

    socket.onmessage = (event) => {
      const text = new TextDecoder().decode(new Uint8Array(event.data));
      received.push(text);
    };

    // Send seq 1 before seq 0 — seq 1 should be buffered
    mockClient._simulateEvent(roomId, "org.mxdx.terminal.data", {
      data: base64Encode(new TextEncoder().encode("second")),
      encoding: "base64",
      seq: 1,
    });

    // Nothing delivered yet (waiting for seq 0)
    expect(received).toEqual([]);

    // Now deliver seq 0 — both should flush in order
    mockClient._simulateEvent(roomId, "org.mxdx.terminal.data", {
      data: base64Encode(new TextEncoder().encode("first")),
      encoding: "base64",
      seq: 0,
    });

    expect(received).toEqual(["first", "second"]);
    socket.close();
  });

  it("duplicate sequence numbers are ignored", () => {
    const socket = new TerminalSocket(mockClient, roomId);
    const received: string[] = [];

    socket.onmessage = (event) => {
      const text = new TextDecoder().decode(new Uint8Array(event.data));
      received.push(text);
    };

    mockClient._simulateEvent(roomId, "org.mxdx.terminal.data", {
      data: base64Encode(new TextEncoder().encode("one")),
      encoding: "base64",
      seq: 0,
    });

    // Duplicate seq 0
    mockClient._simulateEvent(roomId, "org.mxdx.terminal.data", {
      data: base64Encode(new TextEncoder().encode("one-dup")),
      encoding: "base64",
      seq: 0,
    });

    expect(received).toEqual(["one"]);
    socket.close();
  });

  it("incoming compressed events are decompressed", async () => {
    vi.useRealTimers();
    const socket = new TerminalSocket(mockClient, roomId);
    const received: string[] = [];

    socket.onmessage = (event) => {
      const text = new TextDecoder().decode(new Uint8Array(event.data));
      received.push(text);
    };

    const original = "compressed payload data here";
    const compressed = deflateSync(Buffer.from(original));
    const b64 = base64Encode(new Uint8Array(compressed));

    mockClient._simulateEvent(roomId, "org.mxdx.terminal.data", {
      data: b64,
      encoding: "zlib+base64",
      seq: 0,
    });

    // Decompression is async — poll until delivered
    await new Promise<void>((resolve) => {
      const interval = setInterval(() => {
        if (received.length > 0) {
          clearInterval(interval);
          resolve();
        }
      }, 5);
    });

    expect(received).toEqual([original]);
    socket.close();
  });

  it("resize sends org.mxdx.terminal.resize event", async () => {
    const socket = new TerminalSocket(mockClient, roomId);
    await socket.resize(120, 40);

    expect(mockClient._sentEvents).toHaveLength(1);
    const evt = mockClient._sentEvents[0];
    expect(evt.eventType).toBe("org.mxdx.terminal.resize");
    expect(evt.content.cols).toBe(120);
    expect(evt.content.rows).toBe(40);
    socket.close();
  });

  it("close fires onclose and prevents further sends", async () => {
    const socket = new TerminalSocket(mockClient, roomId);
    let closeFired = false;

    socket.onclose = () => {
      closeFired = true;
    };

    socket.close();
    expect(closeFired).toBe(true);
    expect(socket.closed).toBe(true);

    await expect(socket.send("fail")).rejects.toThrow("closed");
  });

  it("close is idempotent", () => {
    const socket = new TerminalSocket(mockClient, roomId);
    let closeCount = 0;
    socket.onclose = () => closeCount++;

    socket.close();
    socket.close();
    expect(closeCount).toBe(1);
  });

  it("requests retransmit when sequence gap detected", () => {
    const socket = new TerminalSocket(mockClient, roomId);
    const received: string[] = [];
    socket.onmessage = (event) => {
      received.push(new TextDecoder().decode(new Uint8Array(event.data)));
    };

    mockClient._simulateEvent(roomId, "org.mxdx.terminal.data", {
      data: base64Encode(new TextEncoder().encode("a")),
      encoding: "base64",
      seq: 0,
    });

    // Gap at seq=1
    mockClient._simulateEvent(roomId, "org.mxdx.terminal.data", {
      data: base64Encode(new TextEncoder().encode("c")),
      encoding: "base64",
      seq: 2,
    });

    // Advance past gap detection timeout (500ms)
    vi.advanceTimersByTime(600);

    const retransmitEvents = mockClient._sentEvents.filter(
      (e) => e.eventType === "org.mxdx.terminal.retransmit",
    );
    expect(retransmitEvents).toHaveLength(1);
    expect(retransmitEvents[0].content).toEqual({ from_seq: 1, to_seq: 1 });
    socket.close();
  });

  it("fills gap when missing event arrives before timeout", () => {
    const socket = new TerminalSocket(mockClient, roomId);
    const received: string[] = [];
    socket.onmessage = (event) => {
      received.push(new TextDecoder().decode(new Uint8Array(event.data)));
    };

    mockClient._simulateEvent(roomId, "org.mxdx.terminal.data", {
      data: base64Encode(new TextEncoder().encode("a")),
      encoding: "base64",
      seq: 0,
    });

    // Gap at seq=1
    mockClient._simulateEvent(roomId, "org.mxdx.terminal.data", {
      data: base64Encode(new TextEncoder().encode("c")),
      encoding: "base64",
      seq: 2,
    });

    // Fill the gap before timeout
    mockClient._simulateEvent(roomId, "org.mxdx.terminal.data", {
      data: base64Encode(new TextEncoder().encode("b")),
      encoding: "base64",
      seq: 1,
    });

    // All three delivered in order, no retransmit sent
    expect(received).toEqual(["a", "b", "c"]);
    expect(
      mockClient._sentEvents.filter((e) => e.eventType === "org.mxdx.terminal.retransmit"),
    ).toHaveLength(0);
    socket.close();
  });

  it("accepts gap after retransmit timeout", () => {
    const socket = new TerminalSocket(mockClient, roomId);
    const received: string[] = [];
    socket.onmessage = (event) => {
      received.push(new TextDecoder().decode(new Uint8Array(event.data)));
    };

    mockClient._simulateEvent(roomId, "org.mxdx.terminal.data", {
      data: base64Encode(new TextEncoder().encode("a")),
      encoding: "base64",
      seq: 0,
    });

    // Gap at seq=1
    mockClient._simulateEvent(roomId, "org.mxdx.terminal.data", {
      data: base64Encode(new TextEncoder().encode("c")),
      encoding: "base64",
      seq: 2,
    });

    // Wait for gap timer (500ms) + retransmit timeout (500ms)
    vi.advanceTimersByTime(1100);

    // seq=2 should be delivered even though seq=1 never came
    expect(received).toContain("a");
    expect(received).toContain("c");
    socket.close();
  });

  it("invalid incoming events are silently skipped", () => {
    const socket = new TerminalSocket(mockClient, roomId);
    const received: unknown[] = [];
    socket.onmessage = (event) => received.push(event);

    // Missing required fields
    mockClient._simulateEvent(roomId, "org.mxdx.terminal.data", {
      data: "not-valid",
    });

    expect(received).toEqual([]);
    socket.close();
  });

  it("send with ArrayBuffer input works", async () => {
    const socket = new TerminalSocket(mockClient, roomId);
    const buf = new TextEncoder().encode("binary").buffer;
    await socket.send(buf);

    const evt = mockClient._sentEvents[0];
    const decoded = base64Decode(evt.content.data as string);
    expect(new TextDecoder().decode(decoded)).toBe("binary");
    socket.close();
  });

  it("exposes roomId and connected state", () => {
    const socket = new TerminalSocket(mockClient, roomId);
    expect(socket.roomId).toBe(roomId);
    expect(socket.connected).toBe(true);
    socket.close();
    expect(socket.connected).toBe(false);
  });
});
