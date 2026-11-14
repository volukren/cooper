use std::{borrow::Cow, cell::Cell, rc::Rc};

use rune::{
    Color, Cursor, Drag, El, GrowthSpec, Key, KeyEvent, Live, Phase, ScrollPos, ScrollSpec, Tx,
    col, el, row, text,
};

use crate::{
    db::{self, ColumnInfo, Page, Session, TableDdl, TableKind},
    theme::Theme,
    ui,
};

use super::format::{NULL, PAGE_SIZE, column_widths, gutter_width};

pub trait Requests {
    fn fetch_page(&self, schema: &str, table: &str, offset: u64, limit: u32) -> u64;
    fn count_rows(&self, schema: &str, table: &str) -> u64;
    fn fetch_ddl(&self, schema: &str, table: &str) -> u64;
}

impl Requests for Session {
    fn fetch_page(&self, schema: &str, table: &str, offset: u64, limit: u32) -> u64 {
        Session::fetch_page(self, schema, table, offset, limit)
    }

    fn count_rows(&self, schema: &str, table: &str) -> u64 {
        Session::count_rows(self, schema, table)
    }

    fn fetch_ddl(&self, schema: &str, table: &str) -> u64 {
        Session::fetch_ddl(self, schema, table)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Hint(&'static str),
    Error(String),
    ClearHint,
}

pub const FIRST_PAGE_HINT: &str = "Already on the first page";
pub const LAST_PAGE_HINT: &str = "Already on the last page";
pub const COUNTING_HINT: &str = "Counting rows to find the last page…";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageArrival {
    Stale,
    Refreshed,
    Moved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    Stale,
    Done,
    Notify(Notice),
}

#[derive(Debug)]
pub struct Grid {
    schema: String,
    table: String,
    kind: TableKind,
    page: Option<Page>,
    offset: u64,
    page_request: Option<u64>,
    count_request: Option<u64>,
    ddl_request: Option<u64>,
    count: Option<u64>,
    count_failed: bool,
    pending_last: bool,
    error: Option<String>,
    row: usize,
    record: bool,
    ddl: Option<TableDdl>,
    ddl_error: Option<String>,
}

impl Grid {
    pub fn open(schema: &str, table: &str, kind: TableKind, requests: &impl Requests) -> Self {
        Self {
            schema: schema.to_string(),
            table: table.to_string(),
            kind,
            page: None,
            offset: 0,
            page_request: Some(requests.fetch_page(schema, table, 0, PAGE_SIZE)),
            count_request: Some(requests.count_rows(schema, table)),
            ddl_request: None,
            count: None,
            count_failed: false,
            pending_last: false,
            error: None,
            row: 0,
            record: false,
            ddl: None,
            ddl_error: None,
        }
    }

    pub fn is(&self, schema: &str, table: &str) -> bool {
        self.schema == schema && self.table == table
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub fn table(&self) -> &str {
        &self.table
    }

    #[cfg(test)]
    pub fn test_requests(&self) -> (Option<u64>, Option<u64>, Option<u64>) {
        (self.page_request, self.count_request, self.ddl_request)
    }

    pub fn kind(&self) -> TableKind {
        self.kind
    }

    pub fn page(&self) -> Option<&Page> {
        self.page.as_ref()
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn ddl(&self) -> Option<&TableDdl> {
        self.ddl.as_ref()
    }

    pub fn ddl_error(&self) -> Option<&str> {
        self.ddl_error.as_deref()
    }

    pub fn count_failed(&self) -> bool {
        self.count_failed
    }

    pub fn is_loading(&self) -> bool {
        self.page_request.is_some()
    }

    pub fn is_counting_for_last(&self) -> bool {
        self.pending_last
    }

    pub fn row(&self) -> usize {
        self.row
    }

    pub fn row_count(&self) -> usize {
        self.page.as_ref().map_or(0, |page| page.rows.len())
    }

    pub fn set_row(&mut self, row: usize) {
        self.row = row.min(self.row_count().saturating_sub(1));
    }

    pub fn total(&self) -> Option<u64> {
        self.count.or_else(|| {
            self.page
                .as_ref()
                .filter(|page| !page.has_more && (page.offset == 0 || !page.rows.is_empty()))
                .map(|page| page.offset + page.rows.len() as u64)
        })
    }

    pub fn is_empty(&self) -> bool {
        self.error.is_none()
            && self
                .page
                .as_ref()
                .is_some_and(|page| page.offset == 0 && page.rows.is_empty())
    }

    pub fn record_open(&self) -> bool {
        self.record && self.error.is_none()
    }

    pub fn can_go_next(&self) -> bool {
        let next = self.offset + u64::from(PAGE_SIZE);
        let shown = self.page.as_ref().filter(|page| page.offset == self.offset);
        shown.is_some_and(|page| page.has_more) || self.total().is_some_and(|total| next < total)
    }

    pub fn can_go_last(&self) -> bool {
        self.total()
            .is_none_or(|total| db::last_page_offset(total, PAGE_SIZE) != self.offset)
    }

    fn go_to(&mut self, offset: u64, requests: &impl Requests) {
        self.offset = offset;
        self.error = None;
        self.pending_last = false;
        self.page_request = Some(requests.fetch_page(&self.schema, &self.table, offset, PAGE_SIZE));
    }

    fn request_count(&mut self, requests: &impl Requests) {
        self.count_request = Some(requests.count_rows(&self.schema, &self.table));
    }

    pub fn next_page(&mut self, requests: &impl Requests) -> Option<Notice> {
        let shown = self
            .page
            .as_ref()
            .is_some_and(|page| page.offset == self.offset);
        if self.can_go_next() {
            self.go_to(self.offset + u64::from(PAGE_SIZE), requests);
            None
        } else if shown {
            Some(Notice::Hint(LAST_PAGE_HINT))
        } else {
            None
        }
    }

    pub fn previous_page(&mut self, requests: &impl Requests) -> Option<Notice> {
        if self.offset == 0 {
            return Some(Notice::Hint(FIRST_PAGE_HINT));
        }
        self.go_to(self.offset.saturating_sub(u64::from(PAGE_SIZE)), requests);
        None
    }

    pub fn first_page(&mut self, requests: &impl Requests) {
        if self.offset == 0 {
            self.pending_last = false;
            self.row = 0;
        } else {
            self.go_to(0, requests);
        }
    }

    pub fn last_page(&mut self, requests: &impl Requests) -> Option<Notice> {
        match self.total() {
            Some(total) => {
                let offset = db::last_page_offset(total, PAGE_SIZE);
                if offset == self.offset {
                    self.row = self.row_count().saturating_sub(1);
                } else {
                    self.go_to(offset, requests);
                }
                None
            }
            None => {
                self.pending_last = true;
                if self.count_request.is_none() {
                    self.request_count(requests);
                }
                Some(Notice::Hint(COUNTING_HINT))
            }
        }
    }

    pub fn refresh(&mut self, requests: &impl Requests) {
        self.ddl = None;
        self.ddl_request = None;
        self.ddl_error = None;
        self.go_to(self.offset, requests);
        self.request_count(requests);
    }

    pub fn fetch_ddl(&mut self, requests: &impl Requests) {
        if self.ddl.is_none() && self.ddl_request.is_none() {
            self.ddl_error = None;
            self.ddl_request = Some(requests.fetch_ddl(&self.schema, &self.table));
        }
    }

    pub fn open_record(&mut self, requests: &impl Requests) -> bool {
        if self.error.is_none() && self.row_count() > 0 {
            self.record = true;
            self.fetch_ddl(requests);
        }
        self.record_open()
    }

    pub fn close_record(&mut self) {
        self.record = false;
    }

    pub fn step_record(&mut self, delta: isize) -> bool {
        let before = self.row;
        self.set_row(self.row.saturating_add_signed(delta));
        self.row != before
    }

    pub fn on_page(&mut self, id: u64, page: Page, requests: &impl Requests) -> PageArrival {
        if self.page_request != Some(id) {
            return PageArrival::Stale;
        }
        self.page_request = None;
        let same_offset = self
            .page
            .as_ref()
            .is_some_and(|shown| shown.offset == page.offset);
        self.offset = page.offset;
        if same_offset {
            self.row = self.row.min(page.rows.len().saturating_sub(1));
        } else {
            self.row = 0;
            self.record = false;
        }
        if page.rows.is_empty() {
            self.record = false;
        }
        self.page = Some(page);
        self.error = None;
        if self.record {
            self.fetch_ddl(requests);
        }
        if same_offset {
            PageArrival::Refreshed
        } else {
            PageArrival::Moved
        }
    }

    pub fn on_count(&mut self, id: u64, count: u64, requests: &impl Requests) -> Update {
        if self.count_request != Some(id) {
            return Update::Stale;
        }
        self.count_request = None;
        self.count_failed = false;
        self.count = Some(count);
        if !std::mem::take(&mut self.pending_last) {
            return Update::Done;
        }
        let offset = db::last_page_offset(count, PAGE_SIZE);
        if offset != self.offset {
            self.go_to(offset, requests);
        }
        Update::Notify(Notice::ClearHint)
    }

    pub fn on_ddl(&mut self, id: u64, ddl: TableDdl) -> bool {
        if self.ddl_request != Some(id) {
            return false;
        }
        self.ddl_request = None;
        self.ddl_error = None;
        self.ddl = Some(ddl);
        true
    }

    pub fn on_error(&mut self, id: u64, message: &str) -> Update {
        if self.page_request == Some(id) {
            self.page_request = None;
            self.error = Some(message.to_string());
            self.record = false;
            Update::Done
        } else if self.ddl_request == Some(id) {
            self.ddl_request = None;
            self.ddl_error = Some(message.to_string());
            Update::Done
        } else if self.count_request == Some(id) {
            self.count_request = None;
            self.count_failed = true;
            self.pending_last = false;
            Update::Notify(Notice::Error(format!("Failed to count rows: {message}")))
        } else {
            Update::Stale
        }
    }
}

pub const ROW_HEIGHT: f32 = 24.0;
pub const HEADER_HEIGHT: f32 = 28.0;
pub const TWO_LINE_HEADER: f32 = 44.0;
pub const CELL_PADDING: f32 = 14.0;
pub const MIN_COLUMN_WIDTH: f32 = 32.0;
const RESIZE_HANDLE: f32 = 6.0;
const FALLBACK_VIEWPORT: f32 = 1600.0;

pub fn mono_cell_width(t: &Theme) -> f32 {
    let advance = rune::text::advance("0", t.mono);
    if advance > 0.0 { advance } else { 8.0 }
}

#[derive(Clone)]
pub struct GridScroll {
    pos: Rc<Cell<ScrollPos>>,
    header: Live<[f32; 2]>,
    gutter: Live<[f32; 2]>,
}

impl Default for GridScroll {
    fn default() -> Self {
        Self {
            pos: Rc::default(),
            header: Live::new([0.0, 0.0]),
            gutter: Live::new([0.0, 0.0]),
        }
    }
}

impl std::fmt::Debug for GridScroll {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GridScroll({:?})", self.pos.get().offset)
    }
}

impl GridScroll {
    pub fn get(&self) -> ScrollPos {
        self.pos.get()
    }

    pub fn reset(&self) {
        self.pos.set(ScrollPos::default());
        self.header.set([0.0, 0.0]);
        self.gutter.set([0.0, 0.0]);
    }

    pub fn watch(&self, viewport: El) -> El {
        let (pos, header, gutter) = (self.pos.clone(), self.header.clone(), self.gutter.clone());
        viewport.on_scroll_offset(move |at: ScrollPos| {
            pos.set(at);
            header.set([-at.offset[0], 0.0]);
            gutter.set([0.0, -at.offset[1]]);
        })
    }
}

#[derive(Debug, Default)]
pub struct DataGrid {
    pub widths: Vec<f32>,
    pub natural: Vec<f32>,
    /// What each column's content needs, in digits of the mono font: the widths follow a font
    /// change from these.
    cells: Vec<usize>,
    gutter_cells: usize,
    pub gutter: f32,
    pub cell_width: f32,
    pub has_types: bool,
    pub rows: usize,
    pub cell: Option<(usize, Option<usize>)>,
    pub highlight: Option<usize>,
    pub window: (usize, usize),
    pub area: [f32; 2],
    pub scroll: GridScroll,
    pub to_top: bool,
    pub to_left: bool,
    pub to_selection: bool,
    resize_from: Option<f32>,
}

pub enum Move {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

impl Move {
    pub fn from_key(ev: &KeyEvent) -> Option<Move> {
        let mods = ev.mods;
        if mods.cmd || mods.ctrl || mods.alt || mods.logo || mods.shift {
            return None;
        }
        Some(match ev.key {
            Key::Up => Move::Up,
            Key::Down => Move::Down,
            Key::Left => Move::Left,
            Key::Right => Move::Right,
            Key::Home => Move::Home,
            Key::End => Move::End,
            Key::PageUp => Move::PageUp,
            Key::PageDown => Move::PageDown,
            _ => return None,
        })
    }
}

impl DataGrid {
    pub fn load(
        &mut self,
        columns: &[ColumnInfo],
        rows: &[Vec<Option<String>>],
        offset: u64,
        cell_width: f32,
    ) {
        let to_px = |cells: usize| cell_width * cells as f32 + CELL_PADDING;
        self.cell_width = cell_width;
        self.cells = column_widths(columns, rows);
        self.gutter_cells = gutter_width(offset, rows.len());
        self.natural = self.cells.iter().copied().map(to_px).collect();
        self.widths = self.natural.clone();
        self.gutter = to_px(self.gutter_cells);
        self.has_types = columns.iter().any(|column| !column.type_name.is_empty());
        self.rows = rows.len();
        self.highlight = None;
        self.resize_from = None;
        let (scroll_x, viewport) = self.scroll_x();
        self.window = self.visible_columns(scroll_x, viewport);
    }

    pub fn clear(&mut self) {
        self.cell = None;
        self.highlight = None;
    }

    pub fn select(&mut self, row: usize, col: Option<usize>) {
        self.cell = Some((row, col));
        self.highlight = Some(row);
    }

    pub fn header_height(&self) -> f32 {
        if self.has_types {
            TWO_LINE_HEADER
        } else {
            HEADER_HEIGHT
        }
    }

    pub fn content_width(&self) -> f32 {
        self.widths.iter().sum::<f32>()
    }

    pub fn column_x(&self, col: usize) -> f32 {
        self.widths.iter().take(col).sum::<f32>()
    }

    fn scroll_x(&self) -> (f32, f32) {
        let pos = self.scroll.get();
        let viewport = if pos.viewport[0] > 0.0 {
            pos.viewport[0]
        } else if self.area[1] > 0.0 {
            self.area[1]
        } else {
            FALLBACK_VIEWPORT
        };
        (pos.offset[0], viewport)
    }

    pub fn visible_columns(&self, scroll_x: f32, viewport: f32) -> (usize, usize) {
        let from = scroll_x - viewport;
        let to = scroll_x + viewport * 2.0;
        let mut x = 0.0;
        let mut start = None;
        let mut end = self.widths.len();
        for (index, width) in self.widths.iter().enumerate() {
            if start.is_none() && x + width >= from {
                start = Some(index);
            }
            if x > to {
                end = index;
                break;
            }
            x += width;
        }
        let start = start.unwrap_or(self.widths.len());
        (start.min(end), end)
    }

    pub fn follow_layout(&mut self, rows_x: f32) -> bool {
        let scroll_x = (self.area[0] - rows_x).max(0.0);
        let viewport = if self.area[1] > 0.0 {
            self.area[1]
        } else {
            FALLBACK_VIEWPORT
        };
        let window = self.visible_columns(scroll_x, viewport);
        if window == self.window {
            return false;
        }
        self.window = window;
        true
    }

    fn page_rows(&self) -> usize {
        let height = self.scroll.get().viewport[1].max(0.0);
        ((height / ROW_HEIGHT).floor() as usize).max(1)
    }

    pub fn step(&mut self, how: Move, rows: usize, cols: usize) -> bool {
        if rows == 0 || cols == 0 {
            return false;
        }
        let last_row = rows - 1;
        let last_col = cols - 1;
        let Some((row, col)) = self.cell else {
            match how {
                Move::End => self.select(0, Some(last_col)),
                _ => self.select(0, None),
            }
            return true;
        };
        let (row, col) = match how {
            Move::Up => (row.saturating_sub(1), col),
            Move::Down => ((row + 1).min(last_row), col),
            Move::Left => (row, col.and_then(|col| col.checked_sub(1))),
            Move::Right => (row, Some(col.map_or(0, |col| (col + 1).min(last_col)))),
            Move::Home => (row, None),
            Move::End => (row, Some(last_col)),
            Move::PageUp => (row.saturating_sub(self.page_rows()), col),
            Move::PageDown => ((row + self.page_rows()).min(last_row), col),
        };
        self.select(row.min(last_row), col.map(|col| col.min(last_col)));
        true
    }

    fn revealed(&self, from: [f32; 2]) -> Option<[f32; 2]> {
        let (row, col) = self.cell?;
        let [view_w, view_h] = self.scroll.get().viewport;
        if view_w <= 0.0 || view_h <= 0.0 {
            return None;
        }
        let [mut x, mut y] = from;
        let top = row as f32 * ROW_HEIGHT;
        if top < y || top + ROW_HEIGHT > y + view_h {
            y = top + ROW_HEIGHT / 2.0 - view_h / 2.0;
        }
        let max_y = (self.rows as f32 * ROW_HEIGHT - view_h).max(0.0);
        match col {
            None => x = 0.0,
            Some(col) => {
                let left = self.column_x(col);
                let width = self.widths.get(col).copied().unwrap_or(0.0);
                if left < x {
                    x = left;
                } else if left + width > x + view_w {
                    x = (left + width - view_w).min(left);
                }
            }
        }
        let max_x = (self.content_width() - view_w).max(0.0);
        Some([x.clamp(0.0, max_x), y.clamp(0.0, max_y)])
    }

    pub fn reveal_from(&mut self, prefix: &str, from: [f32; 2], tx: &mut Tx) {
        let [x, y] = self.revealed(from).unwrap_or(from);
        tx.scroll_to_instant(format!("{prefix}-grid-scroll"), x, y);
        let viewport = self.scroll.get().viewport[0];
        if viewport > 0.0 {
            self.window = self.visible_columns(x, viewport);
        }
    }

    pub fn reveal(&mut self, prefix: &str, tx: &mut Tx) {
        let pos = self.scroll.get();
        let Some([x, y]) = self.revealed(pos.offset) else {
            return;
        };
        if [x, y] != pos.offset {
            tx.scroll_to_instant(format!("{prefix}-grid-scroll"), x, y);
            self.window = self.visible_columns(x, pos.viewport[0]);
        }
    }

    pub fn resize(&mut self, col: usize, d: &Drag) {
        let Some(width) = self.widths.get_mut(col) else {
            return;
        };
        match d.phase {
            Phase::Down => self.resize_from = Some(*width),
            Phase::Move => {
                if let Some(from) = self.resize_from {
                    *width = (from + d.dx).max(MIN_COLUMN_WIDTH);
                }
            }
            Phase::Up => {
                if d.cancelled()
                    && let Some(from) = self.resize_from
                {
                    *width = from;
                }
                self.resize_from = None;
            }
        }
    }

    /// Sizes the columns for a mono font whose digits are `cell_width` wide: a column left at
    /// its natural width follows its content, one the user dragged keeps its size in digits.
    pub fn refit(&mut self, cell_width: f32) {
        if cell_width <= 0.0 || cell_width == self.cell_width || self.cells.is_empty() {
            return;
        }
        let to_px = |cells: usize| cell_width * cells as f32 + CELL_PADDING;
        let scale = if self.cell_width > 0.0 {
            cell_width / self.cell_width
        } else {
            1.0
        };
        for ((width, natural), &cells) in self
            .widths
            .iter_mut()
            .zip(self.natural.iter_mut())
            .zip(&self.cells)
        {
            let fitted = to_px(cells);
            *width = if *width == *natural {
                fitted
            } else {
                (*width * scale).max(MIN_COLUMN_WIDTH)
            };
            *natural = fitted;
        }
        self.gutter = to_px(self.gutter_cells);
        self.cell_width = cell_width;
        let (scroll_x, viewport) = self.scroll_x();
        self.window = self.visible_columns(scroll_x, viewport);
    }

    pub fn reset_width(&mut self, col: usize) {
        if let (Some(width), Some(natural)) = (self.widths.get_mut(col), self.natural.get(col)) {
            *width = *natural;
        }
    }

    fn overlay(&self, index: usize, t: &Theme) -> Color {
        if self.highlight == Some(index) {
            t.table_active.alpha(t.table_active.a * 0.5)
        } else if index % 2 == 1 {
            t.table_stripe
        } else {
            Color::TRANSPARENT
        }
    }
}

pub struct GridData<'a> {
    pub columns: &'a [ColumnInfo],
    pub rows: &'a [Vec<Option<String>>],
    pub offset: u64,
}

pub type GridGet<S> = Rc<dyn for<'a> Fn(&'a S) -> Option<(&'a DataGrid, GridData<'a>)>>;
pub type GridGetMut<S> = Rc<dyn for<'a> Fn(&'a mut S) -> Option<&'a mut DataGrid>>;
pub type CellHandler<S> = Rc<dyn Fn(&mut S, usize, Option<usize>, &mut Tx)>;
pub type TouchHandler<S> = Rc<dyn Fn(&mut S, &mut Tx)>;

pub struct GridSpec<S> {
    pub prefix: String,
    pub get: GridGet<S>,
    pub get_mut: GridGetMut<S>,
    pub on_select: CellHandler<S>,
    pub on_open: CellHandler<S>,
    pub on_touch: TouchHandler<S>,
    pub empty: &'static str,
}

impl<S> Clone for GridSpec<S> {
    fn clone(&self) -> Self {
        Self {
            prefix: self.prefix.clone(),
            get: self.get.clone(),
            get_mut: self.get_mut.clone(),
            on_select: self.on_select.clone(),
            on_open: self.on_open.clone(),
            on_touch: self.on_touch.clone(),
            empty: self.empty,
        }
    }
}

fn clip_chars(value: &str, width: f32, cell: f32) -> String {
    let fit = ((width / cell.max(1.0)).ceil() as usize).saturating_add(2);
    match value.char_indices().nth(fit) {
        Some((end, _)) => value[..end].to_string(),
        None => value.to_string(),
    }
}

fn spacer(width: f32) -> Option<El> {
    (width > 0.0).then(|| el().w(width).h_full())
}

fn header_cell<S: 'static>(
    spec: &GridSpec<S>,
    grid: &DataGrid,
    column: &ColumnInfo,
    index: usize,
    t: &Theme,
) -> El {
    let width = grid.widths.get(index).copied().unwrap_or(100.0);
    let get_mut = spec.get_mut.clone();
    let reset = spec.get_mut.clone();
    let touch = spec.on_touch.clone();
    let touch_reset = spec.on_touch.clone();
    let name = text(clip_chars(&column.name, width, grid.cell_width))
        .font(t.bold(t.mono))
        .color(t.foreground)
        .truncate();
    let labels = col()
        .grow(1.0)
        .min_w(0.0)
        .pl(7.0)
        .justify_center()
        .child(name)
        .children(grid.has_types.then(|| {
            text(clip_chars(&column.type_name, width, grid.cell_width))
                .font(t.mono)
                .color(t.muted)
                .truncate()
        }));
    let handle = el()
        .key(format!("{}-resize-{index}", spec.prefix))
        .w(RESIZE_HANDLE)
        .h_full()
        .cursor(Cursor::ResizeColumn)
        .role("separator")
        .hover(t.k.alpha(t.ring, 0.4))
        .on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
            if d.phase == Phase::Down {
                touch(s, tx);
            }
            if let Some(grid) = get_mut(s) {
                grid.resize(index, d);
            }
        })
        .on_double_press_tx(move |s: &mut S, tx: &mut Tx| {
            touch_reset(s, tx);
            if let Some(grid) = reset(s) {
                grid.reset_width(index);
            }
        });
    row()
        .w(width)
        .h_full()
        .child(labels)
        .child(handle)
        .help(column.name.clone())
}

fn gutter_cell<S: 'static>(
    spec: &GridSpec<S>,
    grid: &DataGrid,
    index: usize,
    offset: u64,
    t: &Theme,
) -> El {
    let on_select = spec.on_select.clone();
    let on_open = spec.on_open.clone();
    let cell = row()
        .w_full()
        .h(ROW_HEIGHT)
        .px(7.0)
        .center_y()
        .justify_end()
        .fill(grid.overlay(index, t))
        .on_press_tx(move |s: &mut S, tx: &mut Tx| on_select(s, index, None, tx))
        .on_double_press_tx(move |s: &mut S, tx: &mut Tx| on_open(s, index, None, tx))
        .child(
            text(ui::group(offset + index as u64 + 1))
                .font(t.mono)
                .color(t.muted),
        );
    if grid.cell == Some((index, None)) {
        cell.fill(t.table_active).border(1.0, t.ring)
    } else {
        cell
    }
}

fn grid_row<S: 'static>(spec: &GridSpec<S>, s: &S, index: usize, t: &Theme) -> El {
    let key = format!("{}-row-{index}", spec.prefix);
    let Some((grid, data)) = (spec.get)(s) else {
        return row().key(key).h(ROW_HEIGHT);
    };
    let values = data.rows.get(index);
    let (start, end) = grid.window;
    let left: f32 = grid.widths.iter().take(start).sum();
    let right: f32 = grid.widths.iter().skip(end).sum();
    let cells = (start..end.min(grid.widths.len())).map(|col_index| {
        let width = grid.widths[col_index];
        let value = values.and_then(|row| row.get(col_index)).cloned().flatten();
        let content = match value {
            Some(value) => text(clip_chars(&value, width, grid.cell_width))
                .font(t.mono)
                .color(t.foreground)
                .truncate(),
            None => text(NULL).font(t.mono.italic()).color(t.muted).truncate(),
        };
        let current = grid.cell == Some((index, Some(col_index)));
        let on_select = spec.on_select.clone();
        let on_open = spec.on_open.clone();
        let cell = row()
            .w(width)
            .h(ROW_HEIGHT)
            .px(7.0)
            .center_y()
            .child(content)
            .on_press_tx(move |s: &mut S, tx: &mut Tx| on_select(s, index, Some(col_index), tx))
            .on_double_press_tx(move |s: &mut S, tx: &mut Tx| {
                on_open(s, index, Some(col_index), tx)
            });
        if current {
            cell.fill(t.table_active).border(1.0, t.ring)
        } else {
            cell
        }
    });
    row()
        .key(key)
        .role("row")
        .w(grid.content_width())
        .h(ROW_HEIGHT)
        .fill(grid.overlay(index, t))
        .children(spacer(left))
        .children(cells)
        .children(spacer(right))
}

pub fn view<S: 'static>(spec: &GridSpec<S>, s: &S, t: &Theme) -> El {
    let prefix = spec.prefix.clone();
    let Some((grid, data)) = (spec.get)(s) else {
        return el();
    };
    let total = grid.content_width();
    let (start, end) = grid.window;
    let left: f32 = grid.widths.iter().take(start).sum();
    let right: f32 = grid.widths.iter().skip(end).sum();
    let header_h = grid.header_height();
    let header = row()
        .key(format!("{prefix}-grid-header"))
        .w(total)
        .h(header_h)
        .live_offset(&grid.scroll.header)
        .children(spacer(left))
        .children(
            data.columns
                .iter()
                .enumerate()
                .take(end)
                .skip(start)
                .map(|(index, column)| header_cell(spec, grid, column, index, t)),
        )
        .children(spacer(right));
    let corner = row()
        .w(grid.gutter)
        .h_full()
        .px(7.0)
        .center_y()
        .justify_end()
        .border_sides([0.0, 1.0, 0.0, 0.0], t.border)
        .child(text("#").font(t.mono).color(t.muted));
    let top = row()
        .key(format!("{prefix}-grid-top"))
        .w_full()
        .h(header_h)
        .fill(t.secondary)
        .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
        .child(corner)
        .child(
            row()
                .grow(1.0)
                .min_w(0.0)
                .h_full()
                .clip_rounded(0.0)
                .child(header),
        );
    let count = data.rows.len();
    let gutter = col()
        .key(format!("{prefix}-grid-gutter"))
        .w(grid.gutter)
        .h_full()
        .fill(t.background)
        .border_sides([0.0, 1.0, 0.0, 0.0], t.border)
        .clip_rounded(0.0)
        .child(
            col()
                .w_full()
                .live_offset(&grid.scroll.gutter)
                .children((0..count).map(|index| gutter_cell(spec, grid, index, data.offset, t))),
        );
    let body = if count == 0 {
        col()
            .key(format!("{prefix}-grid-empty"))
            .w_full()
            .h(80.0)
            .center()
            .child(ui::label(spec.empty, t.text_sm, t.muted))
    } else {
        let key_prefix = prefix.clone();
        let build = spec.clone();
        let dark = t.dark;
        let follow = spec.get_mut.clone();
        col()
            .key(format!("{prefix}-grid-rows"))
            .w(total)
            .growth(GrowthSpec::default())
            .virtual_rows(
                count,
                ROW_HEIGHT,
                move |_: &S, i| Cow::Owned(format!("{key_prefix}-row-{i}")),
                move |s: &S, i| grid_row(&build, s, i, crate::theme::theme(dark)),
            )
            .on_layout_tx(move |s: &mut S, rect: [f32; 4], tx: &mut Tx| {
                let changed = follow(s).is_some_and(|grid| grid.follow_layout(rect[0]));
                if !changed {
                    tx.live_only();
                }
            })
    };
    let area = spec.get_mut.clone();
    let viewport = col()
        .key(format!("{prefix}-grid-scroll"))
        .grow(1.0)
        .min_h(0.0)
        .min_w(0.0)
        .h_full()
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_xy()
        .on_layout_tx(move |s: &mut S, rect: [f32; 4], tx: &mut Tx| {
            let Some(grid) = area(s) else {
                return tx.live_only();
            };
            let next = [rect[0], rect[2]];
            if grid.area == next {
                return tx.live_only();
            }
            grid.area = next;
            let (scroll_x, viewport) = grid.scroll_x();
            let window = grid.visible_columns(scroll_x, viewport);
            if window == grid.window {
                tx.live_only();
            } else {
                grid.window = window;
            }
        })
        .child(body);
    let viewport = grid.scroll.watch(viewport);
    col()
        .key(format!("{prefix}-grid"))
        .role("grid")
        .grow(1.0)
        .min_h(0.0)
        .min_w(0.0)
        .child(top)
        .child(
            row()
                .grow(1.0)
                .min_h(0.0)
                .min_w(0.0)
                .w_full()
                .children((count > 0).then_some(gutter))
                .child(viewport),
        )
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::{
        COUNTING_HINT, FIRST_PAGE_HINT, Grid, LAST_PAGE_HINT, Notice, PageArrival, Requests, Update,
    };
    use crate::browser::table::format::page_footer;
    use crate::db::{ColumnDef, ColumnInfo, FullValues, Page, TableDdl, TableKind};

    #[derive(Default)]
    struct Fake {
        next: Cell<u64>,
        log: RefCell<Vec<String>>,
    }

    impl Fake {
        fn id(&self, what: String) -> u64 {
            self.log.borrow_mut().push(what);
            self.next.set(self.next.get() + 1);
            self.next.get()
        }

        fn sent(&self) -> Vec<String> {
            std::mem::take(&mut self.log.borrow_mut())
        }
    }

    impl Requests for Fake {
        fn fetch_page(&self, schema: &str, table: &str, offset: u64, limit: u32) -> u64 {
            self.id(format!("page {schema}.{table} {offset} {limit}"))
        }

        fn count_rows(&self, schema: &str, table: &str) -> u64 {
            self.id(format!("count {schema}.{table}"))
        }

        fn fetch_ddl(&self, schema: &str, table: &str) -> u64 {
            self.id(format!("ddl {schema}.{table}"))
        }
    }

    fn page_of(offset: u64, rows: usize, has_more: bool) -> Page {
        let column = |name: &str, type_name: &str| ColumnInfo {
            name: name.to_string(),
            type_name: type_name.to_string(),
        };
        Page {
            schema: "main".to_string(),
            table: "users".to_string(),
            columns: vec![
                column("id", "INTEGER"),
                column("name", "TEXT"),
                column("note", ""),
            ],
            rows: (0..rows)
                .map(|i| {
                    let n = offset as usize + i + 1;
                    vec![Some(n.to_string()), Some(format!("user {n}")), None]
                })
                .collect(),
            full: FullValues::new(),
            opaque: Default::default(),
            primary_key: Vec::new(),
            offset,
            has_more,
        }
    }

    fn users_ddl() -> TableDdl {
        TableDdl {
            sql: "CREATE TABLE users (id INTEGER PRIMARY KEY);".to_string(),
            columns: vec![ColumnDef {
                name: "id".to_string(),
                definition: "INTEGER PRIMARY KEY".to_string(),
            }],
        }
    }

    fn open(fake: &Fake) -> Grid {
        let grid = Grid::open("main", "users", TableKind::Table, fake);
        assert_eq!(fake.sent(), ["page main.users 0 100", "count main.users"]);
        grid
    }

    fn answer_page(grid: &mut Grid, fake: &Fake, page: Page) -> PageArrival {
        let id = grid.page_request.expect("a page is awaited");
        let arrival = grid.on_page(id, page, fake);
        assert_ne!(arrival, PageArrival::Stale);
        arrival
    }

    fn answer_count(grid: &mut Grid, fake: &Fake, count: u64) -> Update {
        let id = grid.count_request.expect("a count is awaited");
        let update = grid.on_count(id, count, fake);
        assert_ne!(update, Update::Stale);
        update
    }

    fn footer(grid: &Grid) -> String {
        let page = grid.page().expect("a page is shown");
        page_footer(page, grid.total(), grid.count_failed())
    }

    #[test]
    fn opening_a_table_requests_a_page_and_a_count() {
        let fake = Fake::default();
        let mut grid = open(&fake);
        assert!(grid.is("main", "users"));
        assert!(grid.is_loading());
        assert!(grid.page().is_none());

        assert_eq!(
            answer_page(&mut grid, &fake, page_of(0, 100, true)),
            PageArrival::Moved
        );
        assert!(!grid.is_loading());
        assert_eq!(footer(&grid), "rows 1–100 of … · page 1/…");
        assert_eq!(answer_count(&mut grid, &fake, 1250), Update::Done);
        assert_eq!(footer(&grid), "rows 1–100 of 1,250 · page 1/13");
        assert!(fake.sent().is_empty());
    }

    #[test]
    fn paging_moves_by_page_size() {
        let fake = Fake::default();
        let mut grid = open(&fake);
        answer_page(&mut grid, &fake, page_of(0, 100, true));
        answer_count(&mut grid, &fake, 250);

        assert_eq!(
            grid.previous_page(&fake),
            Some(Notice::Hint(FIRST_PAGE_HINT))
        );
        assert!(!grid.is_loading());

        assert_eq!(grid.next_page(&fake), None);
        assert_eq!(grid.offset(), 100);
        assert!(grid.is_loading());
        assert_eq!(
            footer(&grid),
            "rows 1–100 of 250 · page 1/3",
            "the old page stays"
        );
        answer_page(&mut grid, &fake, page_of(100, 100, true));
        assert_eq!(footer(&grid), "rows 101–200 of 250 · page 2/3");

        grid.next_page(&fake);
        answer_page(&mut grid, &fake, page_of(200, 50, false));
        assert_eq!(footer(&grid), "rows 201–250 of 250 · page 3/3");
        assert!(!grid.can_go_next());
        assert!(!grid.can_go_last());

        assert_eq!(grid.next_page(&fake), Some(Notice::Hint(LAST_PAGE_HINT)));
        assert!(!grid.is_loading());

        grid.previous_page(&fake);
        assert_eq!(grid.offset(), 100);
        grid.previous_page(&fake);
        assert_eq!(grid.offset(), 0);
        assert_eq!(grid.last_page(&fake), None);
        assert_eq!(grid.offset(), 200);
        grid.first_page(&fake);
        assert_eq!(grid.offset(), 0);
    }

    #[test]
    fn next_page_waits_for_the_current_one_when_the_total_is_unknown() {
        let fake = Fake::default();
        let mut grid = open(&fake);
        assert!(!grid.can_go_next());
        assert_eq!(grid.next_page(&fake), None);
        assert_eq!(grid.offset(), 0);
        answer_page(&mut grid, &fake, page_of(0, 100, true));
        grid.next_page(&fake);
        assert_eq!(grid.offset(), 100);
        assert_eq!(grid.next_page(&fake), None);
        assert_eq!(grid.offset(), 100);
        answer_count(&mut grid, &fake, 1000);
        grid.next_page(&fake);
        grid.next_page(&fake);
        assert_eq!(grid.offset(), 300);
    }

    #[test]
    fn last_page_waits_for_the_count() {
        let fake = Fake::default();
        let mut grid = open(&fake);
        answer_page(&mut grid, &fake, page_of(0, 100, true));
        assert!(grid.can_go_last());
        assert_eq!(grid.last_page(&fake), Some(Notice::Hint(COUNTING_HINT)));
        assert_eq!(grid.offset(), 0);
        assert!(grid.is_counting_for_last());
        assert!(fake.sent().is_empty(), "the count is already in flight");

        assert_eq!(
            answer_count(&mut grid, &fake, 250),
            Update::Notify(Notice::ClearHint)
        );
        assert_eq!(grid.offset(), 200);
        assert!(grid.is_loading());
        assert!(!grid.is_counting_for_last());
        answer_page(&mut grid, &fake, page_of(200, 50, false));
        assert_eq!(footer(&grid), "rows 201–250 of 250 · page 3/3");
    }

    #[test]
    fn count_error_is_reported_and_the_last_page_retries_it() {
        let fake = Fake::default();
        let mut grid = open(&fake);
        answer_page(&mut grid, &fake, page_of(0, 100, true));
        grid.last_page(&fake);
        let id = grid.count_request.expect("count");
        assert_eq!(
            grid.on_error(id, "too slow"),
            Update::Notify(Notice::Error("Failed to count rows: too slow".to_string()))
        );
        assert!(!grid.is_counting_for_last());
        assert_eq!(footer(&grid), "rows 1–100 of ? · page 1/?");

        grid.last_page(&fake);
        assert_eq!(fake.sent(), ["count main.users"]);
        assert!(grid.count_request.is_some_and(|new| new != id));
    }

    #[test]
    fn total_is_known_without_a_count_on_the_last_page() {
        let fake = Fake::default();
        let mut grid = open(&fake);
        answer_page(&mut grid, &fake, page_of(0, 42, false));
        assert_eq!(footer(&grid), "rows 1–42 of 42 · page 1/1");
        assert_eq!(grid.last_page(&fake), None);
        assert_eq!(grid.row(), 41);
        assert!(!grid.is_loading());
        grid.first_page(&fake);
        assert_eq!(grid.row(), 0);
        assert!(!grid.is_loading());
    }

    #[test]
    fn stale_responses_are_ignored() {
        let fake = Fake::default();
        let users = open(&fake);
        let (page, count) = (users.page_request.unwrap(), users.count_request.unwrap());
        let mut grid = Grid::open("main", "empty", TableKind::Table, &fake);

        assert_eq!(
            grid.on_page(page, page_of(0, 5, false), &fake),
            PageArrival::Stale
        );
        assert_eq!(grid.on_count(count, 5, &fake), Update::Stale);
        assert_eq!(grid.on_error(page, "late"), Update::Stale);
        assert!(!grid.on_ddl(page, users_ddl()));
        assert!(grid.page().is_none());
        assert!(grid.total().is_none());
        assert!(grid.error().is_none());
    }

    #[test]
    fn page_error_is_shown_and_refresh_retries() {
        let fake = Fake::default();
        let mut grid = open(&fake);
        let id = grid.page_request.unwrap();
        assert_eq!(grid.on_error(id, "relation does not exist"), Update::Done);
        assert_eq!(grid.error(), Some("relation does not exist"));
        assert!(!grid.open_record(&fake));

        grid.refresh(&fake);
        assert_eq!(fake.sent(), ["page main.users 0 100", "count main.users"]);
        assert!(grid.error().is_none());
        assert!(grid.page_request.is_some_and(|new| new != id));
        answer_page(&mut grid, &fake, page_of(0, 2, false));
        assert_eq!(grid.row_count(), 2);
    }

    #[test]
    fn an_empty_table_does_not_request_its_definition() {
        let fake = Fake::default();
        let mut grid = open(&fake);
        answer_page(&mut grid, &fake, page_of(0, 0, false));
        answer_count(&mut grid, &fake, 0);
        assert!(grid.is_empty());
        assert!(fake.sent().is_empty());
        assert_eq!(footer(&grid), "no rows · page 1/1");
        assert!(!grid.open_record(&fake), "the table is empty");
        assert!(fake.sent().is_empty());
    }

    #[test]
    fn row_view_needs_a_row_and_closes_when_the_page_changes() {
        let fake = Fake::default();
        let mut grid = open(&fake);
        assert!(!grid.open_record(&fake), "nothing is loaded yet");
        assert!(fake.sent().is_empty());

        answer_page(&mut grid, &fake, page_of(0, 100, true));
        grid.set_row(2);
        assert!(grid.open_record(&fake));
        assert_eq!(fake.sent(), ["ddl main.users"]);

        grid.refresh(&fake);
        assert_eq!(
            answer_page(&mut grid, &fake, page_of(0, 100, true)),
            PageArrival::Refreshed
        );
        assert!(grid.record_open(), "a refresh keeps the row open");
        assert_eq!(grid.row(), 2);

        grid.refresh(&fake);
        answer_page(&mut grid, &fake, page_of(0, 2, false));
        assert_eq!(grid.row(), 1, "the row is clamped to the shorter page");
        assert!(grid.record_open());

        grid.refresh(&fake);
        answer_page(&mut grid, &fake, page_of(0, 100, true));
        grid.next_page(&fake);
        assert!(grid.record_open(), "open until the other page arrives");
        assert_eq!(
            answer_page(&mut grid, &fake, page_of(100, 100, false)),
            PageArrival::Moved
        );
        assert!(!grid.record_open(), "another page arrived");
        assert_eq!(grid.row(), 0);

        assert!(grid.open_record(&fake));
        grid.refresh(&fake);
        let id = grid.page_request.unwrap();
        assert_eq!(grid.on_error(id, "gone"), Update::Done);
        assert!(!grid.record_open());
        assert!(!grid.open_record(&fake), "the grid shows an error");
    }

    #[test]
    fn row_view_steps_within_the_page() {
        let fake = Fake::default();
        let mut grid = open(&fake);
        answer_page(&mut grid, &fake, page_of(0, 3, false));
        grid.set_row(1);
        assert!(grid.open_record(&fake));
        assert!(grid.step_record(1));
        assert_eq!(grid.row(), 2);
        assert!(!grid.step_record(1));
        assert_eq!(grid.row(), 2);
        grid.step_record(-1);
        grid.step_record(-1);
        assert!(!grid.step_record(-1));
        assert_eq!(grid.row(), 0);
        grid.set_row(99);
        assert_eq!(grid.row(), 2);
        grid.close_record();
        assert!(!grid.record_open());
    }

    #[test]
    fn the_definition_is_cached_until_refresh_and_failures_retry() {
        let fake = Fake::default();
        let mut grid = open(&fake);
        answer_page(&mut grid, &fake, page_of(0, 3, false));
        grid.open_record(&fake);
        let id = grid.ddl_request.unwrap();
        assert!(grid.on_ddl(id, users_ddl()));
        grid.close_record();
        fake.sent();

        grid.open_record(&fake);
        assert!(fake.sent().is_empty(), "it is cached");
        assert!(grid.ddl().is_some());

        grid.close_record();
        grid.refresh(&fake);
        assert!(grid.ddl().is_none());
        answer_page(&mut grid, &fake, page_of(0, 3, false));
        assert!(
            fake.sent().iter().all(|sent| !sent.starts_with("ddl")),
            "no row view"
        );
        grid.open_record(&fake);
        let id = grid.ddl_request.unwrap();
        grid.close_record();
        grid.open_record(&fake);
        assert_eq!(grid.ddl_request, Some(id), "one request is enough");
        assert_eq!(grid.on_error(id, "permission denied"), Update::Done);
        assert!(grid.record_open());
        assert_eq!(grid.ddl_error(), Some("permission denied"));

        grid.close_record();
        grid.open_record(&fake);
        assert!(
            grid.ddl_request.is_some_and(|new| new != id),
            "opening again retries"
        );
        assert!(grid.ddl_error().is_none());
        assert!(
            !grid.on_ddl(id, users_ddl()),
            "a late answer for the old id"
        );
    }
}
