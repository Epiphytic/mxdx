/* tslint:disable */
/* eslint-disable */

/**
 * A machine-readable representation of the authenticity for a `ShieldState`.
 */
export enum ShieldStateCode {
    /**
     * Not enough information available to check the authenticity.
     */
    AuthenticityNotGuaranteed = 0,
    /**
     * The sending device isn't yet known by the Client.
     */
    UnknownDevice = 1,
    /**
     * The sending device hasn't been verified by the sender.
     */
    UnsignedDevice = 2,
    /**
     * The sender hasn't been verified by the Client's user.
     */
    UnverifiedIdentity = 3,
    /**
     * An unencrypted event in an encrypted room.
     */
    SentInClear = 4,
    /**
     * The sender was previously verified but changed their identity.
     */
    VerificationViolation = 5,
    /**
     * The `sender` field on the event does not match the owner of the device
     * that established the Megolm session.
     */
    MismatchedSender = 6,
}

export class WasmMatrixClient {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Bootstrap cross-signing for this device.
     * Makes this device self-verified and establishes the user's signing keys.
     * Tries without UIA first (grace period after login), falls back to password auth.
     */
    bootstrapCrossSigning(password: string): Promise<void>;
    /**
     * Bootstrap cross-signing only if not already set up.
     * No-op if cross-signing keys already exist for this user.
     */
    bootstrapCrossSigningIfNeeded(password: string): Promise<void>;
    /**
     * Sync and collect events from a room. Returns JSON string of event array.
     */
    collectRoomEvents(room_id: string, timeout_secs: number): Promise<string>;
    /**
     * Create a launcher space with exec, status, and logs child rooms.
     * Returns JSON: { space_id, exec_room_id, status_room_id, logs_room_id }
     */
    createLauncherSpace(launcher_id: string): Promise<any>;
    /**
     * Get the device ID of the current session.
     */
    deviceId(): string | undefined;
    /**
     * Export the current session as JSON for persistence.
     * Returns JSON: { user_id, device_id, access_token, homeserver_url }
     * Store this in the OS keyring — never write it to a config file.
     */
    exportSession(): string;
    /**
     * Find an existing launcher space by scanning joined rooms for matching topics.
     * Returns JSON topology or null.
     */
    findLauncherSpace(launcher_id: string): Promise<any>;
    /**
     * Find or create a launcher space (idempotent).
     */
    getOrCreateLauncherSpace(launcher_id: string): Promise<any>;
    /**
     * Invite a user to a room.
     */
    inviteUser(room_id: string, user_id: string): Promise<void>;
    /**
     * Get list of invited room IDs (pending invitations).
     */
    invitedRoomIds(): string[];
    isLoggedIn(): boolean;
    /**
     * Accept a pending room invitation.
     */
    joinRoom(room_id: string): Promise<void>;
    /**
     * Login to a Matrix server.
     */
    static login(server_name: string, username: string, password: string): Promise<WasmMatrixClient>;
    /**
     * Register a new user on a homeserver with a registration token.
     */
    static register(homeserver_url: string, username: string, password: string, registration_token: string): Promise<WasmMatrixClient>;
    /**
     * Restore a previously exported session without logging in again.
     * Reuses the same device_id, avoiding rate limits and preserving cross-signing.
     * The session_json should be the output of exportSession().
     */
    static restoreSession(session_json: string): Promise<WasmMatrixClient>;
    /**
     * Send a custom event to a room.
     */
    sendEvent(room_id: string, event_type: string, content_json: string): Promise<void>;
    /**
     * Send a state event to a room.
     */
    sendStateEvent(room_id: string, event_type: string, state_key: string, content_json: string): Promise<void>;
    syncOnce(): Promise<void>;
    userId(): string | undefined;
}

export function init(): void;

/**
 * Smoke test: returns the matrix-sdk version string to prove it compiled.
 */
export function sdk_version(): string;
