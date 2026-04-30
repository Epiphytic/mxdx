## Summary

<!--
Describe the change and its motivation.
-->

## Checklist

- [ ] Rust unit tests pass: `cargo test --workspace --lib --exclude mxdx-core-wasm`
- [ ] npm unit tests pass: `node --test packages/launcher/tests/runtime-unit.test.js`
- [ ] Both WASM targets present: `packages/core/wasm/nodejs/mxdx_core_wasm.js` and `packages/core/wasm/web/mxdx_core_wasm.js`
- [ ] Every Matrix `send_state_event` / `send_raw` call in `packages/` and `crates/mxdx-core-wasm/` is encryption-aware (MSC4362 or Megolm)

### Cross-reference accuracy (required when touching `packages/launcher/src/` or `crates/mxdx-core-wasm/src/`)

Each `// Rust equivalent: <path>::<item>` comment in `packages/launcher/src/` must point to a real Rust item.

Reviewer: confirm that every path cited in the `Rust equivalent:` comments below resolves in `cargo doc`:

<!--
Run: grep -r 'Rust equivalent:' packages/launcher/src/ packages/core/index.js
For each cited path, verify it exists: grep -rn '<function-or-struct>' crates/
-->

- [ ] All `// Rust equivalent:` citations verified accurate by reviewer
