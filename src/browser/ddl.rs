use std::time::Duration;

use rune::{Cx, El, Key, KeyEvent, ScrollSpec, Tx, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant};

use super::{Browser, table::format};
use crate::{
    db::{self, Session, TableDdl},
    dialog::Dialog,
    icons, keys,
    theme::{RADIUS, Theme},
    ui::{self, Modal, Scroll, ScrollY},
    workspace::{TabId, Workspace},
};

pub const COPIED_FOR: f64 = 1.5;
pub const MAX_BODY_HEIGHT: f32 = 480.;
pub const LINE_STEP: f32 = 24.;
pub const TEXT_KEY: &str = "ddl-text";
pub const BODY_KEY: &str = "ddl-dialog-body";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DdlState {
    Loading,
    Loaded(String),
    Failed(String),
}

pub struct DdlView {
    pub schema: String,
    pub table: String,
    pub request: Option<u64>,
    pub state: DdlState,
    pub scroll: ScrollY,
    pub copied_until: Option<f64>,
}

impl DdlView {
    pub fn new(session: &Session, schema: &str, table: &str) -> Self {
        Self {
            schema: schema.to_string(),
            table: table.to_string(),
            request: Some(session.fetch_ddl(schema, table)),
            state: DdlState::Loading,
            scroll: ScrollY::default(),
            copied_until: None,
        }
    }

    pub fn awaits(&self, id: u64) -> bool {
        self.request == Some(id)
    }

    pub fn title(&self) -> String {
        if self.schema.is_empty() {
            db::render_text(&self.table)
        } else {
            format!(
                "{}.{}",
                db::render_text(&self.schema),
                db::render_text(&self.table)
            )
        }
    }

    pub fn sql(&self) -> Option<&str> {
        match &self.state {
            DdlState::Loaded(sql) => Some(sql),
            DdlState::Loading | DdlState::Failed(_) => None,
        }
    }

    pub fn error(&self) -> Option<&str> {
        match &self.state {
            DdlState::Failed(message) => Some(message),
            DdlState::Loading | DdlState::Loaded(_) => None,
        }
    }
}

pub fn show(s: &mut Workspace, tab: TabId, schema: &str, table: &str) {
    let Some(b) = s.browser_mut(tab) else {
        return;
    };
    b.ddl = Some(DdlView::new(&b.session, schema, table));
    s.dialog = Some(Dialog::Ddl(tab));
}

pub fn on_ddl(b: &mut Browser, id: u64, ddl: TableDdl) -> bool {
    match &mut b.ddl {
        Some(view) if view.awaits(id) => {
            view.request = None;
            view.state = DdlState::Loaded(ddl.sql);
            true
        }
        _ => false,
    }
}

pub fn on_error(b: &mut Browser, id: u64, message: &str) -> bool {
    match &mut b.ddl {
        Some(view) if view.awaits(id) => {
            view.request = None;
            view.state = DdlState::Failed(message.to_string());
            true
        }
        _ => false,
    }
}

pub fn poll(b: &mut Browser, tx: &mut Tx) -> bool {
    let Some(view) = &mut b.ddl else {
        return false;
    };
    match view.copied_until {
        Some(until) if until <= tx.clock() => {
            view.copied_until = None;
            true
        }
        _ => false,
    }
}

pub fn copy(b: &mut Browser, tx: &mut Tx) {
    let Some(view) = &mut b.ddl else {
        return;
    };
    let Some(sql) = view.sql().map(str::to_string) else {
        return;
    };
    view.copied_until = Some(tx.clock() + COPIED_FOR);
    tx.wake_after(Duration::from_secs_f64(COPIED_FOR));
    let message = format::copied(&sql);
    tx.copy(sql);
    b.set_status(message, false);
}

pub fn key(s: &mut Workspace, tab: TabId, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    let Some(view) = &b.ddl else {
        return false;
    };
    let mods = ev.mods;
    if mods.cmd && !mods.shift && !mods.alt && !mods.ctrl {
        if ev.key == Key::Command('c') {
            copy(b, tx);
            return true;
        }
        return false;
    }
    match Scroll::from_key(ev) {
        Some(how) => {
            view.scroll.scroll(tx, TEXT_KEY, how, LINE_STEP);
            true
        }
        None => false,
    }
}

pub fn dialog(s: &Workspace, tab: TabId, t: &Theme, cx: &mut Cx<Workspace>, layer: &mut Vec<El>) {
    let Some(b) = s.browser(tab) else {
        return;
    };
    let Some(view) = b.ddl.as_ref() else {
        return;
    };
    let height = cx
        .layout_with("workspace", |r| {
            (r[3] - 260.0).clamp(120.0, MAX_BODY_HEIGHT).round()
        })
        .unwrap_or(MAX_BODY_HEIGHT);
    let body = match &view.state {
        DdlState::Loaded(sql) => t.k.scroll_bar(
            view.scroll.watch(
                col()
                    .key(TEXT_KEY)
                    .grow(1.0)
                    .min_h(0.0)
                    .min_w(0.0)
                    .scroll(ScrollSpec {
                        stick_bottom: false,
                    })
                    .scroll_xy()
                    .child(el().padding(8.0).child(ui::sql_text(
                        &format::normalize(sql),
                        b.connection.kind,
                        t,
                    ))),
            ),
        ),
        DdlState::Failed(message) => row()
            .key("ddl-error")
            .padding(8.0)
            .gap(8.0)
            .items_start()
            .child(ui::icon_el(&icons::CIRCLE_X, 16.0, t.danger))
            .child(
                text(message.clone())
                    .font(t.text)
                    .color(t.danger)
                    .wrap()
                    .selectable(true)
                    .shrink()
                    .min_w(0.0),
            ),
        DdlState::Loading => row()
            .key("ddl-loading")
            .padding(8.0)
            .gap(8.0)
            .center_y()
            .child(ui::spinner(12.0, t.muted, t))
            .child(ui::label("Loading…", t.text, t.muted)),
    };
    let text_box = col()
        .key("ddl-box")
        .h(height)
        .w_full()
        .min_w(0.0)
        .radius(RADIUS)
        .border(1.0, t.border)
        .fill(t.secondary)
        .clip_rounded(RADIUS)
        .child(body);
    let copied = view.copied_until.is_some();
    let success = t.success;
    let copy_button = if copied {
        Button::new("Copied").label_with(move |font, ink| {
            row()
                .gap(6.0)
                .center_y()
                .child(icon(&icons::CHECK, 16.0).color(success))
                .child(text("Copied").font(font).color(ink))
        })
    } else {
        Button::new("Copy").icon(&icons::COPY)
    }
    .variant(ButtonVariant::Outline)
    .disabled(view.sql().is_none())
    .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
        if let Some(b) = s.browser_mut(tab) {
            copy(b, tx);
        }
    })
    .build(&t.k)
    .key("ddl-copy");
    let close_button = Button::new("Close")
        .variant(ButtonVariant::Submit)
        .on_press_tx(|s: &mut Workspace, tx: &mut Tx| s.close_dialog(tx))
        .build(&t.k)
        .key("ddl-close");
    let content = col()
        .key(BODY_KEY)
        .focusable(true)
        .autofocus()
        .no_focus_ring()
        .w_full()
        .min_w(0.0)
        .gap(12.0)
        .on_key(move |s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| {
            keys::route(s, ev, tx) || key(s, tab, ev, tx)
        })
        .child(text_box)
        .child(
            row()
                .w_full()
                .gap(8.0)
                .justify_end()
                .child(copy_button)
                .child(close_button),
        );
    let title = view.title();
    let header = row()
        .gap(8.0)
        .min_w(0.0)
        .center_y()
        .child(ui::icon_el(&icons::FILE_CODE, 16.0, t.muted))
        .child(
            text(title.clone())
                .font(t.semibold(t.text))
                .color(t.foreground)
                .truncate()
                .shrink()
                .min_w(0.0),
        )
        .child(ui::label("DDL", t.text_sm, t.muted));
    Modal::new("ddl", format!("{title} DDL"))
        .title(header)
        .width(760.0)
        .child(content)
        .build(t, layer, |s: &mut Workspace, tx: &mut Tx| {
            s.close_dialog(tx)
        });
}
