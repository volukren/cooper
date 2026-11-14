use rune::{Cx, El, ScrollSpec, Tx, col, row, text};
use rune_kit::{Button, ButtonVariant, Checkbox, Dialog as KitDialog};

use super::{
    begin_edit, chosen_key, drop_draft,
    edit::usable_key,
    format::{column_list, key_chip},
    status,
};
use crate::{
    db::{self, KeySource, TableKind},
    dialog::Dialog,
    icons,
    theme::{RADIUS, Theme},
    ui::{self, ScrollY},
    workspace::{TabId, Workspace},
};

const CHROME: f32 = 320.0;
const MIN_LIST: f32 = 120.0;

pub struct KeyPicker {
    pub tab: TabId,
    connection: String,
    schema: String,
    table: String,
    columns: Vec<KeyColumn>,
    saved: bool,
    stale: bool,
    resume: Option<usize>,
    scroll: ScrollY,
}

struct KeyColumn {
    name: String,
    type_name: String,
    comparable: bool,
    checked: bool,
}

impl KeyPicker {
    fn chosen(&self) -> Vec<String> {
        self.columns
            .iter()
            .filter(|column| column.checked && column.comparable)
            .map(|column| column.name.clone())
            .collect()
    }

    #[cfg(test)]
    pub fn test_state(&self) -> (Vec<String>, Option<usize>, bool) {
        (self.chosen(), self.resume, self.stale)
    }
}

pub fn open(s: &mut Workspace, tab: TabId, resume: Option<usize>, stale: bool) -> bool {
    let Some((b, dialog)) = s.browser_and_dialog(tab) else {
        return false;
    };
    let Some(grid) = &b.table.grid else {
        return false;
    };
    let Some(page) = grid.page() else {
        return false;
    };
    let name = db::render_text(grid.table());
    if grid.kind() != TableKind::Table {
        status(b, "Rows of a view can't be edited", true);
        return false;
    }
    if page.key.source != KeySource::None {
        let key = key_chip(&page.key, None).map_or_else(
            || format!("{} (primary)", column_list(&page.key.columns)),
            |(label, _)| label.trim_start_matches("key: ").to_string(),
        );
        status(b, format!("{name} already has a key: {key}"), false);
        return false;
    }
    if let Some(draft) = b
        .table
        .draft
        .as_ref()
        .filter(|draft| draft.changes() > 0 || draft.saving)
    {
        let row = draft.target().row_label.clone();
        status(
            b,
            format!("Save or cancel the changes in {row} first"),
            true,
        );
        return true;
    }
    let saved = chosen_key(&b.row_keys, grid);
    let comparable = |at: usize| page.comparable.get(at).copied().unwrap_or(false);
    let kept: Vec<&String> = page
        .columns
        .iter()
        .enumerate()
        .filter(|(at, column)| comparable(*at) && saved.is_some_and(|s| s.contains(&column.name)))
        .map(|(_, column)| &column.name)
        .collect();
    let columns = page
        .columns
        .iter()
        .enumerate()
        .map(|(at, column)| KeyColumn {
            name: column.name.clone(),
            type_name: column.type_name.clone(),
            comparable: comparable(at),
            checked: comparable(at) && (kept.is_empty() || kept.contains(&&column.name)),
        })
        .collect();
    let picker = KeyPicker {
        tab,
        connection: b.connection.name.clone(),
        schema: grid.schema().to_string(),
        table: grid.table().to_string(),
        columns,
        saved: saved.is_some(),
        stale: stale || saved.is_some_and(|saved| !usable_key(page, saved)),
        resume,
        scroll: ScrollY::default(),
    };
    *dialog = Some(Dialog::KeyPicker(Box::new(picker)));
    true
}

fn picker_mut(s: &mut Workspace) -> Option<&mut KeyPicker> {
    match &mut s.dialog {
        Some(Dialog::KeyPicker(picker)) => Some(picker),
        _ => None,
    }
}

fn toggle(s: &mut Workspace, index: usize) {
    if let Some(column) = picker_mut(s)
        .and_then(|picker| picker.columns.get_mut(index))
        .filter(|column| column.comparable)
    {
        column.checked = !column.checked;
    }
}

fn take(s: &mut Workspace) -> Option<Box<KeyPicker>> {
    let Some(Dialog::KeyPicker(picker)) = s.dialog.take() else {
        return None;
    };
    s.refocus = true;
    Some(picker)
}

fn choose(s: &mut Workspace, tx: &mut Tx) {
    if picker_mut(s).is_none_or(|picker| picker.chosen().is_empty()) {
        return;
    }
    let Some(picker) = take(s) else {
        return;
    };
    let columns = picker.chosen();
    let done = format!(
        "Rows of {} are now found by {}",
        db::render_text(&picker.table),
        column_list(&columns)
    );
    remember(s, &picker, Some(columns), done);
    if let Some(column) = picker.resume {
        begin_edit(s, picker.tab, Some(column), true, tx);
        s.refocus = !s.browser(picker.tab).is_some_and(|b| b.table.typing());
    }
}

fn forget(s: &mut Workspace) {
    let Some(picker) = take(s) else {
        return;
    };
    let done = format!(
        "The key columns of {} were forgotten",
        db::render_text(&picker.table)
    );
    remember(s, &picker, None, done);
}

fn cancel(s: &mut Workspace) {
    take(s);
}

fn remember(s: &mut Workspace, picker: &KeyPicker, columns: Option<Vec<String>>, done: String) {
    let saved = s.home.save_row_key(
        &picker.connection,
        &picker.schema,
        &picker.table,
        columns.clone(),
    );
    let Some(b) = s.browser_mut(picker.tab) else {
        return;
    };
    let place = (picker.schema.clone(), picker.table.clone());
    match columns {
        Some(columns) => {
            b.row_keys.insert(place, columns);
        }
        None => {
            b.row_keys.remove(&place);
        }
    }
    if b.table
        .draft
        .as_ref()
        .is_some_and(|draft| draft.changes() == 0 && !draft.saving)
    {
        drop_draft(b);
    }
    match saved {
        Ok(()) => status(b, done, false),
        Err(err) => status(b, format!("Couldn't remember the key columns: {err}"), true),
    }
}

fn column_box(index: usize, column: &KeyColumn, t: &Theme) -> El {
    let type_name = if column.type_name.is_empty() {
        "no type".to_string()
    } else {
        db::render_text(&column.type_name)
    };
    let description = if column.comparable {
        type_name
    } else {
        format!("{type_name}: can't be compared exactly")
    };
    Checkbox::new(column.checked)
        .label(db::render_text(&column.name))
        .description(description)
        .disabled(!column.comparable)
        .on_toggle(move |s: &mut Workspace| toggle(s, index))
        .build(&t.k)
        .key(format!("key-picker-column-{index}"))
}

pub fn dialog(
    _s: &Workspace,
    picker: &KeyPicker,
    t: &Theme,
    cx: &mut Cx<Workspace>,
    layer: &mut Vec<El>,
) {
    let name = db::render_text(&picker.table);
    let room = cx
        .layout_size("workspace")
        .map_or(f32::INFINITY, |[_, height]| (height - CHROME).max(MIN_LIST));
    let list = picker.scroll.watch(
        col()
            .key("key-picker-columns")
            .w_full()
            .max_h(room)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(
                col().w_full().gap(8.0).children(
                    picker
                        .columns
                        .iter()
                        .enumerate()
                        .map(|(index, column)| column_box(index, column, t)),
                ),
            ),
    );
    let body = col()
        .key("key-picker")
        .w_full()
        .px(16.0)
        .gap(12.0)
        .child(
            text(format!(
                "{name} has no primary key, unique index or row id. Choose the columns that \
                 together identify one row. A change is saved only when exactly one row matches \
                 them."
            ))
            .key("key-picker-intro")
            .font(t.text)
            .color(t.foreground)
            .wrap(),
        )
        .children(picker.stale.then(|| {
            row()
                .key("key-picker-stale")
                .w_full()
                .gap(8.0)
                .padding(10.0)
                .center_y()
                .radius(RADIUS)
                .border(1.0, t.warning)
                .fill(t.k.alpha(t.warning, 0.08))
                .child(ui::icon_el(&icons::TRIANGLE_ALERT, 14.0, t.warning))
                .child(
                    ui::label(
                        "Some key columns chosen before are gone; choose them again",
                        t.text_sm,
                        t.foreground,
                    )
                    .wrap()
                    .grow(1.0)
                    .min_w(0.0),
                )
        }))
        .child(list);
    let footer = row()
        .w_full()
        .padding(16.0)
        .gap(8.0)
        .justify_end()
        .children(picker.saved.then(|| {
            Button::new("Forget the key columns")
                .variant(ButtonVariant::Outline)
                .on_press(forget)
                .build(&t.k)
                .key("key-picker-forget")
        }))
        .child(
            Button::new("Cancel")
                .variant(ButtonVariant::Outline)
                .on_press(cancel)
                .build(&t.k)
                .key("key-picker-cancel")
                .autofocus(),
        )
        .child(
            Button::new("Use these columns")
                .variant(ButtonVariant::Submit)
                .disabled(picker.chosen().is_empty())
                .on_press_tx(choose)
                .build(&t.k)
                .key("key-picker-use"),
        );
    KitDialog::new("key-picker", format!("Key columns for {name}"))
        .width(520.0)
        .fit()
        .open(true)
        .on_close(cancel)
        .child(body)
        .child(footer)
        .build(&t.modal, layer);
}
