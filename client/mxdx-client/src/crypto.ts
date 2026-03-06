import {
  initAsync,
  OlmMachine,
  UserId,
  DeviceId,
  RoomId,
  DecryptionSettings,
  TrustRequirement,
  type RequestType,
} from "@matrix-org/matrix-sdk-crypto-wasm";

let wasmReady = false;

async function ensureWasm(): Promise<void> {
  if (!wasmReady) {
    await initAsync();
    wasmReady = true;
  }
}

export interface IdentityKeys {
  curve25519: string;
  ed25519: string;
}

export class CryptoClient {
  private machine: OlmMachine;

  private constructor(machine: OlmMachine) {
    this.machine = machine;
  }

  static async create(
    userId: string,
    deviceId: string,
  ): Promise<CryptoClient> {
    await ensureWasm();
    const uid = new UserId(userId);
    const did = new DeviceId(deviceId);
    const machine = await OlmMachine.initialize(uid, did);
    return new CryptoClient(machine);
  }

  get userId(): string {
    return this.machine.userId.toString();
  }

  get deviceId(): string {
    return this.machine.deviceId.toString();
  }

  get identityKeys(): IdentityKeys {
    const keys = this.machine.identityKeys;
    return {
      curve25519: keys.curve25519.toBase64(),
      ed25519: keys.ed25519.toBase64(),
    };
  }

  async encrypt(
    roomId: string,
    eventType: string,
    content: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    const rid = new RoomId(roomId);
    const encrypted = await this.machine.encryptRoomEvent(
      rid,
      eventType,
      JSON.stringify(content),
    );
    return JSON.parse(encrypted);
  }

  async decrypt(
    event: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    const roomId = event.room_id as string;
    const rid = new RoomId(roomId);
    const settings = new DecryptionSettings(TrustRequirement.Untrusted);
    const decrypted = await this.machine.decryptRoomEvent(
      JSON.stringify(event),
      rid,
      settings,
    );
    return JSON.parse(decrypted.event);
  }

  async outgoingRequests(): Promise<
    Array<{ id: string | undefined; type: RequestType; body: string }>
  > {
    const requests = await this.machine.outgoingRequests();
    return requests.map((r) => ({
      id: r.id,
      type: r.type,
      body: r.body,
    }));
  }

  close(): void {
    this.machine.free();
  }
}
