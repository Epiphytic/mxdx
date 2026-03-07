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
     * Check if logged in.
     */
    isLoggedIn(): boolean;
    /**
     * Login to a Matrix server. server_name can be "matrix.org" or a full URL.
     */
    static login(server_name: string, username: string, password: string): Promise<WasmMatrixClient>;
    /**
     * Register a new user on a homeserver with a registration token.
     */
    static register(homeserver_url: string, username: string, password: string, registration_token: string): Promise<WasmMatrixClient>;
    /**
     * Perform a single sync cycle.
     */
    syncOnce(): Promise<void>;
    /**
     * Get the user ID.
     */
    userId(): string | undefined;
}

export function init(): void;

/**
 * Smoke test: returns the matrix-sdk version string to prove it compiled.
 */
export function sdk_version(): string;
