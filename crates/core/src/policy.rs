//! What a proposal says, and whether this tool will sign or assemble it.

use ootle_network::Network;
use tari_ootle_transaction::{Instruction, UnsignedTransaction};
use tari_template_lib_types::{Amount, ComponentAddress, crypto::RistrettoPublicKeyBytes};

use crate::{
    ComponentReference,
    Fingerprint,
    action::{GOVERNANCE_COMPONENT, GovernanceAction, decode_literal},
    council::Council,
    error::{PolicyError, SignatureError},
    payload::{MAX_MEMO_CHARS, ProposalV1, SignatureV1},
};

/// The facts of a proposal, all derived from the transaction.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposalSummary {
    pub network: Network,
    pub action: GovernanceAction,
    pub fee_account: ComponentAddress,
    pub max_fee: Amount,
    pub min_epoch: Option<u64>,
    pub max_epoch: u64,
    pub seal_public_key: RistrettoPublicKeyBytes,
    /// Whether the seal signer's key counts as a signer (and so toward the council threshold).
    pub seal_signer_authorized: bool,
    pub message_hash: [u8; 64],
    pub fingerprint: Fingerprint,
}

impl ProposalV1 {
    /// Checks the shape of the transaction and summarises it. Needs no network access.
    pub fn inspect(&self) -> Result<ProposalSummary, PolicyError> {
        let UnsignedTransaction::V1(tx) = &self.unsigned;

        let network = Network::try_from(tx.network).map_err(|_| PolicyError::UnknownNetwork(tx.network))?;
        if tx.dry_run {
            return Err(PolicyError::DryRun);
        }
        if !tx.blobs.is_empty() {
            return Err(PolicyError::HasBlobs);
        }
        if self.memo.chars().count() > MAX_MEMO_CHARS {
            return Err(PolicyError::MemoTooLong(self.memo.chars().count()));
        }

        let [fee_instruction] = tx.fee_instructions.as_slice() else {
            return Err(PolicyError::FeeInstructionCount(tx.fee_instructions.len()));
        };
        let (fee_account, max_fee) = decode_pay_fee(fee_instruction)?;

        let [instruction] = tx.instructions.as_slice() else {
            return Err(PolicyError::InstructionCount(tx.instructions.len()));
        };
        let action = GovernanceAction::from_instruction(instruction)?;

        if let Some(input) = tx.inputs.iter().find(|input| input.version.is_some()) {
            return Err(PolicyError::VersionedInput(input.substate_id.to_string()));
        }

        let last_executable = action
            .last_executable_epoch()
            .ok_or(PolicyError::ActivationTooEarly(action.activation_epoch()))?;
        let max_epoch = tx.max_epoch.as_u64();
        if max_epoch > last_executable {
            return Err(PolicyError::OutlivesActivationLead {
                max_epoch,
                activation_epoch: action.activation_epoch(),
                last_executable,
            });
        }

        let message_hash = self.message_hash();
        Ok(ProposalSummary {
            network,
            action,
            fee_account,
            max_fee,
            min_epoch: tx.min_epoch.map(|e| e.as_u64()),
            max_epoch,
            seal_public_key: self.seal_public_key,
            seal_signer_authorized: tx.is_seal_signer_authorized,
            message_hash,
            fingerprint: Fingerprint::from_message_hash(&message_hash),
        })
    }
}

impl ProposalSummary {
    /// Checks the proposal against the live network: the wallet's network, the current epoch and the council.
    pub fn assess(&self, context: &ChainContext<'_>) -> Result<Assessment, PolicyError> {
        if self.network != context.network {
            return Err(PolicyError::NetworkMismatch {
                proposal: self.network.to_string(),
                wallet: context.network.to_string(),
            });
        }
        if let Some(current_epoch) = context.current_epoch &&
            current_epoch > self.max_epoch
        {
            return Err(PolicyError::Expired {
                max_epoch: self.max_epoch,
                current_epoch,
            });
        }
        let council = context.council.clone()?;
        Ok(Assessment {
            epochs_remaining: context.current_epoch.map(|current| self.max_epoch - current),
            threshold: council.threshold,
            council_size: council.members.len(),
            seal_signer_is_member: self.seal_signer_authorized && council.is_member(&self.seal_public_key),
        })
    }

    /// Verifies a returned co-signature against this proposal and the council.
    pub fn verify_signature(&self, signature: &SignatureV1, council: &Council) -> Result<(), SignatureError> {
        if signature.message_hash != self.message_hash {
            return Err(SignatureError::WrongProposal(signature.fingerprint().to_string()));
        }
        let key = *signature.public_key();
        if !signature.signature.verify_message(self.message_hash) {
            return Err(SignatureError::Invalid(key));
        }
        if !council.is_member(&key) {
            return Err(SignatureError::NotCouncilMember(key));
        }
        Ok(())
    }
}

/// What the live network says about a proposal.
pub struct ChainContext<'a> {
    pub network: Network,
    /// `None` when the wallet cannot currently reach the network.
    pub current_epoch: Option<u64>,
    pub council: Result<&'a Council, PolicyError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assessment {
    pub epochs_remaining: Option<u64>,
    pub threshold: u16,
    pub council_size: usize,
    /// Whether the seal signature itself is a council approval.
    pub seal_signer_is_member: bool,
}

fn decode_pay_fee(instruction: &Instruction) -> Result<(ComponentAddress, Amount), PolicyError> {
    let unexpected = || PolicyError::UnexpectedFeeInstruction(instruction.to_string());
    let Instruction::CallMethod { call, method, args } = instruction else {
        return Err(unexpected());
    };
    let ComponentReference::Address(account) = call else {
        return Err(unexpected());
    };
    if &**method != "pay_fee" || *account == GOVERNANCE_COMPONENT {
        return Err(unexpected());
    }
    let [max_fee] = args.as_slice() else {
        return Err(unexpected());
    };
    let max_fee = decode_literal::<Amount>("pay_fee", max_fee)?;
    Ok((*account, max_fee))
}
