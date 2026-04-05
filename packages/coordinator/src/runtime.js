/**
 * Coordinator Runtime — thin JS shell for the WASM coordinator logic.
 * Full routing/watchlist/claim logic is in the Rust mxdx-coordinator crate.
 * This JS wrapper provides the npm-installable entry point.
 */
export class CoordinatorRuntime {
    constructor(config) {
        this.config = config;
    }

    async start() {
        console.log('CoordinatorRuntime: not yet implemented in JS — use native binary');
    }
}
