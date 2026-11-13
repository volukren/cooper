use rune::TextEdit;

use crate::db::{self, Page, RowUpdate, TableKind};

use super::format::record_footer;

pub const MAX_FIELD_ROWS: usize = 8;

pub const BYTES_NOTE: &str = "Binary value: not editable here";
pub const CUT_NOTE: &str = "Longer than the loaded 64 KB: not editable here";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exact {
    Value(Option<String>),
    Opaque,
    Cut,
}

pub fn exact(page: &Page, row: usize, column: usize) -> Exact {
    if page.opaque.contains(&(row, column)) {
        return Exact::Opaque;
    }
    match page.full.get(&(row, column)) {
        Some(full) if full.truncated => Exact::Cut,
        Some(full) => Exact::Value(Some(full.text.clone())),
        None => Exact::Value(
            page.rows
                .get(row)
                .and_then(|values| values.get(column))
                .cloned()
                .flatten(),
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSpec {
    pub name: String,
    pub type_name: String,
    pub key: bool,
    pub original: Option<String>,
    pub locked: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub schema: String,
    pub table: String,
    pub row_label: String,
    pub key: Vec<(String, String)>,
    pub fields: Vec<FieldSpec>,
}

pub fn target(page: &Page, row: usize, kind: TableKind) -> Result<Target, String> {
    if row >= page.rows.len() {
        return Err("No row to edit".to_string());
    }
    if page.primary_key.is_empty() {
        return Err(match kind {
            TableKind::Table => format!(
                "{} has no primary key, so its rows can't be edited",
                db::render_text(&page.table)
            ),
            TableKind::View | TableKind::MaterializedView => {
                "Rows of a view can't be edited".to_string()
            }
        });
    }
    let mut key = Vec::with_capacity(page.primary_key.len());
    for name in &page.primary_key {
        let column = page.columns.iter().position(|column| column.name == *name);
        match column.map(|column| exact(page, row, column)) {
            Some(Exact::Value(Some(value))) => key.push((name.clone(), value)),
            _ => {
                return Err(format!(
                    "The key column {} has no exact value here, so the row can't be edited",
                    db::render_text(name)
                ));
            }
        }
    }
    let fields = page
        .columns
        .iter()
        .enumerate()
        .map(|(column, info)| {
            let (original, locked) = match exact(page, row, column) {
                Exact::Value(value) => (value, None),
                Exact::Opaque => (page.rows[row][column].clone(), Some(BYTES_NOTE)),
                Exact::Cut => (page.rows[row][column].clone(), Some(CUT_NOTE)),
            };
            FieldSpec {
                name: info.name.clone(),
                type_name: info.type_name.clone(),
                key: page.primary_key.contains(&info.name),
                original,
                locked,
            }
        })
        .collect();
    Ok(Target {
        schema: page.schema.clone(),
        table: page.table.clone(),
        row_label: record_footer(page.offset, row),
        key,
        fields,
    })
}

impl Target {
    pub fn changed(&self, index: usize, value: &Option<String>) -> bool {
        self.fields
            .get(index)
            .is_some_and(|field| field.locked.is_none() && field.original != *value)
    }

    pub fn update(&self, values: &[Option<String>]) -> Option<RowUpdate> {
        let changed: Vec<(String, Option<String>)> = values
            .iter()
            .enumerate()
            .filter(|(index, value)| self.changed(*index, value))
            .map(|(index, value)| (self.fields[index].name.clone(), value.clone()))
            .collect();
        (!changed.is_empty()).then(|| RowUpdate {
            schema: self.schema.clone(),
            table: self.table.clone(),
            key: self.key.clone(),
            values: changed,
        })
    }
}

pub struct DraftField {
    pub input: TextEdit,
    pub null: bool,
    stash: String,
}

pub struct Draft {
    target: Target,
    row: usize,
    fields: Vec<Option<DraftField>>,
    open: Option<usize>,
    pub saving: bool,
    pub error: Option<String>,
}

impl Draft {
    pub fn new(target: Target, row: usize) -> Self {
        let fields = target.fields.iter().map(|_| None).collect();
        Self {
            target,
            row,
            fields,
            open: None,
            saving: false,
            error: None,
        }
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    pub fn row(&self) -> usize {
        self.row
    }

    pub fn field(&self, index: usize) -> Option<&DraftField> {
        self.fields.get(index)?.as_ref()
    }

    pub fn field_mut(&mut self, index: usize) -> Option<&mut DraftField> {
        self.fields.get_mut(index)?.as_mut()
    }

    pub fn input_mut(&mut self, index: usize) -> Option<&mut TextEdit> {
        self.field_mut(index).map(|field| &mut field.input)
    }

    pub fn is_open(&self, index: usize) -> bool {
        self.open == Some(index)
    }

    pub fn open_field(&self) -> Option<usize> {
        self.open
    }

    pub fn is_idle(&self) -> bool {
        self.fields.iter().all(Option::is_none)
    }

    pub fn value(&self, index: usize) -> Option<Option<String>> {
        self.field(index)
            .map(|field| (!field.null).then(|| field.input.text.clone()))
    }

    pub fn close(&mut self) {
        let Some(index) = self.open.take() else {
            return;
        };
        if self.changed(index) {
            if let Some(field) = &mut self.fields[index] {
                field.input.focused = false;
            }
        } else {
            self.fields[index] = None;
        }
    }

    pub fn editable(&self, index: usize) -> bool {
        self.target
            .fields
            .get(index)
            .is_some_and(|field| field.locked.is_none())
    }

    pub fn open(&mut self, index: usize) -> bool {
        if !self.editable(index) {
            return false;
        }
        if self.open != Some(index) {
            self.close();
            self.open = Some(index);
        }
        if self.fields[index].is_none() {
            let original = self.target.fields[index].original.clone();
            let mut input = TextEdit::new(original.clone().unwrap_or_default());
            let end = input.text.len();
            input.set_selection(end, end);
            self.fields[index] = Some(DraftField {
                input,
                null: original.is_none(),
                stash: String::new(),
            });
        }
        for (at, field) in self.fields.iter_mut().enumerate() {
            if let Some(field) = field {
                field.input.focused = at == index;
            }
        }
        true
    }

    pub fn on_change(&mut self, index: usize) {
        if let Some(field) = self.fields.get_mut(index).and_then(Option::as_mut)
            && field.null
            && !field.input.text.is_empty()
        {
            field.null = false;
        }
        self.error = None;
    }

    pub fn set_null(&mut self, index: usize, null: bool) {
        let Some(field) = self.fields.get_mut(index).and_then(Option::as_mut) else {
            return;
        };
        if field.null == null {
            return;
        }
        field.null = null;
        let text = if null {
            field.stash = field.input.text.clone();
            String::new()
        } else {
            std::mem::take(&mut field.stash)
        };
        field.input.replace_text(text);
        let end = field.input.text.len();
        field.input.set_selection(end, end);
        self.error = None;
    }

    pub fn values(&self) -> Vec<Option<String>> {
        self.fields
            .iter()
            .zip(&self.target.fields)
            .map(|(field, spec)| match field {
                Some(field) => (!field.null).then(|| field.input.text.clone()),
                None => spec.original.clone(),
            })
            .collect()
    }

    pub fn changed(&self, index: usize) -> bool {
        self.value(index)
            .is_some_and(|value| self.target.changed(index, &value))
    }

    pub fn changes(&self) -> usize {
        (0..self.fields.len())
            .filter(|index| self.changed(*index))
            .count()
    }

    pub fn update(&self) -> Option<RowUpdate> {
        self.target.update(&self.values())
    }

    pub fn focused(&self) -> Option<usize> {
        self.fields
            .iter()
            .position(|field| field.as_ref().is_some_and(|field| field.input.focused))
    }

    pub fn next(&self, forward: bool) -> Option<usize> {
        let editable: Vec<usize> = (0..self.fields.len())
            .filter(|index| self.editable(*index))
            .collect();
        if editable.is_empty() {
            return None;
        }
        let current = self
            .focused()
            .and_then(|focused| editable.iter().position(|index| *index == focused));
        let next = match (current, forward) {
            (Some(at), true) => (at + 1) % editable.len(),
            (Some(at), false) => (at + editable.len() - 1) % editable.len(),
            (None, true) => 0,
            (None, false) => editable.len() - 1,
        };
        Some(editable[next])
    }

    pub fn first_editable(&self, from: usize) -> Option<usize> {
        let count = self.fields.len();
        (0..count)
            .map(|step| (from + step) % count)
            .find(|index| self.editable(*index))
    }

    pub fn blur(&mut self) {
        for field in self.fields.iter_mut().flatten() {
            field.input.focused = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BYTES_NOTE, CUT_NOTE, Exact, exact, target};
    use crate::db::{ColumnInfo, FullValue, FullValues, Page, RowUpdate, TableKind};

    fn page() -> Page {
        let column = |name: &str| ColumnInfo {
            name: name.to_string(),
            type_name: "TEXT".to_string(),
        };
        let mut full = FullValues::new();
        full.insert(
            (0, 1),
            FullValue {
                text: "line\none".to_string(),
                truncated: false,
            },
        );
        full.insert(
            (1, 1),
            FullValue {
                text: "x".repeat(10),
                truncated: true,
            },
        );
        Page {
            schema: "main".to_string(),
            table: "t".to_string(),
            columns: vec![column("id"), column("note"), column("data"), column("tag")],
            rows: vec![
                vec![
                    Some("7".to_string()),
                    Some("line↵one".to_string()),
                    Some("0xdead".to_string()),
                    None,
                ],
                vec![
                    Some("8".to_string()),
                    Some("xxx…".to_string()),
                    None,
                    Some("a".to_string()),
                ],
            ],
            full,
            opaque: [(0, 2)].into_iter().collect(),
            primary_key: vec!["id".to_string()],
            offset: 100,
            has_more: false,
        }
    }

    #[test]
    fn exact_values_come_from_the_full_text() {
        let page = page();
        assert_eq!(exact(&page, 0, 0), Exact::Value(Some("7".to_string())));
        assert_eq!(
            exact(&page, 0, 1),
            Exact::Value(Some("line\none".to_string()))
        );
        assert_eq!(exact(&page, 0, 2), Exact::Opaque);
        assert_eq!(exact(&page, 0, 3), Exact::Value(None));
        assert_eq!(exact(&page, 1, 1), Exact::Cut);
    }

    #[test]
    fn a_target_knows_its_key_and_locks_what_it_cannot_spell() {
        let page = page();
        let first = target(&page, 0, TableKind::Table).unwrap();
        assert_eq!(first.row_label, "row 101");
        assert_eq!(first.key, [("id".to_string(), "7".to_string())]);
        assert!(first.fields[0].key && !first.fields[1].key);
        assert_eq!(first.fields[1].original.as_deref(), Some("line\none"));
        assert_eq!(first.fields[2].locked, Some(BYTES_NOTE));
        assert_eq!(first.fields[3].original, None);
        let second = target(&page, 1, TableKind::Table).unwrap();
        assert_eq!(second.fields[1].locked, Some(CUT_NOTE));
        assert_eq!(second.fields[1].original.as_deref(), Some("xxx…"));
    }

    #[test]
    fn rows_without_an_exact_key_cannot_be_edited() {
        let mut page = page();
        assert_eq!(
            target(&page, 2, TableKind::Table).unwrap_err(),
            "No row to edit"
        );
        page.primary_key = vec!["data".to_string()];
        assert!(
            target(&page, 0, TableKind::Table)
                .unwrap_err()
                .contains("data")
        );
        page.primary_key = vec!["tag".to_string()];
        assert!(target(&page, 0, TableKind::Table).is_err(), "a NULL key");
        page.primary_key.clear();
        assert_eq!(
            target(&page, 0, TableKind::Table).unwrap_err(),
            "t has no primary key, so its rows can't be edited"
        );
        assert_eq!(
            target(&page, 0, TableKind::View).unwrap_err(),
            "Rows of a view can't be edited"
        );
    }

    #[test]
    fn the_update_carries_only_the_changed_columns() {
        let page = page();
        let target = target(&page, 0, TableKind::Table).unwrap();
        let unchanged: Vec<Option<String>> = target
            .fields
            .iter()
            .map(|field| field.original.clone())
            .collect();
        assert_eq!(target.update(&unchanged), None);

        let mut values = unchanged.clone();
        values[1] = None;
        values[2] = Some("ignored: locked".to_string());
        values[3] = Some(String::new());
        assert_eq!(
            target.update(&values),
            Some(RowUpdate {
                schema: "main".to_string(),
                table: "t".to_string(),
                key: vec![("id".to_string(), "7".to_string())],
                values: vec![
                    ("note".to_string(), None),
                    ("tag".to_string(), Some(String::new()))
                ],
            })
        );
        assert!(
            target.changed(3, &Some(String::new())),
            "an empty text is not NULL"
        );
        assert!(!target.changed(2, &None), "locked fields never change");
    }
}
