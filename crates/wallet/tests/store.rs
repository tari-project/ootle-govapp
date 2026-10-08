use ootle_byte_type::ToByteType;
use ootle_gov_core::{
    Network,
    action::{GOVERNANCE_COMPONENT, GovernanceAction},
    armor,
    builder::{ProposalParams, build_unsigned},
    council::Council,
    error::SignatureError,
    payload::{Payload, ProposalV1, SignatureV1},
};
use ootle_gov_wallet::{
    key_ref::parse_key_id,
    store::{AddSignatureError, ProposalRecord, Store},
};
use tari_crypto::{
    keys::{PublicKey, SecretKey},
    ristretto::{RistrettoPublicKey, RistrettoSecretKey},
};
use tari_ootle_common_types::InputDeclaration;
use tari_ootle_transaction::TransactionSignature;
use tari_template_lib_types::{Amount, ComponentAddress, crypto::RistrettoPublicKeyBytes};

fn key(seed: u8) -> (RistrettoSecretKey, RistrettoPublicKeyBytes) {
    let secret = RistrettoSecretKey::from_uniform_bytes(&[seed; 64]).unwrap();
    let public = RistrettoPublicKey::from_secret_key(&secret).to_byte_type();
    (secret, public)
}

fn record(seal: RistrettoPublicKeyBytes) -> ProposalRecord {
    let fee_account: ComponentAddress = format!("component_{}", "ab".repeat(32)).parse().unwrap();
    let unsigned = build_unsigned(&ProposalParams {
        network: Network::LocalNet,
        action: GovernanceAction::set_burn_rate(700, 100).unwrap(),
        fee_account,
        max_fee: Amount::new(10_000_000),
        seal_signer_authorized: true,
        nonce: 1,
    })
    .unwrap()
    .with_inputs([
        InputDeclaration::write(GOVERNANCE_COMPONENT),
        InputDeclaration::write(fee_account),
    ]);
    ProposalRecord {
        proposal: armor::encode(&Payload::ProposalV1(ProposalV1 {
            unsigned,
            seal_public_key: seal,
            memo: String::new(),
        })),
        seal_key_id: parse_key_id("account/0").unwrap(),
        council_key_id: None,
        council_public_key: None,
        signatures: Vec::new(),
        transaction_request_id: None,
        transaction_id: None,
        outcome: None,
        created_at: 1,
    }
}

fn co_sign(record: &ProposalRecord, secret: &RistrettoSecretKey) -> String {
    let proposal = record.decode_proposal().unwrap();
    armor::encode(&Payload::SignatureV1(SignatureV1 {
        message_hash: proposal.message_hash(),
        signature: TransactionSignature::sign(secret, &proposal.seal_public_key, &proposal.unsigned),
    }))
}

#[test]
fn collected_signatures_count_toward_the_threshold_once_each() {
    let (_, proposer) = key(1);
    let (alice_secret, alice) = key(2);
    let (bob_secret, bob) = key(3);
    let council = Council {
        threshold: 3,
        members: vec![proposer, alice, bob],
    };
    let mut record = record(proposer);
    let summary = record.decode_proposal().unwrap().inspect().unwrap();

    assert_eq!(record.approvals(&summary, &council).unwrap(), 1);

    record
        .add_signature(&co_sign(&record, &alice_secret), &summary, &council)
        .unwrap();
    assert_eq!(record.approvals(&summary, &council).unwrap(), 2);

    let again = record.add_signature(&co_sign(&record, &alice_secret), &summary, &council);
    assert!(matches!(
        again,
        Err(AddSignatureError::Signature(SignatureError::Duplicate(_)))
    ));

    record
        .add_signature(&co_sign(&record, &bob_secret), &summary, &council)
        .unwrap();
    assert_eq!(record.approvals(&summary, &council).unwrap(), 3);
}

#[test]
fn records_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (_, proposer) = key(1);
    let (alice_secret, alice) = key(2);
    let council = Council {
        threshold: 2,
        members: vec![proposer, alice],
    };
    let mut record = record(proposer);
    let summary = record.decode_proposal().unwrap().inspect().unwrap();
    record
        .add_signature(&co_sign(&record, &alice_secret), &summary, &council)
        .unwrap();

    Store::open(dir.path()).unwrap().save_proposal(&record).unwrap();
    let loaded = Store::open(dir.path()).unwrap().proposals().unwrap();

    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].proposal, record.proposal);
    assert_eq!(loaded[0].approvals(&summary, &council).unwrap(), 2);
}
