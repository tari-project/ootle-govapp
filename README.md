# Ootle Governance

[![CI](https://github.com/tari-project/ootle-govapp/actions/workflows/ci.yml/badge.svg)](https://github.com/tari-project/ootle-govapp/actions/workflows/ci.yml)

A desktop app (and CLI) that lets Tari Ootle council members co-sign governance transactions. Today it supports one
call, `BurnRateGovernance::set_burn_rate`. Each member's keys stay in their own wallet daemon. Members pass proposals
and signatures to each other as text blocks over chat or email, or as files.

## How it works

The council is the governance component's owner rule: an m-of-n over members' public keys. Every key that signs a
transaction counts toward that threshold.

1. **Propose.** The proposer's walletd builds the transaction and freezes it:
   - fee paid from the proposer's account
   - inputs detected unversioned
   - valid until `activation_epoch - 2`

   The app prints a `PROPOSAL` block with a fingerprint.
2. **Sign.** Each member pastes the block into their app. The app checks:
   - the call shape
   - the network
   - the epochs
   - the live council

   The member reads the fingerprint back to the proposer on a call, then clicks **Request signature**. That creates
   a walletd *signing request*. Once the member approves it in the walletd UI, the app shows a `SIGNATURE` block to
   send back.
3. **Collect & submit.** The proposer pastes each signature; the app verifies it against the proposal and the
   council. At threshold, **Submit** hands the assembled transaction to walletd as a *transaction request*. The
   proposer approves it in the walletd UI, and the app broadcasts it and shows the result.

What every signature commits to:
- the frozen transaction
- the proposer's seal public key, which travels in the proposal

The app is not a general signer. It refuses anything other than one `pay_fee` call plus one known governance call.
It also refuses:
- a dry run
- blobs
- versioned inputs
- a validity window that outlives the activation lead

Block headers are recomputed from the body, so an edited header is rejected.

## Wallet daemon setup

This needs walletd with signing requests ([tari-ootle#2849](https://github.com/tari-project/tari-ootle/pull/2849)).

Create an API key in the walletd UI with these permissions:

```
signing_requests:create  transaction_requests:create  transactions:read
keys:read  accounts:read  substates:read  settings:read
```

Never give it `admin` or any `approve` permission. Approvals belong to a person in the walletd UI.

Keys:
- **Council key.** Must be an `account` or `transactions` key in the member's wallet. Imported keys cannot be listed
  over the API.
- **Fee account.** Must be on chain. Its owner key seals the transaction, and counts as an approval if it is on the
  council.

## Running

```sh
cargo run --release -p ootle-gov-app      # desktop app: ootle-governance
cargo run --release -p ootle-gov-cli -- --help   # CLI: ootle-gov
```

The app keeps the walletd URL per network in the config directory and the API key in the OS keyring.
`OOTLE_GOV_WALLETD_URL` and `OOTLE_GOV_API_KEY` override both; a key from the environment is never stored.

Proposals and signing requests are kept as JSON under the data directory, in `ootle-governance/<network>/`. They
contain nothing secret.

CLI example:

```sh
export OOTLE_GOV_API_KEY=tw_...
ootle-gov status
ootle-gov propose --rate-bps 700 --activation-epoch 4300 --memo "..." > proposal.txt
ootle-gov sign proposal.txt --wait > signature.txt      # each member
ootle-gov add-signature 3F9A-1C07-88D2-B4E1 signature.txt
ootle-gov submit 3F9A-1C07-88D2-B4E1 --wait
```

## Layout

- `crates/core`: payloads, text armor, policy checks, reading the council. Pure, with no network access.
- `crates/wallet`: the walletd flows (propose, signing requests, transaction requests) and the local store.
- `cli`: `ootle-gov`.
- `app`: the gpui desktop app (`gpui-kit`), Propose / Sign / Collect & submit / Settings.

The Tari Ootle crates are git dependencies pinned to a tari-ootle `development` commit. Switch them to
crates.io versions once 0.46.0 is released. To build against a local tari-ootle checkout instead, add a patch in
`.cargo/config.toml` (not committed):

```toml
[patch."https://github.com/tari-project/tari-ootle"]
tari_ootle_transaction = { path = "../tari-ootle/crates/transaction" }
tari_ootle_walletd_client = { path = "../tari-ootle/clients/wallet_daemon_client" }
# ...one line per crate in the root Cargo.toml
```

## Development

The toolchain is pinned in `rust-toolchain.toml`. Formatting uses the nightly rustfmt that tari-ootle uses.

```sh
cargo +nightly-2025-12-05 fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
```

Linux needs `pkg-config libssl-dev libdbus-1-dev libfontconfig-dev libxkbcommon-dev libxkbcommon-x11-dev
libwayland-dev libx11-xcb-dev libvulkan-dev`. macOS and Windows build OpenSSL from source, which needs Perl on
Windows.

CI runs rustfmt, clippy, and the tests on Linux, macOS and Windows for every pull request.
