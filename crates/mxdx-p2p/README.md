# mxdx-p2p

Platform-agnostic P2P transport for mxdx interactive sessions: `WebRtcChannel`
trait + cfg-gated native/wasm implementations, a `P2PTransport` state machine,
the `P2PCrypto` AES-256-GCM defense-in-depth layer over Megolm, Matrix VoIP
`m.call.*` signaling helpers, and the `/voip/turnServer` TURN credential
client.

Runtime wiring (SessionMux, BatchedSender, session lifecycle, telemetry) lives
in `mxdx-worker` and `mxdx-client`. See ADR
`docs/adr/2026-04-15-mxdx-p2p-crate.md` for the architecture rationale.

## Cardinal rule

**Every byte on the P2P data channel is Megolm-ciphertext.** The caller must
produce a `Megolm<Bytes>` via `MatrixClient::encrypt_for_room` before
`P2PTransport::try_send` will accept it — plaintext cannot compile. See ADR
`docs/adr/2026-04-15-megolm-bytes-newtype.md`.

The crate is CI-gated by `scripts/check-no-unencrypted-sends.sh`, which fails
on any occurrence of `send_raw|skip_encryption|unencrypted` in non-test
source.

## Running tests

```sh
# Full unit + integration + trybuild suite (no network required)
cargo test -p mxdx-p2p

# Wasm target compiles (Phase 8 will implement real wasm impls)
cargo check -p mxdx-p2p --target wasm32-unknown-unknown
```

## Manual smoke test — TURN credentials (Phase 2)

`fetch_turn_credentials` talks to a real Matrix homeserver at
`GET /_matrix/client/v3/voip/turnServer`. The CI suite is offline — beta
smoke is an operator step.

### Prerequisites

You need a valid Matrix access token for a beta account. The project stores
these in `test-credentials.toml` at the repository root (not checked in).
Format excerpt:

```toml
[ca1_beta]
homeserver = "https://ca1-beta.mxdx.dev"
user_id    = "@you:ca1-beta.mxdx.dev"
access_token = "syt_..."
```

Copy the `homeserver` and `access_token` into env vars for this session.

### Running the smoke test

```sh
export TEST_HS_URL=https://ca1-beta.mxdx.dev
export TEST_TOKEN=syt_...               # from test-credentials.toml
cargo test -p mxdx-p2p --test turn_smoke -- --ignored --nocapture
```

`--ignored` is required because the test is `#[ignore]`d by default so that
the standard `cargo test` run stays network-free.

### What to expect

On a homeserver that provisions TURN (the common case on the beta fleet), the
test prints non-sensitive metadata — URI count and TTL — and passes:

```
TURN smoke: fetched ok (TurnCredentials { uri_count: 2, username: "<redacted>", \
  password: "<redacted>", ttl: 86400s, fetched_at: SystemTime { .. } })
TURN smoke: 2 uri(s), ttl 24h, refresh_at SystemTime { .. }
```

On a homeserver without TURN configured, the test also passes but prints:

```
TURN smoke: homeserver did not provision TURN (Ok(None)) — treating as pass
```

### Security notes

* The test NEVER prints `username` or `password`. They are redacted by
  `TurnCredentials`'s custom `Debug` impl.
* The bearer token is passed only in the `Authorization` header to the
  homeserver URL. HTTP redirects are disabled (`reqwest::redirect::Policy::none()`)
  so the token can never leak to a different origin.
* `TEST_TOKEN` is your live beta account token — treat it as a secret.
  Prefer putting it only in an environment variable for the duration of the
  test run; do not commit it anywhere.

### CI integration

CI does not run this test. The `rust-p2p-unit` job runs `cargo test -p mxdx-p2p`
(without `--include-ignored`), which skips `turn_smoke`. The `security-grep`
job runs `scripts/check-no-unencrypted-sends.sh`.
