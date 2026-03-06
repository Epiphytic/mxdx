# Phase 3: Test Infrastructure -- Summary

**Status:** Complete
**Date:** 2026-03-06

## Goal

Build real Tuwunel-based test helpers that start actual homeserver instances, with OS-assigned ports and federated pair support.

## Completion Gate

All gates satisfied:

- `TuwunelInstance::start().await` starts a real Tuwunel, health check passes, user registration works
- `FederatedPair::start().await` starts two federated instances
- All ports OS-assigned (security finding mxdx-ji1)
- Integration test CI job runs with Tuwunel installed

## Tasks Completed

### Task 3.1: TuwunelInstance Helper (mxdx-w9n.1)

Implemented `TuwunelInstance` in `tests/helpers/src/tuwunel.rs`:

- Starts a real Tuwunel process with a temporary data directory
- Binds to OS-assigned port (port 0) -- no hardcoded ports
- Provides health check polling until server is ready
- Supports user registration via Matrix client API
- Cleans up on drop (kills process, removes temp dir)

### Task 3.2: FederatedPair Helper (mxdx-w9n.2)

Implemented `FederatedPair` in `tests/helpers/src/federation.rs`:

- Starts two `TuwunelInstance`s configured to federate with each other
- Uses `.localhost` TLD (RFC 6761) -- no mkcert or /etc/hosts needed
- Both instances use OS-assigned ports

### Task 3.3: Integration Test CI Job (mxdx-w9n.3)

Added CI job that installs Tuwunel and runs integration tests requiring a live homeserver.

## Artifacts

- `tests/helpers/src/tuwunel.rs` -- TuwunelInstance helper
- `tests/helpers/src/federation.rs` -- FederatedPair helper
- `tests/helpers/src/matrix_client.rs` -- TestMatrixClient helper
- `tests/helpers/src/lib.rs` -- Test helpers crate root
