//! Ootle Governance: council members co-sign governance transactions through their own wallet daemons and
//! pass proposals and signatures to each other by copy and paste.

mod collect;
mod config;
mod propose;
mod runtime;
mod session;
mod settings;
mod sign;
mod ui;

use gpui_kit::{
    AppContext,
    Bounds,
    Context,
    Entity,
    IntoElement,
    ParentElement,
    Render,
    Styled,
    TitlebarOptions,
    Window,
    WindowBounds,
    WindowOptions,
    component::{
        ActiveTheme,
        button::Button,
        h_flex,
        scroll::ScrollableElement,
        tab::{Tab, TabBar},
        v_flex,
    },
    div,
    prelude::*,
    px,
    size,
};
use ootle_gov_core::{Network, action::format_bps, council::CouncilState};

use crate::{
    collect::CollectView,
    config::Config,
    propose::ProposeView,
    session::{Connection, Status},
    settings::SettingsView,
    sign::SignView,
};

struct AppView {
    connection: Entity<Connection>,
    tab: usize,
    propose: Entity<ProposeView>,
    sign: Entity<SignView>,
    collect: Entity<CollectView>,
    settings: Entity<SettingsView>,
}

impl AppView {
    fn new(keyring_error: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let connection = cx.new(|_| Connection::new(Config::load()));
        cx.observe(&connection, |_, _, cx| cx.notify()).detach();
        let propose = cx.new(|cx| ProposeView::new(connection.clone(), window, cx));
        let sign = cx.new(|cx| SignView::new(connection.clone(), window, cx));
        let collect = cx.new(|cx| CollectView::new(connection.clone(), window, cx));
        let settings = cx.new(|cx| SettingsView::new(connection.clone(), keyring_error, window, cx));

        // Connect straight away when this network's key is already stored.
        let network = connection.read(cx).config.network;
        let tab = if config::load_api_key(network).is_some() {
            settings.update(cx, |settings, cx| settings.connect(cx));
            0
        } else {
            3
        };

        Self {
            connection,
            tab,
            propose,
            sign,
            collect,
            settings,
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let connection = self.connection.read(cx);
        let theme = cx.theme();
        let mut header = h_flex()
            .gap_4()
            .items_center()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(theme.border);

        let Some(session) = connection.session.clone() else {
            let text = match &connection.status {
                Status::Connecting => "Connecting…".to_string(),
                Status::Failed(e) => format!("Not connected: {e}"),
                _ => "Not connected".to_string(),
            };
            return header.child(div().text_color(theme.muted_foreground).child(text));
        };

        let network = session.network();
        let badge = div()
            .px_2()
            .rounded_md()
            .font_weight(gpui_kit::FontWeight::BOLD)
            .when(network == Network::MainNet, |d| {
                d.bg(theme.danger).text_color(theme.danger_foreground)
            })
            .when(network != Network::MainNet, |d| d.bg(theme.secondary))
            .child(network.to_string().to_uppercase());
        let epoch = session
            .info
            .current_epoch
            .map(|e| format!("epoch {e}"))
            .unwrap_or_else(|| "epoch unknown".to_string());
        let council = match &session.governance {
            Ok(view) => match &view.council {
                CouncilState::Seated(c) if view.state.retired_from.is_none() => {
                    format!("council {} of {}", c.threshold, c.members.len())
                },
                CouncilState::Unrecognised(_) => "council unreadable".to_string(),
                _ => "no council".to_string(),
            },
            Err(_) => "council unavailable".to_string(),
        };
        let rate = match (&session.governance, session.info.current_epoch) {
            (Ok(view), Some(epoch)) => view
                .state
                .rate_at(epoch)
                .map(|bps| format!("burn {}", format_bps(bps)))
                .unwrap_or_else(|| "burn per release schedule".to_string()),
            _ => String::new(),
        };
        header = header
            .child(badge)
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(session.wallet.endpoint()),
            )
            .child(div().child(epoch))
            .child(div().child(council))
            .child(div().child(rate))
            .child(div().flex_1())
            .child(
                Button::new("refresh")
                    .label("Refresh")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.connection.update(cx, |c, cx| c.refresh(window, cx));
                    })),
            );
        header
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.tab {
            0 => self.propose.clone().into_any_element(),
            1 => self.sign.clone().into_any_element(),
            2 => self.collect.clone().into_any_element(),
            _ => self.settings.clone().into_any_element(),
        };
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_header(cx))
            .child(
                div().px_4().pt_2().child(
                    TabBar::new("tabs")
                        .selected_index(self.tab)
                        .on_click(cx.listener(|this, index: &usize, _, cx| {
                            this.tab = *index;
                            cx.notify();
                        }))
                        .child(Tab::new().label("Propose"))
                        .child(Tab::new().label("Sign"))
                        .child(Tab::new().label("Collect & submit"))
                        .child(Tab::new().label("Settings")),
                ),
            )
            .child(
                div()
                    .id("body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(div().p_4().child(body)),
            )
    }
}

fn main() {
    let keyring_error = config::init_keyring()
        .err()
        .map(|e| format!("The OS keyring is unavailable ({e}); API keys last for this session only."));

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let bounds = Bounds::centered(None, size(px(1100.), px(820.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Ootle Governance".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| AppView::new(keyring_error.clone(), window, cx))
            })
            .expect("failed to open the window");
            cx.activate(true);
        });
}
