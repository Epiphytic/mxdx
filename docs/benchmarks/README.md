# mxdx Performance Benchmarks

## Prerequisites
- Rust toolchain (cargo build)
- Node.js 22+ (npm)
- tmux installed
- test-credentials.toml with beta server accounts

## Workloads

All benchmarks use the same workloads across all transports:

| Workload | Command | Measures |
|---|---|---|
| echo | `/bin/echo hello world` | session setup + round-trip latency |
| exit-code | `/bin/false` | exit code propagation latency |

## Running Benchmarks

### Rust binary benchmarks (requires test-credentials.toml)
```
cargo test -p mxdx-worker --test e2e_profile -- --ignored --nocapture
```

### Local dev infrastructure tests (requires tuwunel binary)
```
cargo test -p mxdx-worker --test e2e_binary -- --ignored --nocapture
```

## Results
Results are printed to stdout. Save with `--nocapture 2>&1 | tee docs/benchmarks/results-$(date +%Y%m%d).txt`
