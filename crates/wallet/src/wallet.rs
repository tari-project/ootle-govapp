//! The wallet daemon side of the flow.
//!
//! The API key this needs: `signing_requests:create`, `transaction_requests:create`, `transactions:read`,
//! `keys:read`, `accounts:read`, `substates:read` and `settings:read` (each `create` implies `read`). A person
//! approves each signing and transaction request in the walletd UI.

use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hasher},
};

use ootle_gov_core::{
    Network,
    action::{GOVERNANCE_COMPONENT, GovernanceAction},
    armor,
    builder::{ProposalParams, build_unsigned},
    council::GovernanceView,
    error::{PayloadError, PolicyError},
    payload::{Payload, ProposalV1, SignatureV1},
};
use tari_engine_types::substate::SubstateId;
use tari_ootle_transaction::TransactionId;
use tari_ootle_wallet_sdk::models::{
    Account,
    EffectiveStatus,
    KeyId,
    SigningRequestEffectiveStatus,
    TransactionRequestId,
    TransactionStatus,
};
use tari_ootle_walletd_client::{
    WalletDaemonClient,
    error::WalletDaemonClientError,
    types::{
        SigningRequestCreateRequest,
        SigningRequestGetRequest,
        SubstatesGetRequest,
        TransactionDetectInputsRequest,
        TransactionRequestCreateRequest,
        TransactionRequestGetRequest,
        TransactionRequestSubmitRequest,
        TransactionWaitResultRequest,
    },
};
use tari_template_lib_types::{Amount, ComponentAddress, crypto::RistrettoPublicKeyBytes};

use crate::{
    key_ref::{SIGNING_BRANCHES, format_key_id},
    store::{ProposalRecord, SigningRecord, decode_proposal, now_secs},
};

#[derive(Debug, thiserror::Error)]
pub enum WalletError {
    #[error("Wallet daemon: {0}")]
    Client(#[from] WalletDaemonClientError),
    #[error("The wallet daemon reports network byte {0:#04x}, which this tool does not know")]
    UnknownNetwork(u8),
    #[error("The wallet daemon could not read the governance component from the network")]
    GovernanceUnavailable,
    #[error("The governance component could not be read: {0}")]
    GovernanceUnreadable(String),
    #[error("Account {0} is not in this wallet")]
    UnknownAccount(ComponentAddress),
    #[error("Account {0} is not on chain yet; fund it before it can pay a fee")]
    AccountNotOnChain(ComponentAddress),
    #[error("Account {0} is watch-only in this wallet; it cannot seal or pay")]
    AccountNotOwned(ComponentAddress),
    #[error("Key {0} is not an account or transactions key in this wallet")]
    UnknownKey(String),
    #[error(
        "The wallet daemon would sign a different message than this proposal's; refusing (walletd {walletd}, proposal \
         {ours})"
    )]
    MessageMismatch { walletd: String, ours: String },
    #[error("The signature the wallet daemon returned does not verify")]
    BadSignature,
    #[error(transparent)]
    Policy(#[from] PolicyError),
    #[error(transparent)]
    Payload(#[from] PayloadError),
}

/// A key this wallet can sign governance transactions with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletKey {
    pub key_id: KeyId,
    pub public_key: RistrettoPublicKeyBytes,
}

impl WalletKey {
    pub fn label(&self) -> String {
        format_key_id(&self.key_id)
    }
}

#[derive(Debug, Clone)]
pub struct WalletInfo {
    pub network: Network,
    pub version: String,
    pub current_epoch: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ProposeRequest {
    pub action: GovernanceAction,
    /// Pays the fee; its owner key seals.
    pub fee_account: ComponentAddress,
    pub max_fee: Amount,
    /// The proposer's council key, if they are on the council and it is not the fee account's owner key.
    pub council_key: Option<WalletKey>,
    pub memo: String,
}

#[derive(Debug, Clone)]
pub enum SignatureStatus {
    /// Waiting for a person to approve it in the walletd UI.
    Pending,
    Signed(SignatureV1),
    Rejected,
    Expired,
}

#[derive(Debug, Clone)]
pub enum SubmissionStatus {
    /// Waiting for a person to approve the final transaction in the walletd UI.
    AwaitingApproval,
    /// Approved and ready to broadcast.
    Approved,
    Submitting,
    Submitted(TransactionId),
    Rejected,
    Expired,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub status: TransactionStatus,
    pub reject_reason: Option<String>,
    pub final_fee: u64,
    pub timed_out: bool,
}

impl Outcome {
    pub fn describe(&self) -> String {
        if self.timed_out {
            return "Not finalised yet".to_string();
        }
        match &self.reject_reason {
            Some(reason) => format!("{:?}: {reason}", self.status),
            None => format!("{:?} (fee {} µT)", self.status, self.final_fee),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Wallet {
    client: WalletDaemonClient,
    network: Network,
    version: String,
}

impl Wallet {
    /// Connects and learns the daemon's network. [`Wallet::info`] is the first call that uses the API key.
    pub async fn connect(url: &str, api_key: &str) -> Result<Self, WalletError> {
        let mut client = WalletDaemonClient::connect(url, Some(api_key.to_string().into()))?;
        let info = client.get_wallet_info().await?;
        let network =
            Network::try_from(info.network_byte).map_err(|_| WalletError::UnknownNetwork(info.network_byte))?;
        Ok(Self {
            client,
            network,
            version: info.version,
        })
    }

    pub fn network(&self) -> Network {
        self.network
    }

    pub fn endpoint(&self) -> String {
        self.client.endpoint().to_string()
    }

    pub async fn info(&self) -> Result<WalletInfo, WalletError> {
        let settings = self.client().get_settings().await?;
        Ok(WalletInfo {
            network: self.network,
            version: self.version.clone(),
            current_epoch: settings.current_epoch.map(|e| e.as_u64()),
        })
    }

    pub async fn governance(&self) -> Result<GovernanceView, WalletError> {
        let response = self
            .client()
            .get_substate(SubstatesGetRequest {
                substate_id: SubstateId::Component(GOVERNANCE_COMPONENT),
            })
            .await?;
        let substate = response
            .substate_from_remote
            .ok_or(WalletError::GovernanceUnavailable)?;
        GovernanceView::from_substate_value(substate.substate_value()).map_err(WalletError::GovernanceUnreadable)
    }

    /// Account and transactions keys: the branches walletd will co-sign with.
    pub async fn signing_keys(&self) -> Result<Vec<WalletKey>, WalletError> {
        let mut keys = Vec::new();
        for branch in SIGNING_BRANCHES {
            let response = self.client().list_keys(branch).await?;
            keys.extend(
                response
                    .keys
                    .into_iter()
                    .map(|(key_id, public_key, _)| WalletKey { key_id, public_key }),
            );
        }
        Ok(keys)
    }

    pub async fn find_key(&self, key_id: &KeyId) -> Result<WalletKey, WalletError> {
        self.signing_keys()
            .await?
            .into_iter()
            .find(|k| k.key_id == *key_id)
            .ok_or_else(|| WalletError::UnknownKey(format_key_id(key_id)))
    }

    pub async fn accounts(&self) -> Result<Vec<Account>, WalletError> {
        let response = self.client().list_accounts(0, 100).await?;
        Ok(response.accounts.into_iter().map(|a| a.account).collect())
    }

    /// Builds and freezes a proposal. The fee account's owner key will seal; the proposer's council key, if
    /// given and different, is added by walletd at submission.
    pub async fn propose(&self, request: ProposeRequest) -> Result<ProposalRecord, WalletError> {
        let account = self
            .accounts()
            .await?
            .into_iter()
            .find(|a| a.component_address == request.fee_account)
            .ok_or(WalletError::UnknownAccount(request.fee_account))?;
        if !account.is_confirmed_on_chain {
            return Err(WalletError::AccountNotOnChain(request.fee_account));
        }
        let seal_key_id = account
            .owner_key_id
            .ok_or(WalletError::AccountNotOwned(request.fee_account))?;
        let seal_public_key = account.owner_public_key;

        let unsigned = build_unsigned(&ProposalParams {
            network: self.network,
            action: request.action,
            fee_account: request.fee_account,
            max_fee: request.max_fee,
            seal_signer_authorized: true,
            nonce: random_nonce(),
        })?;
        let unsigned = self
            .client()
            .detect_transaction_inputs(TransactionDetectInputsRequest {
                transaction: unsigned,
                use_unversioned: true,
            })
            .await?
            .transaction;

        let proposal = ProposalV1 {
            unsigned,
            seal_public_key,
            memo: request.memo,
        };
        proposal.inspect()?;

        let council_key = request.council_key.filter(|k| k.public_key != seal_public_key);
        Ok(ProposalRecord {
            proposal: armor::encode(&Payload::ProposalV1(proposal)),
            seal_key_id,
            council_key_id: council_key.as_ref().map(|k| k.key_id),
            council_public_key: council_key.map(|k| k.public_key),
            signatures: Vec::new(),
            transaction_request_id: None,
            transaction_id: None,
            outcome: None,
            created_at: now_secs(),
        })
    }

    /// Asks walletd to co-sign. A person approves it in the walletd UI; poll [`Wallet::signature_status`].
    pub async fn request_signature(
        &self,
        armored_proposal: &str,
        key_id: KeyId,
        ttl_secs: Option<u64>,
    ) -> Result<SigningRecord, WalletError> {
        let proposal = decode_proposal(armored_proposal)?;
        let ours = proposal.message_hash();
        let response = self
            .client()
            .create_signing_request(SigningRequestCreateRequest {
                transaction: proposal.unsigned.clone(),
                seal_public_key: proposal.seal_public_key,
                key_id,
                memo: proposal.memo.clone(),
                ttl_secs,
            })
            .await?;
        check_message(&response.message_hash, &ours)?;
        Ok(SigningRecord {
            proposal: armored_proposal.to_string(),
            key_id,
            request_id: response.request_id,
            expires_at: response.expires_at,
            signature: None,
            created_at: now_secs(),
        })
    }

    pub async fn signature_status(&self, record: &SigningRecord) -> Result<SignatureStatus, WalletError> {
        let ours = record.decode_proposal()?.message_hash();
        let info = self
            .client()
            .get_signing_request(SigningRequestGetRequest {
                request_id: record.request_id,
            })
            .await?
            .request;
        check_message(&info.message_hash, &ours)?;
        Ok(match info.status {
            SigningRequestEffectiveStatus::Pending => SignatureStatus::Pending,
            SigningRequestEffectiveStatus::Rejected => SignatureStatus::Rejected,
            SigningRequestEffectiveStatus::Expired => SignatureStatus::Expired,
            SigningRequestEffectiveStatus::Signed => {
                let signature = info.signature.ok_or(WalletError::BadSignature)?;
                if !signature.verify_message(ours) {
                    return Err(WalletError::BadSignature);
                }
                SignatureStatus::Signed(SignatureV1 {
                    message_hash: ours,
                    signature,
                })
            },
        })
    }

    /// Hands the assembled transaction to walletd as a transaction request. A person approves it in the walletd
    /// UI, then [`Wallet::broadcast`] submits it.
    pub async fn submit(&self, record: &ProposalRecord) -> Result<TransactionRequestId, WalletError> {
        let proposal = record.decode_proposal()?;
        let signatures = record.decode_signatures()?;
        let response = self
            .client()
            .create_transaction_request(TransactionRequestCreateRequest {
                transaction: proposal.unsigned,
                seal_signer: record.seal_key_id,
                other_signers: record.council_key_id.into_iter().collect(),
                signatures: signatures.into_iter().map(|s| s.signature).collect(),
                lock_ids: Vec::new(),
                ttl_secs: None,
            })
            .await?;
        Ok(response.request_id)
    }

    pub async fn submission_status(&self, request_id: TransactionRequestId) -> Result<SubmissionStatus, WalletError> {
        let info = self
            .client()
            .get_transaction_request(TransactionRequestGetRequest { request_id })
            .await?
            .request;
        Ok(match (info.status, info.transaction_id) {
            (EffectiveStatus::Pending, _) => SubmissionStatus::AwaitingApproval,
            (EffectiveStatus::Approved, _) => SubmissionStatus::Approved,
            (EffectiveStatus::Submitting, _) => SubmissionStatus::Submitting,
            (EffectiveStatus::Submitted, Some(id)) => SubmissionStatus::Submitted(id),
            (EffectiveStatus::Submitted, None) => SubmissionStatus::Submitting,
            (EffectiveStatus::Rejected, _) => SubmissionStatus::Rejected,
            (EffectiveStatus::Expired, _) => SubmissionStatus::Expired,
        })
    }

    pub async fn broadcast(&self, request_id: TransactionRequestId) -> Result<TransactionId, WalletError> {
        let response = self
            .client()
            .submit_transaction_request(TransactionRequestSubmitRequest { request_id })
            .await?;
        Ok(response.transaction_id)
    }

    pub async fn wait_result(&self, transaction_id: TransactionId, timeout_secs: u64) -> Result<Outcome, WalletError> {
        let response = self
            .client()
            .wait_transaction_result(TransactionWaitResultRequest {
                transaction_id,
                timeout_secs: Some(timeout_secs),
            })
            .await?;
        Ok(Outcome {
            status: response.status,
            reject_reason: response
                .result
                .as_ref()
                .and_then(|r| r.any_reject())
                .map(|r| r.to_string()),
            final_fee: response.final_fee,
            timed_out: response.timed_out,
        })
    }

    fn client(&self) -> WalletDaemonClient {
        self.client.clone()
    }
}

fn check_message(walletd: &[u8; 64], ours: &[u8; 64]) -> Result<(), WalletError> {
    if walletd != ours {
        return Err(WalletError::MessageMismatch {
            walletd: ootle_gov_core::Fingerprint::from_message_hash(walletd).to_string(),
            ours: ootle_gov_core::Fingerprint::from_message_hash(ours).to_string(),
        });
    }
    Ok(())
}

/// Distinguishes otherwise identical proposals. Not a secret, so the OS-seeded hasher is enough.
fn random_nonce() -> u64 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(now_secs());
    hasher.finish()
}
