# mxdx Benchmark Results — {date}

## Environment
- Host: {hostname}
- OS: {os}
- Rust: {rust_version}
- Node.js: {node_version}

## Results

| Transport | Echo (p50) | Echo (p95) | Lifecycle (p50) | Interactive RTT (p50) |
|---|---|---|---|---|
| SSH localhost | - | - | N/A | - |
| mxdx Rust local | - | - | - | - |
| mxdx npm local | - | - | - | - |
| mxdx Rust beta single | - | - | - | - |
| mxdx Rust beta federated | - | - | - | - |
| npm->Rust worker | - | - | - | - |

## Notes
- All latencies in milliseconds
- p50/p95 over {N} measured runs after {M} warmup runs
- "Lifecycle" = task submission to result receipt
