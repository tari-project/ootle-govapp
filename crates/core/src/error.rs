use tari_template_lib_types::crypto::RistrettoPublicKeyBytes;

/// Why this tool refuses to sign or assemble a proposal.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum PolicyError {
    #[error("The transaction names network byte {0:#04x}, which is not a known network")]
    UnknownNetwork(u8),
    #[error("The proposal is for {proposal}, but this wallet is on {wallet}")]
    NetworkMismatch { proposal: String, wallet: String },
    #[error("The transaction is marked as a dry run and can never be submitted")]
    DryRun,
    #[error("The transaction carries blobs; a governance call has no use for them")]
    HasBlobs,
    #[error(
        "The transaction must pay its fee with exactly one `pay_fee` call on an account, found {0} fee instructions"
    )]
    FeeInstructionCount(usize),
    #[error("The fee instruction is not a `pay_fee` call on an account: {0}")]
    UnexpectedFeeInstruction(String),
    #[error("The transaction must make exactly one governance call, found {0} instructions")]
    InstructionCount(usize),
    #[error("Unexpected instruction: {0}")]
    UnexpectedInstruction(String),
    #[error("The call is not to the burn rate governance component")]
    NotGovernanceComponent,
    #[error("`{0}` is not a governance call this tool can sign")]
    UnsupportedMethod(String),
    #[error("Bad arguments to `{method}`: {reason}")]
    BadArguments { method: &'static str, reason: String },
    #[error("A burn rate of {rate_bps} bps is above the 10000 bps maximum")]
    RateAboveMaximum { rate_bps: u16 },
    #[error(
        "Input {0} pins a substate version; signatures take days to collect, and a pinned version goes stale before \
         then"
    )]
    VersionedInput(String),
    #[error(
        "The transaction stays valid until epoch {max_epoch}, but the change activates at epoch {activation_epoch} \
         and must execute by epoch {last_executable}"
    )]
    OutlivesActivationLead {
        max_epoch: u64,
        activation_epoch: u64,
        last_executable: u64,
    },
    #[error("The activation epoch {0} is too early to leave the required lead time")]
    ActivationTooEarly(u64),
    #[error("The transaction expired at epoch {max_epoch}; the network is at epoch {current_epoch}")]
    Expired { max_epoch: u64, current_epoch: u64 },
    #[error("The memo is {0} characters; the limit is {limit}", limit = crate::payload::MAX_MEMO_CHARS)]
    MemoTooLong(usize),
    #[error("The burn rate governance component has no council, so no transaction can authorise this call")]
    NoCouncil,
    #[error("The council has retired, so no transaction can authorise this call")]
    CouncilRetired,
    #[error("The component's owner rule is not a council this tool can read: {0}")]
    UnrecognisedCouncil(String),
    #[error("Key {0} is not on the council")]
    NotCouncilMember(RistrettoPublicKeyBytes),
}

/// A pasted payload that could not be read.
#[derive(Debug, thiserror::Error)]
pub enum PayloadError {
    #[error("No `-----BEGIN OOTLE GOVERNANCE …-----` block found")]
    NoArmor,
    #[error("The `-----END {0}-----` line is missing; the paste may be truncated")]
    Truncated(String),
    #[error("Expected a {expected}, but this is a {found}")]
    WrongKind {
        expected: &'static str,
        found: &'static str,
    },
    #[error("The payload is {0} bytes; the limit is {limit}", limit = crate::armor::MAX_PAYLOAD_BYTES)]
    TooLarge(usize),
    #[error("The payload body is not valid base64: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("The payload body could not be decoded: {0}")]
    Decode(String),
    #[error("Header `{name}` says `{header}` but the payload says `{actual}`; it may have been edited")]
    HeaderMismatch {
        name: &'static str,
        header: String,
        actual: String,
    },
    #[error(transparent)]
    Policy(#[from] PolicyError),
}

/// A returned co-signature that cannot be counted.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum SignatureError {
    #[error("This signature is for a different proposal (fingerprint {0})")]
    WrongProposal(String),
    #[error("The signature from {0} does not verify")]
    Invalid(RistrettoPublicKeyBytes),
    #[error("{0} is not on the council")]
    NotCouncilMember(RistrettoPublicKeyBytes),
    #[error("{0} has already signed")]
    Duplicate(RistrettoPublicKeyBytes),
}
