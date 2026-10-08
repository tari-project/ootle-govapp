//! `ootle-gov`: propose, co-sign, assemble and submit Ootle governance transactions from the command line.

use std::{
    io::{self, Read},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, anyhow, bail};
use clap::{Parser, Subcommand};
use ootle_gov_core::{
    action::{GovernanceAction, format_bps},
    amount::{format_tari, parse_tari},
    armor,
    council::CouncilState,
    payload::Payload,
    policy::ChainContext,
};
use ootle_gov_wallet::{
    ProposeRequest,
    SignatureStatus,
    SubmissionStatus,
    Wallet,
    key_ref::{format_key_id, parse_key_id},
    store::{ProposalRecord, Store, decode_proposal, default_store_dir},
};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Wallet daemon JSON-RPC endpoint.
    #[arg(
        long,
        env = "OOTLE_GOV_WALLETD_URL",
        default_value = "http://127.0.0.1:5100/json_rpc"
    )]
    url: String,
    /// A walletd API key (`tw_…`). See the README for the permissions it needs.
    #[arg(long, env = "OOTLE_GOV_API_KEY", hide_env_values = true)]
    api_key: String,
    /// Where proposals and signing requests are kept. Defaults to the per-user data directory.
    #[arg(long)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Network, epoch, council and burn rate schedule.
    Status,
    /// This wallet's signing keys, marking council members.
    Keys,
    /// Build and freeze a burn rate proposal, and print it for council members.
    Propose {
        /// The new rate in basis points (700 = 7%).
        #[arg(long)]
        rate_bps: u16,
        /// The first epoch the rate applies to.
        #[arg(long)]
        activation_epoch: u64,
        /// The account that pays the fee and seals. Defaults to the wallet's default account.
        #[arg(long)]
        account: Option<String>,
        /// Your council key when it is not the fee account's owner key, e.g. `account/1`.
        #[arg(long)]
        council_key: Option<String>,
        /// Fee ceiling in TARI. Only what is used is charged.
        #[arg(long, default_value = "10")]
        max_fee: String,
        #[arg(long, default_value = "")]
        memo: String,
    },
    /// Show what a pasted proposal or signature says and whether it can be signed. Reads stdin without FILE.
    Inspect { file: Option<PathBuf> },
    /// Ask walletd to co-sign a proposal. Approve it in the walletd UI.
    Sign {
        file: Option<PathBuf>,
        /// The council key to sign with, e.g. `account/0`. Detected when this wallet holds exactly one.
        #[arg(long)]
        key: Option<String>,
        /// Seconds the request waits for approval. Defaults to walletd's setting.
        #[arg(long)]
        ttl_secs: Option<u64>,
        /// Wait for approval and print the signature.
        #[arg(long)]
        wait: bool,
    },
    /// Print the signature for a signing request once it is approved.
    Signature { request_id: i32 },
    /// Proposals this wallet made, with their progress.
    List,
    /// Add a member's signature to one of your proposals.
    AddSignature {
        /// The proposal's fingerprint (with or without dashes).
        fingerprint: String,
        file: Option<PathBuf>,
    },
    /// Hand an assembled proposal to walletd for approval, then broadcast it.
    Submit {
        fingerprint: String,
        /// Wait for approval, broadcast and print the result.
        #[arg(long)]
        wait: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let wallet = Wallet::connect(&cli.url, &cli.api_key)
        .await
        .with_context(|| format!("connecting to {}", cli.url))?;
    let dir = match cli.data_dir {
        Some(dir) => dir,
        None => {
            default_store_dir(wallet.network()).ok_or_else(|| anyhow!("no per-user data directory; pass --data-dir"))?
        },
    };
    let store = Store::open(&dir).with_context(|| format!("opening {}", dir.display()))?;

    match cli.command {
        Command::Status => status(&wallet).await,
        Command::Keys => keys(&wallet).await,
        Command::Propose {
            rate_bps,
            activation_epoch,
            account,
            council_key,
            max_fee,
            memo,
        } => {
            let fee_account = match account {
                Some(a) => a.parse().map_err(|_| anyhow!("`{a}` is not a component address"))?,
                None => {
                    wallet
                        .accounts()
                        .await?
                        .into_iter()
                        .find(|a| a.is_default)
                        .ok_or_else(|| anyhow!("the wallet has no default account; pass --account"))?
                        .component_address
                },
            };
            let council_key = match council_key {
                Some(k) => Some(wallet.find_key(&parse_key_id(&k).map_err(|e| anyhow!(e))?).await?),
                None => None,
            };
            let record = wallet
                .propose(ProposeRequest {
                    action: GovernanceAction::set_burn_rate(rate_bps, activation_epoch)?,
                    fee_account,
                    max_fee: parse_tari(&max_fee).map_err(|e| anyhow!(e))?,
                    council_key,
                    memo,
                })
                .await?;
            store.save_proposal(&record)?;
            print!("{}", record.proposal);
            Ok(())
        },
        Command::Inspect { file } => inspect(&wallet, &read_input(file)?).await,
        Command::Sign {
            file,
            key,
            ttl_secs,
            wait,
        } => {
            let text = read_input(file)?;
            let proposal = decode_proposal(&text)?;
            let summary = proposal.inspect()?;
            let governance = wallet.governance().await?;
            let council = governance.council()?;
            let info = wallet.info().await?;
            summary.assess(&ChainContext {
                network: wallet.network(),
                current_epoch: info.current_epoch,
                council: Ok(council),
            })?;
            let key = match key {
                Some(k) => wallet.find_key(&parse_key_id(&k).map_err(|e| anyhow!(e))?).await?,
                None => {
                    let mut members: Vec<_> = wallet
                        .signing_keys()
                        .await?
                        .into_iter()
                        .filter(|k| council.is_member(&k.public_key))
                        .collect();
                    match members.len() {
                        1 => members.remove(0),
                        0 => bail!("this wallet holds no council key"),
                        _ => bail!("this wallet holds several council keys; choose one with --key"),
                    }
                },
            };
            if !council.is_member(&key.public_key) {
                bail!("{} is not on the council", key.label());
            }
            let mut record = wallet.request_signature(&text, key.key_id, ttl_secs).await?;
            store.save_signing(&record)?;
            eprintln!(
                "Signing request #{} created for {}. Approve it in the walletd UI; fingerprint {}.",
                record.request_id,
                key.label(),
                summary.fingerprint
            );
            if wait {
                loop {
                    match wallet.signature_status(&record).await? {
                        SignatureStatus::Pending => tokio::time::sleep(Duration::from_secs(3)).await,
                        SignatureStatus::Signed(signature) => {
                            let armored = armor::encode(&Payload::SignatureV1(signature));
                            record.signature = Some(armored.clone());
                            store.save_signing(&record)?;
                            print!("{armored}");
                            return Ok(());
                        },
                        SignatureStatus::Rejected => bail!("the signing request was rejected"),
                        SignatureStatus::Expired => bail!("the signing request expired"),
                    }
                }
            }
            Ok(())
        },
        Command::Signature { request_id } => {
            let mut record = store
                .signing_records()?
                .into_iter()
                .find(|r| r.request_id == request_id)
                .ok_or_else(|| anyhow!("no signing request #{request_id} in {}", store.root().display()))?;
            match wallet.signature_status(&record).await? {
                SignatureStatus::Signed(signature) => {
                    let armored = armor::encode(&Payload::SignatureV1(signature));
                    record.signature = Some(armored.clone());
                    store.save_signing(&record)?;
                    print!("{armored}");
                    Ok(())
                },
                SignatureStatus::Pending => bail!("not approved yet; approve it in the walletd UI"),
                SignatureStatus::Rejected => bail!("the signing request was rejected"),
                SignatureStatus::Expired => bail!("the signing request expired"),
            }
        },
        Command::List => {
            let governance = wallet.governance().await.ok();
            for record in store.proposals()? {
                let proposal = record.decode_proposal()?;
                let summary = proposal.inspect()?;
                let progress = match governance.as_ref().map(|g| g.council()) {
                    Some(Ok(council)) => format!("{}/{}", record.approvals(&summary, council)?, council.threshold),
                    _ => "?".to_string(),
                };
                println!(
                    "{}  {}  approvals {progress}  {}",
                    summary.fingerprint,
                    summary.action,
                    record.outcome.as_deref().unwrap_or("not submitted")
                );
            }
            Ok(())
        },
        Command::AddSignature { fingerprint, file } => {
            let mut record = find_proposal(&store, &fingerprint)?;
            let summary = record.decode_proposal()?.inspect()?;
            let governance = wallet.governance().await?;
            let council = governance.council()?;
            let signature = record.add_signature(&read_input(file)?, &summary, council)?;
            store.save_proposal(&record)?;
            println!(
                "Added {}'s signature: {}/{} approvals",
                signature.public_key(),
                record.approvals(&summary, council)?,
                council.threshold
            );
            Ok(())
        },
        Command::Submit { fingerprint, wait } => {
            let mut record = find_proposal(&store, &fingerprint)?;
            let summary = record.decode_proposal()?.inspect()?;
            let governance = wallet.governance().await?;
            let council = governance.council()?;
            let approvals = record.approvals(&summary, council)?;
            if approvals < usize::from(council.threshold) {
                bail!(
                    "{approvals}/{} approvals; collect more signatures first",
                    council.threshold
                );
            }
            let request_id = match record.transaction_request_id {
                Some(id) => id,
                None => {
                    let id = wallet.submit(&record).await?;
                    record.transaction_request_id = Some(id);
                    store.save_proposal(&record)?;
                    id
                },
            };
            eprintln!("Transaction request #{request_id}. Approve it in the walletd UI.");
            if !wait {
                return Ok(());
            }
            let transaction_id = loop {
                match wallet.submission_status(request_id).await? {
                    SubmissionStatus::AwaitingApproval | SubmissionStatus::Submitting => {
                        tokio::time::sleep(Duration::from_secs(3)).await
                    },
                    SubmissionStatus::Approved => break wallet.broadcast(request_id).await?,
                    SubmissionStatus::Submitted(id) => break id,
                    SubmissionStatus::Rejected => bail!("the transaction request was rejected"),
                    SubmissionStatus::Expired => bail!("the transaction request expired"),
                }
            };
            record.transaction_id = Some(transaction_id);
            store.save_proposal(&record)?;
            eprintln!("Submitted {transaction_id}; waiting for the result");
            let outcome = wallet.wait_result(transaction_id, 120).await?;
            if !outcome.timed_out {
                record.outcome = Some(outcome.describe());
                store.save_proposal(&record)?;
            }
            println!("{}", outcome.describe());
            Ok(())
        },
    }
}

async fn status(wallet: &Wallet) -> anyhow::Result<()> {
    let info = wallet.info().await?;
    println!("Wallet      {} ({})", wallet.endpoint(), info.version);
    println!("Network     {}", info.network);
    println!(
        "Epoch       {}",
        info.current_epoch
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown".into())
    );
    let governance = wallet.governance().await?;
    match &governance.council {
        CouncilState::Seated(council) => {
            println!("Council     {} of {}", council.threshold, council.members.len());
            for member in &council.members {
                println!("            {member}");
            }
        },
        CouncilState::None => println!("Council     none"),
        CouncilState::Unrecognised(rule) => println!("Council     unrecognised owner rule: {rule}"),
    }
    if let Some(retired_from) = governance.state.retired_from {
        println!("Retired     from epoch {retired_from}");
    }
    for change in &governance.state.schedule {
        println!(
            "Schedule    {} from epoch {}",
            format_bps(change.rate_bps),
            change.activation_epoch
        );
    }
    Ok(())
}

async fn keys(wallet: &Wallet) -> anyhow::Result<()> {
    let governance = wallet.governance().await?;
    let council = governance.council().ok();
    for key in wallet.signing_keys().await? {
        let member = council.is_some_and(|c| c.is_member(&key.public_key));
        println!(
            "{:<16} {}{}",
            format_key_id(&key.key_id),
            key.public_key,
            if member { "  council member" } else { "" }
        );
    }
    Ok(())
}

async fn inspect(wallet: &Wallet, text: &str) -> anyhow::Result<()> {
    match armor::decode(text)? {
        Payload::ProposalV1(proposal) => {
            let summary = proposal.inspect()?;
            println!("Fingerprint {}", summary.fingerprint);
            println!("Network     {}", summary.network);
            println!("Action      {}", summary.action);
            println!(
                "Fee         up to {} TARI from {}",
                format_tari(summary.max_fee),
                summary.fee_account
            );
            println!("Valid until epoch {}", summary.max_epoch);
            println!("Sealed by   {}", summary.seal_public_key);
            if !proposal.memo.is_empty() {
                println!("Memo        {} (from the proposer, unverified)", proposal.memo);
            }
            let governance = wallet.governance().await?;
            let info = wallet.info().await?;
            match summary.assess(&ChainContext {
                network: wallet.network(),
                current_epoch: info.current_epoch,
                council: governance.council(),
            }) {
                Ok(assessment) => println!(
                    "OK          council {} of {}, {} epochs left to submit",
                    assessment.threshold,
                    assessment.council_size,
                    assessment
                        .epochs_remaining
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "?".into())
                ),
                Err(e) => println!("REFUSED     {e}"),
            }
        },
        Payload::SignatureV1(signature) => {
            println!("Signature for proposal {}", signature.fingerprint());
            println!("Signer      {}", signature.public_key());
        },
    }
    Ok(())
}

fn find_proposal(store: &Store, fingerprint: &str) -> anyhow::Result<ProposalRecord> {
    let wanted = fingerprint.replace('-', "").to_lowercase();
    store
        .proposals()?
        .into_iter()
        .find(|r| r.fingerprint_hex().is_ok_and(|f| f == wanted))
        .ok_or_else(|| anyhow!("no proposal {fingerprint} in {}", store.root().display()))
}

fn read_input(file: Option<PathBuf>) -> anyhow::Result<String> {
    match file {
        Some(path) => std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display())),
        None => {
            let mut text = String::new();
            io::stdin().read_to_string(&mut text)?;
            Ok(text)
        },
    }
}
