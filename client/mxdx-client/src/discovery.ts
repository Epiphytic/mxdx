import type {
  LauncherIdentityEvent,
  HostTelemetryEvent,
} from "./types/index.js";
import {
  LauncherIdentityEvent as LauncherIdentitySchema,
  HostTelemetryEvent as TelemetrySchema,
} from "./types/index.js";
import type { MatrixClient } from "./client.js";

export interface DiscoveredLauncher {
  roomId: string;
  identity: LauncherIdentityEvent;
}

interface StateEvent {
  type: string;
  state_key: string;
  content: Record<string, unknown>;
}

export async function discoverLaunchers(
  client: MatrixClient,
  spaceRoomId: string,
): Promise<DiscoveredLauncher[]> {
  const hierarchy = await client.fetchJson<{
    rooms: Array<{ room_id: string }>;
  }>(
    `/_matrix/client/v1/rooms/${encodeURIComponent(spaceRoomId)}/hierarchy?limit=50`,
  );

  const launchers: DiscoveredLauncher[] = [];

  for (const room of hierarchy.rooms) {
    const stateEvents = await client.fetchJson<StateEvent[]>(
      `/_matrix/client/v3/rooms/${encodeURIComponent(room.room_id)}/state`,
    );

    for (const event of stateEvents) {
      if (event.type === "org.mxdx.launcher.identity") {
        const parsed = LauncherIdentitySchema.safeParse(event.content);
        if (parsed.success) {
          launchers.push({ roomId: room.room_id, identity: parsed.data });
        }
      }
    }
  }

  return launchers;
}

export async function getLauncherStatus(
  client: MatrixClient,
  roomId: string,
  launcherId: string,
): Promise<HostTelemetryEvent | null> {
  try {
    const content = await client.fetchJson<Record<string, unknown>>(
      `/_matrix/client/v3/rooms/${encodeURIComponent(roomId)}/state/org.mxdx.telemetry/${encodeURIComponent(launcherId)}`,
    );
    const parsed = TelemetrySchema.safeParse(content);
    return parsed.success ? parsed.data : null;
  } catch {
    return null;
  }
}
