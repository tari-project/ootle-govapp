//! A short text form for wallet key ids: `account/0`, `transactions/3`, `imported/1`.

use std::str::FromStr;

use tari_ootle_wallet_sdk::models::{KeyBranch, KeyId};

/// The branches a signing request accepts. walletd refuses mask, nonce and view keys.
pub const SIGNING_BRANCHES: [KeyBranch; 2] = [KeyBranch::Account, KeyBranch::Transaction];

pub fn format_key_id(key_id: &KeyId) -> String {
    match key_id {
        KeyId::Derived { key_branch, index } => format!("{}/{index}", key_branch.as_str()),
        KeyId::Imported { local_key_id } => format!("imported/{local_key_id}"),
    }
}

pub fn parse_key_id(s: &str) -> Result<KeyId, String> {
    let (kind, index) = s
        .trim()
        .split_once('/')
        .ok_or_else(|| format!("`{s}` is not a key id; expected e.g. account/0, transactions/3 or imported/1"))?;
    let index = u64::from_str(index).map_err(|_| format!("`{index}` is not a key index"))?;
    match kind {
        "imported" => Ok(KeyId::Imported { local_key_id: index }),
        branch => {
            let key_branch = SIGNING_BRANCHES
                .into_iter()
                .find(|b| b.as_str() == branch)
                .ok_or_else(|| {
                    format!("`{branch}` is not a signing key branch; use account, transactions or imported")
                })?;
            Ok(KeyId::Derived { key_branch, index })
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_ids_round_trip() {
        for s in ["account/0", "transactions/3", "imported/1"] {
            assert_eq!(format_key_id(&parse_key_id(s).unwrap()), s);
        }
    }

    #[test]
    fn non_signing_branches_are_refused() {
        assert!(parse_key_id("stealth_mask/0").is_err());
        assert!(parse_key_id("account").is_err());
    }
}
