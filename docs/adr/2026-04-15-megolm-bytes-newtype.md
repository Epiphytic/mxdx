# ADR 2026-04-15: `Megolm<Bytes>` newtype enforces plaintext-on-wire invariant structurally

**Status:** Accepted
**Date:** 2026-04-15
**Related:** `docs/plans/2026-04-15-rust-p2p-interactive-storm.md`, CLAUDE.md cardinal rule

## Context

The project's cardinal rule states: **every Matrix event and every byte on the P2P data channel must be end-to-end encrypted — no exceptions.** CLAUDE.md is explicit: "If you find yourself calling `send_state_event`, `send_raw`, or any Matrix send API and you are not 100% sure the event will be encrypted on the wire, STOP and audit the call path."

The P2P path is a new send surface. The `P2PTransport::try_send` function takes a payload, encrypts it with AES-GCM (P2PCrypto), and writes it to the data channel. The caller is responsible for Megolm-encrypting the payload *before* passing it to `try_send` — the AES-GCM layer is defense-in-depth only.

In an initial draft of the storm spec, the signature was:

```rust
pub async fn try_send(&self, megolm_ciphertext: &[u8]) -> SendOutcome;
```

with a debug-assert that checked whether the bytes parse as a Megolm-encrypted envelope. The star-chamber review (integrated into the storm spec) flagged this as a runtime-only check: **the compiler does not enforce the invariant.** A future refactor, a mistaken caller, or a third-party integration could pass plaintext to `try_send` and succeed at runtime (in release builds the debug-assert is a no-op).

## Decision

Introduce a Rust newtype `Megolm<T>` in `mxdx-matrix` whose constructor is *package-private* and only callable from `MatrixClient::encrypt_for_room`. The `try_send` signature becomes:

```rust
pub async fn try_send(&self, payload: Megolm<Bytes>) -> SendOutcome;
```

`Megolm<Bytes>` can only be obtained by calling `MatrixClient::encrypt_for_room(&room_id, &event_type, content) -> Megolm<Bytes>`. There is no public constructor. The inner bytes are accessible only via a method named `into_ciphertext_bytes()` whose doc warns: "these bytes are Megolm-encrypted; do not decrypt here."

In parallel:

```rust
pub struct SealedKey(pub(in crate::crypto) Key<Aes256Gcm>);
```

The P2P session key is a similarly sealed newtype, constructible only inside `mxdx-p2p::crypto`. The only way to transport it to a peer is via `signaling::events::build_invite(sealed_key)`, which embeds it in a Megolm-encrypted `m.call.invite`.

## Rationale

- **Structural vs runtime enforcement.** A newtype whose constructor is package-private makes the invariant part of the type system. Plaintext cannot compile; it cannot even be represented at the call site of `try_send`. Debug-asserts do not protect release builds; the type system does.
- **Matches the project rule's intent.** The rule says "if you are not 100% sure the event will be encrypted on the wire, STOP." With `Megolm<Bytes>`, you *cannot* not be sure — the type tells you.
- **Idiomatic Rust.** Newtype wrappers for "this value has passed a check" are standard (e.g., `ValidatedEmail`, `SanitizedInput`). The pattern is well-understood by reviewers.
- **Trybuild tests.** We can add a `trybuild` test that tries to construct `Megolm` outside `mxdx-matrix` and asserts compilation failure. This turns the invariant into a regression test that survives refactors.
- **Same pattern for `SealedKey`.** Both the Megolm payload AND the AES-GCM session key are sealed via the same mechanism, giving symmetric type-level guarantees for the two most security-critical values in the transport.

## Consequences

- `mxdx-matrix` gains a public `Megolm<T>` type with package-private constructor
- `mxdx-matrix` exposes `encrypt_for_room(...) -> Megolm<Bytes>` as the only way to obtain a `Megolm<Bytes>` for P2P send
- `mxdx-matrix` exposes `send_megolm(room_id, Megolm<Bytes>)` for the fallback path (posts the payload through `room.send_raw`, which Megolm-encrypts in-flight using the same room session — see "Semantic equivalence vs byte-identity" below)
- `mxdx-p2p::P2PTransport::try_send` signature requires `Megolm<Bytes>`
- Callers inside `mxdx-worker` and `mxdx-client` always call `encrypt_for_room` first, then `try_send` — there is no way to forget to encrypt
- `mxdx-p2p::crypto::SealedKey` constructor is pub(in crate::crypto); the type appears in `signaling::events::build_invite(sealed_key)` signature and nowhere else callable from outside the crypto module
- Trybuild tests `trybuild/megolm-constructor-fails.rs` and `trybuild/sealedkey-constructor-fails.rs` assert the invariant survives refactors
- The CI grep gate (`scripts/check-no-unencrypted-sends.sh`) remains in place as a backstop against future `send_raw`-style additions

## Alternatives considered and rejected

- **Debug-asserts only:** rejected — release builds strip them, and a grep/review-time discipline is weaker than the type system.
- **`Encrypted<Bytes>` (generic name):** rejected — too easy for a future developer to add a constructor to (e.g., for a different encryption scheme). The name `Megolm<Bytes>` ties the type to Matrix's specific encryption, making the invariant explicit in every signature.
- **A `SendEncrypted` trait that `MatrixClient` and `P2PTransport` both implement:** rejected — does not prevent a caller from calling `send_bytes(plaintext)` on either implementation. The constraint must be on the *value*, not the *operation*.
- **Byte-identical Megolm ciphertext on P2P and Matrix fallback paths (via matrix-sdk `testing` feature or vendor fork):** rejected — see addendum below.

## Addendum (2026-04-16) — Semantic equivalence vs byte-identity

The initial draft of this ADR and the storm spec §3.2/§3.3 used language implying that the Matrix fallback path would post the *byte-identical* Megolm ciphertext produced by `encrypt_for_room`. Phase 1 implementation investigation surfaced that this is not achievable via matrix-sdk 0.16's public API: `OlmMachine::encrypt_room_event_raw` is `pub(crate)`, reachable from outside only via `olm_machine_for_testing` (gated on matrix-sdk's `testing` cargo feature).

Three options were evaluated:

1. **Enable matrix-sdk `testing` feature in production** — rejected. Enabling a third-party crate's test-only feature flag on a security-critical transport is itself a smell; the maintainers marked the API private deliberately and it is not subject to the same compatibility/audit discipline as the stable API.
2. **Vendor matrix-sdk** — rejected. Disproportionate maintenance cost for a marginal benefit; creates a long-term fork-tracking obligation for the project.
3. **Accept semantic equivalence instead of byte-identity** — **accepted**.

### What changes

`Megolm<T>` remains a type-system marker that a payload has *crossed the encryption boundary*. The two transport paths produce ciphertexts that are semantically equivalent but NOT byte-identical:

- **P2P path:** `try_send(Megolm<Bytes>)` places the `Bytes` payload inside an AES-GCM frame. The AES-GCM session key was exchanged inside a Megolm-encrypted `m.call.invite`, so the P2P session is transitively Megolm-authenticated. The `Bytes` inside `Megolm<Bytes>` is the already-Megolm-encrypted event payload from the same room session the receiver uses.
- **Matrix fallback:** `send_megolm(room_id, Megolm<Bytes>)` calls `room.send_raw(event_type, content)` — the matrix-sdk public API — which Megolm-encrypts in-flight using the same room outbound session. The bytes on the wire differ from the P2P-path bytes (fresh encrypt with fresh IV), but the plaintext decrypts to the same event via the same Megolm session on the receiver.

### What stays the same

All six load-bearing properties of the invariant are preserved:

1. **Plaintext-on-wire is a compile error.** `Megolm<Bytes>` has no public constructor; the only way callers get one is via `MatrixClient::encrypt_for_room`.
2. **Every byte on either transport is encrypted.** P2P: AES-GCM-wrapping-Megolm. Matrix: Megolm.
3. **Every Matrix event is E2EE.** Fallback uses `room.send_raw` which inherits the existing room E2EE pipeline.
4. **Receiver pipeline is unchanged.** Both transports arrive as decryptable Megolm events against the same room session. The decrypt path is identical.
5. **Dedupe still works.** `TerminalSocket` sequence numbers (pre-existing in the npm code, preserved in the Rust port) dedupe at the application layer — neither transport relies on ciphertext-hash equality for dedupe.
6. **Type-system enforcement is unchanged.** Trybuild tests still assert that constructing `Megolm` outside `mxdx-matrix` is a compile error.

### Storm spec cross-reference

Storm spec §3.2 and §3.3 were updated in the same commit as this addendum to replace "identical Megolm ciphertext" / "same Megolm payload" language with "semantically equivalent Megolm-encrypted bytes against the same room session". No other sections of the storm spec or the other three ADRs required changes.

## Second addendum (2026-04-16) — Byte-identical ciphertext restored via `testing` feature

ADR `2026-04-16-matrix-sdk-testing-feature.md` authorized enabling matrix-sdk's `testing` cargo feature in production. This unblocks `OlmMachine::encrypt_room_event_raw` via `Client::olm_machine_for_testing()`, which was the original Phase 1 intent before the first addendum relaxed to semantic equivalence.

### What changed

`MatrixClient::encrypt_for_room` now calls `OlmMachine::encrypt_room_event_raw` directly, producing actual `m.room.encrypted` JSON in the `Megolm<Bytes>` wrapper. `MatrixClient::send_megolm` sends this already-encrypted content as `m.room.encrypted` without re-encrypting.

Both transport paths now carry **byte-identical** Megolm ciphertext for a given plaintext + room + session-key state:

- **P2P path:** `try_send(Megolm<Bytes>)` wraps the already-encrypted bytes in an AES-GCM frame.
- **Matrix fallback:** `send_megolm` sends the same bytes as `m.room.encrypted`.

### What stays the same

All six invariants from the first addendum remain unchanged. The type-system enforcement is unchanged — `Megolm<Bytes>` still has no public constructor and trybuild tests still assert compilation failure.

### Compile-time surface change

`Cargo.toml` workspace dependency for `matrix-sdk` adds `"testing"` to its feature list. This pulls `wiremock`, `matrix-sdk-test`, and `assert_matches2` into the production dependency graph — accepted per ADR `2026-04-16-matrix-sdk-testing-feature.md` precondition 3.

### Reversibility

When matrix-sdk ships a non-`testing`-gated accessor for `olm_machine()`, revert `encrypt_for_room` to use the public API and remove `"testing"` from the feature list. The `Megolm<Bytes>` type and all external API signatures are unchanged.
