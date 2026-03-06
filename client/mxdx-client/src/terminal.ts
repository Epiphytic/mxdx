import { TerminalDataEvent, TerminalResizeEvent } from "./types/index.js";

/**
 * Minimal Matrix client interface required by TerminalSocket.
 * Consumers provide a concrete implementation (e.g. MxdxClient).
 */
export interface TerminalMatrixClient {
  sendEvent(roomId: string, eventType: string, content: Record<string, unknown>): Promise<void>;
  onRoomEvent(
    roomId: string,
    eventType: string,
    callback: (content: Record<string, unknown>) => void,
  ): () => void;
}

const COMPRESSION_THRESHOLD = 32;

function base64Encode(data: Uint8Array): string {
  if (typeof Buffer !== "undefined") {
    return Buffer.from(data).toString("base64");
  }
  let binary = "";
  for (let i = 0; i < data.length; i++) {
    binary += String.fromCharCode(data[i]);
  }
  return btoa(binary);
}

function base64Decode(str: string): Uint8Array {
  if (typeof Buffer !== "undefined") {
    return new Uint8Array(Buffer.from(str, "base64"));
  }
  const binary = atob(str);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

async function compress(data: Uint8Array): Promise<Uint8Array> {
  if (typeof globalThis.CompressionStream !== "undefined") {
    const cs = new CompressionStream("deflate");
    const writer = cs.writable.getWriter();
    const reader = cs.readable.getReader();
    writer.write(data);
    writer.close();
    const chunks: Uint8Array[] = [];
    let totalLength = 0;
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
      totalLength += value.length;
    }
    const result = new Uint8Array(totalLength);
    let offset = 0;
    for (const chunk of chunks) {
      result.set(chunk, offset);
      offset += chunk.length;
    }
    return result;
  }
  // Fallback to node:zlib
  const { deflateSync } = await import("node:zlib");
  return new Uint8Array(deflateSync(Buffer.from(data)));
}

async function decompress(data: Uint8Array): Promise<Uint8Array> {
  if (typeof globalThis.DecompressionStream !== "undefined") {
    const ds = new DecompressionStream("deflate");
    const writer = ds.writable.getWriter();
    const reader = ds.readable.getReader();
    writer.write(data);
    writer.close();
    const chunks: Uint8Array[] = [];
    let totalLength = 0;
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
      totalLength += value.length;
    }
    const result = new Uint8Array(totalLength);
    let offset = 0;
    for (const chunk of chunks) {
      result.set(chunk, offset);
      offset += chunk.length;
    }
    return result;
  }
  const { inflateSync } = await import("node:zlib");
  return new Uint8Array(inflateSync(Buffer.from(data)));
}

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

interface PendingEvent {
  seq: number;
  data: Uint8Array;
}

export class TerminalSocket {
  readonly binaryType: string = "arraybuffer";

  onmessage: ((event: { data: ArrayBuffer }) => void) | null = null;
  onclose: ((event: { code?: number; reason?: string }) => void) | null = null;
  onerror: ((event: { message?: string }) => void) | null = null;

  private _client: TerminalMatrixClient;
  private _roomId: string;
  private _sendSeq = 0;
  private _expectedSeq = 0;
  private _buffer: PendingEvent[] = [];
  private _closed = false;
  private _unsubscribe: (() => void) | null = null;
  private _reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private _reconnectDelay = 1000;
  private _connected = true;

  constructor(client: TerminalMatrixClient, roomId: string) {
    this._client = client;
    this._roomId = roomId;
    this._subscribe();
  }

  private _subscribe(): void {
    this._unsubscribe = this._client.onRoomEvent(
      this._roomId,
      "org.mxdx.terminal.data",
      (content: Record<string, unknown>) => {
        this._handleIncomingData(content);
      },
    );
    this._connected = true;
    this._reconnectDelay = 1000;
  }

  private _handleIncomingData(content: Record<string, unknown>): void {
    const parsed = TerminalDataEvent.safeParse(content);
    if (!parsed.success) return;

    const { data, encoding, seq } = parsed.data;

    this._decodeAndEmit(data, encoding, seq);
  }

  private _decodeAndEmit(data: string, encoding: string, seq: number): void {
    const raw = base64Decode(data);

    if (encoding === "zlib+base64") {
      decompress(raw).then((decompressed) => {
        this._enqueue(seq, decompressed);
        this._flush();
      }).catch(() => {
        // Decompression failed, skip event
      });
    } else {
      this._enqueue(seq, raw);
      this._flush();
    }
  }

  private _enqueue(seq: number, data: Uint8Array): void {
    if (seq < this._expectedSeq) return; // duplicate
    this._buffer.push({ seq, data });
    this._buffer.sort((a, b) => a.seq - b.seq);
  }

  private _flush(): void {
    while (this._buffer.length > 0 && this._buffer[0].seq === this._expectedSeq) {
      const event = this._buffer.shift()!;
      this._expectedSeq++;
      if (this.onmessage) {
        this.onmessage({ data: event.data.buffer.slice(
          event.data.byteOffset,
          event.data.byteOffset + event.data.byteLength,
        ) });
      }
    }
  }

  async send(data: string | ArrayBuffer): Promise<void> {
    if (this._closed) throw new Error("TerminalSocket is closed");

    const bytes = typeof data === "string"
      ? textEncoder.encode(data)
      : new Uint8Array(data);

    let encoded: string;
    let encoding: string;

    if (bytes.length >= COMPRESSION_THRESHOLD) {
      const compressed = await compress(bytes);
      encoded = base64Encode(compressed);
      encoding = "zlib+base64";
    } else {
      encoded = base64Encode(bytes);
      encoding = "base64";
    }

    const seq = this._sendSeq++;
    const content: Record<string, unknown> = { data: encoded, encoding, seq };

    try {
      await this._client.sendEvent(this._roomId, "org.mxdx.terminal.data", content);
      this._connected = true;
      this._reconnectDelay = 1000;
    } catch {
      this._handleSyncDrop();
    }
  }

  async resize(cols: number, rows: number): Promise<void> {
    if (this._closed) throw new Error("TerminalSocket is closed");

    const content: Record<string, unknown> = { cols, rows };
    await this._client.sendEvent(this._roomId, "org.mxdx.terminal.resize", content);
  }

  close(): void {
    if (this._closed) return;
    this._closed = true;

    if (this._unsubscribe) {
      this._unsubscribe();
      this._unsubscribe = null;
    }

    if (this._reconnectTimer) {
      clearTimeout(this._reconnectTimer);
      this._reconnectTimer = null;
    }

    if (this.onclose) {
      this.onclose({ code: 1000, reason: "Normal closure" });
    }
  }

  private _handleSyncDrop(): void {
    if (this._closed) return;
    this._connected = false;

    if (this._unsubscribe) {
      this._unsubscribe();
      this._unsubscribe = null;
    }

    this._scheduleReconnect();
  }

  private _scheduleReconnect(): void {
    if (this._closed || this._reconnectTimer) return;

    const delay = this._reconnectDelay;
    this._reconnectDelay = Math.min(this._reconnectDelay * 2, 30000);

    this._reconnectTimer = setTimeout(() => {
      this._reconnectTimer = null;
      if (this._closed) return;
      this._subscribe();
    }, delay);
  }

  get connected(): boolean {
    return this._connected && !this._closed;
  }

  get closed(): boolean {
    return this._closed;
  }

  get roomId(): string {
    return this._roomId;
  }
}
