use std::{borrow::Cow, rc::Rc};

use rune::{
    Color, Cursor, Cx, El, GrowthSpec, Key, KeyEvent, Mods, PointerPress, ScrollSpec, TextEdit, Tx,
    col, el, row, text,
};
use rune_kit::{
    Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, MenuItem, TextInput,
    context_menu,
};

use super::{Browser, Pane, ddl};
use crate::{
    db::{self, Session, TableInfo, TableKind},
    icons,
    keys::{self, Command, Ctx},
    theme::Theme,
    ui,
    workspace::{self, TabId, Workspace},
};

pub trait TreeRequests {
    fn list_schemas(&self) -> u64;
    fn list_tables(&self, schema: &str) -> u64;
}

impl TreeRequests for Session {
    fn list_schemas(&self) -> u64 {
        Session::list_schemas(self)
    }

    fn list_tables(&self, schema: &str) -> u64 {
        Session::list_tables(self, schema)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeRow {
    Schema(usize),
    Table(usize, usize),
    Note(usize),
}

#[derive(Debug, Clone)]
pub struct SchemaNode {
    pub name: String,
    pub expanded: bool,
    pub tables: Option<Vec<TableInfo>>,
    pub request: Option<u64>,
    pub error: Option<String>,
}

impl SchemaNode {
    pub fn new(name: String) -> Self {
        Self {
            name,
            expanded: false,
            tables: None,
            request: None,
            error: None,
        }
    }
}

pub fn visible_rows(nodes: &[SchemaNode], filter: &str) -> Vec<TreeRow> {
    let terms: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    let hit = |table: &TableInfo| {
        let name = table.name.to_lowercase();
        terms.iter().all(|term| name.contains(term.as_str()))
    };
    let mut rows = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        let tables = node.tables.as_deref().unwrap_or_default();
        let hits: Vec<usize> = (0..tables.len())
            .filter(|&j| terms.is_empty() || hit(&tables[j]))
            .collect();
        let pending = node.expanded && node.tables.is_none();
        if !terms.is_empty() && hits.is_empty() && !pending {
            continue;
        }
        rows.push(TreeRow::Schema(i));
        if !node.expanded {
            continue;
        }
        if hits.is_empty() {
            rows.push(TreeRow::Note(i));
        } else {
            rows.extend(hits.into_iter().map(|j| TreeRow::Table(i, j)));
        }
    }
    rows
}

pub fn sanitize_filter(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum ErrorRoute {
    NotOurs,
    Shown,
    Status(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Activation {
    OpenTable {
        schema: String,
        table: String,
        kind: TableKind,
    },
}

type CursorKey = (String, Option<String>);

#[derive(Debug, Default)]
pub struct TreeModel {
    pub nodes: Vec<SchemaNode>,
    pub schemas_request: Option<u64>,
    pub schemas_loaded: bool,
    pub schemas_error: Option<String>,
    pub cursor: usize,
    filter: String,
}

impl TreeModel {
    pub fn rows(&self) -> Vec<TreeRow> {
        visible_rows(&self.nodes, &self.filter)
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn searchable(&self) -> bool {
        self.schemas_loaded && !self.nodes.is_empty()
    }

    pub fn request_schemas(&mut self, requests: &dyn TreeRequests) {
        self.schemas_error = None;
        self.schemas_request = Some(requests.list_schemas());
    }

    pub fn on_schemas(
        &mut self,
        id: u64,
        schemas: Vec<String>,
        requests: &dyn TreeRequests,
    ) -> bool {
        if self.schemas_request != Some(id) {
            return false;
        }
        self.schemas_request = None;
        self.set_schemas(schemas, requests);
        true
    }

    fn set_schemas(&mut self, schemas: Vec<String>, requests: &dyn TreeRequests) {
        let key = self.cursor_key();
        let first_load = !self.schemas_loaded;
        let mut old = std::mem::take(&mut self.nodes);
        self.nodes = schemas
            .into_iter()
            .map(|name| match old.iter().position(|node| node.name == name) {
                Some(index) => old.swap_remove(index),
                None => SchemaNode::new(name),
            })
            .collect();
        self.schemas_loaded = true;
        self.schemas_error = None;
        if first_load && let Some(first) = self.nodes.first_mut() {
            first.expanded = true;
        }
        for node in &mut self.nodes {
            if node.expanded {
                node.request = Some(requests.list_tables(&node.name));
            }
        }
        self.restore_cursor(key);
    }

    pub fn on_tables(&mut self, id: u64, tables: Vec<TableInfo>) -> bool {
        let Some(index) = self.nodes.iter().position(|node| node.request == Some(id)) else {
            return false;
        };
        let key = self.cursor_key();
        let node = &mut self.nodes[index];
        node.request = None;
        node.error = None;
        node.tables = Some(tables);
        self.restore_cursor(key);
        true
    }

    pub fn on_error(&mut self, id: u64, message: &str) -> ErrorRoute {
        if self.schemas_request == Some(id) {
            self.schemas_request = None;
            if self.schemas_loaded {
                return ErrorRoute::Status(format!("Failed to refresh schemas: {message}"));
            }
            self.schemas_error = Some(message.to_string());
            return ErrorRoute::Shown;
        }
        if let Some(node) = self.nodes.iter_mut().find(|node| node.request == Some(id)) {
            node.request = None;
            node.error = Some(message.to_string());
            return ErrorRoute::Status(format!(
                "Failed to list tables of {}: {message}",
                db::render_text(&node.name)
            ));
        }
        ErrorRoute::NotOurs
    }

    pub fn set_expanded(&mut self, index: usize, expanded: bool, requests: &dyn TreeRequests) {
        let key = self.cursor_key();
        let Some(node) = self.nodes.get_mut(index) else {
            return;
        };
        node.expanded = expanded;
        if expanded && node.tables.is_none() && node.request.is_none() {
            node.error = None;
            node.request = Some(requests.list_tables(&node.name));
        }
        self.restore_cursor(key);
    }

    pub fn retry_tables(&mut self, index: usize, requests: &dyn TreeRequests) {
        let Some(node) = self.nodes.get_mut(index) else {
            return;
        };
        if node.error.is_some() && node.request.is_none() {
            node.error = None;
            node.request = Some(requests.list_tables(&node.name));
        }
    }

    pub fn set_filter(&mut self, text: &str) {
        let key = self.cursor_key();
        if text.trim().is_empty() {
            self.filter.clear();
            self.restore_cursor(key);
        } else {
            self.filter = text.to_string();
            let first_table = |row: &TreeRow| matches!(row, TreeRow::Table(..));
            self.cursor = self.rows().iter().position(first_table).unwrap_or(0);
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor + 1 < self.rows().len() {
            self.cursor += 1;
        }
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn current(&self) -> Option<TreeRow> {
        self.rows().get(self.cursor).copied()
    }

    pub fn activate(&mut self, requests: &dyn TreeRequests) -> Option<Activation> {
        match self.current()? {
            TreeRow::Schema(i) => {
                let expanded = self.nodes[i].expanded;
                self.set_expanded(i, !expanded, requests);
                None
            }
            TreeRow::Table(i, j) => self.table_at(i, j),
            TreeRow::Note(i) => {
                self.retry_tables(i, requests);
                None
            }
        }
    }

    pub fn right(&mut self, requests: &dyn TreeRequests) -> Option<Activation> {
        match self.current()? {
            TreeRow::Schema(i) => {
                self.set_expanded(i, true, requests);
                None
            }
            TreeRow::Table(i, j) => self.table_at(i, j),
            TreeRow::Note(_) => None,
        }
    }

    pub fn left(&mut self, requests: &dyn TreeRequests) {
        let rows = self.rows();
        match rows.get(self.cursor).copied() {
            Some(TreeRow::Schema(i)) => self.set_expanded(i, false, requests),
            Some(TreeRow::Table(i, _) | TreeRow::Note(i)) => {
                self.cursor = rows
                    .iter()
                    .position(|row| *row == TreeRow::Schema(i))
                    .unwrap_or(0);
            }
            None => {}
        }
    }

    pub fn table_at(&self, schema: usize, table: usize) -> Option<Activation> {
        let node = self.nodes.get(schema)?;
        let info = node.tables.as_ref()?.get(table)?;
        Some(Activation::OpenTable {
            schema: node.name.clone(),
            table: info.name.clone(),
            kind: info.kind,
        })
    }

    fn cursor_key(&self) -> Option<CursorKey> {
        match *self.rows().get(self.cursor)? {
            TreeRow::Schema(i) | TreeRow::Note(i) => Some((self.nodes[i].name.clone(), None)),
            TreeRow::Table(i, j) => {
                let node = &self.nodes[i];
                let table = node.tables.as_ref()?.get(j)?;
                Some((node.name.clone(), Some(table.name.clone())))
            }
        }
    }

    fn restore_cursor(&mut self, key: Option<CursorKey>) {
        let rows = self.rows();
        let find = |schema: &str, table: Option<&str>| {
            rows.iter().position(|row| match *row {
                TreeRow::Schema(i) => table.is_none() && self.nodes[i].name == schema,
                TreeRow::Table(i, j) => {
                    self.nodes[i].name == schema
                        && self.nodes[i]
                            .tables
                            .as_ref()
                            .and_then(|tables| tables.get(j))
                            .is_some_and(|t| Some(t.name.as_str()) == table)
                }
                TreeRow::Note(_) => false,
            })
        };
        let found = key.and_then(|(schema, table)| {
            find(&schema, table.as_deref()).or_else(|| find(&schema, None))
        });
        self.cursor = found
            .unwrap_or(self.cursor)
            .min(rows.len().saturating_sub(1));
    }
}

pub const ROW_HEIGHT: f32 = 24.;
const INDENT: f32 = 16.;
pub const ROOT_KEY: &str = "schema-tree";
pub const SCROLL_KEY: &str = "schema-tree-scroll";
const RETRY_KEY: &str = "retry-schemas";
const REFRESH_KEY: &str = "refresh-tree";

pub fn row_key(ix: usize) -> String {
    format!("schema-tree-row-{ix}")
}

pub struct SchemaTree {
    pub model: TreeModel,
    pub open_table: Option<(String, String)>,
    pub filter: TextEdit,
    pub menu: Option<PointerPress>,
    pub reveal: bool,
}

impl SchemaTree {
    pub fn new() -> Self {
        Self {
            model: TreeModel::default(),
            open_table: None,
            filter: TextEdit::new(""),
            menu: None,
            reveal: false,
        }
    }

    pub fn is_filtering(&self) -> bool {
        !self.model.filter().is_empty()
    }

    pub fn set_open_table(&mut self, table: Option<(String, String)>) {
        self.open_table = table;
    }
}

impl Default for SchemaTree {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RowId {
    schema: String,
    table: Option<String>,
    note: bool,
}

fn row_id(model: &TreeModel, row: TreeRow) -> Option<RowId> {
    Some(match row {
        TreeRow::Schema(i) => RowId {
            schema: model.nodes.get(i)?.name.clone(),
            table: None,
            note: false,
        },
        TreeRow::Table(i, j) => {
            let node = model.nodes.get(i)?;
            RowId {
                schema: node.name.clone(),
                table: Some(node.tables.as_ref()?.get(j)?.name.clone()),
                note: false,
            }
        }
        TreeRow::Note(i) => RowId {
            schema: model.nodes.get(i)?.name.clone(),
            table: None,
            note: true,
        },
    })
}

fn find_row(model: &TreeModel, id: &RowId) -> Option<usize> {
    model
        .rows()
        .into_iter()
        .position(|row| row_id(model, row).as_ref() == Some(id))
}

pub fn load(b: &mut Browser) {
    b.tree.model.request_schemas(&*b.session);
}

pub fn refresh(b: &mut Browser) {
    load(b);
}

pub fn on_schemas(b: &mut Browser, id: u64, schemas: Vec<String>) -> bool {
    let done = b.tree.model.on_schemas(id, schemas, &*b.session);
    b.tree.reveal |= done;
    done
}

pub fn on_tables(b: &mut Browser, id: u64, _schema: String, tables: Vec<TableInfo>) -> bool {
    let done = b.tree.model.on_tables(id, tables);
    b.tree.reveal |= done;
    done
}

pub fn on_error(b: &mut Browser, id: u64, message: &str) -> bool {
    match b.tree.model.on_error(id, message) {
        ErrorRoute::NotOurs => false,
        ErrorRoute::Shown => true,
        ErrorRoute::Status(text) => {
            b.set_status(text, true);
            true
        }
    }
}

pub fn poll(b: &mut Browser, tx: &mut Tx) -> bool {
    if std::mem::take(&mut b.tree.reveal) {
        reveal(b, tx);
    }
    false
}

fn reveal(b: &mut Browser, tx: &mut Tx) {
    b.tree.reveal = false;
    if !b.tree.model.rows().is_empty() {
        tx.scroll_into_view_in(SCROLL_KEY, row_key(b.tree.model.cursor));
    }
}

pub fn focus(_b: &mut Browser, tx: &mut Tx) {
    tx.focus(ROOT_KEY, false);
}

pub fn contexts(b: &Browser, focused: Option<&str>, out: &mut Vec<Ctx>) {
    if matches!(focused, Some(ROOT_KEY | RETRY_KEY | REFRESH_KEY)) || b.tree.filter.focused {
        out.push(Ctx::SchemaTree);
    }
}

pub fn run(s: &mut Workspace, tab: TabId, cmd: Command, _tx: &mut Tx) -> bool {
    match cmd {
        Command::ShowDdl => show_ddl(s, tab),
        _ => false,
    }
}

pub fn show_ddl(s: &mut Workspace, tab: TabId) -> bool {
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    b.tree.menu = None;
    let Some(TreeRow::Table(i, j)) = b.tree.model.current() else {
        return true;
    };
    if let Some(Activation::OpenTable { schema, table, .. }) = b.tree.model.table_at(i, j) {
        ddl::show(s, tab, &schema, &table);
    }
    true
}

fn open(b: &mut Browser, activation: Option<Activation>, focus_table: bool, tx: &mut Tx) {
    if let Some(Activation::OpenTable {
        schema,
        table,
        kind,
    }) = activation
    {
        b.open_table(&schema, &table, kind);
        if focus_table {
            b.focus_pane(Pane::Main, tx);
        }
    }
}

pub fn filter_changed(b: &mut Browser) {
    let value = b.tree.filter.text.clone();
    let mut clean = sanitize_filter(&value);
    if clean.trim().is_empty() {
        clean.clear();
    }
    if clean != value {
        write_filter(&mut b.tree.filter, &clean);
    }
    b.tree.model.set_filter(&clean);
    b.tree.reveal = true;
}

fn write_filter(edit: &mut TextEdit, text: &str) {
    let focused = edit.focused;
    edit.replace_text(text.to_string());
    edit.set_caret(text.len());
    edit.focused = focused;
}

fn set_filter_text(b: &mut Browser, text: &str) {
    b.tree.model.set_filter(text);
    write_filter(&mut b.tree.filter, text);
}

fn plain(ev: &KeyEvent) -> bool {
    !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo)
}

pub fn root_key(s: &mut Workspace, tab: TabId, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if tx.focused() != Some(ROOT_KEY) || !plain(ev) {
        return false;
    }
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    match &ev.key {
        Key::Down => b.tree.model.move_down(),
        Key::Up => b.tree.model.move_up(),
        Key::Enter => {
            let activation = b.tree.model.activate(&*b.session);
            open(b, activation, true, tx);
        }
        Key::Right => {
            let activation = b.tree.model.right(&*b.session);
            open(b, activation, true, tx);
        }
        Key::Left => b.tree.model.left(&*b.session),
        Key::Escape if b.tree.is_filtering() => set_filter_text(b, ""),
        Key::Backspace if b.tree.is_filtering() => {
            let mut text = b.tree.model.filter().to_string();
            text.pop();
            set_filter_text(b, &text);
        }
        Key::Text(typed) => {
            let typed = sanitize_filter(typed);
            if typed.is_empty() || !b.tree.model.searchable() {
                return false;
            }
            let filter = &mut b.tree.filter;
            filter.focused = true;
            filter.set_caret(filter.text.len());
            filter.insert(&typed);
            filter_changed(b);
        }
        _ => return false,
    }
    reveal(b, tx);
    s.sync_title(tx);
    true
}

pub fn filter_key(s: &mut Workspace, tab: TabId, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.mods != Mods::default() {
        return false;
    }
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    match ev.key {
        Key::Up => b.tree.model.move_up(),
        Key::Down => b.tree.model.move_down(),
        Key::Enter => {
            let activation = b.tree.model.activate(&*b.session);
            open(b, activation, true, tx);
        }
        Key::Escape if b.tree.is_filtering() => set_filter_text(b, ""),
        Key::Escape => tx.focus(ROOT_KEY, false),
        _ => return false,
    }
    reveal(b, tx);
    s.sync_title(tx);
    true
}

fn press_row(s: &mut Workspace, tab: TabId, id: &RowId, double: bool, tx: &mut Tx) {
    let Some(b) = s.browser_mut(tab) else {
        return;
    };
    b.clear_status();
    b.tree.menu = None;
    tx.focus(ROOT_KEY, false);
    let Some(ix) = find_row(&b.tree.model, id) else {
        return;
    };
    b.tree.model.cursor = ix;
    match b.tree.model.current() {
        Some(TreeRow::Schema(i)) => {
            let expanded = b.tree.model.nodes[i].expanded;
            b.tree.model.set_expanded(i, !expanded, &*b.session);
        }
        Some(TreeRow::Table(i, j)) => {
            let activation = b.tree.model.table_at(i, j);
            open(b, activation, double, tx);
        }
        Some(TreeRow::Note(i)) if !double => b.tree.model.retry_tables(i, &*b.session),
        _ => {}
    }
    s.sync_title(tx);
}

fn secondary_press_row(s: &mut Workspace, tab: TabId, id: &RowId, point: PointerPress) {
    let Some(b) = s.browser_mut(tab) else {
        return;
    };
    b.clear_status();
    if let Some(ix) = find_row(&b.tree.model, id) {
        b.tree.model.cursor = ix;
        b.tree.menu = Some(point);
    }
}

pub fn view(
    s: &Workspace,
    b: &Browser,
    t: &Theme,
    _cx: &mut Cx<Workspace>,
    layer: &mut Vec<El>,
) -> El {
    let tab = b.id;
    let title = row()
        .gap(6.0)
        .min_w(0.0)
        .center_y()
        .child(ui::icon_el(&icons::DATABASE, 14.0, t.muted))
        .child(
            text(db::render_text(&b.connection.name))
                .font(t.semibold(t.text_sm))
                .color(t.foreground)
                .truncate()
                .shrink()
                .min_w(0.0),
        );
    let refresh_button = IconButton::new(&icons::REFRESH_CW)
        .variant(IconButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .label("Refresh schemas")
        .help("Refresh schemas")
        .on_press_tx(move |s: &mut Workspace, _tx: &mut Tx| {
            if let Some(b) = s.browser_mut(tab) {
                b.clear_status();
                refresh(b);
            }
        })
        .build(&t.k)
        .key(REFRESH_KEY);
    let focused = s.active_id() == Some(tab) && s.dialog.is_none() && b.focused == Some(Pane::Tree);
    let root = col()
        .key(ROOT_KEY)
        .focusable(true)
        .no_focus_ring()
        .grow(1.0)
        .min_h(0.0)
        .min_w(0.0)
        .on_key(move |s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| {
            keys::route(s, ev, tx) || root_key(s, tab, ev, tx)
        })
        .child(filter_row(b, t))
        .child(
            col()
                .grow(1.0)
                .min_h(0.0)
                .min_w(0.0)
                .child(body(b, t, focused)),
        );
    let menu = context_menu("schema-tree-menu", root, b.tree.menu)
        .item(
            MenuItem::new("Show DDL")
                .icon(&icons::FILE_CODE)
                .shortcut(keys::label(Command::ShowDdl).unwrap_or_default())
                .on_select_tx(move |s: &mut Workspace, tx: &mut Tx| {
                    show_ddl(s, tab);
                    s.sync_title(tx);
                }),
        )
        .on_close(move |s: &mut Workspace| {
            if let Some(b) = s.browser_mut(tab) {
                b.tree.menu = None;
            }
        })
        .build(&t.k, layer)
        .grow(1.0)
        .min_h(0.0)
        .min_w(0.0);
    ui::pane("browser-tree", title, Some(refresh_button), menu, t)
}

fn filter_row(b: &Browser, t: &Theme) -> El {
    let tab = b.id;
    let mut input = TextInput::new(&b.tree.filter)
        .placeholder("Filter tables")
        .label("Filter tables")
        .leading_icon(&icons::SEARCH)
        .leading_icon_size(14.0)
        .text_style(t.text_sm)
        .width(None)
        .height(26.0)
        .disabled(!b.tree.model.searchable())
        .on_edit(move |s: &mut Workspace| workspace::browser_edit(s, tab, |b| &mut b.tree.filter))
        .on_change(move |s: &mut Workspace, _tx: &mut Tx| {
            if let Some(b) = s.browser_mut(tab) {
                filter_changed(b);
            }
        })
        .on_key(move |s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| {
            keys::field_key(s, ev, tx) || filter_key(s, tab, ev, tx)
        });
    if !b.tree.filter.text.is_empty() {
        input = input
            .clear_button()
            .on_action(move |s: &mut Workspace, tx: &mut Tx| {
                if let Some(b) = s.browser_mut(tab) {
                    set_filter_text(b, "");
                    b.tree.filter.focused = true;
                    reveal(b, tx);
                }
            });
    }
    el().w_full()
        .padding(6.0)
        .child(input.build(&t.k).key("schema-tree-filter"))
}

fn message(text_: &str, t: &Theme) -> El {
    row()
        .px(12.0)
        .py(8.0)
        .child(ui::label(text_, t.text_sm, t.muted))
}

fn body(b: &Browser, t: &Theme, focused: bool) -> El {
    let tab = b.id;
    let model = &b.tree.model;
    if let Some(error) = &model.schemas_error {
        return col()
            .key("schema-tree-error")
            .padding(12.0)
            .gap(8.0)
            .child(
                row()
                    .gap(6.0)
                    .center_y()
                    .child(ui::icon_el(&icons::TRIANGLE_ALERT, 14.0, t.danger))
                    .child(ui::label("Failed to load schemas", t.text_sm, t.danger)),
            )
            .child(
                text(error.clone())
                    .font(t.text_sm)
                    .color(t.danger)
                    .wrap()
                    .selectable(true),
            )
            .child(
                row().child(
                    Button::new("Retry")
                        .variant(ButtonVariant::Outline)
                        .control_size(ControlSize::Small)
                        .icon(&icons::REFRESH_CW)
                        .on_press_tx(move |s: &mut Workspace, _tx: &mut Tx| {
                            if let Some(b) = s.browser_mut(tab) {
                                b.clear_status();
                                load(b);
                            }
                        })
                        .build(&t.k)
                        .key(RETRY_KEY),
                ),
            );
    }
    if !model.schemas_loaded {
        return row()
            .key("schema-tree-loading")
            .px(12.0)
            .py(8.0)
            .gap(8.0)
            .center_y()
            .child(ui::spinner(14.0, t.muted, t))
            .child(ui::label("Loading schemas…", t.text_sm, t.muted));
    }
    if model.nodes.is_empty() {
        return message("No schemas", t);
    }
    let rows = Rc::new(model.rows());
    if rows.is_empty() {
        return message("No matches", t);
    }
    let list = col()
        .key("schema-tree-rows")
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            rows.len(),
            ROW_HEIGHT,
            |_s: &Workspace, i: usize| Cow::Owned(row_key(i)),
            move |s: &Workspace, i: usize| match (s.browser(tab), rows.get(i)) {
                (Some(b), Some(&row)) => row_view(b, s.theme(), i, row, focused),
                _ => el().h(ROW_HEIGHT),
            },
        )
        .virtual_rows_version(0u8);
    t.k.scroll_bar(
        b.scrolls.keep(
            SCROLL_KEY,
            col()
                .key(SCROLL_KEY)
                .grow(1.0)
                .min_h(0.0)
                .min_w(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(list),
        ),
    )
}

fn row_view(b: &Browser, t: &Theme, ix: usize, tree_row: TreeRow, focused: bool) -> El {
    let tab = b.id;
    let model = &b.tree.model;
    let Some(id) = row_id(model, tree_row) else {
        return el().h(ROW_HEIGHT);
    };
    let (depth, label, content) = match tree_row {
        TreeRow::Schema(i) => {
            let node = &model.nodes[i];
            let (chevron, folder) = if node.expanded {
                (&icons::CHEVRON_DOWN, &icons::FOLDER_OPEN)
            } else {
                (&icons::CHEVRON_RIGHT, &icons::FOLDER)
            };
            let name = db::render_text(&node.name);
            let content = row()
                .gap(4.0)
                .grow(1.0)
                .min_w(0.0)
                .center_y()
                .child(ui::icon_el(chevron, 12.0, t.muted))
                .child(ui::icon_el(folder, 14.0, t.muted))
                .child(
                    text(name.clone())
                        .font(t.text)
                        .color(t.foreground)
                        .truncate()
                        .shrink()
                        .min_w(0.0),
                )
                .children(node.request.is_some().then(|| {
                    row()
                        .grow(1.0)
                        .justify_end()
                        .child(ui::spinner(12.0, t.muted, t))
                }));
            (0.0, name, content)
        }
        TreeRow::Table(i, j) => {
            let node = &model.nodes[i];
            let Some(info) = node.tables.as_ref().and_then(|tables| tables.get(j)) else {
                return el().h(ROW_HEIGHT);
            };
            let is_open = b
                .tree
                .open_table
                .as_ref()
                .is_some_and(|(schema, table)| *schema == node.name && *table == info.name);
            let (glyph, suffix) = match info.kind {
                TableKind::Table => (&icons::TABLE, None),
                TableKind::View => (&icons::EYE, Some("view")),
                TableKind::MaterializedView => (&icons::ROWS_3, Some("mview")),
            };
            let name = db::render_text(&info.name);
            let (font, color) = if is_open {
                (t.semibold(t.text), t.yellow)
            } else {
                (t.text, t.foreground)
            };
            let content = row()
                .gap(6.0)
                .grow(1.0)
                .min_w(0.0)
                .center_y()
                .child(ui::icon_el(
                    glyph,
                    14.0,
                    if is_open { t.yellow } else { t.muted },
                ))
                .child(
                    text(name.clone())
                        .font(font)
                        .color(color)
                        .truncate()
                        .shrink()
                        .min_w(0.0),
                )
                .children(suffix.map(|suffix| ui::label(suffix, t.text_xs, t.muted)));
            (1.0, name, content)
        }
        TreeRow::Note(i) => {
            let node = &model.nodes[i];
            let (label, content) = if node.error.is_some() {
                let label = "failed · Enter or click to retry";
                (
                    label,
                    row()
                        .gap(4.0)
                        .center_y()
                        .child(ui::icon_el(&icons::TRIANGLE_ALERT, 12.0, t.danger))
                        .child(ui::label(label, t.text_sm, t.danger)),
                )
            } else if node.tables.is_some() {
                let label = "no tables";
                (label, row().child(ui::label(label, t.text_sm, t.muted)))
            } else {
                let label = "loading…";
                (
                    label,
                    row()
                        .gap(4.0)
                        .center_y()
                        .child(ui::spinner(12.0, t.muted, t))
                        .child(ui::label(label, t.text_sm, t.muted)),
                )
            };
            (1.0, label.to_string(), content)
        }
    };
    let selected = ix == model.cursor;
    let (fill, border) = match (selected, focused) {
        (true, true) => (t.list_active, t.list_active_border),
        (true, false) => (t.secondary, t.secondary),
        (false, _) => (Color::TRANSPARENT, Color::TRANSPARENT),
    };
    let (press, double) = (id.clone(), id.clone());
    let mut e = row()
        .role("treeitem")
        .label(label)
        .h(ROW_HEIGHT)
        .w_full()
        .pl(6.0 + INDENT * depth)
        .pr(8.0)
        .center_y()
        .fill(fill)
        .border_sides([0.0, 0.0, 0.0, 2.0], border)
        .cursor(Cursor::Pointer)
        .child(content)
        .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| press_row(s, tab, &press, false, tx))
        .on_double_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
            double_press_row(s, tab, &double, tx)
        });
    if !selected {
        e = e.hover(t.list_hover);
    }
    if let TreeRow::Schema(i) = tree_row {
        e = e.expanded(model.nodes[i].expanded);
    }
    if matches!(tree_row, TreeRow::Table(..)) {
        e = e.on_secondary_press(
            move |s: &mut Workspace, point: PointerPress, _tx: &mut Tx| {
                secondary_press_row(s, tab, &id, point)
            },
        );
    }
    e
}

fn double_press_row(s: &mut Workspace, tab: TabId, id: &RowId, tx: &mut Tx) {
    let Some(b) = s.browser_mut(tab) else {
        return;
    };
    let Some(ix) = find_row(&b.tree.model, id) else {
        return;
    };
    b.tree.model.cursor = ix;
    match b.tree.model.current() {
        Some(TreeRow::Schema(i)) => {
            let expanded = b.tree.model.nodes[i].expanded;
            b.tree.model.set_expanded(i, !expanded, &*b.session);
        }
        Some(TreeRow::Table(i, j)) => {
            let activation = b.tree.model.table_at(i, j);
            open(b, activation, true, tx);
        }
        _ => {}
    }
    s.sync_title(tx);
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::{
        Activation, ErrorRoute, SchemaNode, TreeModel, TreeRequests, TreeRow, sanitize_filter,
        visible_rows,
    };
    use crate::db::{TableInfo, TableKind};

    #[derive(Default)]
    struct Fake {
        next: Cell<u64>,
        sent: RefCell<Vec<String>>,
    }

    impl Fake {
        fn id(&self) -> u64 {
            self.next.set(self.next.get() + 1);
            self.next.get()
        }
    }

    impl TreeRequests for Fake {
        fn list_schemas(&self) -> u64 {
            self.sent.borrow_mut().push("schemas".to_string());
            self.id()
        }

        fn list_tables(&self, schema: &str) -> u64 {
            self.sent.borrow_mut().push(format!("tables {schema}"));
            self.id()
        }
    }

    fn table(name: &str) -> TableInfo {
        TableInfo {
            name: name.to_string(),
            kind: TableKind::Table,
        }
    }

    fn view(name: &str) -> TableInfo {
        TableInfo {
            name: name.to_string(),
            kind: TableKind::View,
        }
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn loaded(fake: &Fake) -> TreeModel {
        let mut model = TreeModel::default();
        model.request_schemas(fake);
        let id = model.schemas_request.unwrap();
        assert!(model.on_schemas(id, names(&["main", "aux"]), fake));
        let id = model.nodes[0].request.unwrap();
        assert!(model.on_tables(id, vec![table("users"), table("empty"), view("active")]));
        model
    }

    #[test]
    fn rows_filter_on_table_names_with_anded_terms() {
        let mut main = SchemaNode::new("main".to_string());
        main.expanded = true;
        main.tables = Some(vec![table("users"), table("empty"), view("active")]);
        let nodes = vec![main, SchemaNode::new("aux".to_string())];

        assert_eq!(
            visible_rows(&nodes, ""),
            [
                TreeRow::Schema(0),
                TreeRow::Table(0, 0),
                TreeRow::Table(0, 1),
                TreeRow::Table(0, 2),
                TreeRow::Schema(1)
            ]
        );
        assert_eq!(
            visible_rows(&nodes, "US"),
            [TreeRow::Schema(0), TreeRow::Table(0, 0)]
        );
        assert_eq!(
            visible_rows(&nodes, "e").len(),
            4,
            "users, empty and active match"
        );
        assert_eq!(
            visible_rows(&nodes, "e t"),
            [
                TreeRow::Schema(0),
                TreeRow::Table(0, 1),
                TreeRow::Table(0, 2)
            ]
        );
        assert_eq!(
            visible_rows(&nodes, "e ti"),
            [TreeRow::Schema(0), TreeRow::Table(0, 2)],
            "every term has to match"
        );
        assert_eq!(visible_rows(&nodes, "main"), [], "schema names never match");
        assert_eq!(visible_rows(&nodes, &"ы".repeat(200)), []);
    }

    #[test]
    fn shows_loading_then_the_tree() {
        let fake = Fake::default();
        let mut model = TreeModel::default();
        assert!(!model.searchable());
        model.request_schemas(&fake);
        assert!(!model.schemas_loaded);
        let model = {
            let id = model.schemas_request.unwrap();
            assert!(model.on_schemas(id, names(&["main", "aux"]), &fake));
            model
        };
        assert!(
            model.nodes[0].expanded,
            "the first schema is expanded on the first load"
        );
        assert!(!model.nodes[1].expanded);
        assert_eq!(
            model.rows(),
            [TreeRow::Schema(0), TreeRow::Note(0), TreeRow::Schema(1)]
        );
        assert_eq!(*fake.sent.borrow(), ["schemas", "tables main"]);
        assert!(model.searchable());
    }

    #[test]
    fn schemas_error_is_shown_and_can_be_retried() {
        let fake = Fake::default();
        let mut model = TreeModel::default();
        model.request_schemas(&fake);
        let id = model.schemas_request.unwrap();
        assert_eq!(model.on_error(id, "permission denied"), ErrorRoute::Shown);
        assert_eq!(model.schemas_error.as_deref(), Some("permission denied"));
        assert_eq!(
            model.on_error(id, "again"),
            ErrorRoute::NotOurs,
            "the id is spent"
        );

        model.request_schemas(&fake);
        assert!(model.schemas_request.is_some_and(|new| new != id));
        assert!(
            model.schemas_error.is_none(),
            "the pane shows Loading… again"
        );
        assert!(!model.schemas_loaded);
    }

    #[test]
    fn a_failed_refresh_keeps_the_tree() {
        let fake = Fake::default();
        let mut model = loaded(&fake);
        model.request_schemas(&fake);
        let id = model.schemas_request.unwrap();
        assert_eq!(
            model.on_error(id, "gone"),
            ErrorRoute::Status("Failed to refresh schemas: gone".to_string())
        );
        assert!(model.schemas_error.is_none());
        assert_eq!(model.rows().len(), 5);
    }

    #[test]
    fn tree_navigation_expands_and_collapses() {
        let fake = Fake::default();
        let mut model = loaded(&fake);
        assert_eq!(model.rows().len(), 5);

        model.move_down();
        model.move_down();
        model.left(&fake);
        assert_eq!(model.cursor, 0);
        model.left(&fake);
        assert_eq!(model.rows(), [TreeRow::Schema(0), TreeRow::Schema(1)]);
        assert_eq!(model.activate(&fake), None);
        assert_eq!(model.rows().len(), 5);
        assert!(model.nodes[0].request.is_none(), "tables are cached");

        for _ in 0..10 {
            model.move_down();
        }
        assert_eq!(model.cursor, 4);
        assert_eq!(model.right(&fake), None);
        let id = model.nodes[1].request.unwrap();
        assert_eq!(model.rows()[5], TreeRow::Note(1), "loading…");
        assert!(model.on_tables(id, Vec::new()));
        assert_eq!(model.nodes[1].tables.as_deref(), Some(&[][..]), "no tables");
        for _ in 0..10 {
            model.move_up();
        }
        assert_eq!(model.cursor, 0);
    }

    #[test]
    fn enter_and_right_open_tables() {
        let fake = Fake::default();
        let mut model = loaded(&fake);
        model.move_down();
        let users = Activation::OpenTable {
            schema: "main".to_string(),
            table: "users".to_string(),
            kind: TableKind::Table,
        };
        assert_eq!(model.activate(&fake), Some(users));
        model.move_down();
        let empty = Activation::OpenTable {
            schema: "main".to_string(),
            table: "empty".to_string(),
            kind: TableKind::Table,
        };
        assert_eq!(model.right(&fake), Some(empty));
        model.move_down();
        let active = Activation::OpenTable {
            schema: "main".to_string(),
            table: "active".to_string(),
            kind: TableKind::View,
        };
        assert_eq!(
            model.activate(&fake),
            Some(active),
            "views open with their kind"
        );
    }

    #[test]
    fn tables_error_can_be_retried() {
        let fake = Fake::default();
        let mut model = loaded(&fake);
        for _ in 0..4 {
            model.move_down();
        }
        model.activate(&fake);
        let id = model.nodes[1].request.unwrap();
        assert_eq!(
            model.on_error(id, "boom"),
            ErrorRoute::Status("Failed to list tables of aux: boom".to_string())
        );
        assert_eq!(model.nodes[1].error.as_deref(), Some("boom"));
        model.move_down();
        assert_eq!(model.current(), Some(TreeRow::Note(1)));
        model.activate(&fake);
        assert!(model.nodes[1].request.is_some_and(|new| new != id));
        assert!(model.nodes[1].error.is_none());
    }

    #[test]
    fn cursor_stays_on_its_item_when_tables_arrive_above_it() {
        let fake = Fake::default();
        let mut model = TreeModel::default();
        model.request_schemas(&fake);
        let id = model.schemas_request.unwrap();
        model.on_schemas(id, names(&["main", "aux"]), &fake);
        model.move_down();
        model.move_down();
        assert_eq!(model.current(), Some(TreeRow::Schema(1)));
        let id = model.nodes[0].request.unwrap();
        model.on_tables(id, vec![table("a"), table("b"), table("c")]);
        assert_eq!(model.current(), Some(TreeRow::Schema(1)));
    }

    #[test]
    fn refresh_keeps_expanded_schemas_and_the_cursor() {
        let fake = Fake::default();
        let mut model = loaded(&fake);
        model.move_down();
        model.move_down();
        model.request_schemas(&fake);
        let id = model.schemas_request.unwrap();
        assert!(model.on_schemas(id, names(&["new", "main"]), &fake));
        assert!(model.nodes[1].expanded);
        assert!(model.nodes[1].request.is_some());
        assert!(
            model.nodes[1].tables.is_some(),
            "old tables stay until the new ones arrive"
        );
        assert!(!model.nodes[0].expanded);
        assert_eq!(model.current(), Some(TreeRow::Table(1, 1)));
    }

    #[test]
    fn stale_responses_are_ignored() {
        let fake = Fake::default();
        let mut model = loaded(&fake);
        let old = model.nodes[0].request;
        assert!(old.is_none());
        model.request_schemas(&fake);
        let first = model.schemas_request.unwrap();
        model.request_schemas(&fake);
        assert!(
            !model.on_schemas(first, names(&["x"]), &fake),
            "superseded schemas request"
        );
        assert!(!model.on_tables(999, vec![table("x")]));
        assert_eq!(model.on_error(999, "late"), ErrorRoute::NotOurs);
        assert_eq!(model.rows().len(), 5);
    }

    #[test]
    fn typing_in_the_tree_filters_the_tables() {
        let fake = Fake::default();
        let mut model = loaded(&fake);
        model.set_filter(" ");
        assert_eq!(model.filter(), "", "a blank filter is cleared");
        model.set_filter(" US");
        assert_eq!(model.rows(), [TreeRow::Schema(0), TreeRow::Table(0, 0)]);
        assert_eq!(model.cursor, 1, "the first match is selected");

        model.set_filter("");
        assert_eq!(model.rows().len(), 5);
        assert_eq!(
            model.current(),
            Some(TreeRow::Table(0, 0)),
            "the cursor stays on its table"
        );
    }

    #[test]
    fn tree_search_edits_and_no_matches() {
        let fake = Fake::default();
        let mut model = loaded(&fake);
        model.set_filter("e");
        assert_eq!(model.rows().len(), 4);
        model.move_down();
        assert_eq!(model.current(), Some(TreeRow::Table(0, 1)));
        model.set_filter("e t");
        assert_eq!(model.cursor, 1, "the cursor returns to the first match");

        model.set_filter("sqrojkhl");
        assert!(model.rows().is_empty());
        model.move_down();
        assert_eq!(model.activate(&fake), None);
        model.left(&fake);
        assert_eq!(model.cursor, 0);
        model.set_filter("");
        assert_eq!(model.rows().len(), 5);
    }

    #[test]
    fn tree_search_follows_folding_and_tables_that_load_later() {
        let fake = Fake::default();
        let mut model = loaded(&fake);
        for _ in 0..4 {
            model.move_down();
        }
        model.right(&fake);
        let id = model.nodes[1].request.unwrap();
        model.set_filter("us");
        assert_eq!(
            model.rows(),
            [
                TreeRow::Schema(0),
                TreeRow::Table(0, 0),
                TreeRow::Schema(1),
                TreeRow::Note(1)
            ],
            "a schema that is still loading stays visible"
        );
        model.on_tables(id, vec![table("logs"), table("status")]);
        assert_eq!(
            model.rows(),
            [
                TreeRow::Schema(0),
                TreeRow::Table(0, 0),
                TreeRow::Schema(1),
                TreeRow::Table(1, 1)
            ]
        );
        assert_eq!(model.current(), Some(TreeRow::Table(0, 0)));

        model.left(&fake);
        model.left(&fake);
        assert_eq!(
            model.rows(),
            [TreeRow::Schema(0), TreeRow::Schema(1), TreeRow::Table(1, 1)],
            "a folded schema keeps its row while it has matches"
        );
        model.set_filter("use");
        assert_eq!(model.rows(), [TreeRow::Schema(0)]);
        assert_eq!(model.cursor, 0);
        model.right(&fake);
        assert_eq!(model.rows(), [TreeRow::Schema(0), TreeRow::Table(0, 0)]);
    }

    #[test]
    fn filter_text_drops_control_characters() {
        assert_eq!(sanitize_filter("ac\tt\r\n"), "act");
        assert_eq!(sanitize_filter("us ers"), "us ers");
    }
}
