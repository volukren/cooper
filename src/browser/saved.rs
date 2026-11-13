use std::borrow::Cow;

use rune::{
    Cx, El, GrowthSpec, Key, KeyEvent, PointerPress, ScrollSpec, TextEdit, Tx, col, row, text,
};
use rune_kit::{Button, ButtonVariant, Dialog as KitDialog, MenuItem, TextInput, context_menu};

use super::{Browser, Pane, console};
use crate::{
    db,
    dialog::Dialog,
    icons,
    keys::{self, BINDINGS, Command, Ctx},
    queries::{QueryStore, SavedQuery},
    theme::Theme,
    ui,
    workspace::{self, TabId, Workspace},
};

pub const ROW_HEIGHT: f32 = 44.;
pub const PAGE: isize = 8;
const PREVIEW_CHARS: usize = 160;
pub const PANE: &str = "saved-queries";
const SCROLL: &str = "saved-list-scroll";

pub struct SavedQueries {
    pub connection: String,
    pub queries: QueryStore,
    pub list: QueryList,
    pub filter: TextEdit,
    pub bound: Option<(String, bool)>,
    pub menu: Option<PointerPress>,
}

impl SavedQueries {
    pub fn new(connection: &str, queries: QueryStore) -> Self {
        Self {
            connection: connection.to_string(),
            queries,
            list: QueryList::default(),
            filter: TextEdit::new(""),
            bound: None,
            menu: None,
        }
    }

    pub fn names(&self) -> Vec<String> {
        (0..self.list.match_count())
            .filter_map(|index| self.list.matched(index))
            .map(|query| query.name.clone())
            .collect()
    }

    pub fn selected_name(&self) -> Option<String> {
        self.list.selected().map(|query| query.name.clone())
    }

    pub fn count_label(&self) -> Option<String> {
        self.list.count_label()
    }

    pub fn failed(&self) -> bool {
        self.list.failed()
    }

    pub fn set_bound(&mut self, bound: Option<(String, bool)>) {
        self.bound = bound;
    }

    pub fn select(&mut self, name: &str) -> bool {
        self.list.select_name(name)
    }
}

fn row_key(index: usize) -> String {
    format!("saved-row-{index}")
}

pub fn reveal(b: &Browser, tx: &mut Tx) {
    if let Some(index) = b.saved.list.selected_index() {
        tx.scroll_into_view_in(SCROLL, row_key(index));
    }
}

pub fn reload(b: &mut Browser) {
    match b.saved.queries.load(&b.saved.connection) {
        Ok(items) => b.saved.list.refresh(items),
        Err(err) => {
            b.saved.list.fail();
            b.set_status(format!("Failed to read saved queries: {err}"), true);
        }
    }
}

pub fn focus(_b: &mut Browser, tx: &mut Tx) {
    tx.focus(PANE, false);
}

pub fn contexts(b: &Browser, focused: Option<&str>, out: &mut Vec<Ctx>) {
    if b.saved.filter.focused {
        out.push(Ctx::SavedQueriesInput);
        out.push(Ctx::SavedQueries);
    } else if focused == Some(PANE) {
        out.push(Ctx::SavedQueries);
    }
}

pub fn run(s: &mut Workspace, tab: TabId, cmd: Command, tx: &mut Tx) -> bool {
    match cmd {
        Command::OpenQuery => open_selected(s, tab, false, tx),
        Command::RunQuery => open_selected(s, tab, true, tx),
        Command::DeleteQuery => {
            if s.browser(tab).is_some_and(|b| b.saved.filter.focused) {
                return false;
            }
            confirm_delete(s, tab)
        }
        _ => return false,
    }
    true
}

fn open_selected(s: &mut Workspace, tab: TabId, run: bool, tx: &mut Tx) {
    let Some(query) = s
        .browser(tab)
        .and_then(|b| b.saved.list.selected().cloned())
    else {
        return;
    };
    console::load_query(s, tab, query, run, tx);
}

fn confirm_delete(s: &mut Workspace, tab: TabId) {
    let Some(name) = s.browser(tab).and_then(|b| b.saved.selected_name()) else {
        return;
    };
    s.dialog = Some(Dialog::DeleteQuery { tab, name });
}

fn delete(s: &mut Workspace, tab: TabId, name: &str, tx: &mut Tx) {
    if matches!(&s.dialog, Some(Dialog::DeleteQuery { tab: open, .. }) if *open == tab) {
        s.dialog = None;
    }
    let Some(b) = s.browser_mut(tab) else {
        return;
    };
    match b.saved.queries.remove(&b.saved.connection, name) {
        Ok(_) => {
            b.saved.list.remove(name);
            reveal(b, tx);
            console::forget_query(b, name);
        }
        Err(err) => {
            let message = format!("Failed to delete \"{}\": {err}", db::render_text(name));
            b.set_status(message, true);
        }
    }
    s.sync_title(tx);
}

fn clear_filter(b: &mut Browser) {
    b.saved.filter.replace_text("");
    b.saved.list.set_filter("");
}

fn refresh_key(s: &mut Workspace, tab: TabId, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let refresh = BINDINGS
        .iter()
        .any(|b| b.cmd == Command::Refresh && b.chord.matches_tx(ev, tx));
    let ours = BINDINGS.iter().any(|b| {
        matches!(b.ctx, Ctx::SavedQueries | Ctx::SavedQueriesInput) && b.chord.matches_tx(ev, tx)
    });
    if !refresh || ours || s.dialog.is_some() {
        return false;
    }
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    b.clear_status();
    reload(b);
    reveal(b, tx);
    true
}

fn list_key(s: &mut Workspace, tab: TabId, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let m = ev.mods;
    if m.cmd || m.ctrl || m.alt || m.logo || m.shift {
        return false;
    }
    let delta = match ev.key {
        Key::Down => 1,
        Key::Up => -1,
        Key::PageDown => PAGE,
        Key::PageUp => -PAGE,
        Key::Escape => {
            let Some(b) = s.browser_mut(tab) else {
                return false;
            };
            if b.saved.list.filtering() {
                clear_filter(b);
                reveal(b, tx);
            } else {
                b.focus_pane(Pane::Tree, tx);
            }
            return true;
        }
        _ => return false,
    };
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    b.saved.list.move_by(delta);
    reveal(b, tx);
    true
}

fn pane_key(s: &mut Workspace, tab: TabId, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if list_key(s, tab, ev, tx) {
        return true;
    }
    let m = ev.mods;
    if m.cmd || m.ctrl || m.alt || m.logo {
        return false;
    }
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    match &ev.key {
        Key::Home => b.saved.list.select(0),
        Key::End => b.saved.list.select(usize::MAX),
        Key::Text(typed) if is_typed_text(typed) => {
            if b.saved.list.is_empty() {
                return false;
            }
            b.saved.filter.focused = true;
            b.saved.filter.insert(typed);
            let text = b.saved.filter.text.clone();
            b.saved.list.set_filter(&text);
        }
        _ => return false,
    }
    reveal(b, tx);
    true
}

fn select_row(s: &mut Workspace, tab: TabId, index: usize, tx: &mut Tx) {
    let Some(b) = s.browser_mut(tab) else {
        return;
    };
    b.clear_status();
    b.saved.list.select(index);
    b.saved.filter.focused = false;
    tx.focus(PANE, false);
}

fn pick(s: &mut Workspace, tab: TabId, cmd: Command, tx: &mut Tx) {
    if let Some(b) = s.browser_mut(tab) {
        b.saved.menu = None;
    }
    run(s, tab, cmd, tx);
}

pub fn view(
    _s: &Workspace,
    b: &Browser,
    t: &Theme,
    _cx: &mut Cx<Workspace>,
    layer: &mut Vec<El>,
) -> El {
    let tab = b.id;
    let title = row()
        .gap(4.)
        .center_y()
        .child(ui::label("Queries", t.bold(t.text_sm), t.blue))
        .children(
            b.saved
                .list
                .count_label()
                .map(|count| ui::label(count, t.text_xs, t.muted).key("saved-count")),
        );
    let body = body(b, tab, t, layer);
    let pane = ui::pane(PANE, title, None, body, t)
        .on_background_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
            if let Some(b) = s.browser_mut(tab) {
                b.clear_status();
            }
            tx.focus(PANE, false);
        })
        .focusable(true)
        .on_key(move |s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| {
            refresh_key(s, tab, ev, tx) || keys::route(s, ev, tx) || pane_key(s, tab, ev, tx)
        });
    col()
        .key("saved-pane")
        .grow(1.)
        .w_full()
        .min_h(0.)
        .min_w(0.)
        .child(pane)
}

fn body(b: &Browser, tab: TabId, t: &Theme, layer: &mut Vec<El>) -> El {
    let list = &b.saved.list;
    if list.failed() {
        return col()
            .key("saved-failed")
            .grow(1.)
            .w_full()
            .center()
            .gap(4.)
            .child(ui::label("Could not read the queries", t.text_sm, t.danger))
            .child(ui::label("Press ⌘R to retry", t.text_sm, t.muted));
    }
    if list.is_empty() {
        return ui::placeholder(
            &[
                ("No saved queries yet", false),
                ("Save a script with ⌘S", false),
            ],
            t,
        );
    }
    let rows = if list.match_count() == 0 {
        col()
            .key("saved-no-matches")
            .grow(1.)
            .w_full()
            .padding(8.)
            .child(ui::label("No matches", t.text_sm, t.muted))
    } else {
        let viewport = col()
            .key(SCROLL)
            .grow(1.)
            .min_h(0.)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(
                col()
                    .key("saved-list")
                    .w_full()
                    .growth(GrowthSpec::default())
                    .virtual_rows(
                        list.match_count(),
                        ROW_HEIGHT,
                        |_: &Workspace, i| Cow::Owned(row_key(i)),
                        move |s: &Workspace, i| saved_row(s, tab, i),
                    ),
            );
        let area = col()
            .key("saved-rows")
            .grow(1.)
            .min_h(0.)
            .w_full()
            .child(t.k.scroll_bar(b.scrolls.keep(SCROLL, viewport)));
        context_menu("saved-menu", area, b.saved.menu)
            .item(
                MenuItem::new("Open").on_select_tx(move |s: &mut Workspace, tx: &mut Tx| {
                    pick(s, tab, Command::OpenQuery, tx)
                }),
            )
            .item(MenuItem::new("Open and Run").on_select_tx(
                move |s: &mut Workspace, tx: &mut Tx| pick(s, tab, Command::RunQuery, tx),
            ))
            .separator()
            .item(MenuItem::new("Delete…").danger(true).on_select_tx(
                move |s: &mut Workspace, tx: &mut Tx| pick(s, tab, Command::DeleteQuery, tx),
            ))
            .on_close(move |s: &mut Workspace| {
                if let Some(b) = s.browser_mut(tab) {
                    b.saved.menu = None;
                }
            })
            .build(&t.k, layer)
            .grow(1.)
            .min_h(0.)
            .w_full()
    };
    let search = TextInput::new(&b.saved.filter)
        .placeholder("Search queries")
        .label("Search queries")
        .leading_icon(&icons::SEARCH)
        .width(None)
        .height(26.)
        .text_style(t.text_sm);
    let search = if b.saved.filter.text.is_empty() {
        search
    } else {
        search.clear_button()
    };
    let search = search
        .on_edit(move |s: &mut Workspace| workspace::browser_edit(s, tab, |b| &mut b.saved.filter))
        .on_change(move |s: &mut Workspace, tx: &mut Tx| {
            if let Some(b) = s.browser_mut(tab) {
                let text = b.saved.filter.text.clone();
                b.saved.list.set_filter(&text);
                reveal(b, tx);
            }
        })
        .on_action(move |s: &mut Workspace, tx: &mut Tx| {
            if let Some(b) = s.browser_mut(tab) {
                clear_filter(b);
                reveal(b, tx);
            }
        })
        .on_key(move |s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| {
            refresh_key(s, tab, ev, tx) || keys::field_key(s, ev, tx) || list_key(s, tab, ev, tx)
        })
        .build(&t.k)
        .key("saved-search");
    col()
        .grow(1.)
        .w_full()
        .min_h(0.)
        .child(col().w_full().padding(4.).child(search))
        .child(rows)
}

fn saved_row(s: &Workspace, tab: TabId, index: usize) -> El {
    let t = s.theme();
    let Some(b) = s.browser(tab) else {
        return col().h(ROW_HEIGHT);
    };
    let list = &b.saved.list;
    let Some(query) = list.matched(index) else {
        return col().h(ROW_HEIGHT);
    };
    let selected = list.selected_index() == Some(index);
    let bound = b.console.bound().filter(|(name, _)| *name == query.name);
    let mark = if bound.as_ref().is_some_and(|(_, unsaved)| *unsaved) {
        "*"
    } else {
        ""
    };
    let name = format!("{}{mark}", db::render_text(&query.name));
    let name = if bound.is_some() {
        text(name).font(t.semibold(t.text_sm)).color(t.warning)
    } else {
        text(name).font(t.text_sm).color(t.foreground)
    };
    let mut el = col()
        .key(row_key(index))
        .role("option")
        .label(query.name.clone())
        .h(ROW_HEIGHT)
        .w_full()
        .px(8.)
        .gap(2.)
        .justify_center()
        .cursor(rune::Cursor::Pointer)
        .child(
            row()
                .gap(4.)
                .min_w(0.)
                .center_y()
                .child(ui::icon_el(&icons::FILE_CODE, 12., t.muted))
                .child(name.truncate().min_w(0.)),
        )
        .child(
            text(preview(&query.sql))
                .font(t.mono_sm)
                .color(t.muted)
                .truncate()
                .min_w(0.),
        )
        .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| select_row(s, tab, index, tx))
        .on_double_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
            select_row(s, tab, index, tx);
            open_selected(s, tab, false, tx);
        })
        .on_secondary_press(move |s: &mut Workspace, press: PointerPress, tx: &mut Tx| {
            select_row(s, tab, index, tx);
            if let Some(b) = s.browser_mut(tab) {
                b.saved.menu = Some(press);
            }
        });
    el = if selected {
        el.fill(t.list_active)
    } else {
        el.hover(t.list_hover)
    };
    el
}

pub fn delete_dialog(s: &Workspace, tab: TabId, name: &str, t: &Theme, layer: &mut Vec<El>) {
    if s.browser(tab).is_none() {
        return;
    }
    let target = name.to_string();
    let footer = row()
        .w_full()
        .padding(16.)
        .gap(8.)
        .justify_end()
        .child(
            Button::new("Cancel")
                .variant(ButtonVariant::Outline)
                .on_press(|s: &mut Workspace| s.dialog = None)
                .build(&t.k)
                .key("delete-query-cancel"),
        )
        .child(
            Button::new("Delete")
                .icon(&icons::DELETE)
                .variant(ButtonVariant::Danger)
                .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| delete(s, tab, &target, tx))
                .build(&t.k)
                .key("delete-query-confirm")
                .default_button()
                .autofocus(),
        );
    KitDialog::new(
        "delete-query",
        format!("Delete \"{}\"?", db::render_text(name)),
    )
    .description("The saved query is removed for good.")
    .width(440.)
    .fit()
    .open(true)
    .on_close(|s: &mut Workspace| s.dialog = None)
    .child(footer)
    .build(&t.modal, layer);
}

fn is_typed_text(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_control)
}

fn preview(sql: &str) -> String {
    let mut out = String::new();
    for word in sql.split_whitespace() {
        if out.chars().count() >= PREVIEW_CHARS {
            out.push('…');
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

#[derive(Debug, Default)]
pub struct QueryList {
    items: Vec<SavedQuery>,
    matches: Vec<usize>,
    filter: String,
    selected: Option<usize>,
    failed: bool,
}

impl QueryList {
    fn set_items(&mut self, items: Vec<SavedQuery>, select: Option<&str>) {
        let keep = select
            .or_else(|| self.selected().map(|query| query.name.as_str()))
            .map(str::to_string);
        self.items = items;
        self.failed = false;
        self.refilter(keep.as_deref());
    }

    fn refresh(&mut self, items: Vec<SavedQuery>) {
        let added = items
            .iter()
            .find(|query| !self.items.iter().any(|old| old.name == query.name))
            .map(|query| query.name.clone());
        self.set_items(items, added.as_deref());
    }

    fn fail(&mut self) {
        self.set_items(Vec::new(), None);
        self.failed = true;
    }

    fn failed(&self) -> bool {
        self.failed
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn filtering(&self) -> bool {
        !self.filter.is_empty()
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn matched(&self, index: usize) -> Option<&SavedQuery> {
        self.items.get(*self.matches.get(index)?)
    }

    fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    fn selected(&self) -> Option<&SavedQuery> {
        self.matched(self.selected?)
    }

    fn count_label(&self) -> Option<String> {
        if self.items.is_empty() {
            None
        } else if self.filtering() {
            Some(format!("{}/{}", self.matches.len(), self.items.len()))
        } else {
            Some(self.items.len().to_string())
        }
    }

    fn set_filter(&mut self, filter: &str) {
        if filter.trim().is_empty() {
            let keep = self.selected().map(|query| query.name.clone());
            self.filter.clear();
            self.refilter(keep.as_deref());
        } else {
            self.filter = filter.to_string();
            self.refilter(None);
        }
    }

    fn remove(&mut self, name: &str) {
        let Some(index) = self.items.iter().position(|query| query.name == name) else {
            return;
        };
        let at = self.selected.unwrap_or(0);
        self.items.remove(index);
        self.refilter(None);
        self.select(at);
    }

    fn select(&mut self, index: usize) {
        let last = self.matches.len().checked_sub(1);
        self.selected = last.map(|last| index.min(last));
    }

    fn select_name(&mut self, name: &str) -> bool {
        let found = self
            .matches
            .iter()
            .position(|&index| self.items[index].name == name);
        if let Some(position) = found {
            self.selected = Some(position);
        }
        found.is_some()
    }

    fn move_by(&mut self, delta: isize) {
        let index = self.selected.unwrap_or(0);
        self.select(index.saturating_add_signed(delta));
    }

    fn refilter(&mut self, keep: Option<&str>) {
        let terms: Vec<String> = self
            .filter
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let hit = |text: &str| {
            let text = text.to_lowercase();
            terms.iter().all(|term| text.contains(term.as_str()))
        };
        let mut by_sql = Vec::new();
        self.matches.clear();
        for (index, query) in self.items.iter().enumerate() {
            if hit(&query.name) {
                self.matches.push(index);
            } else if hit(&format!("{}\n{}", query.name, query.sql)) {
                by_sql.push(index);
            }
        }
        self.matches.extend(by_sql);
        let kept = keep.and_then(|name| {
            self.matches
                .iter()
                .position(|&index| self.items[index].name == name)
        });
        self.select(kept.unwrap_or(0));
    }
}

#[cfg(test)]
mod tests {
    use super::{QueryList, is_typed_text, preview};
    use crate::queries::SavedQuery;

    fn query(name: &str, sql: &str) -> SavedQuery {
        SavedQuery {
            name: name.to_string(),
            sql: sql.to_string(),
        }
    }

    fn list() -> QueryList {
        let mut list = QueryList::default();
        list.set_items(
            vec![
                query("active users", "select * from users where active"),
                query("audit", "select * from audit_log -- users"),
                query("orders report", "select * from orders"),
            ],
            None,
        );
        list
    }

    fn shown(list: &QueryList) -> Vec<&str> {
        (0..list.match_count())
            .filter_map(|index| list.matched(index))
            .map(|query| query.name.as_str())
            .collect()
    }

    fn selected(list: &QueryList) -> Option<&str> {
        list.selected().map(|query| query.name.as_str())
    }

    #[test]
    fn filters_by_name_first_then_by_sql() {
        let mut list = list();
        assert_eq!(list.count_label().as_deref(), Some("3"));
        list.set_filter("USERS");
        assert!(list.filtering());
        assert_eq!(shown(&list), ["active users", "audit"], "names match first");
        assert_eq!(list.count_label().as_deref(), Some("2/3"));
        assert_eq!(selected(&list), Some("active users"));

        list.set_filter("USERSq");
        assert_eq!(shown(&list), Vec::<&str>::new());
        assert_eq!(selected(&list), None, "no match, no selection");

        list.set_filter("rep ord");
        assert_eq!(shown(&list), ["orders report"], "every term must match");
        assert_eq!(list.count_label().as_deref(), Some("1/3"));
    }

    #[test]
    fn a_blank_filter_clears_and_keeps_the_selection() {
        let mut list = list();
        list.set_filter(" ");
        assert!(!list.filtering(), "a leading space is ignored");
        list.set_filter("USERS");
        list.move_by(1);
        assert_eq!(selected(&list), Some("audit"));
        list.set_filter("");
        assert!(!list.filtering());
        assert_eq!(shown(&list).len(), 3);
        assert_eq!(
            selected(&list),
            Some("audit"),
            "the selection is kept by name"
        );
        list.set_filter("o");
        assert_eq!(
            selected(&list),
            Some("orders report"),
            "typing jumps to the first match"
        );
    }

    #[test]
    fn selection_moves_within_bounds() {
        let mut list = list();
        list.move_by(-1);
        assert_eq!(selected(&list), Some("active users"));
        list.move_by(10);
        assert_eq!(selected(&list), Some("orders report"));
        list.select(usize::MAX);
        assert_eq!(list.selected_index(), Some(2));
        list.select(0);
        assert_eq!(list.selected_index(), Some(0));
        assert!(list.select_name("audit"));
        assert_eq!(selected(&list), Some("audit"));
        assert!(!list.select_name("missing"));
        assert_eq!(selected(&list), Some("audit"));
    }

    #[test]
    fn set_items_keeps_the_selection_by_name() {
        let mut list = list();
        list.select(1);
        let mut items = vec![query("aaa", "select 0")];
        items.extend(list.items.clone());
        list.set_items(items.clone(), None);
        assert_eq!(selected(&list), Some("audit"), "kept by the selected name");
        list.set_items(items.clone(), Some("orders report"));
        assert_eq!(
            selected(&list),
            Some("orders report"),
            "an explicit name wins"
        );
        list.set_items(items, Some("gone"));
        assert_eq!(list.selected_index(), Some(0), "falls back to the first");
    }

    #[test]
    fn a_reload_selects_a_newly_saved_query() {
        let mut list = list();
        list.select(2);
        let mut items = list.items.clone();
        list.refresh(items.clone());
        assert_eq!(
            selected(&list),
            Some("orders report"),
            "nothing new: the selection stays"
        );
        items.insert(1, query("b-new", "select 2"));
        list.refresh(items);
        assert_eq!(selected(&list), Some("b-new"));
    }

    #[test]
    fn removing_keeps_the_position() {
        let mut list = list();
        list.select(1);
        list.remove("audit");
        assert_eq!(shown(&list), ["active users", "orders report"]);
        assert_eq!(selected(&list), Some("orders report"));
        list.remove("orders report");
        assert_eq!(selected(&list), Some("active users"), "clamped to the last");
        list.remove("missing");
        assert_eq!(shown(&list), ["active users"]);
        list.remove("active users");
        assert!(list.is_empty());
        assert_eq!(list.selected(), None);
        assert_eq!(list.count_label(), None, "no count for an empty list");
    }

    #[test]
    fn a_failed_load_empties_the_list() {
        let mut list = list();
        list.fail();
        assert!(list.failed());
        assert!(list.is_empty());
        list.set_items(vec![query("one", "select 1")], None);
        assert!(!list.failed());
        assert_eq!(selected(&list), Some("one"));
    }

    #[test]
    fn previews_collapse_whitespace() {
        assert_eq!(
            preview("select *\n  from users\n\twhere id = 1\n"),
            "select * from users where id = 1"
        );
        assert_eq!(preview(""), "");
        let long = "word ".repeat(100);
        let shown = preview(&long);
        assert!(shown.ends_with('…'));
        assert!(shown.chars().count() <= super::PREVIEW_CHARS + 6);
    }

    #[test]
    fn typed_text_excludes_control_keys() {
        assert!(is_typed_text("a"));
        assert!(is_typed_text("é"));
        assert!(!is_typed_text(""));
        assert!(!is_typed_text("\r"));
        assert!(!is_typed_text("\u{1b}"));
    }
}
