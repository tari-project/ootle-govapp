//! Reading the council from the burn rate governance component.
//!
//! The council is the component's owner rule: an m-of-n over the public identity badges of its members, which
//! every signer of a transaction contributes.

use tari_engine_types::substate::SubstateValue;
use tari_template_lib_types::{
    SubstateOwnerRule,
    access_rules::{AccessRule, RequireRule, RestrictedAccessRule, RuleRequirement},
    crypto::RistrettoPublicKeyBytes,
    governance::BurnRateGovernanceState,
};

use crate::error::PolicyError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Council {
    pub threshold: u16,
    /// Distinct members, in on-chain order.
    pub members: Vec<RistrettoPublicKeyBytes>,
}

impl Council {
    pub fn is_member(&self, key: &RistrettoPublicKeyBytes) -> bool {
        self.members.contains(key)
    }

    /// How many distinct members are among `signers`.
    pub fn count_approvals<'a, I>(&self, signers: I) -> usize
    where I: IntoIterator<Item = &'a RistrettoPublicKeyBytes> {
        let mut seen = Vec::new();
        for signer in signers {
            if self.is_member(signer) && !seen.contains(&signer) {
                seen.push(signer);
            }
        }
        seen.len()
    }
}

/// Who, if anyone, can authorise a governance call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CouncilState {
    Seated(Council),
    /// The owner rule admits nobody: the network never seated a council, or the council retired.
    None,
    /// An owner rule other than an m-of-n over public keys.
    Unrecognised(String),
}

impl CouncilState {
    pub fn from_owner_rule(rule: &SubstateOwnerRule) -> Self {
        match rule {
            SubstateOwnerRule::None => Self::None,
            SubstateOwnerRule::ByAccessRule(AccessRule::Restricted(RestrictedAccessRule::Require(
                RequireRule::MOfN(threshold, requirements),
            ))) => {
                let mut members = Vec::with_capacity(requirements.len());
                for requirement in requirements.iter() {
                    let Some(key) = public_key_of(requirement) else {
                        return Self::Unrecognised(format!("m-of-n requirement {requirement:?} is not a public key"));
                    };
                    if !members.contains(&key) {
                        members.push(key);
                    }
                }
                Self::Seated(Council {
                    threshold: *threshold,
                    members,
                })
            },
            other => Self::Unrecognised(format!("{other:?}")),
        }
    }

    pub fn seated(&self) -> Result<&Council, PolicyError> {
        match self {
            Self::Seated(council) => Ok(council),
            Self::None => Err(PolicyError::NoCouncil),
            Self::Unrecognised(rule) => Err(PolicyError::UnrecognisedCouncil(rule.clone())),
        }
    }
}

fn public_key_of(requirement: &RuleRequirement) -> Option<RistrettoPublicKeyBytes> {
    match requirement {
        RuleRequirement::NonFungibleAddress(address) => address.to_public_key(),
        _ => None,
    }
}

/// The governance component as read from chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceView {
    pub council: CouncilState,
    pub state: BurnRateGovernanceState,
}

impl GovernanceView {
    pub fn from_substate_value(value: &SubstateValue) -> Result<Self, String> {
        let SubstateValue::Component(component) = value else {
            return Err("the governance substate is not a component".to_string());
        };
        let state: BurnRateGovernanceState =
            tari_bor::from_value(component.state()).map_err(|e| format!("governance state did not decode: {e}"))?;
        let council = if state.retired_from.is_some() {
            CouncilState::None
        } else {
            CouncilState::from_owner_rule(component.owner_rule())
        };
        Ok(Self { council, state })
    }

    /// The council that can authorise a call now, or why there is none.
    pub fn council(&self) -> Result<&Council, PolicyError> {
        if self.state.retired_from.is_some() {
            return Err(PolicyError::CouncilRetired);
        }
        self.council.seated()
    }
}

#[cfg(test)]
mod tests {
    use tari_template_lib_types::governance::council_owner_rule;

    use super::*;

    fn key(i: u8) -> RistrettoPublicKeyBytes {
        RistrettoPublicKeyBytes::from_bytes(&[i; 32]).unwrap()
    }

    #[test]
    fn reads_the_council_from_its_owner_rule() {
        let rule = council_owner_rule(2, &[key(1), key(2), key(3)]);
        let CouncilState::Seated(council) = CouncilState::from_owner_rule(&rule) else {
            panic!("expected a seated council");
        };
        assert_eq!(council.threshold, 2);
        assert_eq!(council.members, vec![key(1), key(2), key(3)]);
    }

    #[test]
    fn no_owner_means_no_council() {
        assert_eq!(
            CouncilState::from_owner_rule(&SubstateOwnerRule::None),
            CouncilState::None
        );
    }

    #[test]
    fn approvals_count_distinct_members_only() {
        let council = Council {
            threshold: 2,
            members: vec![key(1), key(2), key(3)],
        };
        assert_eq!(council.count_approvals(&[key(1), key(1), key(9)]), 1);
        assert_eq!(council.count_approvals(&[key(1), key(3)]), 2);
    }
}
