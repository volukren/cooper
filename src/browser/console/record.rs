use rune::{El, Tx, col, el, row, text};
use rune_kit::{ControlSize, IconButton, IconButtonVariant};
use unicode_width::UnicodeWidthChar as _;

use super::grid::{NULL, full_value};
use crate::{
    db::ScriptResult,
    icons,
    theme::Theme,
    ui::{self, ScrollY},
    workspace::{TabId, Workspace},
};

pub const CUT_NOTE: &str = "… the value is longer, only its first 64 KB are loaded";
const TAB_WIDTH: usize = 4;
/// The labels' column stops growing at this many digits of its font.
const MAX_LABEL_CHARS: usize = 30;
const LABEL_PADDING: f32 = 24.;
pub const LINE_HEIGHT: f32 = 20.;
pub const VALUE_PREFIX: &str = "record-value-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub definition: String,
    pub value: Option<String>,
    pub cut: bool,
}

pub fn fields(result: &ScriptResult, row: usize) -> Vec<Field> {
    if row >= result.rows.len() {
        return Vec::new();
    }
    result
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| Field {
            name: column.name.clone(),
            definition: column.type_name.clone(),
            value: full_value(result, row, index).map(record_text),
            cut: result
                .full
                .get(&(row, index))
                .is_some_and(|full| full.truncated),
        })
        .collect()
}

pub fn record_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut column = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' | '\u{2028}' | '\u{2029}' => {
                out.push('\n');
                column = 0;
            }
            '\t' => {
                let stop = TAB_WIDTH - column % TAB_WIDTH;
                out.extend(std::iter::repeat_n(' ', stop));
                column += stop;
            }
            c if c.is_control() => {
                out.push('·');
                column += 1;
            }
            c => {
                out.push(c);
                column += c.width().unwrap_or(0);
            }
        }
    }
    out
}

/// The labels' column: the widest name or type in the fonts in use, up to a cap.
pub fn label_width(fields: &[Field], t: &Theme) -> f32 {
    let digit = ui::text_width("0", t.text_sm).max(1.);
    let widest = fields
        .iter()
        .map(|field| {
            ui::text_width(&field.name, t.semibold(t.text_sm))
                .max(ui::text_width(&field.definition, t.text_xs))
        })
        .fold(digit, f32::max);
    widest.min(MAX_LABEL_CHARS as f32 * digit).ceil() + LABEL_PADDING
}

pub fn view(tab: TabId, fields: &[Field], scroll: &ScrollY, t: &Theme) -> El {
    let label_width = label_width(fields, t);
    let blocks = fields.iter().enumerate().map(|(index, field)| {
        let label = col()
            .w(label_width)
            .px(8.)
            .py(4.)
            .gap(2.)
            .child(
                text(field.name.clone())
                    .font(t.semibold(t.text_sm))
                    .color(t.foreground)
                    .wrap_anywhere(),
            )
            .children((!field.definition.is_empty()).then(|| {
                text(field.definition.clone())
                    .font(t.text_xs)
                    .color(t.muted)
                    .wrap_anywhere()
            }));
        let value = match &field.value {
            Some(value) => text(value.clone())
                .font(t.mono)
                .color(t.foreground)
                .wrap_anywhere()
                .selectable(true)
                .key(format!("{VALUE_PREFIX}{index}")),
            None => text(NULL).font(t.text_sm.italic()).color(t.muted),
        };
        let copy_text = field.value.clone().unwrap_or_else(|| NULL.to_string());
        row()
            .key(format!("record-field-{index}"))
            .hover_group()
            .items_start()
            .w_full()
            .border_sides([if index > 0 { 1. } else { 0. }, 0., 0., 0.], t.border)
            .child(label)
            .child(
                col()
                    .grow(1.)
                    .min_w(0.)
                    .px(8.)
                    .py(4.)
                    .gap(2.)
                    .border_sides([0., 0., 0., 1.], t.border)
                    .child(value)
                    .children(field.cut.then(|| {
                        text(CUT_NOTE)
                            .font(t.text_xs.italic())
                            .color(t.muted)
                            .wrap()
                    })),
            )
            .child(
                el().padding(2.).group_opacity(0., 1.).child(
                    IconButton::new(&icons::COPY)
                        .variant(IconButtonVariant::Ghost)
                        .control_size(ControlSize::Small)
                        .label("Copy value")
                        .help("Copy value")
                        .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                            super::copy_value(s, tab, copy_text.clone(), tx)
                        })
                        .build(&t.k)
                        .key(format!("copy-field-{index}")),
                ),
            )
    });
    t.k.scroll_bar(ui::scroll_view(
        super::RECORD,
        scroll,
        col().w_full().children(blocks),
    ))
}

#[cfg(test)]
mod tests {
    use super::{CUT_NOTE, Field, LABEL_PADDING, fields, label_width, record_text};
    use crate::db::{ColumnInfo, FullValue, ScriptResult};
    use crate::ui;

    fn result() -> ScriptResult {
        let column = |name: &str, type_name: &str| ColumnInfo {
            name: name.to_string(),
            type_name: type_name.to_string(),
        };
        let mut result = ScriptResult {
            columns: vec![
                column("id", "INTEGER"),
                column("name", "TEXT"),
                column("note", ""),
            ],
            rows: (1..=3)
                .map(|n| vec![Some(n.to_string()), Some(format!("user {n}")), None])
                .collect(),
            ..Default::default()
        };
        result.full.insert(
            (2, 1),
            FullValue {
                text: "multi\nline".to_string(),
                truncated: true,
            },
        );
        result
    }

    #[test]
    fn fields_prefer_full_values() {
        let row = fields(&result(), 2);
        assert_eq!(
            row,
            [
                Field {
                    name: "id".into(),
                    definition: "INTEGER".into(),
                    value: Some("3".into()),
                    cut: false
                },
                Field {
                    name: "name".into(),
                    definition: "TEXT".into(),
                    value: Some("multi\nline".into()),
                    cut: true
                },
                Field {
                    name: "note".into(),
                    definition: String::new(),
                    value: None,
                    cut: false
                },
            ]
        );
        assert_eq!(fields(&result(), 0)[1].value.as_deref(), Some("user 1"));
        assert!(
            fields(&result(), 3).is_empty(),
            "a missing row has no fields"
        );
        assert!(CUT_NOTE.contains("64 KB"));
    }

    #[test]
    fn values_are_made_displayable() {
        assert_eq!(
            record_text("a\tb\r\nc\u{1}d\u{2028}e\rf"),
            "a   b\nc·d\ne\nf"
        );
        assert_eq!(record_text("ab\tc\n\tx"), "ab  c\n    x");
        assert_eq!(record_text("日\tx"), "日  x", "wide chars count two cells");
        assert_eq!(record_text(""), "");
    }

    #[test]
    fn the_label_column_fits_names_and_types_with_a_cap() {
        let t = crate::theme::theme(true);
        let row = fields(&result(), 0);
        let widest = row
            .iter()
            .map(|field| {
                ui::text_width(&field.name, t.semibold(t.text_sm))
                    .max(ui::text_width(&field.definition, t.text_xs))
            })
            .fold(0., f32::max);
        assert!(widest > 0.);
        assert_eq!(label_width(&row, t), widest.ceil() + LABEL_PADDING);
        let long = |n: usize| Field {
            name: "x".repeat(n),
            definition: String::new(),
            value: None,
            cut: false,
        };
        assert_eq!(
            label_width(&[long(50)], t),
            label_width(&[long(500)], t),
            "long names are capped"
        );
        assert!(label_width(&[], t) > LABEL_PADDING);
    }
}
