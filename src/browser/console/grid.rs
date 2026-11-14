use std::ops::Range;

use rune::{
    Cursor, Drag, El, GrowthSpec, Key, KeyEvent, ScrollSpec, Tx, col, el, row, text,
    text::{advance, measure},
};
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

use crate::{
    db::{ColumnInfo, ScriptResult},
    theme::{self, Theme},
    ui,
    workspace::{TabId, Workspace},
};

pub const NULL: &str = "NULL";
pub const MAX_COLUMN_CHARS: usize = 40;
pub const ROW_HEIGHT: f32 = 24.;
pub const HEADER_HEIGHT: f32 = 28.;
pub const TYPED_HEADER_HEIGHT: f32 = 40.;
pub const CELL_PADDING: f32 = 6.;
const ROUNDING_SLACK: f32 = 2.;
const MIN_WIDTH: f32 = 40.;
const DEFAULT_WIDTH: f32 = 100.;
const INITIAL_COLUMNS: usize = 40;
pub const SCROLL: &str = "console-grid-scroll";

pub fn data_column(col_ix: usize) -> Option<usize> {
    col_ix.checked_sub(1)
}

pub fn cell(result: &ScriptResult, row: usize, column: usize) -> Option<&str> {
    result.rows.get(row)?.get(column)?.as_deref()
}

pub fn full_value(result: &ScriptResult, row: usize, column: usize) -> Option<&str> {
    match result.full.get(&(row, column)) {
        Some(full) => Some(full.text.as_str()),
        None => cell(result, row, column),
    }
}

pub fn copy_cell(result: &ScriptResult, row: usize, column: usize) -> String {
    full_value(result, row, column).unwrap_or(NULL).to_string()
}

pub fn copy_row(result: &ScriptResult, row: usize) -> String {
    (0..result.columns.len())
        .map(|column| copy_cell(result, row, column))
        .collect::<Vec<_>>()
        .join("\t")
}

#[derive(Debug, PartialEq, Eq)]
pub struct Widest {
    pub cell: String,
    pub name: String,
    pub type_name: String,
}

pub fn widest_texts(columns: &[ColumnInfo], rows: &[Vec<Option<String>>]) -> Vec<Widest> {
    columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let cell = rows
                .iter()
                .map(|row| {
                    row.get(index)
                        .map_or("", |cell| cell.as_deref().unwrap_or(NULL))
                })
                .max_by_key(|text| text.width())
                .unwrap_or("");
            Widest {
                cell: clip(cell),
                name: clip(&column.name),
                type_name: clip(&column.type_name),
            }
        })
        .collect()
}

pub fn clip(text: &str) -> String {
    let mut width = 0;
    text.chars()
        .take_while(|c| {
            width += c.width().unwrap_or(0);
            width <= MAX_COLUMN_CHARS
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Cell(usize, usize),
    Row(usize),
}

#[derive(Debug, Default)]
pub struct Grid {
    pub widths: Vec<f32>,
    /// The widths that fit each column's content, which `refit` tells from dragged ones.
    natural: Vec<f32>,
    /// How wide a digit of the mono font was when the widths were fitted.
    digit: f32,
    pub gutter: f32,
    pub typed: bool,
    pub selection: Option<Selection>,
    pub resize_from: Option<f32>,
    pub cols: Range<usize>,
    pub viewport: [f32; 4],
    pub scroll_x: f32,
}

struct Metrics {
    mono: rune::TextStyle,
    header: rune::TextStyle,
    kind: rune::TextStyle,
}

fn metrics() -> Metrics {
    let t = theme::theme(false);
    Metrics {
        mono: t.mono,
        header: t.semibold(t.text_sm),
        kind: t.text_xs,
    }
}

fn text_width(text: &str, style: rune::TextStyle) -> f32 {
    if text.is_empty() {
        return 0.;
    }
    let cell = advance("0", style);
    measure(text, &[], style, None)
        .0
        .max(cell * text.width() as f32)
}

fn padded(content: f32) -> f32 {
    (content + 2. * CELL_PADDING + ROUNDING_SLACK).ceil()
}

pub fn needed_width(cell: &str, name: &str, type_name: &str) -> f32 {
    let m = metrics();
    text_width(cell, m.mono)
        .max(text_width(name, m.header))
        .max(text_width(type_name, m.kind))
}

pub fn fit_width(result: &ScriptResult, index: usize) -> Option<f32> {
    let column = result.columns.get(index)?;
    let widest = result
        .rows
        .iter()
        .map(|row| {
            row.get(index)
                .map_or("", |cell| cell.as_deref().unwrap_or(NULL))
        })
        .max_by_key(|text| text.width())
        .unwrap_or("");
    Some(padded(needed_width(
        widest,
        &column.name,
        &column.type_name,
    )))
}

impl Grid {
    pub fn new(result: &ScriptResult) -> Self {
        let m = metrics();
        let widths: Vec<f32> = widest_texts(&result.columns, &result.rows)
            .into_iter()
            .map(|widest| padded(needed_width(&widest.cell, &widest.name, &widest.type_name)))
            .collect();
        let gutter = padded(text_width(
            &ui::group(result.rows.len().max(1) as u64),
            m.mono,
        ));
        let typed = result
            .columns
            .iter()
            .any(|column| !column.type_name.is_empty());
        let selection = (!result.rows.is_empty() && !result.columns.is_empty())
            .then_some(Selection::Cell(0, 1));
        let cols = 0..widths.len().min(INITIAL_COLUMNS);
        Self {
            natural: widths.clone(),
            digit: advance("0", m.mono),
            widths,
            gutter,
            typed,
            selection,
            resize_from: None,
            cols,
            viewport: [0.; 4],
            scroll_x: 0.,
        }
    }

    /// Sizes the columns again in the fonts in use: a column at its fitted width follows its
    /// content, one the user dragged keeps its size in digits.
    pub fn refit(&mut self, result: &ScriptResult) {
        let fresh = Grid::new(result);
        if fresh.widths.len() != self.widths.len() {
            return;
        }
        let scale = if self.digit > 0. {
            fresh.digit / self.digit
        } else {
            1.
        };
        for ((width, natural), fitted) in self
            .widths
            .iter_mut()
            .zip(&self.natural)
            .zip(&fresh.natural)
        {
            *width = if *width == *natural {
                *fitted
            } else {
                (*width * scale).max(MIN_WIDTH)
            };
        }
        self.natural = fresh.natural;
        self.digit = fresh.digit;
        self.gutter = fresh.gutter;
    }

    /// Puts column `index` back at the width that fits its content.
    pub fn fit(&mut self, index: usize, width: f32) {
        if let Some(slot) = self.widths.get_mut(index) {
            *slot = width;
        }
        if let Some(natural) = self.natural.get_mut(index) {
            *natural = width;
        }
    }

    pub fn selected_cell(&self) -> Option<(usize, usize)> {
        match self.selection {
            Some(Selection::Cell(row, col)) => Some((row, col)),
            _ => None,
        }
    }

    pub fn selected_row(&self) -> Option<usize> {
        match self.selection {
            Some(Selection::Row(row)) => Some(row),
            _ => None,
        }
    }

    pub fn current_row(&self) -> Option<usize> {
        match self.selection? {
            Selection::Cell(row, _) | Selection::Row(row) => Some(row),
        }
    }

    pub fn header_height(&self) -> f32 {
        if self.typed {
            TYPED_HEADER_HEIGHT
        } else {
            HEADER_HEIGHT
        }
    }

    pub fn total_width(&self) -> f32 {
        self.gutter + self.widths.iter().sum::<f32>()
    }

    pub fn fill_width(&self) -> f32 {
        self.total_width().max(self.viewport[2])
    }

    pub fn left_of(&self, index: usize) -> f32 {
        self.gutter
            + self.widths[..index.min(self.widths.len())]
                .iter()
                .sum::<f32>()
    }

    pub fn width_of(&self, index: usize) -> f32 {
        self.widths.get(index).copied().unwrap_or(DEFAULT_WIDTH)
    }

    fn window(&self) -> Range<usize> {
        let width = self.viewport[2];
        if width <= 0. {
            return 0..self.widths.len().min(INITIAL_COLUMNS);
        }
        let (from, to) = (self.scroll_x - width, self.scroll_x + 2. * width);
        let mut x = self.gutter;
        let mut window: Option<Range<usize>> = None;
        for (index, w) in self.widths.iter().enumerate() {
            let right = x + w;
            if right >= from && x <= to {
                window = Some(match window {
                    Some(range) => range.start..index + 1,
                    None => index..index + 1,
                });
            }
            x = right;
        }
        window.unwrap_or(0..0)
    }

    fn rewindow(&mut self) -> bool {
        let (count, width) = (self.widths.len(), self.viewport[2]);
        if width > 0. && self.cols.start < self.cols.end && self.cols.end <= count {
            let built = (self.left_of(self.cols.start), self.left_of(self.cols.end));
            let shown = (self.scroll_x + self.gutter, self.scroll_x + width);
            let left = self.cols.start == 0 || shown.0 - built.0 >= width / 2.;
            let right = self.cols.end == count || built.1 - shown.1 >= width / 2.;
            if left && right {
                return false;
            }
        }
        let cols = self.window();
        if cols == self.cols {
            false
        } else {
            self.cols = cols;
            true
        }
    }

    pub fn step(&mut self, ev: &KeyEvent, rows: usize) -> bool {
        let mods = ev.mods;
        if mods.cmd || mods.ctrl || mods.alt || mods.logo || mods.shift {
            return false;
        }
        let columns = self.widths.len();
        if rows == 0 || columns == 0 {
            return false;
        }
        let (row, col) = match self.selection {
            Some(Selection::Cell(row, col)) => (row, col),
            Some(Selection::Row(row)) => (row, 1),
            None => {
                self.selection = Some(Selection::Cell(0, 0));
                return true;
            }
        };
        let page = ((self.viewport[3] - self.header_height()) / ROW_HEIGHT)
            .floor()
            .max(1.) as usize;
        let last = rows - 1;
        let (row, col) = match ev.key {
            Key::Up => (row.saturating_sub(1), col),
            Key::Down => ((row + 1).min(last), col),
            Key::Left => (row, col.saturating_sub(1)),
            Key::Right => (row, (col + 1).min(columns)),
            Key::Home => (row, 0),
            Key::End => (row, columns),
            Key::PageUp => (row.saturating_sub(page), col),
            Key::PageDown => ((row + page).min(last), col),
            _ => return false,
        };
        self.selection = Some(Selection::Cell(row, col));
        true
    }

    pub fn reveal(&self, tx: &mut Tx) {
        let Some(row) = self.current_row() else {
            return;
        };
        tx.scroll_into_view_in(SCROLL, row_key(row));
        let Some((_, col)) = self.selected_cell() else {
            return;
        };
        let Some(index) = data_column(col) else {
            tx.scroll_to_x(SCROLL, 0.);
            return;
        };
        let (left, right) = (
            self.left_of(index),
            self.left_of(index) + self.width_of(index),
        );
        let view = self.viewport[2] - self.gutter;
        if view <= 0. {
            return;
        }
        if left - self.gutter < self.scroll_x {
            tx.scroll_to_x(SCROLL, left - self.gutter);
        } else if right - self.gutter > self.scroll_x + view {
            tx.scroll_to_x(SCROLL, right - self.gutter - view);
        }
    }
}

pub fn row_key(row: usize) -> String {
    format!("cr-{row}")
}

fn grid_mut(s: &mut Workspace, tab: TabId) -> Option<&mut Grid> {
    s.browser_mut(tab).map(|b| &mut b.console.grid)
}

fn opaque(color: rune::Color, under: rune::Color) -> rune::Color {
    under.mix(color.alpha(1.), color.a)
}

pub fn view(
    tab: TabId,
    grid: &Grid,
    result: &ScriptResult,
    scrolls: &crate::browser::Scrolls,
    t: &Theme,
) -> El {
    let header_fill = opaque(t.secondary, t.background);
    let total = grid.fill_width();
    let cols = grid.cols.start.min(grid.widths.len())..grid.cols.end.min(grid.widths.len());
    let spacer = grid.left_of(cols.start) - grid.gutter;
    let header = row()
        .key("console-grid-header")
        .w(total)
        .h(grid.header_height())
        .fill(header_fill)
        .border_sides([0., 0., 1., 0.], t.border)
        .sticky_top(0.)
        .child(
            row()
                .w(grid.gutter)
                .h_full()
                .px(CELL_PADDING)
                .center_y()
                .justify_end()
                .fill(header_fill)
                .border_sides([0., 1., 0., 0.], t.border)
                .sticky_left(0.)
                .child(text("#").font(t.semibold(t.text_sm)).color(t.muted)),
        )
        .child(el().w(spacer))
        .children(cols.clone().filter_map(|index| {
            let column = result.columns.get(index)?;
            Some(header_cell(
                tab,
                index,
                column,
                grid.width_of(index),
                grid.typed,
                t,
            ))
        }));
    let count = result.rows.len();
    let rows = col()
        .key("console-grid-rows")
        .w(total)
        .growth(GrowthSpec::default())
        .virtual_rows(
            count,
            ROW_HEIGHT,
            |_: &Workspace, i| std::borrow::Cow::Owned(row_key(i)),
            move |s: &Workspace, i| grid_row(s, tab, i),
        )
        .on_layout_tx(move |s: &mut Workspace, r: [f32; 4], tx: &mut Tx| {
            let Some(grid) = grid_mut(s, tab) else {
                tx.live_only();
                return;
            };
            grid.scroll_x = (grid.viewport[0] - r[0]).max(0.);
            if !grid.rewindow() {
                tx.live_only();
            }
        });
    let body = col()
        .child(header)
        .child(rows)
        .children((count == 0).then(|| {
            row()
                .key("console-grid-empty")
                .w_full()
                .h(64.)
                .center()
                .child(ui::label("No rows", t.text_sm, t.muted))
        }));
    let viewport = col()
        .key(SCROLL)
        .grow(1.)
        .min_h(0.)
        .min_w(0.)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_xy()
        .on_layout_tx(move |s: &mut Workspace, r: [f32; 4], tx: &mut Tx| {
            let Some(grid) = grid_mut(s, tab) else {
                tx.live_only();
                return;
            };
            let before = grid.fill_width();
            grid.viewport = r;
            if !grid.rewindow() && grid.fill_width() == before {
                tx.live_only();
            }
        })
        .child(body);
    col()
        .key("console-grid")
        .role("grid")
        .label("Result grid")
        .focusable(true)
        .no_focus_ring()
        .grow(1.)
        .min_h(0.)
        .min_w(0.)
        .on_key(move |s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| {
            crate::keys::route(s, ev, tx) || super::grid_key(s, tab, ev, tx)
        })
        .child(t.k.scroll_bar(scrolls.keep(SCROLL, viewport)))
}

fn header_cell(
    tab: TabId,
    index: usize,
    column: &ColumnInfo,
    width: f32,
    typed: bool,
    t: &Theme,
) -> El {
    let names = col()
        .grow(1.)
        .min_w(0.)
        .h_full()
        .pl(CELL_PADDING)
        .justify_center()
        .child(
            text(column.name.clone())
                .font(t.semibold(t.text_sm))
                .color(t.foreground)
                .truncate(),
        )
        .children(typed.then(|| {
            text(column.type_name.clone())
                .font(t.text_xs)
                .color(t.muted)
                .truncate()
        }));
    let handle = el()
        .key(format!("console-grid-resize-{index}"))
        .w(5.)
        .h_full()
        .cursor(Cursor::ResizeColumn)
        .role("separator")
        .hover(t.k.alpha(t.ring, 0.4))
        .on_drag(move |s: &mut Workspace, d: &mut Drag, _tx: &mut Tx| {
            let Some(grid) = grid_mut(s, tab) else {
                return;
            };
            match d.phase {
                rune::Phase::Down => grid.resize_from = Some(grid.width_of(index)),
                rune::Phase::Move => {
                    if let (Some(from), Some(width)) =
                        (grid.resize_from, grid.widths.get_mut(index))
                    {
                        *width = (from + d.dx).max(MIN_WIDTH);
                    }
                }
                rune::Phase::Up => {
                    if d.cancelled()
                        && let (Some(from), Some(width)) =
                            (grid.resize_from, grid.widths.get_mut(index))
                    {
                        *width = from;
                    }
                    grid.resize_from = None;
                }
            }
        })
        .on_double_press(move |s: &mut Workspace| {
            let Some(b) = s.browser_mut(tab) else {
                return;
            };
            let Some(width) = b
                .console
                .result()
                .and_then(|result| fit_width(result, index))
            else {
                return;
            };
            b.console.grid.fit(index, width.max(MIN_WIDTH));
        });
    row()
        .key(format!("console-grid-col-{index}"))
        .label(column.name.clone())
        .w(width)
        .h_full()
        .border_sides([0., 1., 0., 0.], t.border)
        .child(names)
        .child(handle)
}

fn grid_row(s: &Workspace, tab: TabId, i: usize) -> El {
    let t = s.theme();
    let Some(b) = s.browser(tab) else {
        return el().h(ROW_HEIGHT);
    };
    let grid = &b.console.grid;
    let Some(result) = b.console.result() else {
        return el().h(ROW_HEIGHT);
    };
    let cols = grid.cols.start.min(grid.widths.len())..grid.cols.end.min(grid.widths.len());
    let current = grid.current_row() == Some(i);
    let fill = if grid.selected_row() == Some(i) {
        t.table_active
    } else if current {
        t.table_hover
    } else if i % 2 == 1 {
        t.table_stripe
    } else {
        rune::Color::TRANSPARENT
    };
    let gutter_selected = grid.selected_cell() == Some((i, 0));
    let gutter = row()
        .w(grid.gutter)
        .h_full()
        .px(CELL_PADDING)
        .center_y()
        .justify_end()
        .fill(if gutter_selected {
            opaque(t.table_active, t.background)
        } else {
            opaque(fill, t.background)
        });
    let gutter = if gutter_selected {
        gutter.border(1., t.ring)
    } else {
        gutter.border_sides([0., 1., 0., 0.], t.border)
    }
    .sticky_left(0.)
    .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| super::click_row(s, tab, i, tx))
    .on_double_press_tx(move |s: &mut Workspace, tx: &mut Tx| super::open_record_at(s, tab, i, tx))
    .child(
        text(ui::group(i as u64 + 1))
            .font(t.mono)
            .color(if current { t.foreground } else { t.muted }),
    );
    row()
        .key(row_key(i))
        .role("row")
        .w(grid.fill_width())
        .h(ROW_HEIGHT)
        .fill(fill)
        .child(gutter)
        .child(el().w(grid.left_of(cols.start) - grid.gutter))
        .children(cols.map(|index| {
            let col_ix = index + 1;
            let selected = grid.selected_cell() == Some((i, col_ix));
            let value = cell(result, i, index);
            let content = match value {
                Some(value) => text(value.to_string()).font(t.mono).color(t.foreground),
                None => text(NULL).font(t.mono.italic()).color(t.muted),
            };
            let mut cell = row()
                .role("gridcell")
                .label(value.unwrap_or(NULL).to_string())
                .w(grid.width_of(index))
                .h_full()
                .px(CELL_PADDING)
                .center_y()
                .child(content.truncate().grow(1.).min_w(0.))
                .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                    super::click_cell(s, tab, i, col_ix, tx)
                })
                .on_double_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                    super::open_record_at(s, tab, i, tx)
                });
            if selected {
                cell = cell.fill(t.table_active).border(1., t.ring);
            }
            cell
        }))
}

#[cfg(test)]
mod tests {
    use super::{MAX_COLUMN_CHARS, NULL, Widest, clip, copy_cell, copy_row, widest_texts};
    use crate::db::{ColumnInfo, FullValue, ScriptResult};

    fn column(name: &str, type_name: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            type_name: type_name.to_string(),
        }
    }

    fn widest(cell: &str, name: &str, type_name: &str) -> Widest {
        Widest {
            cell: cell.to_string(),
            name: name.to_string(),
            type_name: type_name.to_string(),
        }
    }

    #[test]
    fn the_widest_texts_follow_content_with_a_cap() {
        let columns = vec![
            column("id", "INTEGER"),
            column("name", "TEXT"),
            column("note", ""),
        ];
        let mut rows = vec![
            vec![Some("1".to_string()), Some("a".to_string()), None],
            vec![Some("22".to_string()), Some("b".to_string()), None],
        ];
        rows[1][1] = Some("x".repeat(200));
        rows[0][2] = Some("日本".to_string());
        assert_eq!(
            widest_texts(&columns, &rows),
            [
                widest("22", "id", "INTEGER"),
                widest(&"x".repeat(MAX_COLUMN_CHARS), "name", "TEXT"),
                widest(NULL, "note", ""),
            ]
        );
        rows.clear();
        assert_eq!(
            widest_texts(&columns, &rows)[0],
            widest("", "id", "INTEGER")
        );
        assert_eq!(widest_texts(&[column("", "")], &[]), [widest("", "", "")]);
        assert_eq!(widest_texts(&columns, &[vec![]])[2], widest("", "note", ""));
    }

    #[test]
    fn clipping_counts_display_cells_and_char_boundaries() {
        assert_eq!(clip("abc"), "abc");
        assert_eq!(clip(&"日".repeat(30)), "日".repeat(MAX_COLUMN_CHARS / 2));
        assert_eq!(clip(&"é".repeat(50)).chars().count(), MAX_COLUMN_CHARS);
    }

    #[test]
    fn copies_full_values_and_nulls() {
        let mut result = ScriptResult {
            columns: vec![column("id", ""), column("body", ""), column("note", "")],
            rows: vec![vec![
                Some("1".to_string()),
                Some("multi↵line".to_string()),
                None,
            ]],
            ..Default::default()
        };
        result.full.insert(
            (0, 1),
            FullValue {
                text: "multi\nline".to_string(),
                truncated: false,
            },
        );
        assert_eq!(copy_cell(&result, 0, 0), "1");
        assert_eq!(copy_cell(&result, 0, 1), "multi\nline");
        assert_eq!(copy_cell(&result, 0, 2), NULL);
        assert_eq!(
            copy_cell(&result, 5, 0),
            NULL,
            "out of range is not a panic"
        );
        assert_eq!(copy_row(&result, 0), "1\tmulti\nline\tNULL");
    }
}
