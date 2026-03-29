## IMPORTANT ##

This is a security service for communication with  servers. All communications must be:
 1) private
 2) auditable
 3) end to end encrypted

** NEVER BYPASS END TO END ENCRYPTION IN ANY CHANGES UNDER ANY CIRCUMSTANCES **

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
