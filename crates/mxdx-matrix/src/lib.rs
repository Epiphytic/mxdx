pub mod backup;
pub mod client;
pub mod crypto_envelope;
pub mod error;
pub mod multi_hs;
pub mod reencrypt;
pub mod rest;
pub mod rooms;
pub mod session;

pub use matrix_sdk;
pub use client::{default_store_base_path, short_hash, MatrixClient};
pub use crypto_envelope::{Bytes, Megolm};
pub use error::MatrixClientError;
pub use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId, RoomId, UserId};
pub use multi_hs::{MultiHsClient, ServerAccount, ServerHealth, ServerStatus};
pub use rooms::LauncherTopology;
pub use session::{
    connect_with_session, normalize_server, password_key, session_key, store_key_key, SessionData,
};
