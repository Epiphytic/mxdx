## IMPORTANT ##

This is a security service for communication with  servers. All communications must be:
 1) private
 2) auditable
 3) end to end encrypted

** NEVER BYPASS END TO END ENCRYPTION IN ANY CHANGES UNDER ANY CIRCUMSTANCES **

** EVERY MATRIX EVENT MUST BE END-TO-END ENCRYPTED — NO EXCEPTIONS **
This includes timeline messages, **state events**, and to-device messages. Unencrypted
events of ANY type are out of spec for this project and constitute a security violation.
Encrypted state events use MSC4362 (`experimental-encrypted-state-events`) which is
already enabled in this project. If you find yourself calling `send_state_event`,
`send_raw`, or any Matrix send API and you are not 100% sure the event will be
encrypted on the wire, STOP and audit the call path. Any code review that lets an
unencrypted Matrix send through has failed.

This project uses Rust for backend services, Rust WASM's for the client and launcher inner workings, and nodejs/typescript for the frontend.

** All sensitive info should be stored encrypted at rest **
** Use OS keystores/keychains for sensitive client/server info when available **
** Use npm/wasm pattern for cross platform compatibility for commonlly used endpoints for servers and clients **
** The goal is for the experience to be as low friction as possible for users **

** Reuse code as much as possible! Many functions are the same between the clients and the launchers, so try and re-use code to reduce the amount of code to maintain **

** All changes require a security review. If the security review doesn't pass, the changes aren't complete **

** All changes require end to end tests using the local tuwunel instances. If these don't pass, the changes don't pass **
** All changes require end to end tests using the beta server accounts in test-credentials.toml (if this is available). If these don't pass, the changes don't pass  **

## End-to-End Test Policy

** End-to-end tests MUST exercise the compiled binaries (mxdx-worker, mxdx-client) as subprocesses, or they are NOT end-to-end tests **
** Tests that use library code directly (e.g., MatrixClient) to send/receive events are INTEGRATION tests for the libraries, not E2E tests **
** Integration tests are valuable and should be kept, but they must be clearly labeled as integration tests, not E2E tests **
** Performance profiling is only meaningful on the actual compiled binaries that users would run — not on library-level integration tests **
** If an E2E test fails, fix the binary — never change the test to work around a broken binary **
** All E2E and integration tests must be runnable by any user who has the binaries and a test-credentials.toml configured **

## Agent Execution Rules

** ALWAYS run tests or any long-running processes using subagents — never block the main conversation context **

## Third-Party `testing` Features in Production

When a dependency's `testing` (or similarly named) cargo feature gates an otherwise-stable API that mxdx needs for security-critical work, **prefer enabling the feature in production over vendor forks or trust-model workarounds**, subject to three preconditions:

1. The gated API is fully public and stable on the upstream crate's `main` branch (verify by URL + commit hash at decision time).
2. No non-gated public equivalent exists.
3. The transitive dev-quality dependencies the feature pulls in are bounded and audited.

Each new use of a `testing`-gated API must cite the authorizing ADR in a comment adjacent to the call site. Policy details, review triggers, and the first authorized uses (`matrix-sdk::Client::olm_machine_for_testing()` → `OlmMachine::sign` / `encrypt_room_event_raw`) are documented in `docs/adr/2026-04-16-matrix-sdk-testing-feature.md`.

This exception does NOT authorize enabling `testing`-style features casually or for non-security-critical work — each crate and each use requires its own decision against the three preconditions.
