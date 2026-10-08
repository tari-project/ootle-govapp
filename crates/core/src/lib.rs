//! Proposals, co-signatures and the checks around them for Ootle governance transactions.
//!
//! A proposer builds a transaction that makes one governance call and freezes it. Council members each sign
//! the same message — the transaction plus the proposer's seal public key — and return the signature. The
//! proposer attaches the signatures and seals. Everything here is pure: wallet and network access live in
//! `ootle_gov_wallet`.

pub mod action;
pub mod amount;
pub mod armor;
pub mod builder;
pub mod council;
pub mod error;
pub mod payload;
pub mod policy;

use std::fmt::{self, Display};

pub use ootle_network::Network;
pub use tari_ootle_transaction::ComponentReference;

/// The leading bytes of a proposal's signing message, for members to compare out of band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 8]);

impl Fingerprint {
    pub fn from_message_hash(message_hash: &[u8; 64]) -> Self {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&message_hash[..8]);
        Self(bytes)
    }

    /// A filesystem- and URL-safe form, e.g. `3f9a1c0788d2b4e1`.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl Display for Fingerprint {
    /// `3F9A-1C07-88D2-B4E1`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hex = hex::encode_upper(self.0);
        let groups: Vec<&str> = (0..4).map(|i| &hex[i * 4..i * 4 + 4]).collect();
        f.write_str(&groups.join("-"))
    }
}
