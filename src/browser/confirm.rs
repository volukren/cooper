#[cfg(test)]
mod ui_tests;

use rune::{El, Span, Tx, col, rich, row, text};
use rune_kit::{Button, ButtonVariant, Dialog as KitDialog};

use super::{console, table};
use crate::{
    db::{self, RowUpdate},
    dialog::Dialog,
    guard::Statement,
    icons,
    theme::{RADIUS, Theme},
    ui,
    workspace::{TabId, Workspace},
};

pub const MAX_LISTED: usize = 5;

const STATEMENT_LINES: usize = 3;

pub enum Pending {
    Script(String),
    Row { update: RowUpdate, row: String },
}

pub struct Confirm {
    pub tab: TabId,
    pub connection: String,
    pub pending: Pending,
    pub statements: Vec<Statement>,
}

impl Confirm {
    fn is_row(&self) -> bool {
        matches!(self.pending, Pending::Row { .. })
    }
}

fn confirm(s: &mut Workspace, tx: &mut Tx) {
    let Some(Dialog::Confirm(c)) = s.dialog.take() else {
        return;
    };
    s.refocus = true;
    let Some(b) = s.browser_mut(c.tab) else {
        return;
    };
    match c.pending {
        Pending::Script(sql) => console::run_confirmed(b, sql, tx),
        Pending::Row { update, .. } => table::save_confirmed(b, update),
    }
}

fn cancel(s: &mut Workspace) {
    let Some(Dialog::Confirm(c)) = s.dialog.take() else {
        return;
    };
    s.refocus = true;
    if let Some(b) = s.browser_mut(c.tab) {
        let message = if c.is_row() {
            "Cancelled: nothing was saved"
        } else {
            "Cancelled: nothing was run"
        };
        b.set_status(message, false);
    }
}

pub fn dialog(_s: &Workspace, c: &Confirm, t: &Theme, layer: &mut Vec<El>) {
    let (title, action, change) = match &c.pending {
        Pending::Row { update, row } => (
            "Save to production?",
            "Save to production",
            format!(
                "This change to {row} of {}.{} will be saved:",
                db::render_text(&update.schema),
                db::render_text(&update.table)
            ),
        ),
        Pending::Script(_) => (
            "Run on production?",
            "Run on production",
            "This script changes data or schema:".to_string(),
        ),
    };
    let quoted = format!("\"{}\"", db::render_text(&c.connection));
    let intro = format!("{quoted} is tagged Production. {change}");
    let listed = c
        .statements
        .iter()
        .take(MAX_LISTED)
        .enumerate()
        .map(|(i, stmt)| statement_row(i, stmt, t));
    let more = c.statements.len().saturating_sub(MAX_LISTED);
    let unbounded = c.statements.iter().any(|stmt| stmt.unbounded);
    let body = col()
        .w_full()
        .px(16.0)
        .gap(10.0)
        .child(
            rich(intro, vec![Span::new(0, quoted.len()).bold()])
                .key("confirm-intro")
                .font(t.text)
                .color(t.foreground)
                .wrap(),
        )
        .child(
            col()
                .key("confirm-statements")
                .w_full()
                .gap(6.0)
                .padding(8.0)
                .radius(RADIUS)
                .border(1.0, t.border)
                .fill(t.secondary)
                .children(listed)
                .children((more > 0).then(|| {
                    ui::label(
                        format!("…and {} more", ui::group(more as u64)),
                        t.text_sm,
                        t.muted,
                    )
                    .key("confirm-more")
                })),
        )
        .children(unbounded.then(|| {
            row()
                .key("confirm-unbounded")
                .w_full()
                .gap(8.0)
                .padding(10.0)
                .center_y()
                .radius(RADIUS)
                .border(1.0, t.danger)
                .fill(t.k.alpha(t.danger, 0.08))
                .child(ui::icon_el(&icons::TRIANGLE_ALERT, 14.0, t.danger))
                .child(
                    ui::label(
                        "UPDATE or DELETE without WHERE changes every row of its table.",
                        t.semibold(t.text_sm),
                        t.danger,
                    )
                    .wrap()
                    .grow(1.0)
                    .min_w(0.0),
                )
        }));
    let footer = row()
        .w_full()
        .padding(16.0)
        .gap(8.0)
        .justify_end()
        .child(
            Button::new("Cancel")
                .variant(ButtonVariant::Outline)
                .on_press(|s: &mut Workspace| cancel(s))
                .build(&t.k)
                .key("confirm-cancel")
                .autofocus(),
        )
        .child(
            Button::new(action)
                .icon(&icons::TRIANGLE_ALERT)
                .variant(ButtonVariant::Danger)
                .on_press_tx(confirm)
                .build(&t.k)
                .key("confirm-run"),
        );
    KitDialog::new("confirm-write", title)
        .width(560.0)
        .fit()
        .open(true)
        .on_close(|s: &mut Workspace| cancel(s))
        .child(body)
        .child(footer)
        .build(&t.modal, layer);
}

fn statement_row(i: usize, stmt: &Statement, t: &Theme) -> El {
    row()
        .key(format!("confirm-statement-{i}"))
        .label(stmt.text.clone())
        .w_full()
        .gap(8.0)
        .items_start()
        .child(
            row()
                .h(18.0)
                .px(5.0)
                .center()
                .radius(3.0)
                .fill(t.k.alpha(t.danger, 0.12))
                .child(
                    text(stmt.verb.clone())
                        .key(format!("confirm-statement-{i}-verb"))
                        .font(t.semibold(t.mono_sm))
                        .color(t.danger),
                ),
        )
        .child(
            text(stmt.text.clone())
                .key(format!("confirm-statement-{i}-text"))
                .font(t.mono_sm)
                .color(t.foreground)
                .wrap_anywhere()
                .max_lines(STATEMENT_LINES)
                .help_when_truncated(stmt.text.clone())
                .grow(1.0)
                .min_w(0.0),
        )
        .children(stmt.unbounded.then(|| {
            ui::label("no WHERE", t.semibold(t.text_xs), t.danger)
                .key(format!("confirm-statement-{i}-unbounded"))
        }))
}
