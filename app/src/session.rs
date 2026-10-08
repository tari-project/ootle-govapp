//! The connection to one wallet daemon, shared by every view.

use std::sync::Arc;

use gpui_kit::{Context, Window};
use ootle_gov_core::{
    Network,
    council::{Council, GovernanceView},
    error::PolicyError,
};
use ootle_gov_wallet::{
    Account,
    Wallet,
    WalletInfo,
    WalletKey,
    store::{Store, default_store_dir},
};

use crate::{config::Config, runtime::io};

pub struct Session {
    pub wallet: Wallet,
    pub info: WalletInfo,
    pub governance: Result<GovernanceView, String>,
    pub keys: Vec<WalletKey>,
    pub accounts: Vec<Account>,
    pub store: Store,
}

impl Session {
    async fn load(url: String, api_key: String, network: Network) -> Result<Self, String> {
        let wallet = Wallet::connect(&url, &api_key).await.map_err(|e| e.to_string())?;
        if wallet.network() != network {
            return Err(format!(
                "{url} is a {} wallet, not {network}. Check the endpoint for {network}.",
                wallet.network()
            ));
        }
        let store = default_store_dir(network)
            .ok_or("no per-user data directory")
            .and_then(|dir| Store::open(dir).map_err(|_| "cannot create the data directory"))?;
        Self::fetch(wallet, store).await
    }

    async fn fetch(wallet: Wallet, store: Store) -> Result<Self, String> {
        let info = wallet.info().await.map_err(|e| e.to_string())?;
        let governance = wallet.governance().await.map_err(|e| e.to_string());
        let keys = wallet.signing_keys().await.map_err(|e| e.to_string())?;
        let accounts = wallet.accounts().await.map_err(|e| e.to_string())?;
        Ok(Self {
            wallet,
            info,
            governance,
            keys,
            accounts,
            store,
        })
    }

    pub fn network(&self) -> Network {
        self.info.network
    }

    pub fn council(&self) -> Result<&Council, PolicyError> {
        match &self.governance {
            Ok(view) => view.council(),
            Err(e) => Err(PolicyError::UnrecognisedCouncil(e.clone())),
        }
    }

    /// This wallet's keys that are on the council.
    pub fn council_keys(&self) -> Vec<WalletKey> {
        let Ok(council) = self.council() else {
            return Vec::new();
        };
        self.keys
            .iter()
            .filter(|k| council.is_member(&k.public_key))
            .cloned()
            .collect()
    }

    /// On-chain accounts whose owner key this wallet holds: the ones that can pay and seal.
    pub fn owned_accounts(&self) -> Vec<Account> {
        self.accounts
            .iter()
            .filter(|a| a.owner_key_id.is_some() && a.is_confirmed_on_chain)
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
}

pub struct Connection {
    pub config: Config,
    pub session: Option<Arc<Session>>,
    pub status: Status,
    /// Bumped whenever local records change, so views showing them reload.
    pub records_revision: u64,
}

impl Connection {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            session: None,
            status: Status::Disconnected,
            records_revision: 0,
        }
    }

    pub fn connect(&mut self, url: String, api_key: String, network: Network, cx: &mut Context<Self>) {
        self.status = Status::Connecting;
        self.session = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = io(Session::load(url, api_key, network)).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(session) => {
                        this.session = Some(Arc::new(session));
                        this.status = Status::Connected;
                    },
                    Err(e) => this.status = Status::Failed(e),
                }
                this.records_revision += 1;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Re-reads the epoch, council, keys, accounts and local records.
    pub fn refresh(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = io(Session::fetch(session.wallet.clone(), session.store.clone())).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(session) => this.session = Some(Arc::new(session)),
                    Err(e) => this.status = Status::Failed(e),
                }
                this.records_revision += 1;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn records_changed(&mut self, cx: &mut Context<Self>) {
        self.records_revision += 1;
        cx.notify();
    }
}
