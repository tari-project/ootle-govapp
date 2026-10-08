//! Building a proposal's transaction.

use ootle_network::Network;
use tari_ootle_common_types::Epoch;
use tari_ootle_transaction::{Transaction, UnsignedTransaction};
use tari_template_lib_types::{Amount, ComponentAddress};

use crate::{
    action::{GOVERNANCE_COMPONENT, GovernanceAction},
    error::PolicyError,
};

#[derive(Debug, Clone)]
pub struct ProposalParams {
    pub network: Network,
    pub action: GovernanceAction,
    /// The account that pays the fee. Its owner key must sign: in practice it is the seal signer.
    pub fee_account: ComponentAddress,
    pub max_fee: Amount,
    /// Whether the seal signer's key counts as a signer. It must, for the seal signer to authorise the fee
    /// payment from its own account.
    pub seal_signer_authorized: bool,
    /// Distinguishes this proposal from an otherwise identical one.
    pub nonce: u64,
}

/// Builds the transaction without inputs. Resolve them (unversioned) before anyone signs.
///
/// `max_epoch` is the last epoch the call can still succeed in, so the transaction cannot outlive its use.
pub fn build_unsigned(params: &ProposalParams) -> Result<UnsignedTransaction, PolicyError> {
    let max_epoch = params
        .action
        .last_executable_epoch()
        .ok_or(PolicyError::ActivationTooEarly(params.action.activation_epoch()))?;
    Ok(Transaction::builder(params.network, Epoch(max_epoch))
        .with_nonce(params.nonce)
        .with_seal_signer_authorized(params.seal_signer_authorized)
        .pay_fee_from_component(params.fee_account, params.max_fee)
        .call_method(GOVERNANCE_COMPONENT, params.action.method(), params.action.args())
        .build_unsigned())
}
