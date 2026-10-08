//! Co-signing a proposal another member sent.

use std::time::Duration;

use gpui_kit::{
    Context,
    Entity,
    IntoElement,
    ParentElement,
    Render,
    SharedString,
    Styled,
    Task,
    Window,
    component::{
        Disableable,
        Selectable,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Textarea, TextareaState},
        v_flex,
    },
    prelude::*,
};
use ootle_gov_core::{
    action::format_bps,
    amount::format_tari,
    armor,
    payload::{Payload, ProposalV1},
    policy::{Assessment, ChainContext, ProposalSummary},
};
use ootle_gov_wallet::{
    KeyId,
    SignatureStatus,
    key_ref::format_key_id,
    store::{SigningRecord, decode_proposal},
};

use crate::{
    runtime::io,
    session::{Connection, Session},
    ui,
};

struct Checked {
    text: String,
    proposal: ProposalV1,
    summary: ProposalSummary,
    verdict: Result<Assessment, String>,
}

pub struct SignView {
    connection: Entity<Connection>,
    paste: Entity<TextareaState>,
    checked: Option<Result<Checked, String>>,
    key: Option<KeyId>,
    busy: bool,
    error: Option<String>,
    record: Option<SigningRecord>,
    status: Option<String>,
    signature: Option<Entity<TextareaState>>,
    history: Vec<SigningRecord>,
    seen_revision: Option<u64>,
    _poll: Option<Task<()>>,
}

impl SignView {
    pub fn new(connection: Entity<Connection>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let paste = ui::new_textarea(
            10,
            "Paste the -----BEGIN OOTLE GOVERNANCE PROPOSAL----- block here",
            window,
            cx,
        );
        cx.observe(&connection, |this, connection, cx| {
            let revision = connection.read(cx).records_revision;
            if this.seen_revision != Some(revision) {
                this.seen_revision = Some(revision);
                this.reload_history(cx);
            }
            cx.notify();
        })
        .detach();
        Self {
            connection,
            paste,
            checked: None,
            key: None,
            busy: false,
            error: None,
            record: None,
            status: None,
            signature: None,
            history: Vec::new(),
            seen_revision: None,
            _poll: None,
        }
    }

    fn session(&self, cx: &Context<Self>) -> Option<std::sync::Arc<Session>> {
        self.connection.read(cx).session.clone()
    }

    fn reload_history(&mut self, cx: &Context<Self>) {
        self.history = self
            .session(cx)
            .and_then(|s| s.store.signing_records().ok())
            .unwrap_or_default();
    }

    fn check(&mut self, cx: &mut Context<Self>) {
        let text = self.paste.read(cx).value().to_string();
        self.record = None;
        self.signature = None;
        self.status = None;
        self.error = None;
        self._poll = None;
        let Some(session) = self.session(cx) else {
            return;
        };
        self.checked = Some(check(&session, text));
        let council_keys = session.council_keys();
        self.key = (council_keys.len() == 1).then(|| council_keys[0].key_id);
        cx.notify();
    }

    fn request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(session), Some(Ok(checked)), Some(key)) = (self.session(cx), &self.checked, self.key) else {
            return;
        };
        let text = checked.text.clone();
        self.busy = true;
        self.error = None;
        cx.notify();
        let connection = self.connection.clone();
        cx.spawn_in(window, async move |this, cx| {
            let wallet = session.wallet.clone();
            let result = io(async move { wallet.request_signature(&text, key, None).await }).await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(record) => {
                        if let Err(e) = session.store.save_signing(&record) {
                            this.error = Some(format!("The signing request could not be saved: {e}"));
                        }
                        connection.update(cx, |c, cx| c.records_changed(cx));
                        this.follow(record, window, cx);
                    },
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Shows a signing request and polls walletd until a person decides it.
    fn follow(&mut self, record: SigningRecord, window: &mut Window, cx: &mut Context<Self>) {
        self.signature = None;
        self.status = Some(format!(
            "Waiting for approval of signing request #{} in the walletd UI…",
            record.request_id
        ));
        if let Some(armored) = &record.signature {
            self.show_signature(armored.clone(), window, cx);
            self.record = Some(record);
            self._poll = None;
            return;
        }
        let Some(session) = self.session(cx) else {
            return;
        };
        self.record = Some(record.clone());
        self._poll = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let wallet = session.wallet.clone();
                let polled = record.clone();
                let status = io(async move { wallet.signature_status(&polled).await }).await;
                let done = this
                    .update_in(cx, |this, window, cx| {
                        let done = match status {
                            Ok(SignatureStatus::Pending) => false,
                            Ok(SignatureStatus::Signed(signature)) => {
                                let armored = armor::encode(&Payload::SignatureV1(signature));
                                let mut record = record.clone();
                                record.signature = Some(armored.clone());
                                let _ = session.store.save_signing(&record);
                                this.record = Some(record);
                                this.show_signature(armored, window, cx);
                                true
                            },
                            Ok(SignatureStatus::Rejected) => {
                                this.status = Some("The signing request was rejected in walletd.".into());
                                true
                            },
                            Ok(SignatureStatus::Expired) => {
                                this.status = Some("The signing request expired before it was approved.".into());
                                true
                            },
                            Err(e) => {
                                this.status = Some(format!("Could not check the signing request: {e}"));
                                false
                            },
                        };
                        cx.notify();
                        done
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(3)).await;
            }
        }));
    }

    fn show_signature(&mut self, armored: String, window: &mut Window, cx: &mut Context<Self>) {
        let output = ui::new_textarea(8, "", window, cx);
        ui::set_text(&output, armored, window, cx);
        self.signature = Some(output);
        self.status = Some("Signed. Send this back to the proposer.".into());
    }

    fn open_history(&mut self, record: SigningRecord, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session(cx) else {
            return;
        };
        ui::set_text(&self.paste, record.proposal.clone(), window, cx);
        self.checked = Some(check(&session, record.proposal.clone()));
        self.key = Some(record.key_id);
        self.follow(record, window, cx);
        cx.notify();
    }

    fn paste_from_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            if let Some(text) = ui::open_file(cx).await {
                this.update_in(cx, |this, window, cx| {
                    ui::set_text(&this.paste, text, window, cx);
                    this.check(cx);
                })
                .ok();
            }
        })
        .detach();
    }
}

fn check(session: &Session, text: String) -> Result<Checked, String> {
    let proposal = decode_proposal(&text).map_err(|e| e.to_string())?;
    let summary = proposal.inspect().map_err(|e| e.to_string())?;
    let verdict = summary
        .assess(&ChainContext {
            network: session.network(),
            current_epoch: session.info.current_epoch,
            council: session.council(),
        })
        .map_err(|e| e.to_string());
    Ok(Checked {
        text,
        proposal,
        summary,
        verdict,
    })
}

impl Render for SignView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(session) = self.session(cx) else {
            return v_flex().child(ui::muted("Connect to a wallet daemon in Settings.", cx));
        };

        let input = ui::section("Proposal", cx).child(Textarea::new(&self.paste)).child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("check")
                        .primary()
                        .label("Check")
                        .on_click(cx.listener(|this, _, _, cx| this.check(cx))),
                )
                .child(
                    Button::new("open")
                        .label("Open file…")
                        .on_click(cx.listener(|this, _, window, cx| this.paste_from_file(window, cx))),
                ),
        );

        let review = match &self.checked {
            None => None,
            Some(Err(e)) => Some(ui::section("Cannot sign", cx).child(ui::error(e.clone(), cx))),
            Some(Ok(checked)) => Some(self.render_review(checked, &session, cx)),
        };

        let history = (!self.history.is_empty()).then(|| {
            let rows = self.history.iter().take(10).map(|record| {
                let label = match record
                    .decode_proposal()
                    .and_then(|p| Ok((p.fingerprint(), p.inspect()?)))
                {
                    Ok((fingerprint, summary)) => format!("{fingerprint}  {}", summary.action),
                    Err(_) => "unreadable proposal".to_string(),
                };
                let state = if record.signature.is_some() {
                    "signed"
                } else {
                    "requested"
                };
                let record_clone = record.clone();
                Button::new(SharedString::from(format!("hist-{}", record.request_id)))
                    .label(format!("#{}  {label}  ({state})", record.request_id))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.open_history(record_clone.clone(), window, cx)),
                    )
            });
            ui::section("Your signing requests", cx).child(v_flex().gap_1().items_start().children(rows))
        });

        v_flex().gap_4().child(input).children(review).children(history)
    }
}

impl SignView {
    fn render_review(&self, checked: &Checked, session: &Session, cx: &mut Context<Self>) -> gpui_kit::Div {
        let summary = &checked.summary;
        let mut section = ui::section("Review", cx)
            .child(ui::row(
                "Fingerprint",
                ui::mono(summary.fingerprint.to_string(), cx).text_xl(),
                cx,
            ))
            .child(ui::muted(
                "Confirm this fingerprint with the proposer over a call or another channel before signing.",
                cx,
            ))
            .child(ui::row("Network", summary.network.to_string(), cx))
            .child(ui::row("Action", summary.action.to_string(), cx))
            .child(ui::row(
                "Fee",
                format!(
                    "up to {} TARI from {}",
                    format_tari(summary.max_fee),
                    summary.fee_account
                ),
                cx,
            ))
            .child(ui::row("Valid until", format!("epoch {}", summary.max_epoch), cx))
            .child(ui::row(
                "Sealed by",
                ui::mono(summary.seal_public_key.to_string(), cx),
                cx,
            ))
            .when(!checked.proposal.memo.is_empty(), |s| {
                s.child(ui::row(
                    "Memo",
                    format!("{} (from the proposer, unverified)", checked.proposal.memo),
                    cx,
                ))
            });

        if let Ok(view) = &session.governance &&
            let Some(current) = session.info.current_epoch
        {
            let in_force = view.state.rate_at(current).map(format_bps);
            section = section.child(ui::row(
                "Rate now",
                in_force.unwrap_or_else(|| "release schedule (no council rate in force)".to_string()),
                cx,
            ));
        }

        let assessment = match &checked.verdict {
            Ok(assessment) => assessment,
            Err(e) => return section.child(ui::error(format!("Cannot sign: {e}"), cx)),
        };
        section = section.child(ui::row(
            "Council",
            format!(
                "{} of {} must sign; {} epochs left to submit",
                assessment.threshold,
                assessment.council_size,
                assessment
                    .epochs_remaining
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "?".into())
            ),
            cx,
        ));

        let council_keys = session.council_keys();
        if council_keys.is_empty() {
            return section.child(ui::error("This wallet holds no council key.", cx));
        }
        let key_buttons = council_keys.iter().map(|key| {
            let key_id = key.key_id;
            Button::new(SharedString::from(format!("sk-{}", format_key_id(&key_id))))
                .label(format!("{}  {}", format_key_id(&key_id), key.public_key))
                .selected(self.key == Some(key_id))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.key = Some(key_id);
                    cx.notify();
                }))
        });

        section = section
            .child(ui::row(
                "Sign with",
                v_flex().gap_1().items_start().children(key_buttons),
                cx,
            ))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        Button::new("request")
                            .primary()
                            .label("Request signature")
                            .loading(self.busy)
                            .disabled(self.key.is_none() || self.record.is_some())
                            .on_click(cx.listener(|this, _, window, cx| this.request(window, cx))),
                    )
                    .when_some(self.error.clone(), |el, e| el.child(ui::error(e, cx))),
            )
            .when_some(self.status.clone(), |s, status| s.child(ui::muted(status, cx)));

        if let (Some(output), Some(record)) = (&self.signature, &self.record) {
            let file_name = format!(
                "signature-{}-{}.txt",
                checked.summary.fingerprint.to_hex(),
                record.request_id
            );
            section = section.child(ui::armored_block("signature", output, file_name, cx));
        }
        section
    }
}
