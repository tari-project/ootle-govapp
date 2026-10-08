//! Collecting co-signatures on your proposals and submitting them.

use std::{sync::Arc, time::Duration};

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
        ActiveTheme,
        Disableable,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Textarea, TextareaState},
        v_flex,
    },
    div,
    prelude::*,
    px,
};
use ootle_gov_core::amount::format_tari;
use ootle_gov_wallet::{SubmissionStatus, key_ref::format_key_id, store::ProposalRecord};

use crate::{
    runtime::io,
    session::{Connection, Session},
    ui,
};

pub struct CollectView {
    connection: Entity<Connection>,
    records: Vec<ProposalRecord>,
    selected: Option<String>,
    proposal_text: Entity<TextareaState>,
    paste: Entity<TextareaState>,
    error: Option<String>,
    status: Option<String>,
    seen_revision: Option<u64>,
    _poll: Option<Task<()>>,
}

impl CollectView {
    pub fn new(connection: Entity<Connection>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let proposal_text = ui::new_textarea(10, "", window, cx);
        let paste = ui::new_textarea(
            6,
            "Paste a member's -----BEGIN OOTLE GOVERNANCE SIGNATURE----- block",
            window,
            cx,
        );
        cx.observe_in(&connection, window, |this, connection, window, cx| {
            let revision = connection.read(cx).records_revision;
            if this.seen_revision != Some(revision) {
                this.seen_revision = Some(revision);
                this.reload(window, cx);
            }
            cx.notify();
        })
        .detach();
        Self {
            connection,
            records: Vec::new(),
            selected: None,
            proposal_text,
            paste,
            error: None,
            status: None,
            seen_revision: None,
            _poll: None,
        }
    }

    fn session(&self, cx: &Context<Self>) -> Option<Arc<Session>> {
        self.connection.read(cx).session.clone()
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.records = self
            .session(cx)
            .and_then(|s| s.store.proposals().ok())
            .unwrap_or_default();
        let selected = self
            .selected
            .clone()
            .filter(|f| {
                self.records
                    .iter()
                    .any(|r| r.fingerprint_hex().ok().as_ref() == Some(f))
            })
            .or_else(|| self.records.first().and_then(|r| r.fingerprint_hex().ok()));
        if selected != self.selected {
            self.select(selected, window, cx);
        }
    }

    fn select(&mut self, fingerprint: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = fingerprint;
        self.error = None;
        self.status = None;
        self._poll = None;
        let text = self.current().map(|r| r.proposal.clone()).unwrap_or_default();
        ui::set_text(&self.proposal_text, text, window, cx);
        if let Some(record) = self.current().cloned() &&
            let Some(request_id) = record.transaction_request_id &&
            record.outcome.is_none()
        {
            self.follow(request_id, window, cx);
        }
        cx.notify();
    }

    fn current(&self) -> Option<&ProposalRecord> {
        let selected = self.selected.as_ref()?;
        self.records
            .iter()
            .find(|r| r.fingerprint_hex().ok().as_ref() == Some(selected))
    }

    fn update_current(&mut self, f: impl FnOnce(&mut ProposalRecord), cx: &mut Context<Self>) -> Result<(), String> {
        let session = self.session(cx).ok_or("Not connected")?;
        let selected = self.selected.clone().ok_or("No proposal selected")?;
        let record = self
            .records
            .iter_mut()
            .find(|r| r.fingerprint_hex().ok().as_ref() == Some(&selected))
            .ok_or("No proposal selected")?;
        f(record);
        session.store.save_proposal(record).map_err(|e| e.to_string())
    }

    fn add_signature(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        let Some(session) = self.session(cx) else {
            return;
        };
        let text = self.paste.read(cx).value().to_string();
        let result = (|| {
            let council = session.council().map_err(|e| e.to_string())?;
            let mut record = self.current().cloned().ok_or("No proposal selected")?;
            let summary = record
                .decode_proposal()
                .and_then(|p| Ok(p.inspect()?))
                .map_err(|e| e.to_string())?;
            let signature = record
                .add_signature(&text, &summary, council)
                .map_err(|e| e.to_string())?;
            Ok::<_, String>((record, signature))
        })();
        match result {
            Ok((record, signature)) => {
                let signatures = record.signatures.clone();
                match self.update_current(|r| r.signatures = signatures, cx) {
                    Ok(()) => {
                        self.status = Some(format!("Added the signature from {}", signature.public_key()));
                        ui::set_text(&self.paste, String::new(), window, cx);
                    },
                    Err(e) => self.error = Some(e),
                }
            },
            Err(e) => self.error = Some(e),
        }
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        let (Some(session), Some(record)) = (self.session(cx), self.current().cloned()) else {
            return;
        };
        if let Some(request_id) = record.transaction_request_id {
            self.follow(request_id, window, cx);
            return;
        }
        self.status = Some("Creating the transaction request…".into());
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let wallet = session.wallet.clone();
            let result = io(async move { wallet.submit(&record).await }).await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(request_id) => match this.update_current(|r| r.transaction_request_id = Some(request_id), cx) {
                        Ok(()) => this.follow(request_id, window, cx),
                        Err(e) => this.error = Some(e),
                    },
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Polls a transaction request: waits for a person to approve it in walletd, broadcasts it, and waits for
    /// the result.
    fn follow(&mut self, request_id: i32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session(cx) else {
            return;
        };
        self._poll = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let wallet = session.wallet.clone();
                let status = io(async move { wallet.submission_status(request_id).await }).await;
                let transaction_id = match status {
                    Ok(SubmissionStatus::AwaitingApproval) => {
                        set_status(
                            &this,
                            cx,
                            format!("Approve transaction request #{request_id} in the walletd UI…"),
                        );
                        None
                    },
                    Ok(SubmissionStatus::Submitting) => {
                        set_status(&this, cx, "Submitting…".into());
                        None
                    },
                    Ok(SubmissionStatus::Approved) => {
                        set_status(&this, cx, "Approved. Broadcasting…".into());
                        let wallet = session.wallet.clone();
                        match io(async move { wallet.broadcast(request_id).await }).await {
                            Ok(id) => Some(id),
                            Err(e) => {
                                set_status(&this, cx, format!("Broadcast failed: {e}"));
                                None
                            },
                        }
                    },
                    Ok(SubmissionStatus::Submitted(id)) => Some(id),
                    Ok(SubmissionStatus::Rejected) => {
                        set_status(&this, cx, "The transaction request was rejected in walletd.".into());
                        return;
                    },
                    Ok(SubmissionStatus::Expired) => {
                        set_status(
                            &this,
                            cx,
                            "The transaction request expired before it was approved.".into(),
                        );
                        return;
                    },
                    Err(e) => {
                        set_status(&this, cx, format!("Could not check the transaction request: {e}"));
                        None
                    },
                };

                if let Some(transaction_id) = transaction_id {
                    let _ = this.update(cx, |this, cx| {
                        let _ = this.update_current(|r| r.transaction_id = Some(transaction_id), cx);
                        this.status = Some(format!("Submitted {transaction_id}. Waiting for the result…"));
                        cx.notify();
                    });
                    loop {
                        let wallet = session.wallet.clone();
                        match io(async move { wallet.wait_result(transaction_id, 30).await }).await {
                            Ok(outcome) if !outcome.timed_out => {
                                let _ = this.update(cx, |this, cx| {
                                    let described = outcome.describe();
                                    let _ = this.update_current(|r| r.outcome = Some(described.clone()), cx);
                                    this.status = Some(described);
                                    cx.notify();
                                });
                                return;
                            },
                            Ok(_) => {},
                            Err(e) => set_status(&this, cx, format!("Could not read the result: {e}")),
                        }
                    }
                }
                cx.background_executor().timer(Duration::from_secs(3)).await;
            }
        }));
    }
}

fn set_status(this: &gpui_kit::WeakEntity<CollectView>, cx: &mut gpui_kit::AsyncWindowContext, status: String) {
    let _ = this.update(cx, |this, cx| {
        this.status = Some(status);
        cx.notify();
    });
}

impl Render for CollectView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(session) = self.session(cx) else {
            return h_flex().child(ui::muted("Connect to a wallet daemon in Settings.", cx));
        };
        let council = session.council().ok().cloned();

        let theme = cx.theme().clone();
        let list = self.records.iter().map(|record| {
            let fingerprint = record.fingerprint_hex().unwrap_or_default();
            let (title, action, progress) = match record
                .decode_proposal()
                .and_then(|p| Ok((p.fingerprint(), p.inspect()?)))
            {
                Ok((fp, summary)) => {
                    let progress = council
                        .as_ref()
                        .and_then(|c| {
                            record
                                .approvals(&summary, c)
                                .ok()
                                .map(|n| format!("{n} of {} approvals", c.threshold))
                        })
                        .unwrap_or_else(|| "approvals unknown".into());
                    let progress = match &record.outcome {
                        Some(outcome) => outcome.clone(),
                        None if record.transaction_request_id.is_some() => format!("{progress}, submitted"),
                        None => progress,
                    };
                    (fp.to_string(), summary.action.to_string(), progress)
                },
                Err(_) => ("unreadable".to_string(), String::new(), String::new()),
            };
            let selected = self.selected.as_ref() == Some(&fingerprint);
            div()
                .id(SharedString::from(format!("p-{fingerprint}")))
                .p_2()
                .rounded_md()
                .cursor_pointer()
                .when(selected, |d| d.bg(theme.list_active))
                .hover(|d| d.bg(theme.list_hover))
                .child(ui::mono(title, cx))
                .child(div().text_sm().child(action))
                .child(ui::muted(progress, cx))
                .on_click(cx.listener(move |this, _, window, cx| this.select(Some(fingerprint.clone()), window, cx)))
        });

        let sidebar = v_flex()
            .w(px(280.))
            .flex_shrink_0()
            .gap_1()
            .child(ui::muted("Your proposals", cx))
            .children(list)
            .when(self.records.is_empty(), |s| {
                s.child(ui::muted("None yet. Create one in Propose.", cx))
            });

        let detail = match self.current().cloned() {
            None => v_flex(),
            Some(record) => self.render_detail(&record, &session, council.as_ref(), cx),
        };

        h_flex()
            .gap_4()
            .items_start()
            .child(sidebar)
            .child(detail.flex_1().min_w_0())
    }
}

impl CollectView {
    fn render_detail(
        &self,
        record: &ProposalRecord,
        session: &Session,
        council: Option<&ootle_gov_core::council::Council>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let summary = match record.decode_proposal().and_then(|p| Ok(p.inspect()?)) {
            Ok(summary) => summary,
            Err(e) => return v_flex().child(ui::error(e.to_string(), cx)),
        };
        let fingerprint = summary.fingerprint.to_string();

        let overview = ui::section(summary.action.to_string(), cx)
            .child(ui::row("Fingerprint", ui::mono(fingerprint.clone(), cx).text_xl(), cx))
            .child(ui::row("Network", summary.network.to_string(), cx))
            .child(ui::row(
                "Fee",
                format!(
                    "up to {} TARI from {}",
                    format_tari(summary.max_fee),
                    summary.fee_account
                ),
                cx,
            ))
            .child(ui::row(
                "Valid until",
                match session.info.current_epoch {
                    Some(current) if current > summary.max_epoch => {
                        format!("epoch {} (expired; the network is at {current})", summary.max_epoch)
                    },
                    Some(current) => format!(
                        "epoch {} ({} epochs left)",
                        summary.max_epoch,
                        summary.max_epoch - current
                    ),
                    None => format!("epoch {}", summary.max_epoch),
                },
                cx,
            ))
            .child(ui::row(
                "Sealed by",
                format!("{} ({})", summary.seal_public_key, format_key_id(&record.seal_key_id)),
                cx,
            ));

        let signatures = record.decode_signatures().unwrap_or_default();
        let is_member = |key| council.is_some_and(|c| c.is_member(key));
        let mut signers = v_flex().gap_1();
        if summary.seal_signer_authorized {
            signers = signers.child(signer_line(
                &summary.seal_public_key,
                "seal key (this wallet)",
                is_member(&summary.seal_public_key),
                cx,
            ));
        }
        if let (Some(key), Some(key_id)) = (&record.council_public_key, &record.council_key_id) {
            signers = signers.child(signer_line(
                key,
                &format!("{} (this wallet, signed at submission)", format_key_id(key_id)),
                is_member(key),
                cx,
            ));
        }
        for signature in &signatures {
            signers = signers.child(signer_line(
                signature.public_key(),
                "co-signature",
                is_member(signature.public_key()),
                cx,
            ));
        }

        let (approvals, threshold) = match council {
            Some(c) => (
                record.approvals(&summary, c).unwrap_or_default(),
                usize::from(c.threshold),
            ),
            None => (0, usize::MAX),
        };
        let ready = approvals >= threshold;
        let progress = match council {
            Some(c) => format!("{approvals} of {} approvals", c.threshold),
            None => format!(
                "Council unavailable: {}",
                session.council().err().map(|e| e.to_string()).unwrap_or_default()
            ),
        };
        let submitted = record.transaction_request_id.is_some();

        let signatures_section = ui::section("Signatures", cx)
            .child(if ready {
                ui::success(progress, cx)
            } else {
                div_text(progress)
            })
            .child(signers)
            .when(!submitted, |s| {
                s.child(Textarea::new(&self.paste)).child(
                    h_flex().child(
                        Button::new("add-sig")
                            .label("Add signature")
                            .on_click(cx.listener(|this, _, window, cx| this.add_signature(window, cx))),
                    ),
                )
            });

        let submit_section = ui::section("Submit", cx)
            .child(ui::muted(
                "Submitting hands the assembled transaction to walletd as a transaction request. Approve it in the \
                 walletd UI; this app then broadcasts it.",
                cx,
            ))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        Button::new("submit")
                            .primary()
                            .label(if submitted { "Resume" } else { "Submit" })
                            .disabled(!ready || record.outcome.is_some())
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    )
                    .when_some(record.transaction_request_id, |el, id| {
                        el.child(ui::muted(format!("Transaction request #{id}"), cx))
                    }),
            )
            .when_some(record.transaction_id, |s, id| {
                s.child(ui::row("Transaction", ui::mono(id.to_string(), cx), cx))
            })
            .when_some(record.outcome.clone(), |s, outcome| {
                s.child(ui::row("Result", outcome, cx))
            });

        let file_name = format!("proposal-{}.txt", summary.fingerprint.to_hex());
        v_flex()
            .gap_4()
            .child(overview.child(ui::armored_block(
                "collect-proposal",
                &self.proposal_text,
                file_name,
                cx,
            )))
            .child(signatures_section)
            .child(submit_section)
            .when_some(self.status.clone(), |el, s| el.child(ui::muted(s, cx)))
            .when_some(self.error.clone(), |el, e| el.child(ui::error(e, cx)))
    }
}

fn signer_line(
    key: &tari_template_lib_types::crypto::RistrettoPublicKeyBytes,
    role: &str,
    member: bool,
    cx: &gpui_kit::App,
) -> gpui_kit::Div {
    let line = format!("{key}  {role}");
    if member {
        ui::mono(format!("✓ {line}"), cx)
    } else {
        ui::mono(format!("✗ {line} (not on the council)"), cx)
            .text_color(gpui_kit::component::ActiveTheme::theme(cx).danger)
    }
}

fn div_text(text: String) -> gpui_kit::Div {
    gpui_kit::div().child(text)
}
