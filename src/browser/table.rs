pub(crate) mod edit;
pub mod format;
pub(crate) mod grid;
pub(crate) mod key_picker;

#[cfg(test)]
mod ui_tests;

use std::rc::Rc;

use rune::{
    Cursor, Cx, El, KeyEvent, PointerButton, Tx, auto_grow, col, el, row, text, text_input,
};
use rune_kit::{Button, ButtonVariant, Checkbox, ControlSize, IconButton, IconButtonVariant};

use super::{
    Browser,
    confirm::{Confirm, Pending},
};
use crate::{
    connection::RowKeys,
    db::{self, ColumnDef, ColumnInfo, KeySource, Page, RowUpdate, TableDdl, TableKind},
    dialog::Dialog,
    guard, icons,
    keys::{self, Command, Ctx},
    theme::Theme,
    ui::{self, Scroll, ScrollY},
    workspace::{self, TabId, Workspace},
};

use self::{
    edit::{Draft, Exact},
    format::{
        CUT_NOTE, MAX_LABEL_WIDTH, NULL, cell_copy_text, copied, definition, full_value, key_chip,
        kind_label, normalize, page_footer, record_footer, row_copy_text,
    },
    grid::{COUNTING_HINT, DataGrid, Grid, GridData, GridSpec, Move, Notice, PageArrival, Update},
};

const KEY: &str = "table-view";
pub const PREFIX: &str = "table";
pub const RECORD_SCROLL: &str = "table-record-scroll";
const LINE_STEP: f32 = 24.0;
const KEY_ICON: f32 = 12.0;

pub struct TableView {
    pub grid: Option<Grid>,
    pub record_scroll: ScrollY,
    pub counting_hint_shown: bool,
    pub save_request: Option<(u64, String)>,
    pub draft: Option<Draft>,
    pub cells: DataGrid,
    pub refocus: bool,
    pub grid_offset: Option<[f32; 2]>,
    pub keyed: bool,
    pub detached: bool,
    pub follow: Option<(usize, edit::Follow)>,
    pub reveal_grid: bool,
}

impl TableView {
    pub fn new() -> Self {
        Self {
            grid: None,
            record_scroll: ScrollY::default(),
            counting_hint_shown: false,
            save_request: None,
            draft: None,
            cells: DataGrid::default(),
            refocus: false,
            grid_offset: None,
            keyed: false,
            detached: false,
            follow: None,
            reveal_grid: false,
        }
    }

    pub fn current(&self) -> Option<(String, String)> {
        self.grid
            .as_ref()
            .map(|grid| (grid.schema().to_string(), grid.table().to_string()))
    }

    pub fn title_context(&self) -> Option<String> {
        let grid = self.grid.as_ref()?;
        let (schema, name) = (grid.schema(), grid.table());
        Some(if schema.is_empty() {
            name.to_string()
        } else {
            format!("{schema}.{name}")
        })
    }

    pub fn draft(&self) -> Option<&Draft> {
        self.draft.as_ref()
    }

    pub fn footer(&self) -> Option<String> {
        self.grid.as_ref().map(footer_text)
    }

    pub fn selected_cell(&self) -> Option<(usize, Option<usize>)> {
        self.cells.cell
    }

    pub fn highlighted_row(&self) -> Option<usize> {
        self.cells.highlight
    }

    pub fn typing(&self) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|draft| draft.focused().is_some())
    }

    pub fn grid_shown(&self) -> bool {
        self.grid.as_ref().is_some_and(|grid| {
            grid.error().is_none()
                && !grid.record_open()
                && grid.page().is_some_and(|page| !page.columns.is_empty())
        })
    }

    fn record_open(&self) -> bool {
        self.grid.as_ref().is_some_and(Grid::record_open)
    }
}

impl Default for TableView {
    fn default() -> Self {
        Self::new()
    }
}

fn footer_text(grid: &Grid) -> String {
    match grid.page() {
        Some(page) if grid.record_open() => record_footer(page.offset, grid.row()),
        Some(page) => page_footer(page, grid.total(), grid.count_failed()),
        None => String::new(),
    }
}

fn status(b: &mut Browser, text: impl Into<String>, error: bool) {
    b.table.counting_hint_shown = false;
    b.set_status(text, error);
}

fn notice(b: &mut Browser, notice: Notice) {
    match notice {
        Notice::Hint(text) => {
            status(b, text, false);
            b.table.counting_hint_shown = text == COUNTING_HINT;
        }
        Notice::Error(text) => status(b, text, true),
        Notice::ClearHint => {
            if b.table.counting_hint_shown {
                b.table.counting_hint_shown = false;
                b.clear_hint();
            }
        }
    }
}

fn apply(b: &mut Browser, update: Update) -> bool {
    match update {
        Update::Stale => return false,
        Update::Done => {}
        Update::Notify(n) => notice(b, n),
    }
    true
}

fn sync_table(b: &mut Browser, to_top: bool) {
    let view = &mut b.table;
    match view.grid.as_ref().and_then(Grid::page) {
        Some(page) => {
            let cell = grid::mono_cell_width(crate::theme::theme(true));
            view.cells
                .load(&page.columns, &page.rows, page.offset, cell);
        }
        None => view.cells = DataGrid::default(),
    }
    if to_top {
        view.cells.to_top = true;
    } else {
        view.cells.to_selection = true;
    }
    select_in_table(b);
}

fn select_in_table(b: &mut Browser) {
    let view = &mut b.table;
    let (rows, row, columns) = view.grid.as_ref().map_or((0, 0, 0), |grid| {
        (
            grid.row_count(),
            grid.row(),
            grid.page().map_or(0, |page| page.columns.len()),
        )
    });
    if rows == 0 || columns == 0 {
        view.cells.clear();
        return;
    }
    let col = view
        .cells
        .cell
        .map_or(Some(0), |(_, col)| col.map(|col| col.min(columns - 1)));
    view.cells.select(row, col);
}

fn reveal(b: &mut Browser, tx: &mut Tx) {
    if b.table.grid_shown() {
        b.table.cells.reveal(PREFIX, tx);
    }
}

pub fn open(b: &mut Browser, schema: &str, table: &str, kind: TableKind) {
    if b.table
        .grid
        .as_ref()
        .is_some_and(|grid| grid.is(schema, table))
    {
        return;
    }
    b.table.grid = Some(Grid::open(schema, table, kind, &*b.session));
    b.table.draft = None;
    b.table.follow = None;
    b.table.counting_hint_shown = false;
    b.table.record_scroll.reset();
    b.table.cells.cell = None;
    b.table.grid_offset = None;
    b.table.cells.to_left = true;
    sync_table(b, true);
    b.tree
        .set_open_table(Some((schema.to_string(), table.to_string())));
}

pub fn reload(b: &mut Browser) {
    if let Some(grid) = &mut b.table.grid {
        grid.refresh(&*b.session);
    }
}

pub fn on_page(b: &mut Browser, id: u64, page: Page) -> bool {
    let Some(grid) = &mut b.table.grid else {
        return false;
    };
    let before = (grid.row(), grid.record_open());
    let arrival = grid.on_page(id, page, &*b.session);
    if arrival == PageArrival::Stale {
        return false;
    }
    let follow = b
        .table
        .follow
        .take()
        .filter(|(row, _)| arrival == PageArrival::Refreshed && before == (*row, true));
    if arrival == PageArrival::Moved {
        b.table.record_scroll.reset();
    }
    let lost = follow.and_then(|(row, values)| follow_row(b, row, &values));
    settle_draft(b);
    sync_table(b, arrival == PageArrival::Moved);
    if let Some(row) = lost {
        status(
            b,
            format!("Saved {row}; it is no longer on this page"),
            false,
        );
    }
    true
}

fn follow_row(b: &mut Browser, row: usize, follow: &edit::Follow) -> Option<String> {
    let grid = b.table.grid.as_mut().filter(|grid| grid.record_open())?;
    let page = grid.page()?;
    let found = edit::find_row(page, follow);
    let label = record_footer(page.offset, row);
    match found {
        Some(found) => {
            if found != row {
                grid.set_row(found);
            }
            None
        }
        None => {
            grid.close_record();
            b.table.reveal_grid = true;
            Some(label)
        }
    }
}

fn settle_draft(b: &mut Browser) {
    let Some(draft) = &b.table.draft else {
        return;
    };
    let shown = b.table.grid.as_ref().is_some_and(|grid| {
        grid.record_open()
            && grid.error().is_none()
            && grid.row() == draft.row()
            && grid.page().is_some_and(|page| {
                edit::target(
                    page,
                    draft.row(),
                    grid.kind(),
                    chosen_key(&b.row_keys, grid),
                )
                .is_ok_and(|target| target.key == draft.target().key)
            })
    });
    if shown {
        return;
    }
    let lost = (draft.changes() > 0 && !draft.saving).then(|| draft.target().row_label.clone());
    drop_draft(b);
    b.table.detached = false;
    if let Some(row) = lost {
        status(
            b,
            format!("Unsaved changes in {row} were thrown away: the row is no longer shown"),
            true,
        );
    }
}

pub fn on_count(b: &mut Browser, id: u64, count: u64) -> bool {
    let Some(grid) = &mut b.table.grid else {
        return false;
    };
    let update = grid.on_count(id, count, &*b.session);
    apply(b, update)
}

pub fn on_ddl(b: &mut Browser, id: u64, ddl: TableDdl) -> bool {
    b.table
        .grid
        .as_mut()
        .is_some_and(|grid| grid.on_ddl(id, ddl))
}

pub fn on_error(b: &mut Browser, id: u64, message: &str) -> bool {
    if b.table
        .save_request
        .as_ref()
        .is_some_and(|(awaited, _)| *awaited == id)
    {
        on_save_failed(b, message);
        return true;
    }
    let Some(grid) = &mut b.table.grid else {
        return false;
    };
    let update = grid.on_error(id, message);
    let applied = apply(b, update);
    if applied {
        settle_draft(b);
    }
    applied
}

pub fn on_row_updated(b: &mut Browser, id: u64) -> bool {
    if b.table
        .save_request
        .as_ref()
        .is_none_or(|(awaited, _)| *awaited != id)
    {
        return false;
    }
    let (_, row) = b.table.save_request.take().unwrap_or_default();
    let ctid = b
        .table
        .grid
        .as_ref()
        .and_then(Grid::page)
        .is_some_and(|page| page.key.source == KeySource::Ctid);
    if let Some(draft) = b.table.draft.as_ref().filter(|draft| draft.saving) {
        let unordered = ctid || draft.target().chosen;
        b.table.follow = unordered.then(|| (draft.row(), draft.follow()));
        drop_draft(b);
    }
    if let Some(grid) = &mut b.table.grid {
        grid.refresh(&*b.session);
    }
    status(b, format!("Saved {row}"), false);
    true
}

fn on_save_failed(b: &mut Browser, message: &str) {
    let (_, row) = b.table.save_request.take().unwrap_or_default();
    match b.table.draft.as_mut().filter(|draft| draft.saving) {
        Some(draft) => {
            draft.saving = false;
            draft.error = Some(draft.target().save_error(message));
        }
        None => status(b, format!("Failed to save {row}: {message}"), true),
    }
}

pub fn poll(b: &mut Browser, tx: &mut Tx) -> bool {
    let mut changed = false;
    if std::mem::take(&mut b.table.refocus) {
        tx.focus(KEY, false);
        changed = true;
    }
    if std::mem::take(&mut b.table.reveal_grid) {
        let from = b.table.grid_offset.take().unwrap_or_default();
        if b.table.grid_shown() {
            b.table.cells.to_selection = false;
            b.table.cells.reveal_from(PREFIX, from, tx);
        }
        changed = true;
    }
    if std::mem::take(&mut b.table.cells.to_left) {
        b.table.cells.to_top = false;
        b.table.cells.scroll.reset();
        tx.scroll_to_instant(format!("{PREFIX}-grid-scroll"), 0.0, 0.0);
    } else if std::mem::take(&mut b.table.cells.to_top) {
        tx.scroll_to_y(format!("{PREFIX}-grid-scroll"), 0.0);
    }
    if std::mem::take(&mut b.table.cells.to_selection) && b.table.grid_shown() {
        b.table.cells.reveal(PREFIX, tx);
    }
    let field = b.table.draft.as_ref().and_then(|draft| {
        let index = draft.open_field()?;
        Some(draft.field(index).is_some_and(|field| field.input.focused))
    });
    let keyed = std::mem::take(&mut b.table.keyed);
    match field {
        Some(true) | None => b.table.detached = false,
        Some(false) if b.table.detached => {}
        Some(false) if keyed => b.table.detached = true,
        Some(false) => {
            collapse_field(b, tx);
            changed = true;
        }
    }
    changed
}

pub fn focus(_b: &mut Browser, tx: &mut Tx) {
    tx.focus(KEY, false);
}

pub fn contexts(b: &Browser, focused: Option<&str>, out: &mut Vec<Ctx>) {
    if b.table.typing() {
        out.push(Ctx::RowEditorInput);
        out.push(Ctx::TableView);
        return;
    }
    match focused {
        Some(KEY) => {
            if b.table.grid_shown() {
                out.push(Ctx::TableGrid);
            }
            out.push(Ctx::TableView);
        }
        Some(key) if key.starts_with("table-") => out.push(Ctx::TableView),
        _ => {}
    }
}

fn drop_draft(b: &mut Browser) {
    if let Some(draft) = b.table.draft.take()
        && draft.focused().is_some()
    {
        b.table.refocus = true;
    }
}

fn end_edit(b: &mut Browser, tx: &mut Tx) {
    let Some(draft) = b.table.draft.take() else {
        return;
    };
    if draft.focused().is_some() {
        tx.focus(KEY, false);
    }
}

fn leave_draft(b: &mut Browser, tx: &mut Tx) -> bool {
    let Some(draft) = &b.table.draft else {
        return true;
    };
    if draft.changes() > 0 || draft.saving {
        let row = draft.target().row_label.clone();
        status(
            b,
            format!("Unsaved changes in {row}: ⌘↵ saves them, Esc throws them away"),
            true,
        );
        return false;
    }
    end_edit(b, tx);
    true
}

fn collapse_field(b: &mut Browser, tx: &mut Tx) {
    let Some(draft) = b
        .table
        .draft
        .as_mut()
        .filter(|draft| draft.open_field().is_some())
    else {
        return;
    };
    let had_focus = draft.focused().is_some();
    draft.close();
    draft.blur();
    b.table.detached = false;
    if draft.is_idle() && !draft.saving {
        b.table.draft = None;
    }
    if had_focus {
        tx.focus(KEY, false);
    }
}

fn note_press(b: &mut Browser, tx: &mut Tx) {
    b.clear_status();
    collapse_field(b, tx);
}

fn chosen_key<'a>(row_keys: &'a RowKeys, grid: &Grid) -> Option<&'a [String]> {
    row_keys
        .get(&(grid.schema().to_string(), grid.table().to_string()))
        .map(Vec::as_slice)
}

enum Begin {
    Done,
    ChooseKey { column: usize, stale: bool },
}

fn edit_value(b: &mut Browser, column: Option<usize>, fallback: bool, tx: &mut Tx) -> Begin {
    if b.connection.read_only {
        status(b, super::READ_ONLY_EDIT, false);
        return Begin::Done;
    }
    let Some(grid) = &b.table.grid else {
        return Begin::Done;
    };
    let Some(page) = grid.page().filter(|page| !page.rows.is_empty()) else {
        return Begin::Done;
    };
    if grid.error().is_some() {
        return Begin::Done;
    }
    let row = grid.row();
    if b.table
        .draft
        .as_ref()
        .is_none_or(|draft| draft.row() != row)
    {
        if b.table.save_request.is_some() {
            status(b, "Still saving the last change…", false);
            return Begin::Done;
        }
        match edit::target(page, row, grid.kind(), chosen_key(&b.row_keys, grid)) {
            Ok(target) => b.table.draft = Some(Draft::new(target, row)),
            Err(edit::Refusal::Reason(reason)) => {
                status(b, reason, true);
                return Begin::Done;
            }
            Err(edit::Refusal::ChooseKey { stale }) => {
                return Begin::ChooseKey {
                    column: column.unwrap_or(0),
                    stale,
                };
            }
        }
    }
    if !b.table.record_open() {
        open_record(b, tx);
    }
    let Some(draft) = &mut b.table.draft else {
        return Begin::Done;
    };
    let wanted = column.unwrap_or(0);
    let index = if draft.editable(wanted) {
        Some(wanted)
    } else if fallback || column.is_none() {
        draft.first_editable(wanted)
    } else {
        None
    };
    let Some(index) = index else {
        let note = draft
            .target()
            .fields
            .get(wanted)
            .and_then(|field| field.locked);
        if draft.changes() == 0 && draft.focused().is_none() {
            b.table.draft = None;
        }
        status(
            b,
            note.unwrap_or("No value of this row can be edited"),
            false,
        );
        return Begin::Done;
    };
    draft.open(index);
    tx.scroll_into_view_in(RECORD_SCROLL, format!("table-field-{index}"));
    Begin::Done
}

fn begin_edit(s: &mut Workspace, tab: TabId, column: Option<usize>, fallback: bool, tx: &mut Tx) {
    let Some(b) = s.browser_mut(tab) else {
        return;
    };
    if let Begin::ChooseKey { column, stale } = edit_value(b, column, fallback, tx) {
        key_picker::open(s, tab, Some(column), stale);
    }
}

fn on_draft_change(b: &mut Browser, index: usize) {
    if let Some(draft) = &mut b.table.draft {
        draft.on_change(index);
    }
}

pub fn set_null(b: &mut Browser, index: usize, null: bool) {
    if let Some(draft) = b.table.draft.as_mut().filter(|draft| !draft.saving) {
        draft.set_null(index, null);
    }
}

fn save_draft(s: &mut Workspace, tab: TabId) {
    let Some((b, dialog)) = s.browser_and_dialog(tab) else {
        return;
    };
    let Some(draft) = b.table.draft.as_mut().filter(|draft| !draft.saving) else {
        return;
    };
    let Some(update) = draft.update() else {
        draft.error = Some("Nothing changed".to_string());
        return;
    };
    if b.connection.read_only {
        draft.error = Some(format!("Not saved: {}", super::READ_ONLY_EDIT));
        return;
    }
    if b.connection.asks_to_confirm() {
        let statement = match guard::row_update(b.connection.kind, &update) {
            Ok(statement) => statement,
            Err(err) => {
                draft.error = Some(format!("Not saved: {err}"));
                return;
            }
        };
        *dialog = Some(Dialog::Confirm(Box::new(Confirm {
            tab,
            connection: b.connection.name.clone(),
            statements: vec![statement],
            pending: Pending::Row {
                row: draft.target().row_label.clone(),
                update,
            },
        })));
        return;
    }
    send_update(b, update);
}

pub(super) fn save_confirmed(b: &mut Browser, update: RowUpdate) {
    let current = b
        .table
        .draft
        .as_ref()
        .filter(|draft| !draft.saving)
        .and_then(|draft| draft.update());
    if current.as_ref() != Some(&update) {
        return status(b, "Not saved: the row is no longer being edited", true);
    }
    send_update(b, update);
}

fn send_update(b: &mut Browser, update: RowUpdate) {
    let Some(draft) = b.table.draft.as_mut() else {
        return;
    };
    let id = b.session.update_row(update);
    b.table.save_request = Some((id, draft.target().row_label.clone()));
    draft.saving = true;
    draft.error = None;
}

fn step_field(b: &mut Browser, forward: bool, tx: &mut Tx) {
    let Some(draft) = &mut b.table.draft else {
        return;
    };
    if let Some(index) = draft.next(forward) {
        draft.open(index);
        tx.scroll_into_view_in(RECORD_SCROLL, format!("table-field-{index}"));
    }
}

fn page_op(
    b: &mut Browser,
    tx: &mut Tx,
    op: impl FnOnce(&mut Grid, &db::Session) -> Option<Notice>,
) {
    let session = b.session.clone();
    let Some(grid) = b.table.grid.as_mut().filter(|grid| !grid.record_open()) else {
        return;
    };
    let row = grid.row();
    let result = op(grid, &session);
    let row_changed = grid.row() != row;
    if let Some(n) = result {
        notice(b, n);
    }
    if row_changed {
        select_in_table(b);
        reveal(b, tx);
    }
}

fn open_record(b: &mut Browser, tx: &mut Tx) {
    let session = b.session.clone();
    let Some(grid) = &mut b.table.grid else {
        return;
    };
    let offset = b.table.cells.scroll.get().offset;
    if grid.open_record(&*session) {
        b.table.grid_offset = Some(offset);
        b.table.record_scroll.reset();
        tx.scroll_to_y(RECORD_SCROLL, 0.0);
        tx.focus(KEY, false);
    }
}

fn close_record(b: &mut Browser, tx: &mut Tx) {
    if !leave_draft(b, tx) {
        return;
    }
    if let Some(grid) = &mut b.table.grid {
        grid.close_record();
    }
    select_in_table(b);
    let from = b.table.grid_offset.take().unwrap_or_default();
    b.table.cells.reveal_from(PREFIX, from, tx);
    tx.focus(KEY, false);
}

fn step_record(b: &mut Browser, delta: isize, tx: &mut Tx) -> bool {
    if !b.table.record_open() {
        return false;
    }
    if !leave_draft(b, tx) {
        return true;
    }
    let Some(grid) = b.table.grid.as_mut() else {
        return true;
    };
    if grid.step_record(delta) {
        b.table.record_scroll.reset();
        tx.scroll_to_y(RECORD_SCROLL, 0.0);
        select_in_table(b);
    }
    true
}

fn copy(b: &mut Browser, column: Option<usize>, tx: &mut Tx) {
    let Some(grid) = &b.table.grid else {
        return;
    };
    let Some(page) = grid.page().filter(|page| !page.rows.is_empty()) else {
        return;
    };
    let text = match column {
        Some(column) => cell_copy_text(page, grid.row(), column),
        None => row_copy_text(page, grid.row()),
    };
    let message = copied(&text);
    tx.copy(text);
    status(b, message, false);
}

fn toggle_record(b: &mut Browser, tx: &mut Tx) {
    if b.table.record_open() {
        close_record(b, tx);
    } else {
        open_record(b, tx);
    }
}

pub fn run(s: &mut Workspace, tab: TabId, cmd: Command, tx: &mut Tx) -> bool {
    use Command::*;
    let focused = tx.focused().map(str::to_owned);
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    let typing = b.table.typing();
    match cmd {
        NextPage => {
            if typing {
                return false;
            }
            page_op(b, tx, |grid, session| grid.next_page(session));
        }
        PrevPage => {
            if typing {
                return false;
            }
            if b.table.record_open() {
                close_record(b, tx);
            } else {
                page_op(b, tx, |grid, session| grid.previous_page(session));
            }
        }
        FirstPage => {
            page_op(b, tx, |grid, session| {
                grid.first_page(session);
                None
            });
            if b.table.grid_shown() {
                tx.scroll_to_y(format!("{PREFIX}-grid-scroll"), 0.0);
            }
        }
        LastPage => page_op(b, tx, |grid, session| grid.last_page(session)),
        ToggleRecord => {
            if typing {
                return false;
            }
            toggle_record(b, tx);
        }
        CloseRecord => {
            if b.table.draft.is_some() {
                end_edit(b, tx);
            } else if b.table.record_open() {
                close_record(b, tx);
            } else if !(focused.as_deref() == Some(KEY) && b.table.grid_shown()) {
                return false;
            }
        }
        NextRecord => return step_record(b, 1, tx),
        PrevRecord => return step_record(b, -1, tx),
        CopyCell => {
            if typing {
                return false;
            }
            let column = b.table.cells.cell.and_then(|(_, col)| col);
            copy(b, column, tx);
        }
        CopyRow => copy(b, None, tx),
        EditRow => {
            let column = b.table.cells.cell.and_then(|(_, col)| col);
            begin_edit(s, tab, column, true, tx);
        }
        ChooseKey => {
            if typing || !key_picker::open(s, tab, None, false) {
                return false;
            }
        }
        SaveRow => {
            if b.table.draft.is_none() {
                return false;
            }
            save_draft(s, tab);
        }
        CancelEdit => end_edit(b, tx),
        NextField => step_field(b, true, tx),
        PrevField => step_field(b, false, tx),
        _ => return false,
    }
    true
}

fn raw_key(s: &mut Workspace, tab: TabId, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if tx.focused() != Some(KEY) {
        return false;
    }
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    if b.table.record_open() {
        let mods = ev.mods;
        if mods.cmd || mods.ctrl || mods.alt || mods.logo || mods.shift {
            return false;
        }
        return match ev.key {
            rune::Key::Right => step_record(b, 1, tx),
            rune::Key::Left => step_record(b, -1, tx),
            rune::Key::Enter => {
                close_record(b, tx);
                true
            }
            _ => match Scroll::from_key(ev) {
                Some(how) => {
                    b.table
                        .record_scroll
                        .scroll(tx, RECORD_SCROLL, how, LINE_STEP);
                    true
                }
                None => false,
            },
        };
    }
    if !b.table.grid_shown() {
        return false;
    }
    let Some(how) = Move::from_key(ev) else {
        return false;
    };
    let (rows, cols) = b
        .table
        .grid
        .as_ref()
        .and_then(Grid::page)
        .map_or((0, 0), |page| (page.rows.len(), page.columns.len()));
    if b.table.cells.step(how, rows, cols) {
        if let (Some(grid), Some((row, _))) = (&mut b.table.grid, b.table.cells.cell) {
            grid.set_row(row);
        }
        b.table.cells.reveal(PREFIX, tx);
    }
    true
}

fn select_cell(s: &mut Workspace, tab: TabId, row: usize, col: Option<usize>, tx: &mut Tx) {
    let Some(b) = s.browser_mut(tab) else {
        return;
    };
    note_press(b, tx);
    if let Some(grid) = &mut b.table.grid {
        grid.set_row(row);
    }
    b.table.cells.select(row, col);
}

fn open_cell(s: &mut Workspace, tab: TabId, row: usize, col: Option<usize>, tx: &mut Tx) {
    select_cell(s, tab, row, col, tx);
    if let Some(b) = s.browser_mut(tab) {
        open_record(b, tx);
    }
}

fn grid_spec(tab: TabId) -> GridSpec<Workspace> {
    GridSpec {
        prefix: PREFIX.to_string(),
        get: Rc::new(move |s: &Workspace| {
            let b = s.browser(tab)?;
            let page = b.table.grid.as_ref()?.page()?;
            Some((
                &b.table.cells,
                GridData {
                    columns: &page.columns,
                    rows: &page.rows,
                    offset: page.offset,
                },
            ))
        }),
        get_mut: Rc::new(move |s: &mut Workspace| s.browser_mut(tab).map(|b| &mut b.table.cells)),
        on_select: Rc::new(move |s: &mut Workspace, row, col, tx: &mut Tx| {
            select_cell(s, tab, row, col, tx)
        }),
        on_open: Rc::new(move |s: &mut Workspace, row, col, tx: &mut Tx| {
            open_cell(s, tab, row, col, tx)
        }),
        on_touch: Rc::new(move |s: &mut Workspace, tx: &mut Tx| {
            if let Some(b) = s.browser_mut(tab) {
                note_press(b, tx);
            }
        }),
        empty: "No rows",
    }
}

fn with_browser(
    tab: TabId,
    f: impl Fn(&mut Browser, &mut Tx) + 'static,
) -> impl Fn(&mut Workspace, &mut Tx) + 'static {
    move |s: &mut Workspace, tx: &mut Tx| {
        if let Some(b) = s.browser_mut(tab) {
            note_press(b, tx);
            f(b, tx);
        }
    }
}

fn icon_button(
    glyph: &'static rune::Icon,
    label: &str,
    shortcut: Option<Command>,
    disabled: bool,
    t: &Theme,
    press: impl Fn(&mut Workspace, &mut Tx) + 'static,
) -> El {
    let mut button = IconButton::new(glyph)
        .variant(IconButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .label(label)
        .help(label)
        .disabled(disabled)
        .on_press_tx(press);
    if let Some(chord) = shortcut.and_then(keys::label) {
        button = button.shortcut(chord);
    }
    button.build(&t.k).focusable(false).no_focus_ring()
}

fn title_view(b: &Browser, grid: &Grid, t: &Theme) -> El {
    let tab = b.id;
    let record = grid.record_open();
    let name = format!(
        "{}.{}",
        db::render_text(grid.schema()),
        db::render_text(grid.table())
    );
    let glyph = match grid.kind() {
        TableKind::Table => &icons::TABLE,
        TableKind::View => &icons::EYE,
        TableKind::MaterializedView => &icons::ROWS_3,
    };
    let mut title = text(name)
        .key("table-title")
        .font(t.bold(t.text_sm))
        .color(t.blue)
        .truncate()
        .min_w(0.0);
    if record {
        title = title
            .cursor(Cursor::Pointer)
            .help(format!(
                "Back to the table  {}",
                keys::label(Command::CloseRecord).unwrap_or_default()
            ))
            .on_press_tx(with_browser(tab, close_record));
    }
    row()
        .gap(6.0)
        .shrink()
        .center_y()
        .children(record.then(|| {
            icon_button(
                &icons::ARROW_LEFT,
                "Back",
                Some(Command::CloseRecord),
                false,
                t,
                with_browser(tab, close_record),
            )
            .key("record-back")
        }))
        .child(ui::icon_el(glyph, 14.0, t.muted))
        .child(title)
        .children(kind_label(grid.kind()).map(|label| {
            row()
                .key("table-kind")
                .px(4.0)
                .radius(3.0)
                .border(1.0, t.border)
                .child(ui::label(label, t.text_xs, t.muted))
        }))
        .children(record.then(|| ui::label("›", t.text_sm, t.muted)))
        .children(record.then(|| {
            ui::label(record_footer(grid.offset(), grid.row()), t.text_sm, t.muted)
                .key("table-record-row")
        }))
}

fn row_key_chip(b: &Browser, grid: &Grid, t: &Theme) -> Option<El> {
    if grid.kind() != TableKind::Table {
        return None;
    }
    let page = grid.page()?;
    let chosen = chosen_key(&b.row_keys, grid).filter(|chosen| edit::usable_key(page, chosen));
    let (label, help) = key_chip(&page.key, chosen)?;
    let chip = row()
        .key("table-row-key")
        .px(4.0)
        .mr(4.0)
        .radius(3.0)
        .border(1.0, t.border)
        .min_w(0.0)
        .shrink()
        .label(label.clone())
        .help(help)
        .child(ui::label(label, t.text_xs, t.muted).truncate().min_w(0.0));
    if page.key.source != KeySource::None {
        return Some(chip);
    }
    let tab = b.id;
    Some(
        chip.cursor(Cursor::Pointer)
            .hover(t.table_hover)
            .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                if let Some(b) = s.browser_mut(tab) {
                    note_press(b, tx);
                }
                key_picker::open(s, tab, None, false);
            }),
    )
}

fn header_extra(b: &Browser, grid: &Grid, t: &Theme) -> El {
    let tab = b.id;
    row()
        .gap(4.0)
        .center_y()
        .children(row_key_chip(b, grid, t))
        .children(grid.is_loading().then(|| {
            row()
                .key("table-loading")
                .gap(4.0)
                .mr(4.0)
                .center_y()
                .child(ui::spinner(12.0, t.muted, t))
                .child(ui::label("loading…", t.text_xs, t.muted))
        }))
        .child(
            icon_button(
                &icons::REFRESH_CW,
                "Reload the table",
                Some(Command::Refresh),
                false,
                t,
                with_browser(tab, |b, tx| {
                    reload(b);
                    focus(b, tx);
                }),
            )
            .help(format!(
                "Reload the page and the row count  {}",
                keys::label(Command::Refresh).unwrap_or_default()
            ))
            .key("table-refresh"),
        )
}

fn error_view(b: &Browser, error: &str, t: &Theme) -> El {
    let tab = b.id;
    col()
        .key("table-error")
        .grow(1.0)
        .w_full()
        .center()
        .gap(8.0)
        .padding(16.0)
        .child(
            row()
                .gap(8.0)
                .max_w(640.0)
                .items_start()
                .child(ui::icon_el(&icons::CIRCLE_X, 16.0, t.danger))
                .child(
                    text(error.to_string())
                        .font(t.text)
                        .color(t.danger)
                        .wrap_anywhere()
                        .min_w(0.0)
                        .shrink(),
                ),
        )
        .child(
            Button::new("Retry")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .icon(&icons::REFRESH_CW)
                .help("Reload the page")
                .shortcut(keys::label(Command::Refresh).unwrap_or_default())
                .on_press_tx(with_browser(tab, |b, tx| {
                    reload(b);
                    focus(b, tx);
                }))
                .build(&t.k)
                .focusable(false)
                .no_focus_ring()
                .key("table-retry"),
        )
}

fn label_width(columns: &[ColumnInfo], defs: &[ColumnDef], key: &[String], t: &Theme) -> f32 {
    let digit = grid::mono_cell_width(t);
    let mark = ui::text_width("•", t.mono) + 4.0;
    let widest = columns
        .iter()
        .map(|column| {
            let icon = if key.contains(&column.name) {
                KEY_ICON + 4.0
            } else {
                0.0
            };
            (icon + ui::text_width(&column.name, t.bold(t.mono)) + mark)
                .max(ui::text_width(&normalize(definition(defs, column)), t.mono))
        })
        .fold(digit, f32::max);
    widest.min(MAX_LABEL_WIDTH as f32 * digit).ceil() + 18.0
}

fn record_view(b: &Browser, grid: &Grid, page: &Page, t: &Theme) -> El {
    let tab = b.id;
    let defs = grid.ddl().map_or(&[][..], |ddl| ddl.columns.as_slice());
    let key_columns = match chosen_key(&b.row_keys, grid) {
        Some(chosen) if page.key.source == KeySource::None && edit::usable_key(page, chosen) => {
            chosen
        }
        _ => page.key.columns.as_slice(),
    };
    let label_w = label_width(&page.columns, defs, key_columns, t);
    let row_index = grid.row();
    let draft = b
        .table
        .draft
        .as_ref()
        .filter(|draft| draft.row() == row_index);
    let saving = draft.is_some_and(|draft| draft.saving);
    let editable_table = grid.kind() == TableKind::Table;
    let fields = page.columns.iter().enumerate().map(|(index, column)| {
        let def = definition(defs, column);
        let cut = page
            .full
            .get(&(row_index, index))
            .is_some_and(|full| full.truncated);
        let editing = draft
            .filter(|draft| draft.is_open(index))
            .and_then(|draft| draft.field(index));
        let kept = draft.and_then(|draft| draft.value(index));
        let changed = draft.is_some_and(|draft| draft.changed(index));
        let editable = editable_table
            && !b.connection.read_only
            && matches!(edit::exact(page, row_index, index), Exact::Value(_));
        let name_color = if changed { t.blue } else { t.foreground };
        let key_field = key_columns.contains(&column.name);
        let name = row()
            .gap(4.0)
            .min_w(0.0)
            .center_y()
            .children(key_field.then(|| {
                ui::icon_el(&icons::KEY_ROUND, KEY_ICON, t.muted)
                    .key(format!("table-key-{index}"))
                    .help("Part of the row's key")
            }))
            .child(
                text(column.name.clone())
                    .font(t.bold(t.mono))
                    .color(name_color)
                    .truncate()
                    .min_w(0.0),
            )
            .children(changed.then(|| ui::label("•", t.mono, t.blue)));
        let label = col()
            .w(label_w)
            .px(8.0)
            .py(4.0)
            .self_stretch()
            .border_sides([0.0, 1.0, 0.0, 0.0], t.border)
            .child(name)
            .children((!def.is_empty()).then(|| {
                text(normalize(def))
                    .font(t.mono)
                    .color(t.muted)
                    .wrap_anywhere()
            }));
        let value = match editing {
            Some(field) => field_editor(tab, index, &column.name, field, saving, t),
            None => {
                let shown = match &kept {
                    Some(value) => value.as_deref(),
                    None => full_value(page, row_index, index),
                };
                let content = match shown {
                    Some(value) => text(normalize(value))
                        .font(t.mono)
                        .color(t.foreground)
                        .wrap_anywhere(),
                    None => text(NULL).font(t.mono.italic()).color(t.muted),
                };
                let mut value = col()
                    .key(format!("table-value-{index}"))
                    .grow(1.0)
                    .min_w(0.0)
                    .self_stretch()
                    .px(8.0)
                    .py(4.0)
                    .child(content)
                    .children(cut.then(|| text(CUT_NOTE).font(t.mono.italic()).color(t.muted)));
                if editable && !saving {
                    value = value
                        .label(format!("Edit {}", column.name))
                        .cursor(Cursor::Text)
                        .hover(t.table_hover)
                        .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                            if let Some(b) = s.browser_mut(tab) {
                                note_press(b, tx);
                            }
                            begin_edit(s, tab, Some(index), false, tx);
                        });
                }
                value
            }
        };
        let copy_button = el().padding(2.0).group_opacity(0.0, 1.0).child(icon_button(
            &icons::COPY,
            "Copy value",
            None,
            false,
            t,
            with_browser(tab, move |b, tx| copy(b, Some(index), tx)),
        ));
        row()
            .key(format!("table-field-{index}"))
            .w_full()
            .items_start()
            .hover_group()
            .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
            .child(label)
            .child(value)
            .child(copy_button)
    });
    let list = t.k.scroll_bar(
        b.table.record_scroll.watch(
            col()
                .key(RECORD_SCROLL)
                .grow(1.0)
                .min_h(0.0)
                .min_w(0.0)
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .child(col().w_full().children(fields)),
        ),
    );
    col()
        .key("record-view")
        .grow(1.0)
        .min_h(0.0)
        .min_w(0.0)
        .child(list)
        .children(draft.map(|draft| save_bar(tab, draft, t)))
}

fn field_editor(
    tab: TabId,
    index: usize,
    name: &str,
    field: &edit::DraftField,
    saving: bool,
    t: &Theme,
) -> El {
    let input = text_input(&field.input, t.mono)
        .key(format!("table-field-input-{index}"))
        .label(name.to_string())
        .read_only(saving)
        .color(if saving { t.muted } else { t.foreground })
        .caret(t.foreground, t.k.selection())
        .placeholder(if field.null { NULL } else { "" }, t.faint)
        .w_full()
        .on_edit(move |s: &mut Workspace| {
            workspace::browser_field(s, tab, |b| {
                b.table
                    .draft
                    .as_mut()
                    .and_then(|draft| draft.input_mut(index))
            })
        })
        .on_input_key(move |s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| {
            if let Some(b) = s.browser_mut(tab) {
                b.table.keyed = true;
            }
            keys::field_key(s, ev, tx) || field_keys(s, tab, ev)
        })
        .on_input_paste(
            move |s: &mut Workspace, _content: &rune::ClipboardContent, _tx: &mut Tx| {
                saving_draft(s, tab)
            },
        )
        .on_input_change(move |s: &mut Workspace, _tx: &mut Tx| {
            if let Some(b) = s.browser_mut(tab) {
                on_draft_change(b, index);
            }
        })
        .on_submit(move |s: &mut Workspace, _tx: &mut Tx| {
            if let Some(input) = s
                .browser_mut(tab)
                .and_then(|b| b.table.draft.as_mut())
                .filter(|draft| !draft.saving)
                .and_then(|draft| draft.input_mut(index))
            {
                input.insert("\n");
            }
        });
    let field_box = auto_grow(input, 1, edit::MAX_FIELD_ROWS)
        .radius(4.0)
        .border(1.0, t.ring)
        .fill(t.background)
        .px(4.0)
        .py(2.0);
    let null = field.null;
    let checkbox = Checkbox::new(null)
        .label(NULL)
        .disabled(saving)
        .on_toggle_tx(move |s: &mut Workspace, _tx: &mut Tx| {
            if let Some(b) = s.browser_mut(tab) {
                b.clear_status();
                set_null(b, index, !null);
                if let Some(input) = b.table.draft.as_mut().and_then(|d| d.input_mut(index)) {
                    input.focused = true;
                }
            }
        })
        .build(&t.k)
        .focusable(false)
        .no_focus_ring()
        .key(format!("record-null-{index}"));
    col()
        .key(format!("table-edit-{index}"))
        .grow(1.0)
        .min_w(0.0)
        .px(4.0)
        .py(2.0)
        .gap(4.0)
        .on_background_press(move |s: &mut Workspace| {
            if let Some(b) = s.browser_mut(tab) {
                b.clear_status();
            }
        })
        .child(field_box)
        .child(checkbox)
}

fn saving_draft(s: &Workspace, tab: TabId) -> bool {
    s.browser(tab)
        .and_then(|b| b.table.draft.as_ref())
        .is_some_and(|draft| draft.saving)
}

fn field_keys(s: &mut Workspace, tab: TabId, ev: &KeyEvent) -> bool {
    use rune::Key;
    if !saving_draft(s, tab) {
        return false;
    }
    match &ev.key {
        Key::Command(key) => !matches!(key, 'c' | 'a'),
        Key::Text(_) | Key::Backspace | Key::Delete | Key::Enter | Key::Tab => true,
        _ => false,
    }
}

fn save_bar(tab: TabId, draft: &Draft, t: &Theme) -> El {
    let changes = draft.changes();
    let left = match &draft.error {
        Some(error) => row()
            .key("row-editor-error")
            .gap(8.0)
            .shrink()
            .center_y()
            .child(ui::icon_el(&icons::CIRCLE_X, 14.0, t.danger))
            .child(
                text(normalize(error))
                    .font(t.text_sm)
                    .color(t.danger)
                    .truncate()
                    .min_w(0.0),
            ),
        None => row().shrink().child(
            ui::label(
                match changes {
                    0 => "Editing: no changes yet".to_string(),
                    1 => "1 value changed".to_string(),
                    n => format!("{n} values changed"),
                },
                t.text_sm,
                t.muted,
            )
            .truncate()
            .shrink(),
        ),
    };
    let save_variant = if draft.saving {
        ButtonVariant::Loading
    } else {
        ButtonVariant::Submit
    };
    row()
        .key("row-editor-bar")
        .w_full()
        .px(8.0)
        .py(4.0)
        .gap(8.0)
        .center_y()
        .justify_between()
        .border_sides([1.0, 0.0, 0.0, 0.0], t.border)
        .child(left)
        .child(
            row()
                .gap(8.0)
                .child(
                    Button::new("Cancel")
                        .variant(ButtonVariant::Outline)
                        .control_size(ControlSize::Small)
                        .help("Throw the changes away")
                        .shortcut(
                            keys::label_in(Command::CancelEdit, Ctx::RowEditorInput)
                                .unwrap_or_default(),
                        )
                        .on_press_tx(with_browser(tab, end_edit))
                        .build(&t.k)
                        .focusable(false)
                        .no_focus_ring()
                        .key("row-editor-cancel"),
                )
                .child(
                    Button::new("Save")
                        .variant(save_variant)
                        .control_size(ControlSize::Small)
                        .disabled(changes == 0 || draft.saving)
                        .help("Save the changes")
                        .shortcut(keys::label(Command::SaveRow).unwrap_or_default())
                        .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                            if let Some(b) = s.browser_mut(tab) {
                                note_press(b, tx);
                            }
                            save_draft(s, tab);
                        })
                        .build(&t.k)
                        .focusable(false)
                        .no_focus_ring()
                        .key("row-editor-save"),
                ),
        )
}

fn footer(b: &Browser, grid: &Grid, t: &Theme) -> El {
    let tab = b.id;
    let buttons = if grid.record_open() {
        let last = grid.row_count().saturating_sub(1);
        row()
            .gap(2.0)
            .center_y()
            .child(icon_button(
                &icons::CHEVRON_UP,
                "Previous row",
                Some(Command::PrevRecord),
                grid.row() == 0,
                t,
                with_browser(tab, |b, tx| {
                    step_record(b, -1, tx);
                }),
            ))
            .child(icon_button(
                &icons::CHEVRON_DOWN,
                "Next row",
                Some(Command::NextRecord),
                grid.row() >= last,
                t,
                with_browser(tab, |b, tx| {
                    step_record(b, 1, tx);
                }),
            ))
            .child(icon_button(
                &icons::COPY,
                "Copy the row",
                Some(Command::CopyRow),
                false,
                t,
                with_browser(tab, |b, tx| copy(b, None, tx)),
            ))
            .child(
                Button::new("Back to the table")
                    .variant(ButtonVariant::Ghost)
                    .control_size(ControlSize::Small)
                    .icon(&icons::ARROW_LEFT)
                    .label_with(|font, ink| text("Table").font(font).color(ink))
                    .help("Back to the table")
                    .shortcut(keys::label(Command::CloseRecord).unwrap_or_default())
                    .on_press_tx(with_browser(tab, close_record))
                    .build(&t.k)
                    .focusable(false)
                    .no_focus_ring()
                    .key("close-record"),
            )
    } else {
        let has_page = grid.page().is_some();
        let at_start = grid.offset() == 0;
        let counting = grid.is_counting_for_last();
        let last = if counting {
            row()
                .key("last-page")
                .size(20.0, 20.0)
                .center()
                .label("Last page")
                .child(ui::spinner(12.0, t.muted, t))
        } else {
            icon_button(
                &icons::CHEVRONS_RIGHT,
                "Last page",
                Some(Command::LastPage),
                !has_page || !grid.can_go_last(),
                t,
                with_browser(tab, |b, tx| {
                    page_op(b, tx, |grid, session| grid.last_page(session))
                }),
            )
        };
        row()
            .gap(2.0)
            .center_y()
            .child(icon_button(
                &icons::CHEVRONS_LEFT,
                "First page",
                Some(Command::FirstPage),
                !has_page || at_start,
                t,
                move |s: &mut Workspace, tx: &mut Tx| {
                    if let Some(b) = s.browser_mut(tab) {
                        note_press(b, tx);
                    }
                    run(s, tab, Command::FirstPage, tx);
                },
            ))
            .child(icon_button(
                &icons::CHEVRON_LEFT,
                "Previous page",
                Some(Command::PrevPage),
                !has_page || at_start,
                t,
                with_browser(tab, |b, tx| {
                    page_op(b, tx, |grid, session| grid.previous_page(session))
                }),
            ))
            .child(icon_button(
                &icons::CHEVRON_RIGHT,
                "Next page",
                Some(Command::NextPage),
                !grid.can_go_next(),
                t,
                with_browser(tab, |b, tx| {
                    page_op(b, tx, |grid, session| grid.next_page(session))
                }),
            ))
            .child(last)
    };
    row()
        .key("table-footer-bar")
        .h(30.0)
        .w_full()
        .px(8.0)
        .gap(8.0)
        .center_y()
        .justify_between()
        .border_sides([1.0, 0.0, 0.0, 0.0], t.border)
        .child(
            text(footer_text(grid))
                .key("table-footer")
                .font(t.text_xs)
                .color(t.muted)
                .truncate()
                .min_w(0.0),
        )
        .child(buttons)
}

fn content(s: &Workspace, b: &Browser, grid: &Grid, t: &Theme) -> El {
    if let Some(error) = grid.error() {
        return error_view(b, error, t);
    }
    let Some(page) = grid.page() else {
        return row()
            .key("table-loading-page")
            .grow(1.0)
            .w_full()
            .center()
            .gap(8.0)
            .child(ui::spinner(16.0, t.muted, t))
            .child(ui::label("Loading…", t.text, t.muted));
    };
    if page.columns.is_empty() {
        return ui::placeholder(&[("No columns", false)], t);
    }
    if grid.record_open() {
        return record_view(b, grid, page, t);
    }
    grid::view(&grid_spec(b.id), s, t)
}

pub fn view(
    s: &Workspace,
    b: &Browser,
    t: &Theme,
    _cx: &mut Cx<Workspace>,
    _layer: &mut Vec<El>,
) -> El {
    let tab = b.id;
    let pane = match &b.table.grid {
        None => ui::pane(
            KEY,
            ui::label("Table", t.text_sm, t.muted),
            None,
            ui::placeholder(
                &[
                    ("No table selected", true),
                    ("Pick a table in the sidebar and double-click it", false),
                    ("or press ⌘E to write SQL", false),
                ],
                t,
            ),
            t,
        ),
        Some(grid) => ui::pane(
            KEY,
            title_view(b, grid, t),
            Some(header_extra(b, grid, t)),
            col()
                .grow(1.0)
                .min_h(0.0)
                .min_w(0.0)
                .child(
                    col()
                        .grow(1.0)
                        .min_h(0.0)
                        .min_w(0.0)
                        .child(content(s, b, grid, t)),
                )
                .child(footer(b, grid, t)),
            t,
        ),
    };
    pane.pointer_buttons([PointerButton::Back])
        .on_background_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
            let back = tx.button() == PointerButton::Back;
            if let Some(b) = s.browser_mut(tab) {
                if back {
                    if b.table.record_open() {
                        close_record(b, tx);
                    }
                } else {
                    note_press(b, tx);
                }
            }
        })
        .focusable(true)
        .on_key(move |s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| {
            keys::route(s, ev, tx) || raw_key(s, tab, ev, tx)
        })
}

#[cfg(test)]
pub fn test_clear_kit_selection(b: &mut Browser) {
    b.table.cells.clear();
    if b.table
        .grid
        .as_ref()
        .is_some_and(|grid| grid.row_count() > 0)
    {
        select_in_table(b);
    }
}
