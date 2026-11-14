use std::{cell::Cell, rc::Rc, time::Duration};

use rune::{
    Color, Cursor, Drag, El, Icon, Phase, ScrollPos, ScrollSpec, Span, TextStyle, Tx, col, el,
    icon, rich, row, text,
};

use crate::{
    connection::{DbKind, Tag},
    db::ScriptResult,
    highlight::Highlighter,
    theme::{RADIUS, Theme},
};

pub fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn plural(count: u64, noun: &str) -> String {
    let ending = if count == 1 { "" } else { "s" };
    format!("{} {noun}{ending}", group(count))
}

pub fn elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 60 {
        format!("{} min {} s", group(seconds / 60), seconds % 60)
    } else if seconds >= 1 {
        format!("{:.1} s", duration.as_secs_f64())
    } else {
        format!("{} ms", duration.as_millis())
    }
}

pub fn summary(result: &ScriptResult) -> String {
    let mut parts = Vec::new();
    if result.result_sets > 0 {
        let rows = plural(result.rows.len() as u64, "row");
        parts.push(if result.truncated {
            format!("first {rows}")
        } else {
            rows
        });
    }
    if result.statements > 1 {
        parts.push(plural(result.statements as u64, "statement"));
    }
    if result.affected > 0 || result.result_sets == 0 {
        parts.push(format!("{} affected", plural(result.affected, "row")));
    }
    parts.push(elapsed(result.elapsed));
    parts.join(" · ")
}

pub fn sql_spans(sql: &str, kind: DbKind, t: &Theme) -> Vec<Span> {
    let mut lexer = Highlighter::new(kind);
    let mut out = Vec::new();
    let mut line_start = 0;
    for line in sql.split('\n') {
        let mut start = 0;
        for (end, token) in lexer.line(line) {
            out.extend(t.token_span(token, line_start + start, line_start + end));
            start = end;
        }
        line_start += line.len() + 1;
    }
    out
}

pub fn sql_text(sql: &str, kind: DbKind, t: &Theme) -> El {
    rich(sql.to_string(), sql_spans(sql, kind, t))
        .font(t.mono)
        .color(t.foreground)
        .selectable(true)
}

pub fn icon_el(glyph: &'static Icon, size: f32, color: Color) -> El {
    icon(glyph, size).color(color)
}

/// How wide `content` is on one line in `style`, in the fonts in use: for columns sized to their
/// labels, which a guess per character gets wrong once the font changes.
pub fn text_width(content: &str, style: TextStyle) -> f32 {
    if content.is_empty() {
        return 0.0;
    }
    rune::text::measure(content, &[], style, None).0
}

pub fn label(content: impl Into<String>, style: TextStyle, color: Color) -> El {
    text(content).font(style).color(color)
}

pub fn tag_badge(tag: Tag, t: &Theme) -> El {
    let color = t.tag_color(tag);
    row()
        .role("status")
        .label(tag.label())
        .h(16.0)
        .px(5.0)
        .center()
        .radius(3.0)
        .fill(t.k.alpha(color, 0.14))
        .border(1.0, t.k.alpha(color, 0.55))
        .child(
            text(tag.short())
                .font(t.semibold(t.text_xs))
                .color(color)
                .a11y_hidden(),
        )
}

pub fn pane(key: impl Into<String>, title: El, extra: Option<El>, body: El, t: &Theme) -> El {
    col()
        .key(key)
        .grow(1.0)
        .min_h(0.0)
        .min_w(0.0)
        .radius(RADIUS)
        .border(1.0, t.border)
        .border_layout()
        .fill(t.background)
        .clip_rounded(RADIUS)
        .focus_within_ring(1.0, -1.0, t.ring)
        .child(
            row()
                .h(28.0)
                .w_full()
                .px(8.0)
                .gap(8.0)
                .center_y()
                .justify_between()
                .fill(t.secondary)
                .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
                .child(row().gap(4.0).shrink().center_y().child(title))
                .children(extra),
        )
        .child(col().grow(1.0).min_h(0.0).min_w(0.0).child(body))
}

pub fn placeholder(lines: &[(&str, bool)], t: &Theme) -> El {
    col()
        .grow(1.0)
        .w_full()
        .h_full()
        .center()
        .gap(4.0)
        .children(lines.iter().map(|(line, strong)| {
            if *strong {
                text(line.to_string())
                    .font(t.bold(t.text))
                    .color(t.foreground)
            } else {
                text(line.to_string()).font(t.text_sm).color(t.muted)
            }
        }))
}

pub fn status_line(key: impl Into<String>, message: &str, error: bool, t: &Theme) -> El {
    let color = if error { t.danger } else { t.warning };
    row()
        .key(key)
        .h(26.0)
        .w_full()
        .px(12.0)
        .gap(8.0)
        .center_y()
        .fill(t.status_bar)
        .border_sides([1.0, 0.0, 0.0, 0.0], t.status_bar_border)
        .children(error.then(|| icon_el(&crate::icons::CIRCLE_X, 12.0, t.danger)))
        .child(
            text(message.to_string())
                .font(t.text_xs)
                .color(color)
                .truncate()
                .min_w(0.0),
        )
}

pub fn spinner(size: f32, color: Color, t: &Theme) -> El {
    rune_kit::Spinner::new().size(size).color(color).build(&t.k)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Split {
    pub size: f32,
    pub default: f32,
    pub min: f32,
    pub max: f32,
    pub drag_from: Option<f32>,
}

impl Split {
    pub const fn new(default: f32, min: f32, max: f32) -> Self {
        Self {
            size: default,
            default,
            min,
            max,
            drag_from: None,
        }
    }

    pub fn clamped(&self, room: Option<f32>) -> f32 {
        let max = room.map_or(self.max, |room| self.max.min(room));
        self.size.clamp(self.min, max.max(self.min))
    }

    pub fn drag(&mut self, d: &Drag, delta: f32) {
        match d.phase {
            Phase::Down => self.drag_from = Some(self.size),
            Phase::Move => {
                if let Some(from) = self.drag_from {
                    self.size = (from + delta).clamp(self.min, self.max);
                }
            }
            Phase::Up => {
                if d.cancelled()
                    && let Some(from) = self.drag_from
                {
                    self.size = from;
                }
                self.drag_from = None;
            }
        }
    }
}

pub fn h_handle<S: 'static>(
    key: impl Into<String>,
    t: &Theme,
    split: impl for<'a> Fn(&'a mut S) -> Option<&'a mut Split> + Clone + 'static,
) -> El {
    let reset = split.clone();
    el().key(key)
        .w(5.0)
        .h_full()
        .cursor(Cursor::ResizeColumn)
        .role("separator")
        .hover(t.k.alpha(t.ring, 0.4))
        .on_drag(move |s: &mut S, d: &mut Drag, _tx: &mut Tx| {
            if let Some(split) = split(s) {
                split.drag(d, d.dx);
            }
        })
        .on_double_press(move |s: &mut S| {
            if let Some(split) = reset(s) {
                split.size = split.default;
            }
        })
}

pub fn v_handle<S: 'static>(
    key: impl Into<String>,
    t: &Theme,
    split: impl for<'a> Fn(&'a mut S) -> Option<&'a mut Split> + Clone + 'static,
) -> El {
    let reset = split.clone();
    el().key(key)
        .h(5.0)
        .w_full()
        .cursor(Cursor::ResizeRow)
        .role("separator")
        .hover(t.k.alpha(t.ring, 0.4))
        .on_drag(move |s: &mut S, d: &mut Drag, _tx: &mut Tx| {
            if let Some(split) = split(s) {
                split.drag(d, d.dy);
            }
        })
        .on_double_press(move |s: &mut S| {
            if let Some(split) = reset(s) {
                split.size = split.default;
            }
        })
}

pub struct Modal {
    key: String,
    label: String,
    title: Option<El>,
    width: f32,
    body: Vec<El>,
}

impl Modal {
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            title: None,
            width: 480.0,
            body: Vec::new(),
        }
    }

    pub fn title(mut self, title: El) -> Self {
        self.title = Some(title);
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn child(mut self, body: El) -> Self {
        self.body.push(body);
        self
    }

    pub fn build<S: 'static>(
        self,
        t: &Theme,
        layer: &mut Vec<El>,
        dismiss: impl Fn(&mut S, &mut Tx) + 'static,
    ) {
        let title = self.title.unwrap_or_else(|| {
            text(self.label.clone())
                .font(t.semibold(t.text))
                .color(t.foreground)
        });
        let header = row()
            .key("modal-header")
            .w_full()
            .px(16.0)
            .pt(16.0)
            .pb(8.0)
            .gap(8.0)
            .center_y()
            .child(title);
        let body = col()
            .key("modal-body")
            .w_full()
            .px(16.0)
            .pb(16.0)
            .gap(12.0)
            .min_h(0.0)
            .children(self.body);
        let panel = col()
            .key(format!("{}-dialog", self.key))
            .role("dialog")
            .label(self.label)
            .w_full()
            .max_w(self.width)
            .radius(RADIUS)
            .fill(t.background)
            .border(1.0, t.modal_border)
            .child(header)
            .child(body)
            .modal_tx(true, dismiss);
        let panel = t.k.elevate(panel, &t.k.elevation.overlay);
        layer.push(
            el().key(format!("{}-scrim", self.key))
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .fill(t.k.c.scrim)
                .a11y_hidden(),
        );
        layer.push(
            el().key(format!("{}-layer", self.key))
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .padding(16.0)
                .center()
                .child(panel),
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    LineUp,
    LineDown,
    PageUp,
    PageDown,
    Top,
    Bottom,
}

impl Scroll {
    pub fn from_key(ev: &rune::KeyEvent) -> Option<Scroll> {
        use rune::Key;
        let mods = ev.mods;
        if mods.cmd || mods.ctrl || mods.alt || mods.logo || mods.shift {
            return None;
        }
        Some(match &ev.key {
            Key::Up => Scroll::LineUp,
            Key::Down => Scroll::LineDown,
            Key::PageUp => Scroll::PageUp,
            Key::PageDown => Scroll::PageDown,
            Key::Text(t) if t == " " => Scroll::PageDown,
            Key::Home => Scroll::Top,
            Key::End => Scroll::Bottom,
            _ => return None,
        })
    }
}

#[derive(Clone, Default)]
pub struct ScrollY(Rc<Cell<ScrollPos>>);

impl std::fmt::Debug for ScrollY {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ScrollY({:?})", self.0.get().offset)
    }
}

impl ScrollY {
    pub fn offset(&self) -> f32 {
        self.0.get().offset[1]
    }

    pub fn max(&self) -> f32 {
        self.0.get().max[1]
    }

    pub fn viewport(&self) -> f32 {
        self.0.get().viewport[1]
    }

    pub fn reset(&self) {
        self.0.set(ScrollPos::default());
    }

    pub fn watch(&self, viewport: El) -> El {
        let cell = self.0.clone();
        viewport.on_scroll_offset(move |pos: ScrollPos| cell.set(pos))
    }

    pub fn scroll(&self, tx: &mut Tx, key: &str, how: Scroll, line: f32) {
        let y = self.offset();
        match how {
            Scroll::LineUp => tx.scroll_to_y(key, (y - line).max(0.0)),
            Scroll::LineDown => tx.scroll_to_y(key, y + line),
            Scroll::PageUp => tx.scroll_page(key, -1),
            Scroll::PageDown => tx.scroll_page(key, 1),
            Scroll::Top => tx.scroll_to_y(key, 0.0),
            Scroll::Bottom => tx.scroll_to_y(key, f32::MAX),
        }
    }
}

pub fn scroll_view(key: impl Into<String>, scroll: &ScrollY, content: El) -> El {
    scroll.watch(
        col()
            .key(key)
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(content),
    )
}

#[cfg(test)]
mod tests {
    use super::{elapsed, group, summary};
    use crate::db::ScriptResult;
    use std::time::Duration;

    fn result(
        rows: usize,
        truncated: bool,
        statements: usize,
        sets: usize,
        affected: u64,
        ms: u64,
    ) -> ScriptResult {
        ScriptResult {
            rows: vec![vec![]; rows],
            truncated,
            statements,
            result_sets: sets,
            affected,
            elapsed: Duration::from_millis(ms),
            ..Default::default()
        }
    }

    #[test]
    fn summary_formats() {
        assert_eq!(summary(&result(1, false, 1, 1, 0, 12)), "1 row · 12 ms");
        assert_eq!(
            summary(&result(1000, true, 3, 1, 1, 2340)),
            "first 1,000 rows · 3 statements · 1 row affected · 2.3 s"
        );
        assert_eq!(
            summary(&result(0, false, 1, 0, 0, 3_725_000)),
            "0 rows affected · 62 min 5 s"
        );
        assert_eq!(
            summary(&result(0, false, 2, 0, 2, 1500)),
            "2 statements · 2 rows affected · 1.5 s"
        );
        assert_eq!(elapsed(Duration::ZERO), "0 ms");
        assert_eq!(elapsed(Duration::from_millis(999)), "999 ms");
        assert_eq!(elapsed(Duration::from_secs(59)), "59.0 s");
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1000), "1,000");
        assert_eq!(group(1234567), "1,234,567");
    }
}
