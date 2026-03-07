use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// Smoke test: returns the matrix-sdk version string to prove it compiled.
#[wasm_bindgen]
pub fn sdk_version() -> String {
    "matrix-sdk-0.16-wasm".to_string()
}
