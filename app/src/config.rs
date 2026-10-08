//! Wallet daemon endpoints per network, and their API keys in the OS keyring.

use std::{collections::BTreeMap, fs, path::PathBuf};

use ootle_gov_core::Network;
use serde::{Deserialize, Serialize};

const KEYRING_SERVICE: &str = "ootle-governance";

pub const NETWORKS: [Network; 6] = [
    Network::LocalNet,
    Network::Esmeralda,
    Network::Igor,
    Network::NextNet,
    Network::StageNet,
    Network::MainNet,
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub network: Network,
    /// walletd JSON-RPC URL per network.
    pub endpoints: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            network: Network::LocalNet,
            endpoints: BTreeMap::new(),
        }
    }
}

impl Config {
    fn path() -> Option<PathBuf> {
        Some(dirs::config_dir()?.join("ootle-governance").join("config.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path().ok_or_else(|| anyhow::anyhow!("no per-user config directory"))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn endpoint(&self, network: Network) -> String {
        if let Ok(url) = std::env::var("OOTLE_GOV_WALLETD_URL") &&
            !url.is_empty()
        {
            return url;
        }
        self.endpoints
            .get(network.as_key_str())
            .cloned()
            .unwrap_or_else(|| "http://127.0.0.1:5100/json_rpc".to_string())
    }

    pub fn set_endpoint(&mut self, network: Network, url: String) {
        self.endpoints.insert(network.as_key_str().to_string(), url);
    }
}

/// An API key from `OOTLE_GOV_API_KEY`. It applies to whichever network is selected and lasts for the session.
pub fn env_api_key() -> Option<String> {
    std::env::var("OOTLE_GOV_API_KEY").ok().filter(|k| !k.is_empty())
}

pub fn load_api_key(network: Network) -> Option<String> {
    if let Some(key) = env_api_key() {
        return Some(key);
    }
    keyring_core::Entry::new(KEYRING_SERVICE, network.as_key_str())
        .and_then(|e| e.get_password())
        .ok()
}

pub fn save_api_key(network: Network, api_key: &str) -> anyhow::Result<()> {
    keyring_core::Entry::new(KEYRING_SERVICE, network.as_key_str())?.set_password(api_key)?;
    Ok(())
}

/// Installs the OS credential store. Without one, API keys last for the session only.
pub fn init_keyring() -> Result<(), String> {
    let store: keyring_core::Result<std::sync::Arc<keyring_core::CredentialStore>> = {
        #[cfg(target_os = "macos")]
        {
            apple_native_keyring_store::keychain::Store::new().map(|s| s as _)
        }
        #[cfg(target_os = "linux")]
        {
            dbus_secret_service_keyring_store::Store::new().map(|s| s as _)
        }
        #[cfg(target_os = "windows")]
        {
            windows_native_keyring_store::Store::new().map(|s| s as _)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            Err(keyring_core::Error::NotSupportedByStore(
                "no OS keyring on this platform".to_string(),
            ))
        }
    };
    let store = store.map_err(|e| e.to_string())?;
    keyring_core::set_default_store(store);
    Ok(())
}
