//! The two messages members pass to each other.
//!
//! Neither carries anything a reader has to trust: every human-readable fact is derived from the transaction
//! itself, and the memo is labelled as unverified wherever it is shown.

use minicbor::{CborLen, Decode, Encode};
use tari_ootle_transaction::{TransactionSignature, UnsignedTransaction};
use tari_template_lib_types::crypto::RistrettoPublicKeyBytes;

use crate::Fingerprint;

pub const MAX_MEMO_CHARS: usize = 280;

/// A frozen governance transaction plus the key that will seal it.
///
/// Every co-signature commits to the seal signer's public key, so it travels with the transaction.
#[derive(Debug, Clone, Encode, Decode, CborLen)]
pub struct ProposalV1 {
    #[n(0)]
    pub unsigned: UnsignedTransaction,
    #[n(1)]
    pub seal_public_key: RistrettoPublicKeyBytes,
    /// Free text from the proposer. Nothing verifies it.
    #[n(2)]
    pub memo: String,
}

impl ProposalV1 {
    /// The exact message every co-signer signs.
    pub fn message_hash(&self) -> [u8; 64] {
        TransactionSignature::create_message(&self.seal_public_key, &self.unsigned)
    }

    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::from_message_hash(&self.message_hash())
    }
}

/// One member's co-signature over a proposal.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode, CborLen)]
pub struct SignatureV1 {
    /// The message the signature signs. A signature pasted against the wrong proposal is reported as such before
    /// any verification.
    #[n(0)]
    #[cbor(with = "minicbor::bytes")]
    pub message_hash: [u8; 64],
    #[n(1)]
    pub signature: TransactionSignature,
}

impl SignatureV1 {
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::from_message_hash(&self.message_hash)
    }

    pub fn public_key(&self) -> &RistrettoPublicKeyBytes {
        self.signature.public_key()
    }
}

/// The CBOR body of an armored block. The variant index is the payload version: each layout gets its own variant,
/// and existing variants are frozen.
#[derive(Debug, Clone, Encode, Decode, CborLen)]
pub enum Payload {
    #[n(0)]
    ProposalV1(#[n(0)] ProposalV1),
    #[n(1)]
    SignatureV1(#[n(0)] SignatureV1),
}

impl Payload {
    pub fn kind(&self) -> PayloadKind {
        match self {
            Self::ProposalV1(_) => PayloadKind::Proposal,
            Self::SignatureV1(_) => PayloadKind::Signature,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadKind {
    Proposal,
    Signature,
}

impl PayloadKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Proposal => "PROPOSAL",
            Self::Signature => "SIGNATURE",
        }
    }

    pub const fn noun(self) -> &'static str {
        match self {
            Self::Proposal => "proposal",
            Self::Signature => "signature",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "PROPOSAL" => Some(Self::Proposal),
            "SIGNATURE" => Some(Self::Signature),
            _ => None,
        }
    }
}
