//! Local state, one JSON file per proposal. Holds no secrets: proposals and signatures are what members
//! paste to each other anyway.

use std::{
    fs,
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use ootle_gov_core::{
    armor,
    council::Council,
    error::{PayloadError, SignatureError},
    payload::{Payload, PayloadKind, ProposalV1, SignatureV1},
    policy::ProposalSummary,
};
use serde::{Deserialize, Serialize};
use tari_ootle_transaction::TransactionId;
use tari_ootle_wallet_sdk::models::{KeyId, SigningRequestId, TransactionRequestId};
use tari_template_lib_types::crypto::RistrettoPublicKeyBytes;

/// A proposal this wallet made, and how far it has got.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalRecord {
    /// The armored proposal, exactly as handed to members.
    pub proposal: String,
    /// The fee account's owner key, which seals.
    pub seal_key_id: KeyId,
    /// The proposer's own council key when it is not the seal key. walletd signs with it at submission.
    pub council_key_id: Option<KeyId>,
    pub council_public_key: Option<RistrettoPublicKeyBytes>,
    /// Armored co-signatures, each verified when added.
    pub signatures: Vec<String>,
    pub transaction_request_id: Option<TransactionRequestId>,
    pub transaction_id: Option<TransactionId>,
    /// The final result, once known.
    pub outcome: Option<String>,
    pub created_at: u64,
}

impl ProposalRecord {
    pub fn decode_proposal(&self) -> Result<ProposalV1, PayloadError> {
        decode_proposal(&self.proposal)
    }

    pub fn decode_signatures(&self) -> Result<Vec<SignatureV1>, PayloadError> {
        self.signatures.iter().map(|s| decode_signature(s)).collect()
    }

    /// Keys the submitted transaction will be signed by, apart from the collected co-signatures.
    pub fn own_signers(&self, summary: &ProposalSummary) -> Vec<RistrettoPublicKeyBytes> {
        let mut keys = Vec::with_capacity(2);
        if summary.seal_signer_authorized {
            keys.push(summary.seal_public_key);
        }
        keys.extend(self.council_public_key);
        keys
    }

    /// Distinct council members among every signer of the submitted transaction.
    pub fn approvals(&self, summary: &ProposalSummary, council: &Council) -> Result<usize, PayloadError> {
        let signatures = self.decode_signatures()?;
        let own = self.own_signers(summary);
        Ok(council.count_approvals(own.iter().chain(signatures.iter().map(|s| s.public_key()))))
    }

    /// Verifies a co-signature and records it.
    pub fn add_signature(
        &mut self,
        armored: &str,
        summary: &ProposalSummary,
        council: &Council,
    ) -> Result<SignatureV1, AddSignatureError> {
        let signature = decode_signature(armored)?;
        summary.verify_signature(&signature, council)?;
        let key = *signature.public_key();
        let already = self.own_signers(summary).contains(&key) ||
            self.decode_signatures()?.iter().any(|s| *s.public_key() == key);
        if already {
            return Err(SignatureError::Duplicate(key).into());
        }
        self.signatures
            .push(armor::encode(&Payload::SignatureV1(signature.clone())));
        Ok(signature)
    }

    pub fn fingerprint_hex(&self) -> Result<String, PayloadError> {
        Ok(self.decode_proposal()?.fingerprint().to_hex())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AddSignatureError {
    #[error(transparent)]
    Payload(#[from] PayloadError),
    #[error(transparent)]
    Signature(#[from] SignatureError),
}

/// A proposal this wallet was asked to co-sign.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SigningRecord {
    pub proposal: String,
    pub key_id: KeyId,
    pub request_id: SigningRequestId,
    /// Unix seconds.
    pub expires_at: i64,
    /// The armored signature once walletd has produced it.
    pub signature: Option<String>,
    pub created_at: u64,
}

impl SigningRecord {
    pub fn decode_proposal(&self) -> Result<ProposalV1, PayloadError> {
        decode_proposal(&self.proposal)
    }
}

pub fn decode_proposal(text: &str) -> Result<ProposalV1, PayloadError> {
    match armor::decode(text)? {
        Payload::ProposalV1(proposal) => Ok(proposal),
        other => Err(PayloadError::WrongKind {
            expected: PayloadKind::Proposal.noun(),
            found: other.kind().noun(),
        }),
    }
}

pub fn decode_signature(text: &str) -> Result<SignatureV1, PayloadError> {
    match armor::decode(text)? {
        Payload::SignatureV1(signature) => Ok(signature),
        other => Err(PayloadError::WrongKind {
            expected: PayloadKind::Signature.noun(),
            found: other.kind().noun(),
        }),
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// The per-user directory for one network's records.
pub fn default_store_dir(network: ootle_gov_core::Network) -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("ootle-governance").join(network.to_string()))
}

/// A directory of records for one network.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join("proposals"))?;
        fs::create_dir_all(root.join("signing"))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn save_proposal(&self, record: &ProposalRecord) -> io::Result<()> {
        let name = record.fingerprint_hex().map_err(io::Error::other)?;
        write_json(&self.root.join("proposals").join(format!("{name}.json")), record)
    }

    /// Newest first.
    pub fn proposals(&self) -> io::Result<Vec<ProposalRecord>> {
        let mut records: Vec<ProposalRecord> = read_dir_json(&self.root.join("proposals"))?;
        records.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        Ok(records)
    }

    pub fn save_signing(&self, record: &SigningRecord) -> io::Result<()> {
        let fingerprint = record.decode_proposal().map_err(io::Error::other)?.fingerprint();
        let name = format!("{}-{}", fingerprint.to_hex(), record.request_id);
        write_json(&self.root.join("signing").join(format!("{name}.json")), record)
    }

    /// Newest first.
    pub fn signing_records(&self) -> io::Result<Vec<SigningRecord>> {
        let mut records: Vec<SigningRecord> = read_dir_json(&self.root.join("signing"))?;
        records.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        Ok(records)
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(value).map_err(io::Error::other)?)?;
    fs::rename(tmp, path)
}

fn read_dir_json<T: for<'de> Deserialize<'de>>(dir: &Path) -> io::Result<Vec<T>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "json") {
            let bytes = fs::read(&path)?;
            match serde_json::from_slice(&bytes) {
                Ok(record) => out.push(record),
                Err(e) => return Err(io::Error::other(format!("{}: {e}", path.display()))),
            }
        }
    }
    Ok(out)
}
