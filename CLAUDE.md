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
** All changes require end to end tests using the matrix.org accounts in test-credentials.toml (if this is available). If these don't pass, the changes don't pass  **
