pub mod grid;
pub mod record;
pub mod syntax;
#[cfg(test)]
mod ui_tests;

use std::{
    cell::RefCell,
    ops::Range,
    rc::Rc,
    sync::LazyLock,
    time::{Duration, Instant},
};

use rune::{
    Align, Chord, ClipboardContent, Cx, El, Key, KeyEvent, ScrollSpec, TextEdit, TextHighlight, Tx,
    col, row, text, text::advance, text_input,
};
use rune_kit::{
    Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, TextInput, search::Fold,
};
use unicode_width::UnicodeWidthStr as _;

use self::{
    grid::{Grid, Selection, copy_cell, copy_row, data_column},
    syntax::{Palette, SqlLines},
};
use super::{
    Browser, Main, Pane,
    confirm::{Confirm, Pending},
};
use crate::{
    connection::DbKind,
    db::ScriptResult,
    dialog::Dialog,
    guard, icons,
    keys::{self, Command, Ctx},
    theme::Theme,
    ui::{self, Scroll, ScrollY, Split},
    workspace::{self, TabId, Workspace},
};

pub const SCRIPT_ROW_LIMIT: u32 = 1000;
pub const TICK: Duration = Duration::from_millis(250);
pub const LINE_HEIGHT: f32 = 18.;
const WINDOW_BLOCK: usize = 50;
const WINDOW_MARGIN: usize = 100;
const WINDOW_SETTLE: f64 = 0.15;
const GUTTER_PADDING: f32 = 8.;
const INDENT: &str = "  ";
const PAIRS: [(char, char); 5] = [('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('\'', '\'')];
const CLOSE_BEFORE: &str = ";:.,=}])>";

pub const SQL: &str = "console-sql";
pub const RESULT: &str = "console-result";
pub const GRID: &str = "console-grid";
pub const RECORD: &str = "console-record";
const EDITOR_SCROLL: &str = "console-editor-scroll";
pub const FIND: &str = "console-find";
const FIND_MATCH: &str = "console-find-match";
const FIND_CONTROLS: [&str; 3] = ["find-prev", "find-next", "find-close"];
const ERROR_TEXT: &str = "script-error-text";
const TOOLBAR: [&str; 3] = ["console", "run-script", "cancel-script"];
const RESULT_CONTROLS: [&str; 5] = [
    "record-prev",
    "record-next",
    "record-copy",
    "record-close",
    "copy-error",
];

static FIND_CHORD: LazyLock<Chord> = LazyLock::new(|| chord("mod+f"));
static FIND_NEXT_CHORD: LazyLock<Chord> = LazyLock::new(|| chord("mod+g"));
static FIND_PREV_CHORD: LazyLock<Chord> = LazyLock::new(|| chord("mod+shift+g"));

fn chord(spec: &str) -> Chord {
    Chord::parse(spec).unwrap_or_else(|err| panic!("chord {spec:?}: {err}"))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Seek {
    Here,
    Next,
    Prev,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConsoleArea {
    Editor,
    Result,
    Message,
    Record,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub id: u64,
    pub started: Instant,
    pub seconds: u64,
    pub cancelling: bool,
}

#[derive(Clone, Debug)]
pub enum Outcome {
    Done(Rc<ScriptResult>),
    Failed(String),
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EditorView {
    pub top: f32,
    pub width: f32,
    pub height: f32,
    pub block: usize,
    pub pending: Option<usize>,
    pub moved: f64,
}

pub struct Console {
    pub sql: TextEdit,
    pub run: Option<Run>,
    pub outcome: Option<Outcome>,
    pub grid: Grid,
    pub record: Option<usize>,
    pub record_scroll: ScrollY,
    pub status_shown: bool,
    pub in_result: bool,
    pub split: Split,
    pub syntax: RefCell<SqlLines>,
    pub editor: EditorView,
    pub refocus: bool,
    pub reset_scroll: bool,
    pub find: TextEdit,
    pub finding: bool,
}

impl Console {
    pub fn new(draft: Option<String>) -> Self {
        let mut sql = TextEdit::new(normalize(&draft.unwrap_or_default()));
        sql.set_caret(sql.text.len());
        Self {
            sql,
            run: None,
            outcome: None,
            grid: Grid::default(),
            record: None,
            record_scroll: ScrollY::default(),
            status_shown: false,
            in_result: false,
            split: Split::new(240.0, 90.0, 4000.0),
            syntax: RefCell::new(SqlLines::new(DbKind::Sqlite)),
            editor: EditorView::default(),
            refocus: false,
            reset_scroll: false,
            find: TextEdit::new(""),
            finding: false,
        }
    }

    pub fn text(&self) -> &str {
        &self.sql.text
    }

    pub fn draft(&self) -> String {
        self.sql.text.clone()
    }

    pub fn is_running(&self) -> bool {
        self.run.is_some()
    }

    pub fn title_context(&self) -> String {
        "Console".to_string()
    }

    pub fn running_status(&self) -> Option<String> {
        let run = self.run.as_ref()?;
        Some(super::running_status(run.seconds, run.cancelling))
    }

    pub fn ticker(&self) -> Option<String> {
        let run = self.run.as_ref()?;
        Some(ticker(run.seconds, run.cancelling))
    }

    pub fn outcome_text(&self) -> Option<String> {
        Some(match self.outcome.as_ref()? {
            Outcome::Done(result) => ui::summary(result),
            Outcome::Failed(message) => format!("error: {message}"),
            Outcome::Cancelled => "cancelled".to_string(),
        })
    }

    pub fn result(&self) -> Option<&Rc<ScriptResult>> {
        match self.outcome.as_ref()? {
            Outcome::Done(result) => Some(result),
            Outcome::Failed(_) | Outcome::Cancelled => None,
        }
    }

    pub fn has_result(&self) -> bool {
        self.result()
            .is_some_and(|result| !result.columns.is_empty())
    }

    pub fn area(&self) -> ConsoleArea {
        match (self.in_result, self.record) {
            (false, _) => ConsoleArea::Editor,
            (true, None) if self.has_result() => ConsoleArea::Result,
            (true, None) => ConsoleArea::Message,
            (true, Some(_)) => ConsoleArea::Record,
        }
    }

    pub fn find_hits(&self) -> Vec<Range<usize>> {
        if !self.finding {
            return Vec::new();
        }
        Fold::default().find(&self.sql.text, &self.find.text)
    }

    pub fn find_status(&self) -> Option<String> {
        if !self.finding || self.find.text.is_empty() {
            return None;
        }
        let hits = self.find_hits();
        if hits.is_empty() {
            return Some("No matches".to_string());
        }
        let (start, end) = self.sql.range();
        let total = ui::group(hits.len() as u64);
        Some(match hits.iter().position(|hit| *hit == (start..end)) {
            Some(at) => format!("{} of {total}", ui::group(at as u64 + 1)),
            None if hits.len() == 1 => "1 match".to_string(),
            None => format!("{total} matches"),
        })
    }
}

fn is_result_key(key: &str) -> bool {
    key == RESULT || RESULT_CONTROLS.contains(&key) || key.starts_with("copy-field-")
}

fn result_focused(focused: Option<&str>) -> bool {
    focused.is_some_and(|key| key == GRID || is_result_key(key))
}

fn hint(b: &mut Browser, text: impl Into<String>) {
    b.console.status_shown = true;
    b.set_status(text, false);
}

fn clear_status(b: &mut Browser) {
    if std::mem::take(&mut b.console.status_shown) {
        b.clear_status();
    }
}

fn copy(b: &mut Browser, text: String, message: String, tx: &mut Tx) {
    tx.copy(text);
    hint(b, message);
}

pub fn on_script(b: &mut Browser, id: u64, result: ScriptResult) -> bool {
    let done = finish(b, id, Ok(result));
    if done {
        b.console.refocus = true;
    }
    done
}

pub fn on_error(b: &mut Browser, id: u64, message: &str) -> bool {
    finish(b, id, Err(message.to_string()))
}

fn finish(b: &mut Browser, id: u64, result: Result<ScriptResult, String>) -> bool {
    let c = &mut b.console;
    let Some(run) = c.run.take_if(|run| run.id == id) else {
        return false;
    };
    c.outcome = Some(match result {
        Ok(result) => {
            let result = Rc::new(result);
            c.record = None;
            c.grid = Grid::new(&result);
            c.reset_scroll = true;
            Outcome::Done(result)
        }
        Err(_) if run.cancelling => Outcome::Cancelled,
        Err(message) => Outcome::Failed(message),
    });
    super::tree::refresh(b);
    true
}

pub fn cancel(b: &mut Browser) -> bool {
    let Some(run) = b.console.run.as_mut().filter(|run| !run.cancelling) else {
        return false;
    };
    run.cancelling = true;
    b.session.cancel_script(run.id);
    hint(b, "Cancelling the script…");
    true
}

pub fn poll(b: &mut Browser, tx: &mut Tx) -> bool {
    let shown = b.main == Main::Console && b.focused == Some(Pane::Main);
    let in_result = shown && result_focused(tx.focused());
    let c = &mut b.console;
    let mut changed = false;
    if c.sql.focused && c.in_result {
        c.in_result = false;
        changed = true;
    } else if in_result && !c.in_result {
        c.in_result = true;
        changed = true;
    }
    if std::mem::take(&mut c.reset_scroll) {
        tx.scroll_to_instant(grid::SCROLL, 0., 0.);
    }
    if let Some(block) = c.editor.pending {
        let wait = WINDOW_SETTLE - (tx.clock() - c.editor.moved);
        if wait <= 0. {
            c.editor.block = block;
            c.editor.pending = None;
            changed = true;
        } else {
            tx.wake_after(Duration::from_secs_f64(wait));
        }
    }
    if std::mem::take(&mut c.refocus)
        && shown
        && tx.focused() == Some(RESULT)
        && focus_result(b, tx)
    {
        changed = true;
    }
    let Some(run) = &mut b.console.run else {
        return changed;
    };
    tx.wake_after(TICK);
    let seconds = run.started.elapsed().as_secs();
    if seconds != run.seconds {
        run.seconds = seconds;
        changed = true;
    }
    changed
}

pub fn focus(b: &mut Browser, _tx: &mut Tx) {
    b.console.sql.focused = true;
    b.console.in_result = false;
}

pub fn focus_result(b: &mut Browser, tx: &mut Tx) -> bool {
    let c = &mut b.console;
    if !c.has_result() {
        return false;
    }
    c.sql.focused = false;
    c.in_result = true;
    tx.focus(if c.record.is_some() { RESULT } else { GRID }, false);
    true
}

fn focus_result_pane(b: &mut Browser, tx: &mut Tx) {
    b.console.sql.focused = false;
    b.console.in_result = true;
    tx.focus(RESULT, false);
}

pub fn focus_step(b: &mut Browser, forward: bool, tx: &mut Tx) -> bool {
    let focused = tx.focused().map(str::to_owned);
    if forward {
        let in_editor = b.console.sql.focused || focused.as_deref() == Some(SQL);
        in_editor && focus_result(b, tx)
    } else if result_focused(focused.as_deref()) {
        focus(b, tx);
        true
    } else {
        false
    }
}

pub fn focus_last(b: &mut Browser, tx: &mut Tx) {
    if !focus_result(b, tx) {
        focus(b, tx);
    }
}

pub fn focus_last_area(b: &mut Browser, tx: &mut Tx) {
    match b.console.area() {
        ConsoleArea::Editor => focus(b, tx),
        ConsoleArea::Message => focus_result_pane(b, tx),
        ConsoleArea::Result | ConsoleArea::Record => focus_last(b, tx),
    }
}

pub fn contexts(b: &Browser, focused: Option<&str>, out: &mut Vec<Ctx>) {
    if b.console.finding && b.console.find.focused {
        out.push(Ctx::Console);
        return;
    }
    if b.console.sql.focused || focused == Some(SQL) {
        out.push(Ctx::SqlEditorInput);
        out.push(Ctx::Console);
        return;
    }
    let Some(key) = focused else {
        return;
    };
    if key == GRID {
        out.extend([Ctx::ConsoleGrid, Ctx::ConsoleResult, Ctx::Console]);
    } else if is_result_key(key) {
        out.extend([Ctx::ConsoleResult, Ctx::Console]);
    } else if TOOLBAR.contains(&key) || FIND_CONTROLS.contains(&key) || key == EDITOR_SCROLL {
        out.push(Ctx::Console);
    }
}

pub fn run(s: &mut Workspace, tab: TabId, cmd: Command, tx: &mut Tx) -> bool {
    use Command::*;
    if cmd == RunScript {
        let exists = s.browser_mut(tab).is_some();
        run_script(s, tab, tx);
        return exists;
    }
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    match cmd {
        FocusResult => {
            if !focus_result(b, tx) {
                hint(b, "No result yet · run the script with ⌘⏎ or F5");
            }
        }
        FocusEditor => focus(b, tx),
        ToggleRecord => match b.console.record {
            Some(_) => close_record(b, tx),
            None => match b.console.grid.current_row() {
                Some(row) => open_record(b, row, tx),
                None => return false,
            },
        },
        CloseRecord => match b.console.record {
            Some(_) => close_record(b, tx),
            None if b.console.run.is_none() => b.focus_pane(Pane::Tree, tx),
            None => return false,
        },
        NextRecord | PrevRecord => match b.console.record {
            Some(_) => step_record(b, if cmd == NextRecord { 1 } else { -1 }, tx),
            None => return false,
        },
        CopyCell if b.console.result().is_none() || result_text_selected(tx) => return false,
        CopyRow if b.console.result().is_none() => return false,
        CopyCell => copy_selected_cell(b, tx),
        CopyRow => copy_selected_row(b, tx),
        _ => return false,
    }
    true
}

fn result_text_selected(tx: &Tx) -> bool {
    tx.text_selection().is_some_and(|selection| {
        selection.texts().iter().any(|text| {
            text.is_selected()
                && text
                    .key()
                    .is_some_and(|key| key == ERROR_TEXT || key.starts_with(record::VALUE_PREFIX))
        })
    })
}

const ALREADY_RUNNING: &str = "A script is already running · Esc cancels it";

/// Runs the script in the editor, or only its selection. On a connection tagged Production a
/// script that may change data or schema opens the confirmation prompt instead.
fn run_script(s: &mut Workspace, tab: TabId, tx: &mut Tx) {
    let Some((b, dialog)) = s.browser_and_dialog(tab) else {
        return;
    };
    if b.console.run.is_some() {
        return hint(b, ALREADY_RUNNING);
    }
    let Some(sql) = runnable(&b.console.sql.text, b.console.sql.selected()) else {
        return hint(b, "Nothing to run: the editor is empty");
    };
    clear_status(b);
    if b.connection.asks_to_confirm() {
        let statements = guard::writes(&sql, b.connection.kind);
        if !statements.is_empty() {
            *dialog = Some(Dialog::Confirm(Box::new(Confirm {
                tab,
                connection: b.connection.name.clone(),
                pending: Pending::Script(sql),
                statements,
            })));
            return;
        }
    }
    start_script(b, &sql, tx);
}

/// Runs `sql` exactly as it was confirmed, not what the editor holds now.
pub(super) fn run_confirmed(b: &mut Browser, sql: String, tx: &mut Tx) {
    if b.console.run.is_some() {
        return hint(b, ALREADY_RUNNING);
    }
    start_script(b, &sql, tx);
}

fn start_script(b: &mut Browser, sql: &str, tx: &mut Tx) {
    let id = b.session.run_script(sql, SCRIPT_ROW_LIMIT);
    start(b, id, tx);
}

fn start(b: &mut Browser, id: u64, tx: &mut Tx) {
    if tx.focused() == Some(GRID) {
        focus_result_pane(b, tx);
    }
    let c = &mut b.console;
    c.outcome = None;
    c.record = None;
    c.run = Some(Run {
        id,
        started: Instant::now(),
        seconds: 0,
        cancelling: false,
    });
    tx.wake_after(TICK);
}

fn open_record(b: &mut Browser, row: usize, tx: &mut Tx) {
    let c = &mut b.console;
    let Some(result) = c.result() else {
        return;
    };
    if row >= result.rows.len() || result.columns.is_empty() {
        return;
    }
    c.record = Some(row);
    c.record_scroll.reset();
    tx.scroll_to_instant(RECORD, 0., 0.);
    focus_result_pane(b, tx);
}

fn close_record(b: &mut Browser, tx: &mut Tx) {
    if b.console.record.take().is_some() {
        b.console.in_result = true;
        tx.focus(GRID, false);
    }
}

fn step_record(b: &mut Browser, delta: isize, tx: &mut Tx) {
    let c = &mut b.console;
    let (Some(row), Some(result)) = (c.record, c.result()) else {
        return;
    };
    let last = result.rows.len().saturating_sub(1);
    let next = row.saturating_add_signed(delta).min(last);
    if next == row {
        return;
    }
    c.record = Some(next);
    c.record_scroll.reset();
    tx.scroll_to_instant(RECORD, 0., 0.);
    c.grid.selection = Some(match c.grid.selected_cell() {
        Some((_, column)) => Selection::Cell(next, column),
        None => Selection::Row(next),
    });
}

fn copy_selected_cell(b: &mut Browser, tx: &mut Tx) {
    let Some(result) = b.console.result().cloned() else {
        return;
    };
    let grid = &b.console.grid;
    if let Some((row, column)) = grid.selected_cell() {
        if let Some(column) = data_column(column) {
            copy(
                b,
                copy_cell(&result, row, column),
                "Copied 1 cell".to_string(),
                tx,
            );
        }
    } else if let Some(row) = grid.selected_row() {
        copy(b, copy_row(&result, row), "Copied 1 row".to_string(), tx);
    }
}

fn copy_selected_row(b: &mut Browser, tx: &mut Tx) {
    let Some(result) = b.console.result().cloned() else {
        return;
    };
    let row = b.console.record.or(b.console.grid.current_row());
    if let Some(row) = row.filter(|row| *row < result.rows.len()) {
        copy(b, copy_row(&result, row), "Copied 1 row".to_string(), tx);
    }
}

pub(crate) fn click_cell(s: &mut Workspace, tab: TabId, row: usize, column: usize, tx: &mut Tx) {
    select_in_grid(s, tab, Selection::Cell(row, column), tx);
}

pub(crate) fn click_row(s: &mut Workspace, tab: TabId, row: usize, tx: &mut Tx) {
    select_in_grid(s, tab, Selection::Row(row), tx);
}

fn select_in_grid(s: &mut Workspace, tab: TabId, selection: Selection, tx: &mut Tx) {
    let Some(b) = s.browser_mut(tab) else {
        return;
    };
    b.clear_status();
    b.console.grid.selection = Some(selection);
    b.console.sql.focused = false;
    b.console.in_result = true;
    tx.focus(GRID, false);
}

pub(crate) fn open_record_at(s: &mut Workspace, tab: TabId, row: usize, tx: &mut Tx) {
    if let Some(b) = s.browser_mut(tab) {
        open_record(b, row, tx);
    }
}

pub(crate) fn grid_key(s: &mut Workspace, tab: TabId, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    let c = &mut b.console;
    let rows = c.result().map_or(0, |result| result.rows.len());
    if c.grid.step(ev, rows) {
        c.grid.reveal(tx);
        true
    } else {
        false
    }
}

pub(crate) fn copy_value(s: &mut Workspace, tab: TabId, text: String, tx: &mut Tx) {
    if let Some(b) = s.browser_mut(tab) {
        let message = copied_message(&text);
        copy(b, text, message, tx);
    }
}

fn plain(ev: &KeyEvent) -> bool {
    let m = ev.mods;
    !(m.cmd || m.ctrl || m.alt || m.logo || m.shift)
}

fn result_key(s: &mut Workspace, tab: TabId, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    if !plain(ev) {
        return false;
    }
    if ev.key == Key::Escape && b.console.run.is_some() {
        return true;
    }
    if b.console.record.is_none() {
        if ev.key == Key::Enter
            && let Some(row) = b.console.grid.current_row()
        {
            open_record(b, row, tx);
            return true;
        }
        return false;
    }
    match &ev.key {
        Key::Right => step_record(b, 1, tx),
        Key::Left => step_record(b, -1, tx),
        Key::Enter => close_record(b, tx),
        Key::Text(text) if text == " " => return false,
        _ => match Scroll::from_key(ev) {
            Some(how) => b
                .console
                .record_scroll
                .scroll(tx, RECORD, how, record::LINE_HEIGHT),
            None => return false,
        },
    }
    true
}

pub(crate) fn editor_input_key(s: &mut Workspace, tab: TabId, ev: &KeyEvent, tx: &mut Tx) -> bool {
    editor_find_key(s, tab, ev, tx) || keys::field_key(s, ev, tx) || editor_key(s, tab, ev, tx)
}

fn editor_find_key(s: &mut Workspace, tab: TabId, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    let c = &mut b.console;
    if FIND_CHORD.matches_tx(ev, tx) {
        open_find(c);
        return true;
    }
    if !c.finding {
        return false;
    }
    if ev.key == Key::Escape && plain(ev) && c.run.is_none() {
        c.finding = false;
        return true;
    }
    let seek = if FIND_NEXT_CHORD.matches_tx(ev, tx) {
        Seek::Next
    } else if FIND_PREV_CHORD.matches_tx(ev, tx) {
        Seek::Prev
    } else {
        return false;
    };
    find_seek(c, seek, tx);
    true
}

fn open_find(c: &mut Console) {
    let selected = c.sql.selected();
    if !selected.is_empty() && !selected.contains('\n') {
        let query = selected.to_string();
        c.find.replace_text(&query);
    }
    c.find.set_selection(0, c.find.text.len());
    c.finding = true;
    c.find.focused = true;
    c.sql.focused = false;
    c.in_result = false;
}

fn close_find(c: &mut Console) {
    c.finding = false;
    c.find.focused = false;
    c.sql.focused = true;
    c.in_result = false;
}

fn find_seek(c: &mut Console, seek: Seek, tx: &mut Tx) {
    let hits = c.find_hits();
    let (Some(first), Some(last)) = (hits.first(), hits.last()) else {
        return;
    };
    let (start, end) = c.sql.range();
    let hit = match seek {
        Seek::Here => hits.iter().find(|hit| hit.start >= start).unwrap_or(first),
        Seek::Next => hits
            .iter()
            .find(|hit| hit.start > start || (start == end && hit.start == start))
            .unwrap_or(first),
        Seek::Prev => hits
            .iter()
            .rev()
            .find(|hit| hit.end <= start)
            .unwrap_or(last),
    };
    c.sql.set_selection(hit.start, hit.end);
    tx.scroll_into_view(FIND_MATCH);
}

fn find_key(s: &mut Workspace, tab: TabId, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    let c = &mut b.console;
    let m = ev.mods;
    let bare = !(m.cmd || m.ctrl || m.alt || m.logo);
    if FIND_CHORD.matches_tx(ev, tx) {
        c.find.set_selection(0, c.find.text.len());
        return true;
    }
    let seek = match ev.key {
        Key::Escape if plain(ev) && c.run.is_none() => {
            close_find(c);
            return true;
        }
        Key::Enter if bare && m.shift => Seek::Prev,
        Key::Enter if bare => Seek::Next,
        _ if FIND_NEXT_CHORD.matches_tx(ev, tx) => Seek::Next,
        _ if FIND_PREV_CHORD.matches_tx(ev, tx) => Seek::Prev,
        _ => return false,
    };
    find_seek(c, seek, tx);
    true
}

fn find_changed(s: &mut Workspace, tab: TabId, tx: &mut Tx) {
    if let Some(b) = s.browser_mut(tab) {
        find_seek(&mut b.console, Seek::Here, tx);
    }
}

fn editor_key(s: &mut Workspace, tab: TabId, ev: &KeyEvent, _tx: &mut Tx) -> bool {
    let Some(b) = s.browser_mut(tab) else {
        return false;
    };
    let kind = b.connection.kind;
    match &ev.key {
        Key::Escape if plain(ev) && b.console.run.is_some() => true,
        Key::Text(typed) if !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo) => {
            type_pair(&mut b.console, kind, typed)
        }
        Key::Backspace if plain(ev) => delete_pair(&mut b.console, kind),
        Key::Tab if !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo) => {
            indent(&mut b.console.sql, ev.mods.shift);
            true
        }
        Key::PageUp | Key::PageDown
            if !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo) =>
        {
            let page = ((b.console.editor.height / LINE_HEIGHT).floor() as usize).max(1);
            page_caret(
                &mut b.console.sql,
                page,
                ev.key == Key::PageDown,
                ev.mods.shift,
            );
            true
        }
        Key::Home | Key::End if !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo) => {
            let edit = &mut b.console.sql;
            let at = edit.focus.min(edit.text.len());
            let target = if ev.key == Key::Home {
                edit.text[..at].rfind('\n').map_or(0, |i| i + 1)
            } else {
                edit.text[at..]
                    .find('\n')
                    .map_or(edit.text.len(), |i| at + i)
            };
            if ev.mods.shift {
                edit.set_selection(edit.anchor, target);
            } else {
                edit.set_caret(target);
            }
            true
        }
        _ => false,
    }
}

fn closer_of(open: char) -> Option<char> {
    PAIRS
        .iter()
        .find(|(o, _)| *o == open)
        .map(|(_, close)| *close)
}

fn escaped(text: &str, at: usize) -> bool {
    text[..at].chars().rev().take_while(|c| *c == '\\').count() % 2 == 1
}

fn in_literal(c: &Console, kind: DbKind, at: usize) -> bool {
    let mut syntax = c.syntax.borrow_mut();
    syntax.sync(kind, &c.sql.text);
    syntax.in_literal(at)
}

fn type_pair(c: &mut Console, kind: DbKind, typed: &str) -> bool {
    let mut chars = typed.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return false;
    };
    let (at, end) = c.sql.range();
    if at != end {
        return false;
    }
    let text = &c.sql.text;
    let next = text[at..].chars().next();
    let quote = matches!(ch, '"' | '\'');
    let closes = PAIRS.iter().any(|(_, close)| *close == ch);
    if closes && next == Some(ch) && !escaped(text, at) && (quote || !in_literal(c, kind, at)) {
        c.sql.set_caret(at + ch.len_utf8());
        return true;
    }
    let Some(close) = closer_of(ch) else {
        return false;
    };
    if next.is_some_and(|n| !n.is_whitespace() && !CLOSE_BEFORE.contains(n)) {
        return false;
    }
    let after_word = text[..at]
        .chars()
        .next_back()
        .is_some_and(|p| p.is_alphanumeric() || p == '_');
    if quote && (escaped(text, at) || after_word) {
        return false;
    }
    if in_literal(c, kind, at) {
        return false;
    }
    c.sql.insert(&format!("{ch}{close}"));
    c.sql.set_caret(at + ch.len_utf8());
    true
}

fn delete_pair(c: &mut Console, kind: DbKind) -> bool {
    let (at, end) = c.sql.range();
    if at != end {
        return false;
    }
    let text = &c.sql.text;
    let (Some(open), Some(next)) = (text[..at].chars().next_back(), text[at..].chars().next())
    else {
        return false;
    };
    if closer_of(open) != Some(next) {
        return false;
    }
    let start = at - open.len_utf8();
    if escaped(text, start) || in_literal(c, kind, start) {
        return false;
    }
    c.sql.set_selection(start, at + next.len_utf8());
    c.sql.insert("");
    true
}

fn newline(c: &mut Console, kind: DbKind) {
    let indent = line_indent(&c.sql);
    let (at, end) = c.sql.range();
    let text = &c.sql.text;
    let split = at == end
        && text[..at]
            .chars()
            .next_back()
            .filter(|open| matches!(open, '(' | '[' | '{'))
            .and_then(closer_of)
            .is_some_and(|close| text[at..].starts_with(close))
        && !in_literal(c, kind, at);
    if split {
        let inner = format!("{indent}{INDENT}");
        c.sql.insert(&format!("\n{inner}\n{indent}"));
        c.sql.set_caret(at + 1 + inner.len());
    } else {
        c.sql.insert(&format!("\n{indent}"));
    }
}

fn indent(edit: &mut TextEdit, outdent: bool) {
    let (start, end) = edit.range();
    if start == end && !outdent {
        edit.insert(INDENT);
        return;
    }
    let text = &edit.text;
    let first = text[..start].rfind('\n').map_or(0, |at| at + 1);
    let last = text[end..].find('\n').map_or(text.len(), |at| end + at);
    let mut block = String::with_capacity(last - first + INDENT.len());
    let mut edits = Vec::new();
    let mut line_start = first;
    for (index, line) in text[first..last].split('\n').enumerate() {
        if index > 0 {
            block.push('\n');
        }
        if !outdent {
            edits.push(line_start);
            block.push_str(INDENT);
            block.push_str(line);
        } else if let Some(rest) = line.strip_prefix(INDENT) {
            edits.push(line_start);
            block.push_str(rest);
        } else {
            block.push_str(line);
        }
        line_start += line.len() + 1;
    }
    if edits.is_empty() {
        return;
    }
    let step = INDENT.len();
    let map = |at: usize| {
        if outdent {
            edits.iter().fold(at, |out, &edit| {
                out - at.min(edit + step).saturating_sub(edit)
            })
        } else {
            at + edits.iter().filter(|&&edit| edit <= at).count() * step
        }
    };
    let (anchor, focus) = (
        map(edit.anchor.min(text.len())),
        map(edit.focus.min(text.len())),
    );
    edit.set_selection(first, last);
    edit.insert(&block);
    edit.set_selection(anchor, focus);
}

fn page_caret(edit: &mut TextEdit, page: usize, down: bool, extend: bool) {
    let at = edit.focus.min(edit.text.len());
    let line = edit.line_of_byte(at);
    let last = edit.line_of_byte(edit.text.len());
    let target = if down {
        line + page
    } else {
        line.wrapping_sub(page)
    };
    let to = if down && target > last {
        edit.text.len()
    } else if !down && page > line {
        0
    } else {
        let column = edit.text[edit.line_range(line).start..at].chars().count();
        let range = edit.line_range(target);
        edit.text[range.clone()]
            .char_indices()
            .nth(column)
            .map_or(range.end, |(offset, _)| range.start + offset)
    };
    if extend {
        edit.set_selection(edit.anchor, to);
    } else {
        edit.set_caret(to);
    }
}

fn line_indent(edit: &TextEdit) -> String {
    let (start, _) = edit.range();
    let line = edit.text[..start].rfind('\n').map_or(0, |at| at + 1);
    edit.text[line..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

fn editor_paste(s: &mut Workspace, tab: TabId, content: &ClipboardContent) -> bool {
    let text = content.text();
    let clean = normalize(text);
    if clean == text {
        return false;
    }
    match s.browser_mut(tab) {
        Some(b) => {
            b.console.sql.insert(&clean);
            true
        }
        None => false,
    }
}

fn editor_changed(s: &mut Workspace, tab: TabId, tx: &mut Tx) {
    let Some(b) = s.browser_mut(tab) else {
        return;
    };
    clear_status(b);
    s.sync_title(tx);
}

fn small_button(label: &str, variant: ButtonVariant) -> Button {
    Button::new(label)
        .variant(variant)
        .control_size(ControlSize::Small)
}

fn shortcut(cmd: Command) -> String {
    keys::label_in(cmd, Ctx::Console).unwrap_or_default()
}

fn icon_button(glyph: &'static rune::Icon, label: &str) -> IconButton {
    IconButton::new(glyph)
        .variant(IconButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .label(label)
        .help(label)
}

pub fn view(
    _s: &Workspace,
    b: &Browser,
    t: &Theme,
    cx: &mut Cx<Workspace>,
    _layer: &mut Vec<El>,
) -> El {
    let tab = b.id;
    let room = cx
        .layout_size("console")
        .map(|[_, height]| height - b.console.split.min - 5.);
    let height = b.console.split.clamped(room);
    col()
        .key("console")
        .grow(1.)
        .w_full()
        .min_h(0.)
        .min_w(0.)
        .child(
            col()
                .h(height)
                .min_h(0.)
                .min_w(0.)
                .child(editor_pane(b, tab, t)),
        )
        .child(ui::v_handle(
            "console-split",
            t,
            move |s: &mut Workspace| s.browser_mut(tab).map(|b| &mut b.console.split),
        ))
        .child(
            col()
                .grow(1.)
                .min_h(0.)
                .min_w(0.)
                .child(result_pane(b, tab, t)),
        )
}

fn editor_pane(b: &Browser, tab: TabId, t: &Theme) -> El {
    let c = &b.console;
    let title =
        row()
            .gap(4.)
            .min_w(0.)
            .center_y()
            .child(ui::label("SQL", t.bold(t.text_sm), t.blue));
    let mut toolbar = row()
        .gap(4.)
        .center_y()
        .children(c.finding.then(|| find_bar(c, tab, t)));
    match &c.run {
        Some(run) => {
            toolbar = toolbar
                .child(
                    text(ticker(run.seconds, run.cancelling))
                        .font(t.text_xs)
                        .color(t.warning)
                        .key("console-ticker")
                        .mr(4.),
                )
                .child(
                    small_button("Cancel", ButtonVariant::Danger)
                        .icon(&icons::SQUARE)
                        .disabled(run.cancelling)
                        .help("Cancel the running script")
                        .shortcut(shortcut(Command::CancelScript))
                        .on_press(move |s: &mut Workspace| {
                            if let Some(b) = s.browser_mut(tab) {
                                b.clear_status();
                                cancel(b);
                            }
                        })
                        .build(&t.k)
                        .key("cancel-script"),
                );
        }
        None => {
            toolbar = toolbar.child(
                small_button("Run", ButtonVariant::Submit)
                    .icon(&icons::PLAY)
                    .help("Run the script (only the selection, if any)")
                    .shortcut(shortcut(Command::RunScript))
                    .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                        if let Some(b) = s.browser_mut(tab) {
                            b.clear_status();
                        }
                        run_script(s, tab, tx);
                    })
                    .build(&t.k)
                    .key("run-script"),
            );
        }
    }
    ui::pane("console-editor", title, Some(toolbar), editor(b, tab, t), t)
}

fn find_bar(c: &Console, tab: TabId, t: &Theme) -> El {
    let input = TextInput::new(&c.find)
        .placeholder("Find")
        .label("Find in the script")
        .leading_icon(&icons::SEARCH)
        .leading_icon_size(14.)
        .text_style(t.text_sm)
        .height(22.)
        .width(Some(200.))
        .on_edit(move |s: &mut Workspace| workspace::browser_edit(s, tab, |b| &mut b.console.find))
        .on_change(move |s: &mut Workspace, tx: &mut Tx| find_changed(s, tab, tx))
        .on_key(move |s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| {
            find_key(s, tab, ev, tx) || keys::field_key(s, ev, tx)
        })
        .build(&t.k)
        .key(FIND);
    let seek = move |how: Seek| {
        move |s: &mut Workspace, tx: &mut Tx| {
            if let Some(b) = s.browser_mut(tab) {
                find_seek(&mut b.console, how, tx);
            }
        }
    };
    row()
        .key("console-find-bar")
        .gap(2.)
        .center_y()
        .mr(8.)
        .child(input)
        .children(c.find_status().map(|status| {
            let color = if status == "No matches" {
                t.danger
            } else {
                t.muted
            };
            text(status)
                .font(t.text_xs)
                .color(color)
                .key("console-find-status")
                .ml(4.)
                .mr(2.)
        }))
        .child(
            icon_button(&icons::CHEVRON_UP, "Previous match")
                .shortcut(FIND_PREV_CHORD.label())
                .on_press_tx(seek(Seek::Prev))
                .build(&t.k)
                .key("find-prev"),
        )
        .child(
            icon_button(&icons::CHEVRON_DOWN, "Next match")
                .shortcut(FIND_NEXT_CHORD.label())
                .on_press_tx(seek(Seek::Next))
                .build(&t.k)
                .key("find-next"),
        )
        .child(
            icon_button(&icons::CLOSE, "Close the search")
                .on_press(move |s: &mut Workspace| {
                    if let Some(b) = s.browser_mut(tab) {
                        close_find(&mut b.console);
                    }
                })
                .build(&t.k)
                .key("find-close"),
        )
}

fn find_highlights(c: &Console, window: Range<usize>, t: &Theme) -> Vec<TextHighlight> {
    if !c.finding {
        return Vec::new();
    }
    let (start, end) = c.sql.range();
    let current = start..end;
    c.find_hits()
        .into_iter()
        .filter(|hit| *hit == current || (hit.end >= window.start && hit.start <= window.end))
        .map(|hit| {
            if hit == current {
                TextHighlight::new(hit, t.warning.alpha(0.45)).key(FIND_MATCH)
            } else {
                TextHighlight::new(hit, t.warning.alpha(0.2))
            }
        })
        .collect()
}

fn editor(b: &Browser, tab: TabId, t: &Theme) -> El {
    let c = &b.console;
    let text_now = &c.sql.text;
    let (lines, spans, window) = {
        let mut syntax = c.syntax.borrow_mut();
        syntax.sync(b.connection.kind, text_now);
        let first = (c.editor.block * WINDOW_BLOCK).saturating_sub(WINDOW_MARGIN);
        let visible = (c.editor.height / LINE_HEIGHT).ceil().max(40.) as usize;
        let last = c.editor.block * WINDOW_BLOCK + WINDOW_BLOCK + visible + WINDOW_MARGIN;
        (
            syntax.len().max(1),
            syntax.spans(first..last, &Palette::new(t)),
            syntax.byte_range(first..last),
        )
    };
    let highlights = find_highlights(c, window, t);
    let cell = advance("0", t.mono).max(1.);
    let longest = text_now
        .split('\n')
        .map(|line| line.width())
        .max()
        .unwrap_or(0);
    let digits = lines.to_string().len().max(2);
    let gutter_width = digits as f32 * cell + 2. * GUTTER_PADDING;
    let content_width = (longest as f32 + 4.) * cell + 8.;
    let field_width = content_width.max(c.editor.width - gutter_width - 4.);
    let height = (lines as f32 * LINE_HEIGHT + 8.).max(c.editor.height - 4.);
    let numbers = (1..=lines)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let gutter = col()
        .key("console-gutter")
        .w(gutter_width)
        .min_h(height)
        .px(GUTTER_PADDING)
        .pt(4.)
        .fill(t.background)
        .border_sides([0., 1., 0., 0.], t.border)
        .sticky_left(0.)
        .a11y_hidden()
        .child(
            text(numbers)
                .font(t.mono)
                .color(t.faint)
                .align(Align::End)
                .w_full(),
        );
    let field = text_input(&c.sql, t.mono)
        .key(SQL)
        .label("SQL editor")
        .color(t.foreground)
        .caret(t.foreground, t.k.selection())
        .placeholder("Type SQL here", t.faint)
        .spans(spans)
        .text_highlights(highlights)
        .w(field_width)
        .min_h(height)
        .pl(6.)
        .pt(4.)
        .no_focus_ring()
        .on_edit(move |s: &mut Workspace| workspace::browser_edit(s, tab, |b| &mut b.console.sql))
        .on_submit(move |s: &mut Workspace, _tx: &mut Tx| {
            if let Some(b) = s.browser_mut(tab) {
                let kind = b.connection.kind;
                newline(&mut b.console, kind);
            }
        })
        .on_input_key(move |s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| {
            editor_input_key(s, tab, ev, tx)
        })
        .on_input_paste(
            move |s: &mut Workspace, content: &ClipboardContent, _tx: &mut Tx| {
                editor_paste(s, tab, content)
            },
        )
        .on_input_change(move |s: &mut Workspace, tx: &mut Tx| editor_changed(s, tab, tx))
        .on_layout_tx(move |s: &mut Workspace, r: [f32; 4], tx: &mut Tx| {
            let Some(b) = s.browser_mut(tab) else {
                tx.live_only();
                return;
            };
            let view = &mut b.console.editor;
            let scrolled = (view.top - r[1]).max(0.);
            let block = (scrolled / LINE_HEIGHT) as usize / WINDOW_BLOCK;
            tx.live_only();
            if block == view.block {
                view.pending = None;
            } else {
                view.pending = Some(block);
                view.moved = tx.clock();
                tx.wake_after(Duration::from_secs_f64(WINDOW_SETTLE));
            }
        });
    let viewport = col()
        .key(EDITOR_SCROLL)
        .grow(1.)
        .min_h(0.)
        .min_w(0.)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_xy()
        .scroll_margin(LINE_HEIGHT)
        .on_layout_tx(move |s: &mut Workspace, r: [f32; 4], tx: &mut Tx| {
            let Some(b) = s.browser_mut(tab) else {
                tx.live_only();
                return;
            };
            let view = &mut b.console.editor;
            if (view.top, view.width, view.height) == (r[1], r[2], r[3]) {
                tx.live_only();
            } else {
                view.top = r[1];
                view.width = r[2];
                view.height = r[3];
            }
        })
        .child(row().items_start().child(gutter).child(field));
    t.k.scroll_bar(b.scrolls.keep(EDITOR_SCROLL, viewport))
}

fn result_pane(b: &Browser, tab: TabId, t: &Theme) -> El {
    let title = ui::label("Result", t.bold(t.text_sm), t.blue);
    let extra = result_extra(b, tab, t);
    let body = result_body(b, tab, t);
    ui::pane(RESULT, title, extra, body, t)
        .focusable(true)
        .on_key(move |s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| {
            keys::route(s, ev, tx) || result_key(s, tab, ev, tx)
        })
}

fn result_extra(b: &Browser, tab: TabId, t: &Theme) -> Option<El> {
    let c = &b.console;
    let result = c.result()?;
    let summary = text(ui::summary(result))
        .font(t.text_xs)
        .color(t.muted)
        .truncate()
        .key("console-summary");
    let Some(row_index) = c.record else {
        return Some(summary);
    };
    let total = result.rows.len();
    let last = total.saturating_sub(1);
    let controls = row()
        .gap(2.)
        .center_y()
        .child(
            text(format!(
                "row {} of {}",
                ui::group(row_index as u64 + 1),
                ui::group(total as u64)
            ))
            .font(t.text_xs)
            .color(t.muted)
            .key("console-record-position")
            .mr(4.),
        )
        .child(
            icon_button(&icons::CHEVRON_UP, "Previous row")
                .shortcut(shortcut(Command::PrevRecord))
                .disabled(row_index == 0)
                .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                    if let Some(b) = s.browser_mut(tab) {
                        step_record(b, -1, tx);
                    }
                })
                .build(&t.k)
                .key("record-prev"),
        )
        .child(
            icon_button(&icons::CHEVRON_DOWN, "Next row")
                .shortcut(shortcut(Command::NextRecord))
                .disabled(row_index >= last)
                .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                    if let Some(b) = s.browser_mut(tab) {
                        step_record(b, 1, tx);
                    }
                })
                .build(&t.k)
                .key("record-next"),
        )
        .child(
            icon_button(&icons::COPY, "Copy the row (tab-separated)")
                .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                    if let Some(b) = s.browser_mut(tab) {
                        copy_selected_row(b, tx);
                    }
                })
                .build(&t.k)
                .key("record-copy"),
        )
        .child(
            icon_button(&icons::CLOSE, "Back to the grid")
                .shortcut(shortcut(Command::CloseRecord))
                .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                    if let Some(b) = s.browser_mut(tab) {
                        close_record(b, tx);
                    }
                })
                .build(&t.k)
                .key("record-close"),
        );
    Some(controls)
}

fn centered() -> El {
    col().grow(1.).w_full().h_full().center().gap(8.)
}

fn result_body(b: &Browser, tab: TabId, t: &Theme) -> El {
    let c = &b.console;
    if let Some(run) = &c.run {
        let lines: &[&str] = if run.cancelling {
            &["Cancelling the script…"]
        } else {
            &["Running the script…", "Press Esc or ⌘. to cancel it"]
        };
        return centered()
            .key("console-running")
            .child(ui::spinner(20., t.muted, t))
            .children(
                lines
                    .iter()
                    .map(|line| ui::label(*line, t.text_sm, t.muted)),
            );
    }
    match &c.outcome {
        None => ui::placeholder(
            &[
                ("Nothing has been run yet", true),
                ("Write a script above and press ⌘⏎ or F5 to run it", false),
            ],
            t,
        ),
        Some(Outcome::Cancelled) => centered()
            .key("console-cancelled")
            .child(ui::icon_el(&icons::CIRCLE_X, 20., t.warning))
            .child(ui::label("Script cancelled", t.text, t.warning)),
        Some(Outcome::Failed(message)) => {
            let copied = message.clone();
            col()
                .key("console-failed")
                .grow(1.)
                .w_full()
                .min_h(0.)
                .padding(12.)
                .gap(8.)
                .child(
                    row()
                        .gap(8.)
                        .center_y()
                        .child(ui::icon_el(&icons::TRIANGLE_ALERT, 16., t.danger))
                        .child(ui::label("Script failed", t.bold(t.text), t.danger))
                        .child(
                            icon_button(&icons::COPY, "Copy the error message")
                                .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                                    copy_value(s, tab, copied.clone(), tx)
                                })
                                .build(&t.k)
                                .key("copy-error"),
                        ),
                )
                .child(
                    col()
                        .key("script-error")
                        .grow(1.)
                        .min_h(0.)
                        .scroll(ScrollSpec {
                            stick_bottom: false,
                        })
                        .child(
                            text(message.clone())
                                .font(t.text_sm)
                                .color(t.danger)
                                .wrap_anywhere()
                                .selectable(true)
                                .key(ERROR_TEXT),
                        ),
                )
        }
        Some(Outcome::Done(result)) if result.result_sets == 0 => centered()
            .key("console-completed")
            .child(ui::icon_el(&icons::CIRCLE_CHECK, 20., t.success))
            .child(ui::label("Script completed", t.bold(t.text), t.success))
            .child(ui::label(ui::summary(result), t.text_sm, t.muted)),
        Some(Outcome::Done(result)) if result.columns.is_empty() => {
            ui::placeholder(&[("No columns", false)], t)
        }
        Some(Outcome::Done(result)) => match c.record {
            Some(row) => record::view(tab, &record::fields(result, row), &c.record_scroll, t),
            None => grid::view(tab, &c.grid, result, &b.scrolls, t),
        },
    }
}

fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' | '\u{2028}' | '\u{2029}' => out.push('\n'),
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn is_blank(text: &str) -> bool {
    text.trim().is_empty()
}

fn runnable(text: &str, selection: &str) -> Option<String> {
    let sql = if is_blank(selection) { text } else { selection };
    (!is_blank(sql)).then(|| sql.to_string())
}

fn ticker(seconds: u64, cancelling: bool) -> String {
    if cancelling {
        "cancelling…".to_string()
    } else {
        format!("running… {}s", ui::group(seconds))
    }
}

fn copied_message(text: &str) -> String {
    let lines = text.lines().count();
    if lines > 1 {
        format!("Copied {}", ui::plural(lines as u64, "line"))
    } else {
        format!(
            "Copied {}",
            ui::plural(text.chars().count() as u64, "character")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{copied_message, normalize, runnable, ticker};

    #[test]
    fn pasted_text_is_normalized() {
        assert_eq!(
            normalize("SELECT\r\n\t1,\r2\u{0}\u{1b}[31m;\n"),
            "SELECT\n    1,\n2[31m;\n"
        );
        assert_eq!(normalize("a\u{2028}b\u{2029}c"), "a\nb\nc");
        assert_eq!(normalize("ю😀é"), "ю😀é");
    }

    #[test]
    fn runs_the_selection_or_the_whole_text() {
        assert_eq!(
            runnable("select 1;\nselect 2;", ""),
            Some("select 1;\nselect 2;".to_string())
        );
        assert_eq!(
            runnable("select 1;\nselect 2;", "select 2;"),
            Some("select 2;".to_string())
        );
        assert_eq!(
            runnable("select 1;", "  \n"),
            Some("select 1;".to_string()),
            "a blank selection is ignored"
        );
        assert_eq!(runnable("", ""), None);
        assert_eq!(runnable(" \n\t", ""), None);
        assert_eq!(
            runnable("select 1 ", ""),
            Some("select 1 ".to_string()),
            "the raw text is sent"
        );
    }

    #[test]
    fn ticker_shows_grouped_seconds() {
        assert_eq!(ticker(0, false), "running… 0s");
        assert_eq!(ticker(75, false), "running… 75s");
        assert_eq!(ticker(3600, false), "running… 3,600s");
        assert_eq!(ticker(5, true), "cancelling…");
    }

    #[test]
    fn copy_messages_count_lines_or_characters() {
        assert_eq!(copied_message("a\nb\nc"), "Copied 3 lines");
        assert_eq!(copied_message("abc"), "Copied 3 characters");
        assert_eq!(copied_message("x"), "Copied 1 character");
        assert_eq!(copied_message(""), "Copied 0 characters");
    }
}
