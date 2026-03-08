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
     * Generates cross-signing keys and uploads them. Handles the two-step UIA
     * flow by capturing the session ID from the 401 response and including it
     * in the password auth retry.
     */
    bootstrapCrossSigning(password: string): Promise<void>;
    /**
     * Bootstrap cross-signing only if not already set up.
     * No-op if keys exist and private parts are in the local crypto store.
     * Falls back to full bootstrap if private keys are missing (e.g. after
     * session restore with ephemeral crypto store).
     */
    bootstrapCrossSigningIfNeeded(password: string): Promise<void>;
    /**
     * Sync and collect events from a room. Returns JSON string of event array.
     */
    collectRoomEvents(room_id: string, timeout_secs: number): Promise<string>;
    /**
     * Create a direct message room with E2EE and history_visibility: joined.
     */
    createDmRoom(user_id: string): Promise<string>;
    /**
     * Create a launcher space with exec, status, and logs child rooms.
     * Returns JSON: { space_id, exec_room_id, status_room_id, logs_room_id }
     */
    createLauncherSpace(launcher_id: string): Promise<any>;
    /**
     * Diagnostic: check encryption readiness for a room.
     * Returns JSON with room encryption status, member count, etc.
     */
    debugRoomEncryption(room_id: string): Promise<string>;
    /**
     * Debug: dump all event types from a room (including encrypted).
     */
    debugRoomEvents(room_id: string): Promise<string>;
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
     * Check if a user's identity is verified from our perspective.
     */
    isUserVerified(user_id_str: string): Promise<boolean>;
    /**
     * Accept a pending room invitation.
     */
    joinRoom(room_id: string): Promise<void>;
    /**
     * List all launcher spaces by scanning joined rooms for matching topic patterns.
     */
    listLauncherSpaces(): Promise<string>;
    /**
     * Login to a Matrix server.
     */
    static login(server_name: string, username: string, password: string): Promise<WasmMatrixClient>;
    /**
     * Wait for a specific event type to appear in a room.
     * Returns the first new event matching the type, or "null" on timeout.
     */
    onRoomEvent(room_id: string, event_type: string, timeout_secs: number): Promise<string>;
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
     * Send a custom event to a room (with E2EE encryption).
     * Uses Room::send_raw() which handles encryption inline:
     *   1. Syncs room members
     *   2. Queries device keys for all members
     *   3. Preshares the Megolm room key
     *   4. Encrypts and sends via HTTP
     */
    sendEvent(room_id: string, event_type: string, content_json: string): Promise<void>;
    /**
     * Send a state event to a room.
     */
    sendStateEvent(room_id: string, event_type: string, state_key: string, content_json: string): Promise<void>;
    syncOnce(): Promise<void>;
    userId(): string | undefined;
    /**
     * Verify our own user identity (marks it as locally verified).
     * This is needed before verifying other users — our own identity must
     * be verified first.
     */
    verifyOwnIdentity(): Promise<void>;
    /**
     * Verify another user's identity by signing their master key with our
     * user-signing key. Both users must have bootstrapped cross-signing first.
     * This is a one-way operation — the other user must also call this to
     * verify us back.
     */
    verifyUser(user_id_str: string): Promise<void>;
}

export function init(): void;

/**
 * Smoke test: returns the matrix-sdk version string to prove it compiled.
 */
export function sdk_version(): string;
