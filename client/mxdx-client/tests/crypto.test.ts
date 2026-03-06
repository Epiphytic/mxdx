import { describe, it, expect, beforeAll } from "vitest";
import { CryptoClient } from "../src/crypto.js";

describe("CryptoClient", () => {
  let client: CryptoClient;

  beforeAll(async () => {
    client = await CryptoClient.create("@test:localhost", "TESTDEVICE");
  });

  it("creates an instance with correct identifiers", () => {
    expect(client.userId).toBe("@test:localhost");
    expect(client.deviceId).toBe("TESTDEVICE");
  });

  it("exposes device curve25519 key", () => {
    const key = client.identityKeys.curve25519;
    expect(typeof key).toBe("string");
    expect(key.length).toBeGreaterThan(0);
  });

  it("exposes device ed25519 key", () => {
    const key = client.identityKeys.ed25519;
    expect(typeof key).toBe("string");
    expect(key.length).toBeGreaterThan(0);
  });

  it("decrypt rejects on invalid ciphertext", async () => {
    const fakeEvent = {
      type: "m.room.encrypted",
      event_id: "$fake",
      room_id: "!noroom:localhost",
      sender: "@other:localhost",
      origin_server_ts: Date.now(),
      content: {
        algorithm: "m.megolm.v1.aes-sha2",
        ciphertext: "not-real-ciphertext",
        sender_key: "fakekey",
        session_id: "fakesession",
        device_id: "FAKEDEVICE",
      },
    };
    await expect(
      client.decrypt(fakeEvent as Record<string, unknown>),
    ).rejects.toThrow();
  });

  it("returns outgoing key requests after creation", async () => {
    const requests = await client.outgoingRequests();
    expect(Array.isArray(requests)).toBe(true);
    expect(requests.length).toBeGreaterThan(0);
  });

  it("creates separate instances with distinct keys", async () => {
    const other = await CryptoClient.create(
      "@other:localhost",
      "OTHERDEVICE",
    );
    expect(other.identityKeys.curve25519).not.toBe(
      client.identityKeys.curve25519,
    );
    other.close();
  });

  it("dispose frees resources without error", async () => {
    const disposable = await CryptoClient.create(
      "@dispose:localhost",
      "DISPOSEDEV",
    );
    expect(() => disposable.close()).not.toThrow();
  });
});
