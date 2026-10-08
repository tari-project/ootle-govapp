//! Choosing a network and its wallet daemon.

use gpui_kit::{
    AppContext,
    Context,
    Entity,
    IntoElement,
    ParentElement,
    Render,
    SharedString,
    Styled,
    Window,
    component::{
        Selectable,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputState},
        v_flex,
    },
    prelude::*,
};
use ootle_gov_core::Network;

use crate::{
    config::{NETWORKS, env_api_key, load_api_key, save_api_key},
    session::{Connection, Status},
    ui,
};

const PERMISSIONS: &str = "signing_requests:create, transaction_requests:create, transactions:read, keys:read, \
                           accounts:read, substates:read, settings:read";

pub struct SettingsView {
    connection: Entity<Connection>,
    network: Network,
    url: Entity<InputState>,
    api_key: Entity<InputState>,
    keyring_error: Option<String>,
}

impl SettingsView {
    pub fn new(
        connection: Entity<Connection>,
        keyring_error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let network = connection.read(cx).config.network;
        let url_value = connection.read(cx).config.endpoint(network);
        let url = cx.new(|cx| InputState::new(window, cx).default_value(url_value));
        let key_value = load_api_key(network).unwrap_or_default();
        let api_key = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("tw_…")
                .default_value(key_value)
        });
        cx.observe(&connection, |_, _, cx| cx.notify()).detach();
        Self {
            connection,
            network,
            url,
            api_key,
            keyring_error,
        }
    }

    fn select_network(&mut self, network: Network, window: &mut Window, cx: &mut Context<Self>) {
        self.network = network;
        let url = self.connection.read(cx).config.endpoint(network);
        let key = load_api_key(network).unwrap_or_default();
        self.url.update(cx, |s, cx| s.set_value(url, window, cx));
        self.api_key.update(cx, |s, cx| s.set_value(key, window, cx));
        cx.notify();
    }

    /// Saves the endpoint and key, then connects.
    pub fn connect(&mut self, cx: &mut Context<Self>) {
        let network = self.network;
        let url = self.url.read(cx).value().trim().to_string();
        let api_key = self.api_key.read(cx).value().trim().to_string();
        let from_env = env_api_key().is_some_and(|k| k == api_key);
        self.keyring_error = (!from_env)
            .then(|| save_api_key(network, &api_key).err())
            .flatten()
            .map(|e| {
                format!("The API key could not be stored in the OS keyring ({e}); it lasts for this session only.")
            });
        self.connection.update(cx, |connection, cx| {
            connection.config.network = network;
            connection.config.set_endpoint(network, url.clone());
            let _ = connection.config.save();
            connection.connect(url, api_key, network, cx);
        });
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.connection.read(cx).status.clone();
        let network_buttons = NETWORKS.iter().map(|network| {
            let network = *network;
            Button::new(SharedString::from(format!("net-{network}")))
                .label(network.to_string())
                .selected(self.network == network)
                .on_click(cx.listener(move |this, _, window, cx| this.select_network(network, window, cx)))
        });

        v_flex()
            .gap_4()
            .child(
                ui::section("Network", cx)
                    .child(h_flex().gap_2().flex_wrap().children(network_buttons))
                    .when(self.network == Network::MainNet, |s| {
                        s.child(ui::error(
                            "Mainnet: every proposal you sign changes the live network.",
                            cx,
                        ))
                    }),
            )
            .child(
                ui::section("Wallet daemon", cx)
                    .child(ui::row("JSON-RPC URL", Input::new(&self.url), cx))
                    .child(ui::row("API key", Input::new(&self.api_key), cx))
                    .child(ui::muted(
                        format!(
                            "Create the key in the walletd UI with these permissions: {PERMISSIONS}. Never grant \
                             admin or any approve permission: approvals happen in the walletd UI."
                        ),
                        cx,
                    ))
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                Button::new("connect")
                                    .primary()
                                    .label("Save and connect")
                                    .loading(status == Status::Connecting)
                                    .on_click(cx.listener(|this, _, _, cx| this.connect(cx))),
                            )
                            .child(match &status {
                                Status::Connected => ui::success("Connected", cx),
                                Status::Connecting => ui::muted("Connecting…", cx),
                                Status::Disconnected => ui::muted("Not connected", cx),
                                Status::Failed(e) => ui::error(e.clone(), cx),
                            }),
                    )
                    .children(self.keyring_error.clone().map(|e| ui::error(e, cx))),
            )
    }
}
