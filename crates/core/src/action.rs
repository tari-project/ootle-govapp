//! The governance calls this tool will build and co-sign.
//!
//! The set is closed on purpose: a member's app signs only a transaction whose main instruction decodes into
//! one of these, so it cannot be used as a general-purpose signer.

use std::fmt::{self, Display};

use tari_ootle_transaction::{Instruction, args, args::InstructionArg, builder::named_args::NamedArg};
use tari_template_lib_types::{
    ComponentAddress,
    constants::BURN_RATE_GOVERNANCE_COMPONENT_ADDRESS,
    governance::{MAX_EXHAUST_BURN_RATE_BPS, MIN_BURN_RATE_ACTIVATION_LEAD_EPOCHS},
};

use crate::{ComponentReference, error::PolicyError};

pub const GOVERNANCE_COMPONENT: ComponentAddress = BURN_RATE_GOVERNANCE_COMPONENT_ADDRESS;

/// The fewest epochs between the epoch a change executes in and the epoch it activates.
pub const ACTIVATION_LEAD_EPOCHS: u64 = MIN_BURN_RATE_ACTIVATION_LEAD_EPOCHS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovernanceAction {
    SetBurnRate { rate_bps: u16, activation_epoch: u64 },
}

impl GovernanceAction {
    /// A burn rate change, refused if the rate is above what the component accepts.
    pub fn set_burn_rate(rate_bps: u16, activation_epoch: u64) -> Result<Self, PolicyError> {
        if rate_bps > MAX_EXHAUST_BURN_RATE_BPS {
            return Err(PolicyError::RateAboveMaximum { rate_bps });
        }
        Ok(Self::SetBurnRate {
            rate_bps,
            activation_epoch,
        })
    }

    pub fn activation_epoch(&self) -> u64 {
        match *self {
            Self::SetBurnRate { activation_epoch, .. } => activation_epoch,
        }
    }

    pub fn method(&self) -> &'static str {
        match self {
            Self::SetBurnRate { .. } => "set_burn_rate",
        }
    }

    pub fn args(&self) -> Vec<NamedArg> {
        match *self {
            Self::SetBurnRate {
                rate_bps,
                activation_epoch,
            } => args![rate_bps, activation_epoch],
        }
    }

    /// The latest epoch the transaction can execute in and still be accepted by the component.
    ///
    /// The component requires the activation to be at least [`MIN_BURN_RATE_ACTIVATION_LEAD_EPOCHS`] after the
    /// epoch the transaction executes in, so a proposal's `max_epoch` is set to this: the transaction stops
    /// being valid exactly when it could no longer succeed.
    pub fn last_executable_epoch(&self) -> Option<u64> {
        match *self {
            Self::SetBurnRate { activation_epoch, .. } => {
                activation_epoch.checked_sub(MIN_BURN_RATE_ACTIVATION_LEAD_EPOCHS)
            },
        }
    }

    /// Decodes a main instruction, accepting only a call this tool knows how to describe.
    pub fn from_instruction(instruction: &Instruction) -> Result<Self, PolicyError> {
        let Instruction::CallMethod { call, method, args } = instruction else {
            return Err(PolicyError::UnexpectedInstruction(instruction.to_string()));
        };
        if *call != ComponentReference::Address(GOVERNANCE_COMPONENT) {
            return Err(PolicyError::NotGovernanceComponent);
        }
        match &**method {
            "set_burn_rate" => {
                let [rate_bps, activation_epoch] = args.as_slice() else {
                    return Err(PolicyError::BadArguments {
                        method: "set_burn_rate",
                        reason: format!("expected 2 arguments, got {}", args.len()),
                    });
                };
                let rate_bps = decode_literal::<u16>("set_burn_rate", rate_bps)?;
                let activation_epoch = decode_literal::<u64>("set_burn_rate", activation_epoch)?;
                Self::set_burn_rate(rate_bps, activation_epoch)
            },
            other => Err(PolicyError::UnsupportedMethod(other.to_string())),
        }
    }
}

impl Display for GovernanceAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SetBurnRate {
                rate_bps,
                activation_epoch,
            } => write!(
                f,
                "Set burn rate to {} ({rate_bps} bps) from epoch {activation_epoch}",
                format_bps(*rate_bps)
            ),
        }
    }
}

/// Renders basis points as a percentage with two decimals, e.g. `700` → `7.00%`.
pub fn format_bps(bps: u16) -> String {
    format!("{}.{:02}%", bps / 100, bps % 100)
}

pub(crate) fn decode_literal<T>(method: &'static str, arg: &InstructionArg) -> Result<T, PolicyError>
where T: for<'a> tari_bor::Decode<'a, ()> {
    let InstructionArg::Literal(bytes) = arg else {
        return Err(PolicyError::BadArguments {
            method,
            reason: "argument is not a literal value".to_string(),
        });
    };
    tari_bor::decode(bytes).map_err(|e| PolicyError::BadArguments {
        method,
        reason: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_basis_points_as_a_percentage() {
        assert_eq!(format_bps(0), "0.00%");
        assert_eq!(format_bps(5), "0.05%");
        assert_eq!(format_bps(700), "7.00%");
        assert_eq!(format_bps(10_000), "100.00%");
    }

    #[test]
    fn a_rate_above_the_maximum_is_refused() {
        assert!(GovernanceAction::set_burn_rate(MAX_EXHAUST_BURN_RATE_BPS, 10).is_ok());
        assert!(matches!(
            GovernanceAction::set_burn_rate(MAX_EXHAUST_BURN_RATE_BPS + 1, 10),
            Err(PolicyError::RateAboveMaximum { .. })
        ));
    }

    #[test]
    fn the_last_executable_epoch_leaves_the_activation_lead() {
        let action = GovernanceAction::set_burn_rate(700, 100).unwrap();
        assert_eq!(
            action.last_executable_epoch(),
            Some(100 - MIN_BURN_RATE_ACTIVATION_LEAD_EPOCHS)
        );
        assert_eq!(
            GovernanceAction::set_burn_rate(700, 1).unwrap().last_executable_epoch(),
            None
        );
    }
}
