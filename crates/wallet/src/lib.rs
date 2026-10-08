//! Governance co-signing against a Tari Ootle wallet daemon.

pub mod key_ref;
pub mod store;
pub mod wallet;

pub use tari_ootle_wallet_sdk::models::{Account, KeyId};
pub use wallet::*;
