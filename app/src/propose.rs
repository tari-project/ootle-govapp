//! Proposing a burn rate change.

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
        input::{Input, InputState, TextareaState},
        v_flex,
    },
    prelude::*,
};
use ootle_gov_core::{
    action::{ACTIVATION_LEAD_EPOCHS, GovernanceAction, format_bps},
    amount::parse_tari,
};
use ootle_gov_wallet::{KeyId, ProposeRequest, key_ref::format_key_id, store::ProposalRecord};
use tari_template_lib_types::ComponentAddress;

use crate::{runtime::io, session::Connection, ui};

pub struct ProposeView {
    connection: Entity<Connection>,
    rate: Entity<InputState>,
    activation: Entity<InputState>,
    max_fee: Entity<InputState>,
    memo: Entity<InputState>,
    fee_account: Option<ComponentAddress>,
    council_key: Option<KeyId>,
    busy: bool,
    error: Option<String>,
    created: Option<(ProposalRecord, Entity<TextareaState>)>,
}

impl ProposeView {
    pub fn new(connection: Entity<Connection>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rate = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. 700 for 7%"));
        let activation = cx.new(|cx| InputState::new(window, cx).placeholder("epoch number"));
        let max_fee = cx.new(|cx| InputState::new(window, cx).default_value("10"));
        let memo = cx.new(|cx| InputState::new(window, cx).placeholder("Why, for the other members (optional)"));
        for input in [&rate, &activation] {
            cx.observe(input, |_, _, cx| cx.notify()).detach();
        }
        cx.observe(&connection, |this, connection, cx| {
            this.pick_defaults(&connection, cx);
            cx.notify();
        })
        .detach();
        Self {
            connection,
            rate,
            activation,
            max_fee,
            memo,
            fee_account: None,
            council_key: None,
            busy: false,
            error: None,
            created: None,
        }
    }

    /// Selects the default account and, when the fee account's own key is not on the council, this wallet's
    /// only council key.
    fn pick_defaults(&mut self, connection: &Entity<Connection>, cx: &mut Context<Self>) {
        let Some(session) = connection.read(cx).session.clone() else {
            return;
        };
        let accounts = session.owned_accounts();
        if self
            .fee_account
            .is_none_or(|a| !accounts.iter().any(|x| x.component_address == a))
        {
            self.fee_account = accounts
                .iter()
                .find(|a| a.is_default)
                .or(accounts.first())
                .map(|a| a.component_address);
        }
        let council_keys = session.council_keys();
        let seal_is_member = self
            .fee_account
            .and_then(|a| accounts.iter().find(|x| x.component_address == a))
            .is_some_and(|a| council_keys.iter().any(|k| k.public_key == a.owner_public_key));
        if seal_is_member {
            self.council_key = None;
        } else if self.council_key.is_none() && council_keys.len() == 1 {
            self.council_key = Some(council_keys[0].key_id);
        }
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        let Some(session) = self.connection.read(cx).session.clone() else {
            return;
        };
        let request = match self.request(cx) {
            Ok(request) => request,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            },
        };
        self.busy = true;
        cx.notify();
        let connection = self.connection.clone();
        cx.spawn_in(window, async move |this, cx| {
            let wallet = session.wallet.clone();
            let result = io(async move { wallet.propose(request).await }).await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(record) => match session.store.save_proposal(&record) {
                        Ok(()) => {
                            let output = ui::new_textarea(12, "", window, cx);
                            ui::set_text(&output, record.proposal.clone(), window, cx);
                            this.created = Some((record, output));
                            connection.update(cx, |c, cx| c.records_changed(cx));
                        },
                        Err(e) => this.error = Some(format!("The proposal could not be saved: {e}")),
                    },
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn request(&self, cx: &Context<Self>) -> Result<ProposeRequest, String> {
        let session = self.connection.read(cx).session.clone().ok_or("Not connected")?;
        let rate_bps: u16 = self
            .rate
            .read(cx)
            .value()
            .trim()
            .parse()
            .map_err(|_| "Enter the rate in basis points, 0 to 10000")?;
        let activation_epoch: u64 = self
            .activation
            .read(cx)
            .value()
            .trim()
            .parse()
            .map_err(|_| "Enter the activation epoch")?;
        let action = GovernanceAction::set_burn_rate(rate_bps, activation_epoch).map_err(|e| e.to_string())?;
        if let Some(current) = session.info.current_epoch {
            let earliest = current + ACTIVATION_LEAD_EPOCHS;
            if activation_epoch < earliest {
                return Err(format!("The activation epoch must be at least {earliest}"));
            }
        }
        let max_fee = parse_tari(&self.max_fee.read(cx).value())?;
        let fee_account = self.fee_account.ok_or("Choose the account that pays the fee")?;
        let council_key = match self.council_key {
            Some(key_id) => Some(
                session
                    .keys
                    .iter()
                    .find(|k| k.key_id == key_id)
                    .cloned()
                    .ok_or("The selected council key is no longer in the wallet")?,
            ),
            None => None,
        };
        Ok(ProposeRequest {
            action,
            fee_account,
            max_fee,
            council_key,
            memo: self.memo.read(cx).value().trim().to_string(),
        })
    }
}

impl Render for ProposeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(session) = self.connection.read(cx).session.clone() else {
            return v_flex().child(ui::muted("Connect to a wallet daemon in Settings.", cx));
        };

        if let Some((record, output)) = &self.created {
            let fingerprint = record
                .decode_proposal()
                .map(|p| p.fingerprint().to_string())
                .unwrap_or_default();
            let file_name = format!("proposal-{}.txt", fingerprint.to_lowercase());
            return v_flex()
                .gap_4()
                .child(
                    ui::section("Proposal created", cx)
                        .child(ui::row("Fingerprint", ui::mono(fingerprint, cx).text_xl(), cx))
                        .child(ui::muted(
                            "Send this to the other council members by chat or email, and read the fingerprint to \
                             them over another channel. Add their signatures in Collect.",
                            cx,
                        ))
                        .child(ui::armored_block("proposal", output, file_name, cx)),
                )
                .child(
                    Button::new("another")
                        .label("New proposal")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.created = None;
                            cx.notify();
                        })),
                );
        }

        let current = session.info.current_epoch;
        let rate_hint = self
            .rate
            .read(cx)
            .value()
            .trim()
            .parse::<u16>()
            .ok()
            .map(format_bps)
            .unwrap_or_default();
        let activation_hint = match (current, self.activation.read(cx).value().trim().parse::<u64>().ok()) {
            (Some(current), Some(activation)) if activation >= current + ACTIVATION_LEAD_EPOCHS => {
                let window = activation - ACTIVATION_LEAD_EPOCHS - current;
                format!(
                    "Members have {window} epochs (≈ {} h at 20 min per epoch) to sign and submit, until epoch {}.",
                    window / 3,
                    activation - ACTIVATION_LEAD_EPOCHS
                )
            },
            (Some(current), _) => format!(
                "The network is at epoch {current}. The earliest activation is {}; leave days for signatures (about \
                 72 epochs a day).",
                current + ACTIVATION_LEAD_EPOCHS
            ),
            (None, _) => "The wallet cannot see the current epoch.".to_string(),
        };

        let accounts = session.owned_accounts();
        let council_keys = session.council_keys();
        let fee_account = self.fee_account;
        let account_buttons = accounts.iter().map(|account| {
            let address = account.component_address;
            let member = council_keys.iter().any(|k| k.public_key == account.owner_public_key);
            let label = format!(
                "{}{}",
                account.name.clone().unwrap_or_else(|| address.to_string()),
                if member { " (council member)" } else { "" }
            );
            Button::new(SharedString::from(format!("acct-{address}")))
                .label(label)
                .selected(fee_account == Some(address))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.fee_account = Some(address);
                    cx.notify();
                }))
        });
        let key_buttons = council_keys.iter().map(|key| {
            let key_id = key.key_id;
            Button::new(SharedString::from(format!("ck-{}", format_key_id(&key_id))))
                .label(format!("{}  {}", format_key_id(&key_id), key.public_key))
                .selected(self.council_key == Some(key_id))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.council_key = Some(key_id);
                    cx.notify();
                }))
        });

        v_flex()
            .gap_4()
            .child(
                ui::section("Set the exhaust burn rate", cx)
                    .child(ui::row(
                        "Rate (bps)",
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(div_w(Input::new(&self.rate)))
                            .child(rate_hint),
                        cx,
                    ))
                    .child(ui::row("Activation epoch", div_w(Input::new(&self.activation)), cx))
                    .child(ui::muted(activation_hint, cx)),
            )
            .child(
                ui::section("Fee and signers", cx)
                    .child(ui::row(
                        "Fee account",
                        if accounts.is_empty() {
                            ui::error("This wallet has no on-chain account it can pay from.", cx)
                        } else {
                            v_flex().gap_1().items_start().children(account_buttons)
                        },
                        cx,
                    ))
                    .child(ui::row("Max fee (TARI)", div_w(Input::new(&self.max_fee)), cx))
                    .child(ui::row(
                        "Your council key",
                        v_flex()
                            .gap_1()
                            .items_start()
                            .child(
                                Button::new("ck-none")
                                    .label("Not adding one")
                                    .selected(self.council_key.is_none())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.council_key = None;
                                        cx.notify();
                                    })),
                            )
                            .children(key_buttons),
                        cx,
                    ))
                    .child(ui::muted(
                        "The fee account's key seals the transaction and counts as an approval if it is on the \
                         council. Pick a separate council key only if yours is a different key in this wallet.",
                        cx,
                    ))
                    .child(ui::row("Memo", Input::new(&self.memo), cx)),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        Button::new("create")
                            .primary()
                            .label("Create proposal")
                            .loading(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
                    )
                    .when_some(self.error.clone(), |el, e| el.child(ui::error(e, cx))),
            )
    }
}

fn div_w(child: impl IntoElement) -> gpui_kit::Div {
    gpui_kit::div().w(gpui_kit::px(240.)).child(child)
}
