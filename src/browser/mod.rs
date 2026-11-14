pub mod confirm;
pub mod console;
pub mod ddl;
pub mod table;
pub mod tree;

#[cfg(test)]
mod ui_tests;

use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Duration};

use rune::{Cx, El, Key, KeyEvent, Mods, ScrollPos, Tx, Waker, col, el, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize};

use crate::{
    connection::Connection,
    db::{self, Response, Session},
    dialog::Dialog,
    icons,
    keys::{self, Command, Ctx},
    theme::Theme,
    ui::{self, Split},
    workspace::{TabId, Workspace},
};

use self::{console::Console, ddl::DdlView, table::TableView, tree::SchemaTree};

const PROD_BANNER: &str =
    "Production — scripts that change data or schema and row edits ask for confirmation";

pub enum Polled {
    Idle,
    Changed,
    Lost(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Main {
    Table,
    Console,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Tree,
    Main,
}

pub fn cycle_pane(current: Option<Pane>, forward: bool) -> Pane {
    const ORDER: [Pane; 2] = [Pane::Tree, Pane::Main];
    let index = current
        .and_then(|pane| ORDER.iter().position(|p| *p == pane))
        .unwrap_or(0);
    let next = if forward {
        index + 1
    } else {
        index + ORDER.len() - 1
    };
    ORDER[next % ORDER.len()]
}

pub fn running_status(seconds: u64, cancelling: bool) -> String {
    if cancelling {
        "Cancelling the script…".to_string()
    } else {
        format!(
            "Running the script… {}s · Esc or ⌘. cancels it",
            ui::group(seconds)
        )
    }
}

#[derive(Clone, Default)]
pub struct Scrolls(Rc<RefCell<HashMap<String, [f32; 2]>>>);

impl Scrolls {
    pub fn keep(&self, key: impl Into<String>, viewport: El) -> El {
        let (kept, key) = (self.0.clone(), key.into());
        viewport.chain_scroll_offset(move |at: ScrollPos| {
            kept.borrow_mut().insert(key.clone(), at.offset);
        })
    }

    pub fn get(&self, key: &str) -> Option<[f32; 2]> {
        self.0.borrow().get(key).copied()
    }

    fn restore(&self, tx: &mut Tx) {
        for (key, [x, y]) in self.0.borrow().iter() {
            tx.scroll_to_instant(key.clone(), *x, *y);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Fields {
    tree: bool,
    sql: bool,
    row: Option<usize>,
}

impl Fields {
    fn of(b: &Browser) -> Self {
        Self {
            tree: b.tree.filter.focused,
            sql: b.console.sql.focused,
            row: b.table.draft.as_ref().and_then(|draft| draft.focused()),
        }
    }

    fn entered(self, before: Self) -> bool {
        (self.tree && !before.tree)
            || (self.sql && !before.sql)
            || (self.row.is_some() && self.row != before.row)
    }
}

pub struct Browser {
    pub id: TabId,
    pub connection: Connection,
    pub server_info: Option<String>,
    pub session: Rc<Session>,
    pub tree: SchemaTree,
    pub table: TableView,
    pub console: Console,
    pub ddl: Option<DdlView>,
    pub main: Main,
    pub table_stale: bool,
    pub pane: Pane,
    pub focused: Option<Pane>,
    pub status: Option<(String, bool)>,
    pub sidebar: Split,
    pub drains: usize,
    pub scrolls: Scrolls,
    status_gen: u64,
    status_seen: u64,
    fields_seen: Option<Fields>,
}

impl Browser {
    pub fn new(id: TabId, connection: Connection, session: Session, draft: Option<String>) -> Self {
        session.drop_in_background();
        let mut browser = Self {
            id,
            server_info: None,
            session: Rc::new(session),
            tree: SchemaTree::new(),
            table: TableView::new(),
            console: Console::new(draft),
            ddl: None,
            main: Main::Table,
            table_stale: false,
            pane: Pane::Tree,
            focused: None,
            status: None,
            sidebar: Split::new(280.0, 180.0, 560.0),
            drains: 0,
            scrolls: Scrolls::default(),
            status_gen: 0,
            status_seen: 0,
            fields_seen: None,
            connection,
        };
        tree::load(&mut browser);
        browser
    }

    pub fn id(&self) -> TabId {
        self.id
    }

    pub fn set_server_info(&mut self, server_info: String) {
        self.server_info = (!server_info.trim().is_empty()).then_some(server_info);
    }

    pub fn set_waker(&self, waker: Waker) {
        self.session.set_waker(move || waker());
    }

    pub fn drop_session_in_foreground(&self) {
        self.session.drop_in_foreground();
    }

    pub fn draft(&self) -> Option<String> {
        Some(self.console.draft())
    }

    pub fn title_context(&self) -> Option<String> {
        match self.main {
            Main::Table => self.table.title_context(),
            Main::Console => Some(self.console.title_context()),
        }
    }

    pub fn set_status(&mut self, text: impl Into<String>, error: bool) {
        let text = text.into();
        self.status = (!text.is_empty()).then_some((text, error));
        self.status_gen += 1;
    }

    pub fn clear_status(&mut self) {
        self.status = None;
        self.status_gen += 1;
    }

    pub fn clear_hint(&mut self) {
        if self.status.as_ref().is_some_and(|(_, error)| !error) {
            self.clear_status();
        }
    }

    pub fn close_menus(&mut self) -> bool {
        self.tree.menu.take().is_some()
    }

    pub fn activate(&self, tx: &mut Tx) {
        tx.wake_after(Duration::ZERO);
        self.scrolls.restore(tx);
        let [x, y] = self.table.cells.scroll.get().offset;
        tx.scroll_to_instant(format!("{}-grid-scroll", table::PREFIX), x, y);
        tx.scroll_to_instant(table::RECORD_SCROLL, 0.0, self.table.record_scroll.offset());
        tx.scroll_to_instant(console::RECORD, 0.0, self.console.record_scroll.offset());
    }

    pub fn focus(&mut self, tx: &mut Tx) {
        if self.pane == Pane::Main && self.main == Main::Console {
            return console::focus_last_area(self, tx);
        }
        self.focus_pane(self.pane, tx);
    }

    pub fn focus_pane(&mut self, pane: Pane, tx: &mut Tx) {
        self.pane = pane;
        match pane {
            Pane::Tree => tree::focus(self, tx),
            Pane::Main => match self.main {
                Main::Table => table::focus(self, tx),
                Main::Console => console::focus(self, tx),
            },
        }
    }

    pub fn focused_pane(&self, focused: Option<&str>) -> Option<Pane> {
        let mut out = Vec::new();
        tree::contexts(self, focused, &mut out);
        if !out.is_empty() {
            return Some(Pane::Tree);
        }
        match self.main {
            Main::Table => table::contexts(self, focused, &mut out),
            Main::Console => console::contexts(self, focused, &mut out),
        }
        (!out.is_empty()).then_some(Pane::Main)
    }

    pub fn cycle_focus(&mut self, forward: bool, focused: Option<&str>, tx: &mut Tx) {
        let current = self.focused_pane(focused);
        let console = self.main == Main::Console;
        if console && current == Some(Pane::Main) && console::focus_step(self, forward, tx) {
            return;
        }
        match cycle_pane(current, forward) {
            Pane::Main if console && !forward => {
                self.pane = Pane::Main;
                console::focus_last(self, tx);
            }
            pane => self.focus_pane(pane, tx),
        }
    }

    pub fn open_table(&mut self, schema: &str, table: &str, kind: db::TableKind) {
        if self.table.current() != Some((schema.to_string(), table.to_string())) {
            self.table_stale = false;
        }
        table::open(self, schema, table, kind);
        self.set_main(Main::Table);
    }

    pub fn contexts(&self, focused: Option<&str>, out: &mut Vec<Ctx>) {
        let before = out.len();
        tree::contexts(self, focused, out);
        match self.main {
            Main::Table => table::contexts(self, focused, out),
            Main::Console => console::contexts(self, focused, out),
        }
        let input = out[before..]
            .iter()
            .any(|ctx| matches!(ctx, Ctx::RowEditorInput | Ctx::SqlEditorInput))
            || self.tree.filter.focused;
        if input {
            out.push(Ctx::BrowserInput);
        }
        out.push(Ctx::Browser);
    }

    pub fn show(&mut self, main: Main, tx: &mut Tx) {
        self.set_main(main);
        self.focus_pane(Pane::Main, tx);
    }

    pub fn set_main(&mut self, main: Main) {
        self.main = main;
        if main == Main::Table && std::mem::take(&mut self.table_stale) {
            table::reload(self);
        }
    }
}

pub fn smoke_sql() -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var("COOPER_SMOKE_SQL").ok()
    } else {
        None
    }
}

pub fn smoke(s: &mut Workspace, tab: TabId, tx: &mut Tx) {
    if cfg!(debug_assertions) {
        let table = std::env::var("COOPER_SMOKE_TABLE").ok();
        smoke_with(s, tab, table.as_deref(), smoke_sql().is_some(), tx);
    }
}

pub fn smoke_with(s: &mut Workspace, tab: TabId, table: Option<&str>, run_sql: bool, tx: &mut Tx) {
    let Some(b) = s.browser_mut(tab) else {
        return;
    };
    if let Some((schema, name)) = table.and_then(|spec| spec.split_once('.')) {
        b.open_table(schema, name, db::TableKind::Table);
    }
    if run_sql {
        b.show(Main::Console, tx);
        console::run(s, tab, Command::RunScript, tx);
    }
}

pub fn before_key(s: &mut Workspace, tab: TabId, ev: &KeyEvent, _tx: &mut Tx) -> bool {
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    if keys::is_modifier(ev) {
        return false;
    }
    b.clear_status();
    ev.key == Key::Escape && ev.mods == Mods::default() && console::cancel(b)
}

pub fn run(s: &mut Workspace, tab: TabId, cmd: Command, tx: &mut Tx) -> bool {
    let done = dispatch(s, tab, cmd, tx);
    if done {
        tx.wake_after(Duration::ZERO);
        if let Some(b) = s.browser_mut(tab) {
            b.close_menus();
        }
    }
    done
}

fn dispatch(s: &mut Workspace, tab: TabId, cmd: Command, tx: &mut Tx) -> bool {
    use Command::*;
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    match cmd {
        Refresh => {
            tree::refresh(b);
            match b.main {
                Main::Table => table::reload(b),
                Main::Console => b.table_stale = b.table.current().is_some(),
            }
            true
        }
        OpenConsole => {
            b.show(Main::Console, tx);
            true
        }
        FocusTree => {
            b.focus_pane(Pane::Tree, tx);
            true
        }
        FocusNextPane | FocusPrevPane => {
            let focused = tx.focused().map(str::to_owned);
            b.cycle_focus(cmd == FocusNextPane, focused.as_deref(), tx);
            true
        }
        CancelScript => console::cancel(b),
        ShowDdl => tree::run(s, tab, cmd, tx),
        RunScript | FocusResult | FocusEditor => console::run(s, tab, cmd, tx),
        NextPage | PrevPage | FirstPage | LastPage | EditRow | SaveRow | CancelEdit | NextField
        | PrevField => table::run(s, tab, cmd, tx),
        ToggleRecord | CloseRecord | NextRecord | PrevRecord | CopyCell | CopyRow => match b.main {
            Main::Table => table::run(s, tab, cmd, tx),
            Main::Console => console::run(s, tab, cmd, tx),
        },
        _ => false,
    }
}

pub fn poll(s: &mut Workspace, tab: TabId, tx: &mut Tx) -> Polled {
    let active = s.active_id() == Some(tab);
    let Some((b, dialog)) = s.browser_and_dialog(tab) else {
        return Polled::Idle;
    };
    let mut changed = false;
    let focused = if active && dialog.is_none() {
        b.focused_pane(tx.focused())
    } else {
        None
    };
    if focused != b.focused {
        b.focused = focused;
        changed = true;
    }
    if let Some(pane) = focused
        && pane != b.pane
    {
        b.pane = pane;
        changed = true;
    }
    if (!active || dialog.is_some()) && b.close_menus() {
        changed = true;
    }
    let fields = (active && dialog.is_none()).then(|| Fields::of(b));
    if let (Some(now), Some(before)) = (fields, b.fields_seen)
        && now.entered(before)
        && b.status_gen == b.status_seen
        && b.status.is_some()
    {
        b.clear_status();
        changed = true;
    }
    b.fields_seen = fields;
    if b.ddl.is_some() && !matches!(dialog, Some(Dialog::Ddl(open)) if *open == tab) {
        b.ddl = None;
    }
    let mut drained = false;
    while let Some(response) = b.session.try_recv() {
        drained = true;
        match response {
            Response::Connected { .. } => {}
            Response::Schemas { id, schemas } => {
                tree::on_schemas(b, id, schemas);
            }
            Response::Tables { id, schema, tables } => {
                tree::on_tables(b, id, schema, tables);
            }
            Response::Page { id, page } => {
                table::on_page(b, id, page);
            }
            Response::Count { id, count } => {
                table::on_count(b, id, count);
            }
            Response::Ddl { id, ddl } => {
                if b.ddl.as_ref().is_some_and(|view| view.awaits(id)) {
                    ddl::on_ddl(b, id, ddl);
                } else {
                    table::on_ddl(b, id, ddl);
                }
            }
            Response::Script { id, result } => {
                console::on_script(b, id, result);
            }
            Response::RowUpdated { id } => {
                table::on_row_updated(b, id);
            }
            Response::Error {
                id: Some(id),
                message,
            } => {
                let _handled = tree::on_error(b, id, &message)
                    || table::on_error(b, id, &message)
                    || ddl::on_error(b, id, &message)
                    || console::on_error(b, id, &message);
            }
            Response::Error { id: None, message } => return Polled::Lost(message),
        }
    }
    if drained {
        b.drains += 1;
        changed = true;
    }
    if active {
        changed |= tree::poll(b, tx);
        changed |= table::poll(b, tx);
        changed |= console::poll(b, tx);
    }
    changed |= ddl::poll(b, tx);
    b.status_seen = b.status_gen;
    if changed {
        Polled::Changed
    } else {
        Polled::Idle
    }
}

pub fn view(
    s: &Workspace,
    tab: TabId,
    t: &Theme,
    cx: &mut Cx<Workspace>,
    layer: &mut Vec<El>,
) -> El {
    let Some(b) = s.browser(tab) else {
        return el();
    };
    let sidebar = col()
        .key("browser-sidebar")
        .w(b.sidebar.size)
        .h_full()
        .min_h(0.0)
        .padding(4.0)
        .pr(0.0)
        .child(tree::view(s, b, t, cx, layer));
    let main = match b.main {
        Main::Table => table::view(s, b, t, cx, layer),
        Main::Console => console::view(s, b, t, cx, layer),
    };
    let main = col()
        .key("browser-main")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .padding(4.0)
        .gap(4.0)
        .child(row().child(main_switch(b, t)))
        .child(col().grow(1.0).min_w(0.0).min_h(0.0).child(main));
    let browser = col()
        .key("browser")
        .grow(1.0)
        .w_full()
        .min_h(0.0)
        .fill(t.background)
        .on_background_press(move |s: &mut Workspace| {
            if let Some(b) = s.browser_mut(tab) {
                b.clear_status();
            }
        })
        .children(prod_banner(b, t))
        .child(
            row()
                .key("browser-split")
                .grow(1.0)
                .min_h(0.0)
                .w_full()
                .child(sidebar)
                .child(ui::h_handle(
                    "browser-split-handle",
                    t,
                    move |s: &mut Workspace| s.browser_mut(tab).map(|b| &mut b.sidebar),
                ))
                .child(main),
        )
        .child(status_bar(b, t));
    col()
        .key(format!("browser-{}", tab.0))
        .grow(1.0)
        .w_full()
        .min_h(0.0)
        .child(browser)
}

fn prod_banner(b: &Browser, t: &Theme) -> Option<El> {
    b.connection.asks_to_confirm().then(|| {
        row()
            .key("prod-banner")
            .label(PROD_BANNER)
            .h(26.0)
            .w_full()
            .px(12.0)
            .gap(8.0)
            .center_y()
            .fill(t.k.alpha(t.danger, 0.12))
            .border_sides([0.0, 0.0, 1.0, 0.0], t.k.alpha(t.danger, 0.45))
            .child(ui::icon_el(&icons::TRIANGLE_ALERT, 14.0, t.danger))
            .child(ui::label(PROD_BANNER, t.semibold(t.text_sm), t.danger).truncate())
    })
}

fn main_switch(b: &Browser, t: &Theme) -> El {
    let tab = b.id;
    let mode = |label: &str, glyph: &'static rune::Icon, help: &str, main: Main| {
        let on = b.main == main;
        let mut button = Button::new(label)
            .icon(glyph)
            .control_size(ControlSize::Small)
            .variant(if on {
                ButtonVariant::Submit
            } else {
                ButtonVariant::Outline
            })
            .help(help)
            .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                if let Some(b) = s.browser_mut(tab) {
                    b.clear_status();
                    b.show(main, tx);
                }
                s.sync_title(tx);
            });
        if main == Main::Console
            && let Some(shortcut) = keys::label_in(Command::OpenConsole, Ctx::Browser)
        {
            button = button.shortcut(shortcut);
        }
        button.build(&t.k).checked(on).focus_selected(on)
    };
    row()
        .key("browser-main-switch")
        .gap(4.0)
        .center_y()
        .child(mode("Data", &icons::TABLE, "Table data", Main::Table).key("show-data"))
        .child(mode("SQL", &icons::FILE_CODE, "SQL console", Main::Console).key("show-sql"))
}

fn status_bar(b: &Browser, t: &Theme) -> El {
    let left = b.status.as_ref().map(|(message, error)| {
        row()
            .key("browser-status")
            .gap(4.0)
            .center_y()
            .min_w(0.0)
            .children(error.then(|| ui::icon_el(&icons::CIRCLE_X, 12.0, t.danger)))
            .child(
                text(message.clone())
                    .font(t.text_xs)
                    .color(if *error { t.danger } else { t.warning })
                    .truncate(),
            )
    });
    let info = b
        .server_info
        .clone()
        .unwrap_or_else(|| format!("{} · {}", b.connection.kind.label(), b.connection.summary()));
    row()
        .key("browser-status-bar")
        .h(24.0)
        .w_full()
        .px(8.0)
        .gap(16.0)
        .center_y()
        .fill(t.status_bar)
        .border_sides([1.0, 0.0, 0.0, 0.0], t.status_bar_border)
        .child(row().grow(1.0).min_w(0.0).children(left))
        .children(b.console.running_status().map(|status| {
            row()
                .gap(4.0)
                .center_y()
                .key("running-script")
                .child(ui::spinner(12.0, t.warning, t))
                .child(ui::label(status, t.text_xs, t.warning))
        }))
        .child(
            row()
                .gap(4.0)
                .center_y()
                .child(ui::icon_el(&icons::DATABASE, 12.0, t.muted))
                .child(ui::label(
                    db::render_text(&b.connection.name),
                    t.medium(t.text_xs),
                    t.muted,
                ))
                .child(ui::label("·", t.text_xs, t.muted))
                .child(
                    text(info)
                        .font(t.text_xs)
                        .color(t.muted)
                        .truncate()
                        .max_w(360.0),
                ),
        )
}

#[cfg(test)]
impl Browser {
    pub fn test_session(&self) -> std::rc::Weak<Session> {
        Rc::downgrade(&self.session)
    }
}

#[cfg(test)]
mod tests {
    use super::{Pane, cycle_pane, running_status};

    #[test]
    fn focus_cycles_tree_main() {
        assert_eq!(cycle_pane(Some(Pane::Tree), true), Pane::Main);
        assert_eq!(cycle_pane(Some(Pane::Main), true), Pane::Tree);
        assert_eq!(cycle_pane(Some(Pane::Tree), false), Pane::Main);
        assert_eq!(cycle_pane(Some(Pane::Main), false), Pane::Tree);
        assert_eq!(
            cycle_pane(None, true),
            Pane::Main,
            "outside the cycle counts as the tree"
        );
        assert_eq!(cycle_pane(None, false), Pane::Main);
    }

    #[test]
    fn running_status_counts_seconds_or_says_cancelling() {
        assert_eq!(
            running_status(3, false),
            "Running the script… 3s · Esc or ⌘. cancels it"
        );
        assert_eq!(
            running_status(4_000, false),
            "Running the script… 4,000s · Esc or ⌘. cancels it"
        );
        assert_eq!(running_status(3, true), "Cancelling the script…");
    }
}
