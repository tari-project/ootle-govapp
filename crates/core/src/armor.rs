//! Text armor for pasting payloads through chat and email.
//!
//! ```text
//! -----BEGIN OOTLE GOVERNANCE PROPOSAL-----
//! Network: esmeralda
//! Action: Set burn rate to 7.00% (700 bps) from epoch 41234
//! Fingerprint: 3F9A-1C07-88D2-B4E1
//!
//! <base64 of the CBOR payload, wrapped>
//! -----END OOTLE GOVERNANCE PROPOSAL-----
//! ```
//!
//! The headers are for the humans relaying the block. The decoder recomputes each one it recognises from the
//! body and refuses the block if they disagree, so an edited header cannot misdescribe what is signed.

use base64::{Engine, engine::general_purpose::STANDARD};

use crate::{
    error::PayloadError,
    payload::{Payload, PayloadKind},
};

/// Large enough for any governance transaction, small enough that a hostile paste cannot exhaust memory.
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;

const PREFIX: &str = "OOTLE GOVERNANCE ";
const LINE_WIDTH: usize = 64;

pub fn encode(payload: &Payload) -> String {
    let label = payload.kind().label();
    let body = STANDARD.encode(tari_bor::encode(payload).expect("encoding to a Vec cannot fail"));

    let mut out = format!("-----BEGIN {PREFIX}{label}-----\n");
    for (name, value) in headers(payload) {
        out.push_str(&format!("{name}: {value}\n"));
    }
    out.push('\n');
    for chunk in body.as_bytes().chunks(LINE_WIDTH) {
        out.push_str(std::str::from_utf8(chunk).expect("base64 is ASCII"));
        out.push('\n');
    }
    out.push_str(&format!("-----END {PREFIX}{label}-----\n"));
    out
}

/// Decodes the first armored block in `text`.
///
/// Tolerates what mail and chat clients do to pasted text: surrounding prose, quote markers (`>`), indentation
/// and re-wrapped lines.
pub fn decode(text: &str) -> Result<Payload, PayloadError> {
    let mut lines = text.lines().map(clean_line);

    let kind = lines
        .by_ref()
        .find_map(|line| begin_label(line).map(str::to_string))
        .ok_or(PayloadError::NoArmor)?;
    let end_line = format!("-----END {PREFIX}{kind}-----");

    let mut claimed_headers = Vec::new();
    let mut body = String::new();
    let mut ended = false;
    for line in lines {
        if line == end_line {
            ended = true;
            break;
        }
        if line.is_empty() {
            continue;
        }
        match line.split_once(": ") {
            Some((name, value)) if body.is_empty() && is_header_name(name) => {
                claimed_headers.push((name.to_string(), value.to_string()));
            },
            _ => body.extend(line.chars().filter(|c| !c.is_whitespace())),
        }
    }
    if !ended {
        return Err(PayloadError::Truncated(format!("{PREFIX}{kind}")));
    }
    if body.len() > MAX_PAYLOAD_BYTES * 4 / 3 + 4 {
        return Err(PayloadError::TooLarge(body.len() * 3 / 4));
    }

    let bytes = STANDARD.decode(body.as_bytes())?;
    let payload: Payload = tari_bor::decode(&bytes).map_err(|e| PayloadError::Decode(e.to_string()))?;

    let armored_kind = PayloadKind::from_label(&kind).ok_or(PayloadError::NoArmor)?;
    if payload.kind() != armored_kind {
        return Err(PayloadError::WrongKind {
            expected: armored_kind.noun(),
            found: payload.kind().noun(),
        });
    }

    let actual = headers(&payload);
    for (name, claimed) in &claimed_headers {
        if let Some((name, actual)) = actual.iter().find(|(n, _)| *n == name.as_str()) &&
            claimed != actual
        {
            return Err(PayloadError::HeaderMismatch {
                name,
                header: claimed.clone(),
                actual: actual.clone(),
            });
        }
    }

    Ok(payload)
}

fn clean_line(line: &str) -> &str {
    line.trim().trim_start_matches(['>', ' ']).trim()
}

fn begin_label(line: &str) -> Option<&str> {
    line.strip_prefix("-----BEGIN ")?
        .strip_suffix("-----")?
        .strip_prefix(PREFIX)
}

fn is_header_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic() || c == '-')
}

/// The headers written for, and checked against, a payload.
fn headers(payload: &Payload) -> Vec<(&'static str, String)> {
    match payload {
        Payload::ProposalV1(proposal) => {
            let mut out = Vec::with_capacity(3);
            let network = proposal.unsigned.network();
            out.push((
                "Network",
                ootle_network::Network::try_from(network)
                    .map(|n| n.to_string())
                    .unwrap_or_else(|_| format!("unknown ({network:#04x})")),
            ));
            if let Ok(summary) = proposal.inspect() {
                out.push(("Action", summary.action.to_string()));
            }
            out.push(("Fingerprint", proposal.fingerprint().to_string()));
            out
        },
        Payload::SignatureV1(signature) => vec![
            ("Fingerprint", signature.fingerprint().to_string()),
            ("Signer", signature.public_key().to_string()),
        ],
    }
}
