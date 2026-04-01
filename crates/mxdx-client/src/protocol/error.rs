// Standard JSON-RPC 2.0 error codes
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

// Application error codes
pub const NO_WORKER: i32 = -1;
pub const WORKER_OFFLINE: i32 = -2;
pub const WORKER_STALE: i32 = -3;
pub const UNAUTHORIZED: i32 = -4;
pub const SESSION_NOT_FOUND: i32 = -5;
pub const TRANSPORT_EXISTS: i32 = -6;
pub const MATRIX_UNAVAILABLE: i32 = -7;
pub const CREDENTIAL_MISMATCH: i32 = -8;
