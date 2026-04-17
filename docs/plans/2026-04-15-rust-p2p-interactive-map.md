# Plan: Rust Interactive Sessions (TURN + P2P) — Map Phase

**Slug:** rust-p2p-interactive
**ADRs:**
- docs/adr/2026-04-15-mxdx-p2p-crate.md
- docs/adr/2026-04-15-datachannel-rs.md
- docs/adr/2026-04-15-mcall-wire-format.md
- docs/adr/2026-04-15-megolm-bytes-newtype.md

**Research:** docs/plans/2026-04-15-rust-p2p-interactive-storm.md (storm spec used as research)
**Mode:** --parallel
**Branch:** brains/rust-p2p-interactive

---

## Overview

This plan ports interactive terminal mode (TURN + P2P data channel) from the npm+wasm path to native Rust binaries (`mxdx-worker`, `mxdx-client`) and the browser WASM target (`mxdx-core-wasm`). A new workspace crate `mxdx-p2p` is the single home for the platform-agnostic `P2PTransport` state machine, `P2PCrypto`, `WebRtcChannel` trait, signaling helpers, and TURN client; runtime wiring stays in the existing worker and client crates. The Rust port adopts the standard Matrix VoIP `m.call.*` wire format and the `Megolm<Bytes>` newtype invariant that makes plaintext-on-wire a compile error. All existing E2E tests must continue to pass throughout; new acceptance gates run against ca1-beta.mxdx.dev and ca2-beta.mxdx.dev and include both single-HS and federated topologies plus an 8-combination interop matrix between Rust and npm peers.

---

## Phase 0: Scaffolding

### T-00 — Create `crates/mxdx-p2p` workspace skeleton

Add the new crate directory with a `Cargo.toml` (no real dependencies yet — placeholder stubs only), a `src/lib.rs` that re-exports nothing, and empty module files matching the layout in the ADR. Register the crate as a workspace member in the root `Cargo.toml`.

**Acceptance criteria:**
- `cargo check -p mxdx-p2p` passes from a clean checkout
- Root `Cargo.toml` `[workspace] members` includes `crates/mxdx-p2p`
- Directory structure matches `crates/mxdx-p2p/src/{lib,crypto,turn,signaling/,channel/,transport/}` with stub `mod` declarations

**Dependencies:** none
**Type:** task
**Size:** S

### T-01 — Delete `crates/mxdx-types/src/events/webrtc.rs` and compile-fix

Remove `crates/mxdx-types/src/events/webrtc.rs` and its `mod webrtc;` declaration. Find every reference to `WebRtcOffer`, `WebRtcAnswer`, `WebRtcSdp`, `WebRtcIce` and the `org.mxdx.session.webrtc.*` / `org.mxdx.webrtc.*` constants across the workspace and remove or stub them so the workspace compiles.

**Acceptance criteria:**
- `crates/mxdx-types/src/events/webrtc.rs` no longer exists
- `cargo check --workspace` passes after this deletion
- No remaining `use mxdx_types::events::webrtc` import anywhere in the workspace
- Existing tests (non-P2P) all still pass

**Dependencies:** T-00
**Type:** task
**Size:** S

### T-02 — Delete `crates/mxdx-worker/src/webrtc.rs` and compile-fix worker

Remove `crates/mxdx-worker/src/webrtc.rs` (the always-bails stub) and its `mod webrtc;` declaration. Remove the `WebRtcManager` import and any usage sites in `crates/mxdx-worker/src/lib.rs` and `crates/mxdx-worker/src/session_mux.rs`. Replace any removed call sites with `// TODO: wired in Phase 6` comments so the worker compiles.

**Acceptance criteria:**
- `crates/mxdx-worker/src/webrtc.rs` no longer exists
- `cargo test -p mxdx-worker` passes (existing test suite, not P2P tests)
- `cargo check -p mxdx-client` passes (no transitive breakage)
- `cargo clippy --workspace` produces no new errors

**Dependencies:** T-01
**Type:** task
**Size:** S

### T-03 — Add CI jobs for `mxdx-p2p` + security-grep gate

Add three CI job stubs: `rust-p2p-unit`, `rust-p2p-loopback`, `wasm-p2p-smoke`. All three run but produce "no tests" until later phases. Also add a `security-grep` job that runs `scripts/check-no-unencrypted-sends.sh` — create the script as a grep gate that fails if `mxdx-p2p` contains `send_raw|skip_encryption|unencrypted`.

**Acceptance criteria:**
- All three CI jobs present and green (vacuously)
- `security-grep` job exists and the script exists at `scripts/check-no-unencrypted-sends.sh`
- Script correctly fails on a synthetic test file containing `send_raw`
- Existing CI jobs remain unmodified and green

**Dependencies:** T-02
**Type:** task
**Size:** S

---

## Phase 1: P2PCrypto + Cross-Language Vectors

### T-10 — Add `Megolm<T>` and `SealedKey` newtypes

Introduce `Megolm<T>` in `crates/mxdx-matrix` with a package-private constructor, `into_ciphertext_bytes()` accessor, and a corresponding `encrypt_for_room(...) -> Megolm<Bytes>` method on `MatrixClient`. Add `send_megolm(room_id, Megolm<Bytes>)` as the Matrix-path fallback sender. `SealedKey` is defined in `crates/mxdx-p2p/src/crypto.rs` (pub(in crate::crypto) constructor).

**Acceptance criteria:**
- `MatrixClient::encrypt_for_room` returns `Megolm<Bytes>`
- `MatrixClient::send_megolm` accepts `Megolm<Bytes>` and sends via existing room send path
- `Megolm<Bytes>` has no public constructor
- `cargo test -p mxdx-matrix` passes

**Dependencies:** T-00
**Type:** feature
**Size:** M

### T-11 — Implement `P2PCrypto` (AES-256-GCM, wire-compatible with npm)

Implement `P2PCrypto`: key generation, `encrypt`, `decrypt`. `EncryptedFrame` serializes to `{"c": "<base64>", "iv": "<base64>"}` matching `packages/core/p2p-crypto.js` exactly. `generate()` returns `(P2PCrypto, SealedKey)`. `from_sealed(SealedKey)` reconstructs from a transported key.

**Acceptance criteria:**
- AES-256-GCM with random 96-bit IV per frame
- `EncryptedFrame` JSON shape matches npm
- Roundtrip test: encrypt in Rust, decrypt in Rust
- `cargo test -p mxdx-p2p` (crypto unit tests) passes

**Dependencies:** T-10
**Type:** feature
**Size:** M

### T-12 — Cross-language vector test fixtures

Generate test vectors from npm side: fixed key + known plaintexts → known ciphertexts. Create `packages/e2e-tests/scripts/regenerate-p2p-vectors.mjs`. Write `crates/mxdx-p2p/tests/crypto_vectors.rs` and `packages/e2e-tests/tests/rust-npm-crypto-vectors.test.js`.

**Acceptance criteria:**
- Vector files committed at `crates/mxdx-p2p/tests/fixtures/crypto-vectors.json`
- `cargo test -p mxdx-p2p --test crypto_vectors` passes
- `node packages/e2e-tests/tests/rust-npm-crypto-vectors.test.js` passes without a homeserver
- CI `cross-vectors` job is green

**Dependencies:** T-11
**Type:** test
**Size:** M

### T-13 — Trybuild negative tests for `Megolm<Bytes>` and `SealedKey`

Add `trybuild` dev-dependency. Write `trybuild/megolm-constructor-fails.rs` (asserts constructing `Megolm` outside `mxdx-matrix` is a compile error) and `trybuild/sealedkey-constructor-fails.rs` (asserts constructing `SealedKey` outside `mxdx-p2p::crypto` is a compile error).

**Acceptance criteria:**
- Both trybuild tests exist and pass (illegal code compilation fails as expected)
- `cargo test -p mxdx-matrix` and `cargo test -p mxdx-p2p` remain green
- If a future developer accidentally adds a public `Megolm::new()`, the trybuild test turns red

**Dependencies:** T-11
**Type:** test
**Size:** S

---

## Phase 2: TurnCredentials

### T-20 — Implement `TurnCredentials` struct and `fetch_turn_credentials`

Add `crates/mxdx-p2p/src/turn.rs` with the `TurnCredentials` struct and `fetch_turn_credentials(homeserver, token) -> Result<Option<TurnCredentials>>` calling `GET /_matrix/client/v3/voip/turnServer`. Implement `expires_at()`, `refresh_at()` (TTL/2), and `is_expired()` helpers.

**Acceptance criteria:**
- `fetch_turn_credentials` parses the Matrix VoIP TURN response correctly
- Returns `Ok(None)` when the homeserver has no TURN configured
- Unit tests cover parsing, expiry math, and HTTP error handling
- `cargo test -p mxdx-p2p` passes

**Dependencies:** T-00
**Type:** feature
**Size:** S

### T-21 — Active-call TURN refresh helpers

Add `TurnRefreshTask`: a background task that wakes at `refresh_at()`, fetches new credentials, returns them via channel for the driver to pass into `WebRtcChannel::restart_ice`. Implement expiry-during-reconnect serialization: if reconnect pending and creds expired, re-fetch before `m.call.invite`. Define `TurnRefreshOutcome` enum.

**Acceptance criteria:**
- `TurnRefreshTask` can be constructed and its channel polled without a real homeserver (mock HTTP client)
- Unit test: simulated TTL expiry triggers `Expired` outcome
- Unit test: successful refresh before TTL/2 returns `Refreshed` with new credentials
- No network calls required to run the unit tests

**Dependencies:** T-20
**Type:** feature
**Size:** M

### T-22 — Manual smoke test documentation and CI skip annotation

Document a manual smoke procedure for `fetch_turn_credentials` against ca1-beta (run `cargo test -p mxdx-p2p -- --ignored turn_smoke` with `TEST_HS_URL` and `TEST_TOKEN` env vars). Write procedure in `crates/mxdx-p2p/README.md`.

**Acceptance criteria:**
- `README.md` documents the smoke procedure
- `#[ignore]`-annotated test exists and passes when run manually against ca1-beta
- CI unit job does not require beta credentials
- `cargo test -p mxdx-p2p` (without `--include-ignored`) passes in zero-network environments

**Dependencies:** T-21
**Type:** task
**Size:** S

---

## Phase 3: WebRtcChannel Trait + Native Impl

### T-30 — Define `WebRtcChannel` trait and `ChannelEvent` enum

Add `crates/mxdx-p2p/src/channel/mod.rs` with the `WebRtcChannel` trait (create/accept offer, accept answer, add ICE, restart ICE, send bytes, events receiver, close), the `ChannelEvent` enum, `IceServer`, `Sdp`, and `SdpKind`. Trait uses `async_trait` with cfg-gated `?Send` for wasm.

**Acceptance criteria:**
- `cargo check -p mxdx-p2p` passes (trait compiles for both native and wasm targets)
- All method signatures match the ADR exactly
- `ChannelEvent` is `Send + 'static` on native target
- No datachannel-rs or web-sys dependencies yet (trait only)

**Dependencies:** T-00
**Type:** feature
**Size:** S

### T-31 — Native `WebRtcChannel` implementation (`datachannel-rs`)

Add `crates/mxdx-p2p/src/channel/native.rs` under `cfg(not(target_arch = "wasm32"))`. Implement `WebRtcChannel` using the `datachannel` crate (FFI to libdatachannel). Handle offer/answer/ICE translation. Translate backend-specific close-reason and buffer events into `ChannelEvent` variants.

**Acceptance criteria:**
- `cargo build -p mxdx-p2p` on Linux with libdatachannel headers available succeeds
- All `WebRtcChannel` methods have non-panicking implementations
- `ChannelEvent` translations cover all reachable libdatachannel callbacks
- The native implementation does not appear in `wasm-pack build` dep graph

**Dependencies:** T-30
**Type:** feature
**Size:** XL (upgraded from L per council review — C++ FFI binding + callback-to-channel bridge is higher-effort than L implies)

### T-32 — Loopback integration test (native only)

Add `crates/mxdx-p2p/tests/loopback.rs`: wire two `NativeWebRtcChannel` instances via in-memory mpsc using `MockSignaling`. Assert both sides reach `Open` within 10s; message sent on A arrives on B; test completes in under 2s on CI.

**Acceptance criteria:**
- `cargo test -p mxdx-p2p --test loopback` passes in CI without a homeserver
- Both channel sides reach `ChannelEvent::Open`
- Roundtrip message confirmed
- Test runtime < 2s (enforced with `tokio::time::timeout`)

**Dependencies:** T-31
**Type:** test
**Size:** M

### T-33 — libdatachannel CI build dependency setup

Document and automate the native dependency install in `.github/workflows/ci.yml`. Add `build-only-native` CI job that runs `cargo build -p mxdx-p2p` on both Linux and macOS without running tests.

**Acceptance criteria:**
- `rust-p2p-loopback` job installs native deps and passes on CI (Linux)
- `build-only-native` job passes on Linux and macOS matrix
- Contributor documentation in `crates/mxdx-p2p/README.md` covers local dev setup
- wasm-pack build is unaffected

**Dependencies:** T-32
**Type:** task
**Size:** M

### T-34 — Musl/Alpine cross-compile verification for `mxdx-worker`

Verify that `mxdx-worker` with `p2p_enabled=true` cross-compiles cleanly to musl (Alpine Linux) — the production deployment target. libdatachannel and its transitive deps (OpenSSL, libjuice, libsrtp, usrsctp) must either statically link or have musl builds available. Document the build procedure in `crates/mxdx-worker/README.md` and add a `musl-build` CI job that runs `cargo build -p mxdx-worker --target x86_64-unknown-linux-musl --features p2p`.

**Acceptance criteria:**
- `musl-build` CI job exists and passes on `x86_64-unknown-linux-musl`
- Resulting binary runs without glibc in an Alpine container (validated by running `--version`)
- Build procedure documented in `README.md`
- If static linking is infeasible, Alpine package dependencies are listed

**Dependencies:** T-33
**Type:** task
**Size:** M

---

## Phase 4: m.call.* Signaling Helpers + Glare Resolver

### T-40 — Define `m.call.*` event types

Add `crates/mxdx-p2p/src/signaling/events.rs` with serde structs for `CallInvite`, `CallAnswer`, `CallCandidates`, `CallHangup`, `CallSelectAnswer`. Include standard Matrix VoIP fields. Add `mxdx_session_key` extension field (base64 `SealedKey` bytes) to `CallInvite`. Add `build_invite(sealed_key, ...) -> CallInvite` as the only constructor that carries a key.

**Acceptance criteria:**
- All five event types serialize to/from valid Matrix VoIP JSON
- Golden JSON round-trip tests pass for each type
- `mxdx_session_key` is absent from serialized output when `None`
- `SealedKey` is only reachable via `build_invite`

**Dependencies:** T-13
**Type:** feature
**Size:** M

### T-41 — Implement call event parser (`signaling/parse.rs`)

Add parser that converts incoming `RawJsonValue` Matrix events into typed call event variants. Return a `ParsedCallEvent` enum. Unknown call event types produce `ParsedCallEvent::Unknown`.

**Acceptance criteria:**
- Parser handles all five known event types
- Unknown types produce `Unknown` variant, not panic or error
- Fuzz target compiles
- `crates/mxdx-p2p/tests/signaling_parse.rs` golden tests pass

**Dependencies:** T-40
**Type:** feature
**Size:** S

### T-42 — Glare resolver (`signaling/glare.rs`)

Add `resolve(our_user_id, their_user_id, our_call_id, their_call_id) -> GlareResult`. Pure function, no side effects. Lower lexicographic `user_id` wins; tie-break on `call_id`.

**Acceptance criteria:**
- `resolve` is a pure function with no `async`, no I/O, no panics
- Property test (`proptest`) asserts both peers always agree
- Property test asserts the function is total
- `crates/mxdx-p2p/tests/glare_resolution.rs` proptest suite is in CI

**Dependencies:** T-40
**Type:** feature
**Size:** S

### T-43 — Add `m.call.*` receive path to `mxdx-matrix` sync filter

Extend `crates/mxdx-matrix` sync filter to include `m.call.invite`, `m.call.answer`, `m.call.candidates`, `m.call.hangup`, `m.call.select_answer`. Add `send_call_event(room_id, kind, payload)` helper on `MatrixClient`.

**Acceptance criteria:**
- Sync filter includes all five `m.call.*` types
- `send_call_event` compiles and routes through existing `send_event` path
- No new encryption code paths introduced
- `cargo test -p mxdx-matrix` passes

**Dependencies:** T-40
**Type:** feature
**Size:** S

### T-44 — npm wire-format migration: `session_key` → `mxdx_session_key` + `lifetime` default 30000

Coordinated npm side of the Phase 4 wire-format reconciliation (see ADR `2026-04-15-mcall-wire-format.md` 2026-04-16 addendum and ADR `2026-04-16-coordinated-rust-npm-releases.md`). Update npm emitters and parsers to use `mxdx_session_key` (formerly `session_key`) on `m.call.invite`. Also lower the default `lifetime` from `60000` to `30000` to match storm §4.1 and the Rust `CallInvite::DEFAULT_LIFETIME_MS`.

Scope of changes:
- `packages/core/p2p-signaling.js`: rename `content.session_key = sessionKey` → `content.mxdx_session_key = sessionKey`; default `lifetime` param 60000 → 30000.
- `packages/launcher/src/runtime.js`: offerer at `sendInvite({ sessionKey })` call site is unchanged (parameter name is internal); answerer at read-site change `inviteContent.session_key` → `inviteContent.mxdx_session_key`.
- `packages/web-console/src/terminal-view.js`: offerer `sendInvite({ sessionKey })` unchanged; answerer `inviteContent.session_key` → `inviteContent.mxdx_session_key`.
- Any JS test fixtures / golden files referencing `session_key` in P2P invite content.
- Rebuild `packages/web-console` dist bundles if they're committed.

**Acceptance criteria:**
- `grep -r 'session_key' packages/ | grep -v 'mxdx_session_key' | grep -v node_modules` returns nothing (the only remaining matches should be the new `mxdx_session_key`, or unrelated references like API-key context)
- `grep -r 'lifetime.*60000' packages/core/p2p-signaling.js` returns nothing
- `npm test --workspaces` passes (all JS tests)
- Existing beta E2E tests (`packages/e2e-tests/tests/p2p-*.test.js`) pass after the rename — NONE disabled or modified to accept both names. If a test hard-codes the old name in a fixture, update the fixture to the new name.
- Coordinated-release commit message includes the cross-language parity claim and references both the Rust T-40 commit and this task.

**Dependencies:** T-40 (must land first so the Rust emitter uses the new name; this npm change matches it)
**Type:** task
**Size:** S

---

## Phase 5: P2PTransport State Machine

### T-50 — `P2PState` enum and transition table

Add `crates/mxdx-p2p/src/transport/state.rs` with `P2PState` enum (9 states: `Idle`, `FetchingTurn`, `Inviting`, `Answering`, `Glare`, `Connecting`, `Verifying`, `Open`, `Failed`) and pure `transition(state, event) -> TransitionResult` function. Illegal transitions return `TransitionResult::Illegal`.

**Acceptance criteria:**
- All 9 states defined with correct field types
- Table-driven unit tests cover ≥30 `(from_state, input_event) -> expected_to_state` rows
- Illegal inputs produce `Illegal` result, not panic
- `cargo test -p mxdx-p2p` passes

**Dependencies:** T-42, T-43
**Type:** feature
**Size:** M

### T-51 — `P2PTransport` public API and driver loop skeleton

Add `P2PTransport` struct with `try_send(Megolm<Bytes>) -> SendOutcome`, `start`, `hangup`, `state`, `incoming`. Add driver loop (`tokio::select!` native / `wasm_bindgen_futures::spawn_local` wasm) dispatching to transition table from T-50.

**Acceptance criteria:**
- `try_send` returns `FallbackToMatrix` immediately when state is not `Open` (no blocking)
- `state()` is non-blocking
- Driver loop compiles for both native and wasm targets
- No panic paths in driver dispatch

**Dependencies:** T-50, T-31
**Type:** feature
**Size:** L

### T-52 — Idle timeout watchdog

Add `IdleWatchdog` tracking `last_io`. Fires when no I/O for configured idle window (default 5 min). Emits `Timeout` control message; driver transitions to `Idle`, sends `m.call.hangup(reason="idle_timeout")`, releases TURN.

**Acceptance criteria:**
- `tokio::time::pause/advance` unit test: idle fires after configured window with no I/O
- I/O activity resets the watchdog
- Idle hangup emits `p2p.state_transition` and `m.call.hangup` side-effect commands
- Test does not require real wall-clock time

**Dependencies:** T-51
**Type:** feature
**Size:** S

### T-53 — Verifying handshake (nonce + Ed25519 transcript)

Implement Verifying state: generate 32-byte nonce, AES-GCM encrypt and send, receive peer nonce, build transcript (`domain_sep_tag || room_id || session_uuid || call_id || our_nonce || peer_nonce || our_party_id || peer_party_id || sdp_fingerprint`), sign with device Ed25519 via `MatrixClient::device_keys`, verify peer signature. Mismatch → hangup, `security_event`, mark device `unverified_p2p`.

**Acceptance criteria:**
- Correct transcript construction (all 9 components in correct order)
- Integration test: wrong peer signature prevents `Open`
- Integration test: correct signature allows `Open`
- `security_event` telemetry emitted on verification failure
- Replay detection: replayed nonce from prior call rejected

**Dependencies:** T-51
**Type:** feature
**Size:** L

### T-54 — State machine unit test suite

Write unit tests covering: all 9 states exercised by happy-path tests, illegal-transition table, TURN-expiry-during-reconnect serialization, outbound queue overflow returning `FallbackToMatrix` at depth 256, fallback path posting identical `Megolm<Bytes>`.

**Acceptance criteria:**
- ≥30 state-transition test cases (happy + error)
- TURN expiry serialization test passes
- Overflow test returns `FallbackToMatrix` at depth 256
- Fallback path asserts identical byte content of Megolm payload
- All tests run without homeserver or real WebRTC

**Dependencies:** T-53
**Type:** test
**Size:** M

---

## Phase 6: Worker + Client Wiring

### T-60 — Wire `P2PTransport` into `mxdx-worker` session_mux

Construct `P2PTransport` per session when `p2p_enabled = true`. Route outbound through `try_send`; on `FallbackToMatrix` use existing Matrix path with same `Megolm<Bytes>`. Feed inbound `m.call.*` into the transport. Add `p2p_enabled: bool` to worker TOML (default `false`).

**Acceptance criteria:**
- Worker compiles with `p2p_enabled = false` (existing behavior unchanged)
- Worker compiles with `p2p_enabled = true`
- `cargo test -p mxdx-worker` passes
- `crates/mxdx-worker/tests/p2p_wiring.rs` integration test passes

**Dependencies:** T-54
**Type:** feature
**Size:** M

### T-61 — Wire `BatchedSender` window flip in worker

Add `set_batch_window(Duration)` to `BatchedSender`. Flip to 10ms on transport `Open`; revert to 200ms on non-`Open` transition. Transport emits `BatchWindowChange` side-effect command.

**Acceptance criteria:**
- Window is 200ms at startup regardless of `p2p_enabled`
- Window flips to 10ms when transport is `Open`
- Window reverts to 200ms on any non-`Open` transition
- Unit test covers both transitions without real WebRTC connection

**Dependencies:** T-60
**Type:** feature
**Size:** S

### T-62 — Wire `P2PTransport` into `mxdx-client` daemon

Symmetric to T-60 for `mxdx-client`. Add `--no-p2p` CLI flag for diagnostics. Wire `BatchedSender` window flip as in T-61.

**Acceptance criteria:**
- Client compiles with both `p2p_enabled` values
- `--no-p2p` forces `FallbackToMatrix` on every `try_send`
- `cargo test -p mxdx-client` passes
- `crates/mxdx-client/tests/p2p_wiring.rs` matches worker pattern

**Dependencies:** T-60
**Type:** feature
**Size:** M

### T-63 — npm perf baseline capture (EARLY — runs before any Rust wiring)

Before any Rust P2P wiring hits beta (i.e., before T-60 starts), run `perf-terminal.test.js` against the npm path on single-HS (ca1-beta) and federated (ca1↔ca2) topologies. Save results to `packages/e2e-tests/results/npm-p2p-baseline-<git-sha>.json` as the reference for the ±10% gate. **Council refinement:** this task was moved earlier so the baseline reflects pure npm behavior with zero Rust interference.

**Acceptance criteria:**
- Baseline results file committed
- All 6 metrics captured for both topologies
- Obtained against live beta servers with `test-credentials.toml`
- File header comment: "npm baseline — do not delete"
- Captured *before* any PR that sets `p2p_enabled=true` on beta

**Dependencies:** T-54 (runs in parallel with T-60 start; must complete before T-74)
**Type:** task
**Size:** S

### T-64 — Regression test pass: full existing E2E suite green

With T-60 and T-62 merged, `p2p_enabled = false`, run full existing E2E suite against both local tuwunel and beta. All previously-passing tests must pass. Blocking gate before Phase 7.

**Acceptance criteria:**
- All pre-existing JS E2E tests pass against local tuwunel
- All pre-existing beta tests pass
- `cargo test --workspace` passes
- No test was disabled or modified to achieve this

**Dependencies:** T-63
**Type:** test
**Size:** S

---

## Phase 7: JS E2E Suite (Beta)

### T-70 — `packages/e2e-tests/src/beta.js` shared helper

Factor out inlined `loadCredentials()` duplicated across existing beta tests. Add `loadBetaCredentials`, `skipIfNoBetaCredentials`, `loginBeta` (returns `WasmMatrixClient` or `RustClientHandle`), `provisionFederatedRoom`, `provisionSameHsRoom`, `assertTimingTolerant`. Update existing beta tests to use the module.

**Acceptance criteria:**
- Existing beta tests continue to pass after refactor
- `skipIfNoBetaCredentials` cleanly skips when `test-credentials.toml` absent
- `assertTimingTolerant` usable from any test file
- No duplication of credential loading logic remains

**Dependencies:** T-64
**Type:** task
**Size:** S

### T-71 — `rust-p2p-beta-single-hs.test.js` + fallback + glare suites

Write three test files targeting ca1-beta: basic P2P call (100 keystrokes, ≥95% via P2P); fallback when channel forced-closed mid-session; glare resolution when both sides send `m.call.invite`. Spawn `mxdx-worker` and `mxdx-client` as subprocesses.

**Acceptance criteria:**
- All three test files pass against ca1-beta
- P2P transport confirmed via telemetry log output
- Fallback test: no message loss during forced close
- Glare test: exactly one side wins, session reaches `Open`

**Dependencies:** T-70
**Type:** test
**Size:** L

### T-72 — `rust-p2p-beta-federated.test.js` (cross-HS topology)

Worker on ca2-beta, client on ca1-beta, shared encrypted room via `provisionFederatedRoom`. Assert P2P establishes across federated HSes. Verify Megolm decryption works on both sides.

**Acceptance criteria:**
- Test passes with `test-credentials.toml` for both ca1 and ca2 accounts
- Worker and client on different HSes reach `P2PState::Open`
- 100 keystrokes delivered, echoes in order, ≥95% via P2P
- Test skips cleanly when either beta server unreachable

**Dependencies:** T-71
**Type:** test
**Size:** M

### T-73 — `rust-npm-interop-beta.test.js` (8 combinations)

Write 8-combination interop matrix: {Rust client, npm client} × {Rust worker, npm launcher} × {single-HS, federated}. Each: 100 keystrokes, assert decrypted echoes in order, ≥95% P2P transport where both sides support P2P.

**Acceptance criteria:**
- All 8 combinations pass when Rust and npm peers available
- t4a/t4b (npm↔npm) confirm no npm regression
- t2/t3 combinations confirm Rust ↔ npm interop both directions
- Any combination can be skipped via env flag for debugging

**Dependencies:** T-72
**Type:** test
**Size:** XL (upgraded from L per council review — 8 combinations × topology setup + teardown + cross-runtime process coordination is higher-effort than L implies)

### T-74 — `rust-p2p-beta-perf.test.js` (perf gate)

Reuse `perf-terminal.test.js` harness. Collect all 6 metrics for Rust, median of 5 runs. Compare against npm baseline from T-63. Fail if any metric exceeds absolute SLO OR Rust more than 10% worse than npm. Network-weather mitigation: measure HS-to-HS RTT per run; discard if > 200ms (max 2 retries); normalize by RTT floor.

**Acceptance criteria:**
- Perf gate passes against ca1-beta (single-HS) and ca1↔ca2 (federated)
- Results written to `packages/e2e-tests/results/rust-p2p-beta-perf-<git-sha>.json`
- Test fails correctly on synthetic regression (validated with slow mock)
- Network-weather discards work correctly

**Dependencies:** T-73
**Type:** test
**Size:** L

### T-75 — `rust-p2p-beta-security.test.js` (security suite)

Security tests: wrong-peer signature (no `Open`, emits `verify_failure`); replay detection; plaintext-on-wire fuzzer (1-min federated, no frame decodes as plaintext); crypto downgrade (rate-limited hangup after 3/sec); signaling tamper (corrupted invite → clean error). Federated key-leak audit: observer on both HSes captures 30s of events (room timeline AND to-device), asserts no plaintext match.

**Acceptance criteria:**
- All 6 security scenarios tested and passing
- Each scenario asserts correct telemetry event is emitted
- Federated key-leak audit requires `test-credentials.toml` coordinator account
- `security-grep` CI check still passes after commit

**Dependencies:** T-74
**Type:** test
**Size:** L

---

## Phase 8: WASM + web-console

### T-80 — WASM `WebRtcChannel` implementation (`web-sys`)

Add `crates/mxdx-p2p/src/channel/wasm.rs` under `cfg(target_arch = "wasm32")`. Implement `WebRtcChannel` using `web-sys::RtcPeerConnection`, `web-sys::RtcDataChannel`, `wasm-bindgen-futures`. Translate JS promise callbacks to Rust futures. Translate datachannel events to `ChannelEvent` variants.

**Acceptance criteria:**
- `wasm-pack build crates/mxdx-p2p --target web` succeeds
- `wasm-pack build crates/mxdx-p2p --target nodejs` succeeds
- No libdatachannel symbols in wasm output
- All `WebRtcChannel` methods have non-panicking wasm implementations

**Dependencies:** T-30
**Type:** feature
**Size:** L

### T-81 — `mxdx-core-wasm` re-export of P2P surface

Add `P2PTransport`, `P2PCrypto`, `fetchTurnCredentials` to `mxdx-core-wasm` public API. Wire to `mxdx-p2p` implementations. Rebuild both nodejs and web WASM targets.

**Acceptance criteria:**
- `wasm-pack build crates/mxdx-core-wasm --target nodejs` and `--target web` both succeed
- `import { P2PTransport, P2PCrypto, fetchTurnCredentials } from '@mxdx/core'` resolves
- `wasm-pack test --headless --firefox crates/mxdx-p2p` channel surface tests pass
- WASM binary size does not exceed prior by more than 20%

**Dependencies:** T-80
**Type:** feature
**Size:** M

### T-82 — web-console swap to WASM exports

Update `packages/web-console` to import `P2PTransport`, `P2PCrypto`, `fetchTurnCredentials` from `@mxdx/core` (WASM) instead of `packages/core/p2p-*.js`. Ensure web-console still builds and E2E tests pass.

**Acceptance criteria:**
- `packages/web-console` imports no symbols from `p2p-crypto.js`, `p2p-signaling.js`, `p2p-transport.js`
- `npm run build` in `packages/web-console` succeeds
- Existing web-console E2E tests pass
- No behavior change visible in UI

**Dependencies:** T-81
**Type:** feature
**Size:** M

### T-83 — Deprecate npm P2P shims

Add JSDoc `@deprecated` and `console.warn` to `packages/core/p2p-*.js`. Do NOT delete yet. Update `MANIFEST.md`. Create tracking note for final deletion in T-C2.

**Acceptance criteria:**
- All three shim files annotated with deprecation warnings
- `console.warn` fires when any shim is imported directly
- `packages/launcher` (if still using shims) still works
- `MANIFEST.md` updated

**Dependencies:** T-82
**Type:** task
**Size:** S

### T-83a — `packages/launcher` migration to WASM exports (or explicit opt-out)

Audit `packages/launcher` for usage of `packages/core/p2p-*.js` shims. For each usage: either migrate the call site to import from `@mxdx/core` (WASM), or document explicitly why the launcher stays on the npm path. If the launcher stays on the npm path, T-C2 must be updated to preserve the shims for the launcher-only consumer (keeping the shims, but marking them launcher-internal).

**Acceptance criteria:**
- Audit report committed (list of every `p2p-*.js` import in launcher)
- Decision documented: migrate to WASM, or keep on npm path (with rationale)
- If migrating: launcher builds and tests pass after the swap
- If keeping: T-C2 acceptance criteria updated to exclude launcher-path shims from deletion

**Dependencies:** T-82
**Type:** task
**Size:** M

### T-84 — wasm-pack tests + Playwright federated smoke

Run `wasm-pack test --headless --firefox crates/mxdx-p2p` covering channel surface. Write `packages/e2e-tests/tests/web-console-rust-p2p-beta.test.js`: Playwright opens web-console against ca1-beta, connects to Rust worker on ca2-beta (federated), runs 20-keystroke session.

**Acceptance criteria:**
- `wasm-pack test --headless --firefox` channel tests pass
- Playwright federated smoke test passes against ca1+ca2
- `wasm-smoke` CI job runs on every PR (skips on forks)
- No plaintext events in federated session log

**Dependencies:** T-83
**Type:** test
**Size:** M

---

## Phase 9: Default-On Flip

### T-90 — Nightly perf monitoring for 3 consecutive green runs

Configure `e2e-beta-perf` CI to run nightly (plus per-PR). Add `scripts/check-perf-streak.sh` that reads last N nightly result files and reports green/not-green. Default-on flip cannot proceed until 3 consecutive green runs across both topologies.

**Acceptance criteria:**
- Nightly perf job runs on cron trigger in CI
- `check-perf-streak.sh 3` exits 0 only when last 3 runs all green
- Script is idempotent, handles missing result files gracefully
- Human approves T-91 after script confirms

**Dependencies:** T-74
**Type:** task
**Size:** S

### T-91 — Default-on config flip + release note

Change `p2p_enabled` default from `false` to `true` in both worker and client default TOML. Update `CHANGELOG.md` and user docs. Tag release. Supersede npm-era P2P beads (T-C0).

**Acceptance criteria:**
- Default config files updated with `p2p_enabled = true`
- `--no-p2p` still works and forces Matrix fallback
- `CHANGELOG.md` entry present
- Release tag created after human sign-off
- T-90 gate confirmed green before start

**Dependencies:** T-90
**Type:** task
**Size:** S

---

## Cleanup Phase

### T-C0 — Supersede npm-era P2P beads

Close or supersede:
- `mxdx-8y1` (P2P WebRTC wrappers) → T-31 + T-80
- `mxdx-vuy` (P2P Signaling) → T-40 + T-41 + T-42
- `mxdx-4yf` (P2P Integration) → T-60 + T-62
- `mxdx-eud` (P2P UI) → T-82
- `mxdx-xi6` (P2P E2E) → T-71 through T-75

Each with closure comment referencing the Rust task that replaced it.

**Acceptance criteria:**
- All five beads closed with superseded-by comment
- No open work items reference the npm-era P2P schema
- MANIFEST.md updated to remove deprecated shim entries

**Dependencies:** T-91
**Type:** task
**Size:** S

### T-C1 — Final docs pass

Update ADRs with "Implemented in: <git-sha>" annotations. Update `MANIFEST.md` with new `mxdx-p2p` modules. Update `crates/mxdx-p2p/README.md` to final state.

**Acceptance criteria:**
- All four ADRs have "Implemented in" line
- `MANIFEST.md` has entries for `P2PTransport`, `P2PCrypto`, `WebRtcChannel`, `TurnCredentials`, `signaling::*`
- `README.md` reflects final state, no TODOs in src/

**Dependencies:** T-C0
**Type:** task
**Size:** S

### T-C2 — Delete deprecated npm P2P shims (conditional on T-83a)

If T-83a migrated `packages/launcher` to WASM: delete `packages/core/p2p-crypto.js`, `p2p-signaling.js`, `p2p-transport.js`. Remove shim entries from `packages/core/index.js`. Rebuild `@mxdx/core` and verify all consumers work.

If T-83a opted to keep launcher on npm path: leave shims in place, remove only the deprecation warnings but keep files, mark them as launcher-internal in `MANIFEST.md`.

**Acceptance criteria:**
- All three shim files deleted
- No import of any deleted file anywhere in the workspace
- `npm run build` in all packages succeeds
- Full E2E suite passes after deletion
- Shim deletion announced in `CHANGELOG.md`

**Dependencies:** T-C1
**Type:** task
**Size:** S

---

## Nurture Umbrella

### T-NUR — Monitor P2P health post-default-on

After default-on flip, run 2-week monitoring window: nightly perf continues, telemetry dashboards reviewed for regression in `p2p.fallback` rates, `p2p.security_event` counts, handshake latency drift. Any SLO-violating metric triggers rollback consideration. Closes when 14 consecutive nightly green runs recorded.

**Acceptance criteria:**
- Nightly perf job continues post-T-91
- `p2p.fallback` rate below 5% in steady state
- Zero `verify_failure` or `replay_detected` security events in production logs
- 14 consecutive nightly green runs logged before closing

**Dependencies:** T-91
**Type:** task
**Size:** M

---

## Secure Umbrella

### T-SEC — Security review: full P2P surface audit

Conduct complete security review of all code merged under this plan before T-91. Scope: `mxdx-p2p` crate in full, `Megolm<Bytes>` + `SealedKey` invariant enforcement, Verifying transcript, CI grep gate, federated key-leak audit results. Produce `docs/reviews/security/2026-04-15-rust-p2p-security-review.md`. No unresolved HIGH or CRITICAL findings.

**Acceptance criteria:**
- Security review document exists at specified path
- No unresolved HIGH or CRITICAL findings
- Trybuild negative tests (T-13) confirmed passing
- `check-no-unencrypted-sends.sh` confirmed in CI and passing
- Federated key-leak audit (T-75) confirmed passing

**Dependencies:** T-75
**Type:** task
**Size:** M

---

## Dependency Graph (phase-level)

```
Phase 0: T-00 → T-01 → T-02 → T-03
                                 │
Phase 1: (T-00) → T-10 → T-11 → T-12
                          └─── T-13
Phase 2: (T-00) → T-20 → T-21 → T-22
Phase 3: (T-00) → T-30 → T-31 → T-32 → T-33
Phase 4: (T-13) → T-40 → T-41
                    ├── T-42
                    ├── T-43
                    └── T-44 (npm coordinated release)
                                 │
Phase 5: (T-42, T-43, T-31) → T-50 → T-51 → T-52
                                         ├── T-53
                                         └── T-54
                                 │
Phase 6: (T-54) → T-60 → T-61
                    ├── T-62
                    └── T-63 → T-64
                                 │
Phase 7: (T-64) → T-70 → T-71 → T-72 → T-73 → T-74 → T-75
                                 │
Phase 8: (T-30) → T-80 → T-81 → T-82 → T-83 → T-84
                                 │
Phase 9: (T-74, T-84) → T-90 → T-91
         (T-75) → T-SEC
                  T-91 → T-NUR
                                 │
Cleanup: (T-91) → T-C0 → T-C1 → T-C2
```

**Parallel tracks after Phase 0:** crypto (Phase 1), TURN (Phase 2), channel trait (Phase 3), signaling (Phase 4) can run in parallel. Converge at Phase 5.

**WASM track:** T-80 can begin as soon as T-30 is done — independent of native impl and state machine.

**T-SEC** runs in parallel with T-90 once T-75 is complete.

---

## Task Matrix

| Phase | Tasks | Estimated Duration |
|---|---|---|
| Phase 0: Scaffolding | 4 | 1–2 days |
| Phase 1: P2PCrypto + Vectors | 4 | 2–4 days |
| Phase 2: TurnCredentials | 3 | 1–2 days |
| Phase 3: WebRtcChannel + Native | 5 | 5–9 days (T-31 upgraded to XL; + T-34 musl) |
| Phase 4: Signaling + Glare | 5 | 2–4 days (+ T-44 coordinated npm migration) |
| Phase 5: P2PTransport SM | 5 | 5–9 days |
| Phase 6: Worker + Client Wiring | 5 | 3–5 days (T-63 moved earlier, now runs parallel with T-60) |
| Phase 7: JS E2E Suite | 6 | 6–11 days (T-73 upgraded to XL) |
| Phase 8: WASM + web-console | 6 | 5–9 days (+ T-83a launcher migration) |
| Phase 9: Default-On Flip | 2 | 1–2 days |
| Cleanup | 3 | 1–2 days |
| Nurture (umbrella) | 1 | 14-day monitor window |
| Secure (umbrella) | 1 | 1–2 days |
| **Total** | **50** | **~33–60 dev-days** |

---

## Superseded Beads

The following npm-era P2P beads are superseded by this plan and must be closed in T-C0:

| Bead ID | npm-era Description | Superseded By |
|---|---|---|
| `mxdx-8y1` | P2P WebRTC wrappers | T-31 (native), T-80 (wasm) |
| `mxdx-vuy` | P2P Signaling | T-40, T-41, T-42 |
| `mxdx-4yf` | P2P Integration | T-60, T-62 |
| `mxdx-eud` | P2P UI | T-82 |
| `mxdx-xi6` | P2P E2E | T-71–T-75, T-84 |

---

## Open Questions Flagged by Planning Subagent

1. **`scripts/check-no-unencrypted-sends.sh` is net-new** — the megolm-bytes ADR lists it as a consequence but it does not yet exist. T-03 creates it.

2. **libdatachannel musl/static build** — T-33 documents native dep setup for glibc Linux and macOS, but if the deployment target includes Alpine/musl, a dedicated cross-compile task may be needed between T-31 and T-32.

3. **Loopback test mechanism (T-32)** — libdatachannel may require real ICE negotiation even for same-process loopback. If the library insists on network sockets, T-32 may need a 127.0.0.1 TURN stub or a `MockWebRtcChannel` variant. Resolved during implementation.

4. **Perf baseline timing (T-63)** — the npm baseline must be captured before any Rust P2P code is active on beta. Confirm npm baseline is captured at the *start* of T-63, not after Rust wiring merges.

5. **`packages/launcher` shim usage** — after T-83 the shims are deprecated but still usable; T-C2 deletes them. Confirm whether `packages/launcher` will migrate to WASM imports (required before T-C2) or continues on the npm path (in which case T-C2 needs to exclude `packages/launcher` consumers).
