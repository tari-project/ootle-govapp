use ootle_byte_type::ToByteType;
use ootle_gov_core::{
    Network,
    action::{GOVERNANCE_COMPONENT, GovernanceAction},
    armor,
    builder::{ProposalParams, build_unsigned},
    council::Council,
    error::{PayloadError, PolicyError, SignatureError},
    payload::{Payload, ProposalV1, SignatureV1},
    policy::ChainContext,
};
use tari_crypto::{
    keys::{PublicKey, SecretKey},
    ristretto::{RistrettoPublicKey, RistrettoSecretKey},
};
use tari_ootle_common_types::{Epoch, InputDeclaration};
use tari_ootle_transaction::{TransactionSignature, UnsignedTransaction};
use tari_template_lib_types::{Amount, ComponentAddress, crypto::RistrettoPublicKeyBytes};

struct Member {
    secret: RistrettoSecretKey,
    public: RistrettoPublicKeyBytes,
}

fn member(seed: u8) -> Member {
    let secret = RistrettoSecretKey::from_uniform_bytes(&[seed; 64]).unwrap();
    let public = RistrettoPublicKey::from_secret_key(&secret).to_byte_type();
    Member { secret, public }
}

fn fee_account() -> ComponentAddress {
    format!("component_{}", "ab".repeat(32)).parse().unwrap()
}

fn unsigned(rate_bps: u16, activation_epoch: u64) -> UnsignedTransaction {
    let unsigned = build_unsigned(&ProposalParams {
        network: Network::LocalNet,
        action: GovernanceAction::set_burn_rate(rate_bps, activation_epoch).unwrap(),
        fee_account: fee_account(),
        max_fee: Amount::new(10_000_000),
        seal_signer_authorized: true,
        nonce: 42,
    })
    .unwrap();
    unsigned.with_inputs([
        InputDeclaration::write(GOVERNANCE_COMPONENT),
        InputDeclaration::write(fee_account()),
    ])
}

fn proposal(seal: &Member) -> ProposalV1 {
    ProposalV1 {
        unsigned: unsigned(700, 100),
        seal_public_key: seal.public,
        memo: "Raise the burn to 7%".to_string(),
    }
}

fn co_sign(proposal: &ProposalV1, signer: &Member) -> SignatureV1 {
    SignatureV1 {
        message_hash: proposal.message_hash(),
        signature: TransactionSignature::sign(&signer.secret, &proposal.seal_public_key, &proposal.unsigned),
    }
}

fn council(members: &[&Member], threshold: u16) -> Council {
    Council {
        threshold,
        members: members.iter().map(|m| m.public).collect(),
    }
}

#[test]
fn a_built_proposal_summarises_its_transaction() {
    let proposer = member(1);
    let summary = proposal(&proposer).inspect().unwrap();

    assert_eq!(summary.network, Network::LocalNet);
    assert_eq!(summary.action, GovernanceAction::SetBurnRate {
        rate_bps: 700,
        activation_epoch: 100
    });
    assert_eq!(summary.fee_account, fee_account());
    assert_eq!(summary.max_fee, Amount::new(10_000_000));
    assert_eq!(summary.max_epoch, 98);
    assert!(summary.seal_signer_authorized);
}

#[test]
fn a_proposal_survives_the_armor_round_trip() {
    let proposer = member(1);
    let proposal = proposal(&proposer);
    let text = armor::encode(&Payload::ProposalV1(proposal.clone()));

    assert!(text.contains("Action: Set burn rate to 7.00% (700 bps) from epoch 100"));
    assert!(text.contains(&format!("Fingerprint: {}", proposal.fingerprint())));

    let Payload::ProposalV1(decoded) = armor::decode(&text).unwrap() else {
        panic!("expected a proposal");
    };
    assert_eq!(decoded.message_hash(), proposal.message_hash());
    assert_eq!(decoded.memo, proposal.memo);
}

#[test]
fn armor_tolerates_mail_quoting_and_surrounding_text() {
    let proposer = member(1);
    let proposal = proposal(&proposer);
    let text = armor::encode(&Payload::ProposalV1(proposal.clone()));
    let quoted: String = text.lines().map(|line| format!(">   {line}\n")).collect();
    let pasted = format!("Hi all, please sign this:\n\n{quoted}\nThanks\n");

    let Payload::ProposalV1(decoded) = armor::decode(&pasted).unwrap() else {
        panic!("expected a proposal");
    };
    assert_eq!(decoded.message_hash(), proposal.message_hash());
}

#[test]
fn an_edited_header_is_refused() {
    let proposer = member(1);
    let text = armor::encode(&Payload::ProposalV1(proposal(&proposer)));
    let edited = text.replace("Set burn rate to 7.00% (700 bps)", "Set burn rate to 1.00% (100 bps)");

    assert!(matches!(
        armor::decode(&edited),
        Err(PayloadError::HeaderMismatch { name: "Action", .. })
    ));
}

#[test]
fn a_truncated_paste_is_refused() {
    let proposer = member(1);
    let text = armor::encode(&Payload::ProposalV1(proposal(&proposer)));
    let truncated: String = text.lines().take(6).map(|l| format!("{l}\n")).collect();

    assert!(matches!(armor::decode(&truncated), Err(PayloadError::Truncated(_))));
}

#[test]
fn a_signature_block_is_not_accepted_as_a_proposal() {
    let proposer = member(1);
    let proposal = proposal(&proposer);
    let signature = co_sign(&proposal, &member(2));
    let text = armor::encode(&Payload::SignatureV1(signature)).replace("SIGNATURE", "PROPOSAL");

    assert!(matches!(armor::decode(&text), Err(PayloadError::WrongKind { .. })));
}

#[test]
fn a_co_signature_verifies_against_the_proposal() {
    let proposer = member(1);
    let alice = member(2);
    let proposal = proposal(&proposer);
    let summary = proposal.inspect().unwrap();
    let council = council(&[&proposer, &alice], 2);

    let text = armor::encode(&Payload::SignatureV1(co_sign(&proposal, &alice)));
    let Payload::SignatureV1(signature) = armor::decode(&text).unwrap() else {
        panic!("expected a signature");
    };

    summary.verify_signature(&signature, &council).unwrap();
}

#[test]
fn a_signature_over_another_proposal_is_refused() {
    let proposer = member(1);
    let alice = member(2);
    let ours = proposal(&proposer);
    let theirs = ProposalV1 {
        unsigned: unsigned(800, 100),
        ..ours.clone()
    };
    let council = council(&[&alice], 1);

    assert!(matches!(
        ours.inspect()
            .unwrap()
            .verify_signature(&co_sign(&theirs, &alice), &council),
        Err(SignatureError::WrongProposal(_))
    ));
}

#[test]
fn a_signature_bound_to_another_seal_signer_is_invalid() {
    let proposer = member(1);
    let alice = member(2);
    let ours = proposal(&proposer);
    let mut signature = co_sign(
        &ProposalV1 {
            seal_public_key: member(3).public,
            ..ours.clone()
        },
        &alice,
    );
    signature.message_hash = ours.message_hash();

    assert!(matches!(
        ours.inspect()
            .unwrap()
            .verify_signature(&signature, &council(&[&alice], 1)),
        Err(SignatureError::Invalid(_))
    ));
}

#[test]
fn a_non_member_signature_does_not_count() {
    let proposer = member(1);
    let outsider = member(9);
    let proposal = proposal(&proposer);

    assert!(matches!(
        proposal
            .inspect()
            .unwrap()
            .verify_signature(&co_sign(&proposal, &outsider), &council(&[&proposer], 1)),
        Err(SignatureError::NotCouncilMember(_))
    ));
}

#[test]
fn a_versioned_input_is_refused() {
    let proposer = member(1);
    let mut proposal = proposal(&proposer);
    proposal.unsigned = proposal
        .unsigned
        .with_inputs([InputDeclaration::write_versioned(fee_account(), 7u64)]);

    assert!(matches!(proposal.inspect(), Err(PolicyError::VersionedInput(_))));
}

#[test]
fn a_transaction_valid_past_the_activation_lead_is_refused() {
    let proposer = member(1);
    let mut proposal = proposal(&proposer);
    proposal.unsigned.set_max_epoch(Epoch(99));

    assert!(matches!(
        proposal.inspect(),
        Err(PolicyError::OutlivesActivationLead { .. })
    ));
}

#[test]
fn a_dry_run_is_refused() {
    let proposer = member(1);
    let mut proposal = proposal(&proposer);
    proposal.unsigned = proposal.unsigned.with_dry_run(true);

    assert_eq!(proposal.inspect().unwrap_err(), PolicyError::DryRun);
}

#[test]
fn assessment_checks_network_expiry_and_council() {
    let proposer = member(1);
    let alice = member(2);
    let summary = proposal(&proposer).inspect().unwrap();
    let council = council(&[&proposer, &alice], 2);

    let assessment = summary
        .assess(&ChainContext {
            network: Network::LocalNet,
            current_epoch: Some(90),
            council: Ok(&council),
        })
        .unwrap();
    assert_eq!(assessment.epochs_remaining, Some(8));
    assert!(assessment.seal_signer_is_member);

    assert!(matches!(
        summary.assess(&ChainContext {
            network: Network::Esmeralda,
            current_epoch: Some(90),
            council: Ok(&council),
        }),
        Err(PolicyError::NetworkMismatch { .. })
    ));
    assert!(matches!(
        summary.assess(&ChainContext {
            network: Network::LocalNet,
            current_epoch: Some(99),
            council: Ok(&council),
        }),
        Err(PolicyError::Expired { .. })
    ));
    assert_eq!(
        summary
            .assess(&ChainContext {
                network: Network::LocalNet,
                current_epoch: Some(90),
                council: Err(PolicyError::NoCouncil),
            })
            .unwrap_err(),
        PolicyError::NoCouncil
    );
}
