use rune::TextEdit;

use crate::db::{self, KeySource, Page, RowUpdate, TableKind};

use super::format::record_footer;

pub const MAX_FIELD_ROWS: usize = 8;

pub const BYTES_NOTE: &str = "Binary value: not editable here";
pub const CUT_NOTE: &str = "Longer than the loaded 64 KB: not editable here";
pub const MORE_KEYS: &str =
    "Cancel the edit, then choose more key columns to tell these rows apart.";
pub const NO_FINGERPRINT: &str = "No value in this row can be compared exactly, so it can't be \
                                  found safely by its row id; add a primary key or a unique index";

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
    pub key: Vec<(String, Option<String>)>,
    pub chosen: bool,
    pub fields: Vec<FieldSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Reason(String),
    ChooseKey { stale: bool },
}

fn refuse(reason: impl Into<String>) -> Refusal {
    Refusal::Reason(reason.into())
}

pub fn target(
    page: &Page,
    row: usize,
    kind: TableKind,
    chosen: Option<&[String]>,
) -> Result<Target, Refusal> {
    if row >= page.rows.len() {
        return Err(refuse("No row to edit"));
    }
    if kind != TableKind::Table {
        return Err(refuse("Rows of a view can't be edited"));
    }
    let (columns, chosen) = match chosen {
        _ if page.key.source != KeySource::None => (page.key.columns.as_slice(), false),
        Some(columns) if usable_key(page, columns) => (columns, true),
        Some(_) => return Err(Refusal::ChooseKey { stale: true }),
        None => return Err(Refusal::ChooseKey { stale: false }),
    };
    let mut key = Vec::with_capacity(columns.len());
    for name in columns {
        match key_value(page, row, name) {
            Some(value) => key.push((name.clone(), value)),
            None if chosen => {
                return Err(refuse(format!(
                    "The key column {} has no exact value in this row, so it can't be edited; \
                     choose other key columns",
                    db::render_text(name)
                )));
            }
            None => {
                return Err(refuse(format!(
                    "The key column {} has no exact value here, so the row can't be edited",
                    db::render_text(name)
                )));
            }
        }
    }
    if page.key.source == KeySource::RowId {
        let id = key.len();
        fingerprint(page, row, &mut key);
        if key.len() == id {
            return Err(refuse(NO_FINGERPRINT));
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
                key: columns.contains(&info.name),
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
        chosen,
        fields,
    })
}

pub fn usable_key(page: &Page, columns: &[String]) -> bool {
    !columns.is_empty()
        && columns.iter().all(|name| {
            page.columns
                .iter()
                .position(|column| column.name == *name)
                .is_some_and(|at| page.comparable.get(at).copied().unwrap_or(false))
        })
}

fn key_value(page: &Page, row: usize, name: &str) -> Option<Option<String>> {
    if let Some(at) = page.hidden.iter().position(|hidden| hidden == name) {
        return page.ids.get(row)?.get(at)?.clone().map(Some);
    }
    let column = page.columns.iter().position(|column| column.name == name)?;
    match exact(page, row, column) {
        Exact::Value(value) => Some(value),
        Exact::Opaque | Exact::Cut => None,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Follow {
    pub all: Vec<(usize, Option<String>)>,
    pub core: Vec<(usize, Option<String>)>,
}

pub fn row_matches(page: &Page, row: usize, follow: &[(usize, Option<String>)]) -> bool {
    row < page.rows.len()
        && follow.iter().all(|(column, value)| {
            *column < page.columns.len()
                && matches!(exact(page, row, *column), Exact::Value(shown) if shown == *value)
        })
}

pub fn find_row(page: &Page, follow: &Follow) -> Option<usize> {
    for values in [&follow.all, &follow.core] {
        if values.is_empty() {
            continue;
        }
        let mut matching = (0..page.rows.len()).filter(|row| row_matches(page, *row, values));
        match (matching.next(), matching.next()) {
            (Some(row), None) => return Some(row),
            (None, _) => {}
            (Some(_), Some(_)) => return None,
        }
    }
    None
}

fn fingerprint(page: &Page, row: usize, key: &mut Vec<(String, Option<String>)>) {
    for (column, info) in page.columns.iter().enumerate() {
        let comparable = page.comparable.get(column).copied().unwrap_or(false);
        if !comparable || key.iter().any(|(name, _)| *name == info.name) {
            continue;
        }
        if let Exact::Value(value) = exact(page, row, column) {
            key.push((info.name.clone(), value));
        }
    }
}

impl Target {
    pub fn save_error(&self, message: &str) -> String {
        if self.chosen && message.starts_with(db::MANY_ROWS) {
            format!("Not saved: {message}. {MORE_KEYS}")
        } else {
            format!("Not saved: {message}")
        }
    }

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

    pub fn follow(&self) -> Follow {
        let mut follow = Follow::default();
        for (index, field) in self.target.fields.iter().enumerate() {
            if field.locked.is_some() {
                continue;
            }
            let changed = self.changed(index);
            let value = if changed {
                self.value(index).flatten()
            } else {
                field.original.clone()
            };
            if changed || field.key {
                follow.core.push((index, value.clone()));
            }
            follow.all.push((index, value));
        }
        follow
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
    use super::{
        BYTES_NOTE, CUT_NOTE, Draft, Exact, Follow, MORE_KEYS, NO_FINGERPRINT, Refusal, exact,
        find_row, row_matches, target,
    };
    use crate::db::{
        ColumnInfo, FullValue, FullValues, KeySource, Page, RowKey, RowUpdate, TableKind,
    };

    fn key(source: KeySource, columns: &[&str]) -> RowKey {
        RowKey {
            source,
            columns: columns.iter().map(|c| c.to_string()).collect(),
        }
    }

    fn reason(text: &str) -> Refusal {
        Refusal::Reason(text.to_string())
    }

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
            key: key(KeySource::Primary, &["id"]),
            comparable: vec![true; 4],
            offset: 100,
            ..Page::default()
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
        let first = target(&page, 0, TableKind::Table, None).unwrap();
        assert_eq!(first.row_label, "row 101");
        assert_eq!(first.key, [("id".to_string(), Some("7".to_string()))]);
        assert!(first.fields[0].key && !first.fields[1].key);
        assert_eq!(first.fields[1].original.as_deref(), Some("line\none"));
        assert_eq!(first.fields[2].locked, Some(BYTES_NOTE));
        assert_eq!(first.fields[3].original, None);
        let second = target(&page, 1, TableKind::Table, None).unwrap();
        assert_eq!(second.fields[1].locked, Some(CUT_NOTE));
        assert_eq!(second.fields[1].original.as_deref(), Some("xxx…"));
    }

    #[test]
    fn rows_without_an_exact_key_cannot_be_edited() {
        let mut page = page();
        assert_eq!(
            target(&page, 2, TableKind::Table, None).unwrap_err(),
            reason("No row to edit")
        );
        page.key = key(KeySource::Primary, &["data"]);
        assert_eq!(
            target(&page, 0, TableKind::Table, None).unwrap_err(),
            reason("The key column data has no exact value here, so the row can't be edited")
        );
        page.key = key(KeySource::Primary, &["note"]);
        assert!(
            target(&page, 1, TableKind::Table, None).is_err(),
            "a cut key"
        );
        page.key = RowKey::default();
        assert_eq!(
            target(&page, 0, TableKind::Table, None).unwrap_err(),
            Refusal::ChooseKey { stale: false }
        );
        assert_eq!(
            target(&page, 0, TableKind::View, None).unwrap_err(),
            reason("Rows of a view can't be edited")
        );
    }

    #[test]
    fn a_view_is_refused_before_its_key_is_looked_at() {
        let page = page();
        assert_eq!(
            target(&page, 0, TableKind::MaterializedView, None).unwrap_err(),
            reason("Rows of a view can't be edited")
        );
    }

    #[test]
    fn a_null_key_value_finds_the_row_by_is_null() {
        let mut page = page();
        page.key = key(KeySource::Primary, &["id", "tag"]);
        let first = target(&page, 0, TableKind::Table, None).unwrap();
        assert_eq!(
            first.key,
            [
                ("id".to_string(), Some("7".to_string())),
                ("tag".to_string(), None)
            ]
        );
        assert!(first.fields[3].key);
    }

    #[test]
    fn a_hidden_key_comes_from_the_ids() {
        let mut page = page();
        page.key = key(KeySource::Unique("t_code".to_string()), &["code"]);
        page.hidden = vec!["code".to_string()];
        page.ids = vec![vec![Some("c7".to_string())], vec![None]];
        let first = target(&page, 0, TableKind::Table, None).unwrap();
        assert_eq!(first.key, [("code".to_string(), Some("c7".to_string()))]);
        assert_eq!(first.fields.len(), 4, "hidden columns are not fields");
        assert!(first.fields.iter().all(|field| !field.key));
        assert_eq!(
            target(&page, 1, TableKind::Table, None).unwrap_err(),
            reason("The key column code has no exact value here, so the row can't be edited")
        );
    }

    #[test]
    fn a_row_id_is_checked_against_the_exact_comparable_values() {
        let pairs = |pairs: &[(&str, Option<&str>)]| -> Vec<(String, Option<String>)> {
            pairs
                .iter()
                .map(|(c, v)| (c.to_string(), v.map(str::to_string)))
                .collect()
        };
        let mut page = page();
        page.key = key(KeySource::RowId, &["rowid"]);
        page.hidden = vec!["rowid".to_string()];
        page.ids = vec![vec![Some("1".to_string())], vec![Some("2".to_string())]];
        page.comparable = vec![true, true, true, false];
        let first = target(&page, 0, TableKind::Table, None).unwrap();
        assert_eq!(
            first.key,
            pairs(&[
                ("rowid", Some("1")),
                ("id", Some("7")),
                ("note", Some("line\none"))
            ]),
            "not the opaque data nor the tag that can't be compared"
        );
        assert!(
            first.fields.iter().all(|field| !field.key),
            "only the row id is the key"
        );

        page.comparable = vec![true; 4];
        let second = target(&page, 1, TableKind::Table, None).unwrap();
        assert_eq!(
            second.key,
            pairs(&[
                ("rowid", Some("2")),
                ("id", Some("8")),
                ("data", None),
                ("tag", Some("a"))
            ]),
            "not the cut note; NULL is matched by IS NULL"
        );

        page.key = key(KeySource::Unique("t_rowid".to_string()), &["rowid"]);
        assert_eq!(
            target(&page, 1, TableKind::Table, None).unwrap().key,
            pairs(&[("rowid", Some("2"))]),
            "other sources are not fingerprinted"
        );
    }

    #[test]
    fn a_row_id_alone_is_not_trusted() {
        let mut page = page();
        page.key = key(KeySource::RowId, &["rowid"]);
        page.hidden = vec!["rowid".to_string()];
        page.ids = vec![vec![Some("1".to_string())], vec![Some("2".to_string())]];
        page.comparable = vec![false; 4];
        assert_eq!(
            target(&page, 0, TableKind::Table, None).unwrap_err(),
            reason(NO_FINGERPRINT),
            "a reused or renumbered row id would find another row"
        );
        page.comparable = vec![false, true, false, false];
        assert_eq!(
            target(&page, 1, TableKind::Table, None).unwrap_err(),
            reason(NO_FINGERPRINT),
            "the only comparable value is cut"
        );
        assert!(target(&page, 0, TableKind::Table, None).is_ok());
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn a_table_without_a_key_asks_for_the_chosen_columns() {
        let mut page = page();
        page.key = RowKey::default();
        page.comparable = vec![true, true, false, true];
        let chosen = names(&["id", "tag"]);
        let first = target(&page, 0, TableKind::Table, Some(&chosen)).unwrap();
        assert!(first.chosen);
        assert_eq!(
            first.key,
            [
                ("id".to_string(), Some("7".to_string())),
                ("tag".to_string(), None)
            ],
            "a NULL in a chosen column is matched by IS NULL"
        );
        assert!(first.fields[0].key && first.fields[3].key && !first.fields[1].key);

        for stale in [names(&["gone"]), names(&["id", "data"]), Vec::new()] {
            assert_eq!(
                target(&page, 0, TableKind::Table, Some(&stale)).unwrap_err(),
                Refusal::ChooseKey { stale: true },
                "{stale:?}: a missing or incomparable column"
            );
        }
        assert_eq!(
            target(&page, 0, TableKind::View, Some(&chosen)).unwrap_err(),
            reason("Rows of a view can't be edited"),
            "the view check comes first"
        );

        page.key = key(KeySource::Primary, &["id"]);
        let own = target(&page, 0, TableKind::Table, Some(&names(&["tag"]))).unwrap();
        assert!(!own.chosen, "a key of its own wins over a chosen one");
        assert_eq!(own.key, [("id".to_string(), Some("7".to_string()))]);
    }

    #[test]
    fn a_chosen_key_without_an_exact_value_asks_for_other_columns() {
        let mut page = page();
        page.key = RowKey::default();
        let opaque = names(&["id", "data"]);
        assert_eq!(
            target(&page, 0, TableKind::Table, Some(&opaque)).unwrap_err(),
            reason(
                "The key column data has no exact value in this row, so it can't be edited; \
                 choose other key columns"
            )
        );
        assert!(
            target(&page, 1, TableKind::Table, Some(&opaque)).is_ok(),
            "the other row has a NULL there"
        );
        let cut = names(&["note"]);
        assert_eq!(
            target(&page, 1, TableKind::Table, Some(&cut)).unwrap_err(),
            reason(
                "The key column note has no exact value in this row, so it can't be edited; \
                 choose other key columns"
            )
        );
        let chosen = target(&page, 0, TableKind::Table, Some(&cut)).unwrap();
        assert_eq!(
            chosen.key,
            [("note".to_string(), Some("line\none".to_string()))],
            "no row id, so no fingerprint"
        );
    }

    #[test]
    fn many_matches_of_a_chosen_key_ask_for_more_key_columns() {
        let many = format!("{} 2 rows (nothing was saved)", crate::db::MANY_ROWS);
        let mut target = target(&page(), 0, TableKind::Table, None).unwrap();
        assert!(!target.chosen);
        assert_eq!(target.save_error(&many), format!("Not saved: {many}"));
        target.chosen = true;
        assert_eq!(
            target.save_error(&many),
            "Not saved: the row's key matches 2 rows (nothing was saved). Cancel the edit, then \
             choose more key columns to tell these rows apart."
        );
        assert!(target.save_error(&many).ends_with(MORE_KEYS));
        assert_eq!(
            target.save_error("the row was not found"),
            "Not saved: the row was not found"
        );
    }

    fn follow(all: &[(usize, Option<&str>)], core: &[(usize, Option<&str>)]) -> Follow {
        let values = |values: &[(usize, Option<&str>)]| {
            values
                .iter()
                .map(|(column, value)| (*column, value.map(str::to_string)))
                .collect()
        };
        Follow {
            all: values(all),
            core: values(core),
        }
    }

    #[test]
    fn a_saved_row_is_followed_by_the_exact_values_it_has_now() {
        let mut page = page();
        page.key = RowKey::default();
        let chosen = names(&["id"]);
        let mut draft = Draft::new(
            target(&page, 0, TableKind::Table, Some(&chosen)).unwrap(),
            0,
        );
        assert!(draft.open(3));
        draft.input_mut(3).unwrap().replace_text("t".to_string());
        draft.on_change(3);
        let follow = draft.follow();
        assert_eq!(
            follow,
            self::follow(
                &[(0, Some("7")), (1, Some("line\none")), (3, Some("t"))],
                &[(0, Some("7")), (3, Some("t"))]
            ),
            "not the opaque data; the changed tag as saved; the key and the saved tag tell it apart"
        );

        let mut saved = page.clone();
        saved.rows[0][3] = Some("t".to_string());
        assert_eq!(find_row(&saved, &follow), Some(0));
        assert_eq!(
            find_row(&page, &follow),
            None,
            "the saved value is not there"
        );

        let mut moved = saved.clone();
        moved.rows.swap(0, 1);
        moved.full = moved
            .full
            .into_iter()
            .map(|((row, column), value)| ((1 - row, column), value))
            .collect();
        moved.opaque = [(1, 2)].into_iter().collect();
        assert!(
            !row_matches(&moved, 0, &follow.all),
            "another row took its place"
        );
        assert_eq!(find_row(&moved, &follow), Some(1), "it moved down");

        moved.rows[1][0] = Some("9".to_string());
        assert_eq!(find_row(&moved, &follow), None, "it is gone");
    }

    #[test]
    fn a_saved_row_is_told_apart_from_its_twin() {
        let mut page = page();
        page.key = RowKey::default();
        page.rows[1] = page.rows[0].clone();
        page.opaque.insert((1, 2));
        page.full.remove(&(1, 1));
        page.full.insert((1, 1), page.full[&(0, 1)].clone());
        let chosen = names(&["id"]);
        let mut draft = Draft::new(
            target(&page, 0, TableKind::Table, Some(&chosen)).unwrap(),
            0,
        );
        assert!(draft.open(3));
        draft.input_mut(3).unwrap().replace_text("t".to_string());
        draft.on_change(3);
        let follow = draft.follow();

        let mut refreshed = page.clone();
        refreshed.rows[1][3] = Some("t".to_string());
        assert!(
            !row_matches(&refreshed, 0, &follow.all),
            "the twin that kept the old value"
        );
        assert_eq!(find_row(&refreshed, &follow), Some(1));

        refreshed.rows[0][3] = Some("t".to_string());
        assert_eq!(
            find_row(&refreshed, &follow),
            None,
            "two rows look like the saved one"
        );
    }

    #[test]
    fn a_saved_row_is_found_by_its_core_values_when_another_value_changed() {
        let page = page();
        let follow = follow(
            &[(0, Some("7")), (1, Some("line\none")), (3, Some("t"))],
            &[(3, Some("t"))],
        );
        let mut touched = page.clone();
        touched.rows[0][3] = Some("t".to_string());
        touched.full.remove(&(0, 1));
        touched.rows[0][1] = Some("rewritten".to_string());
        assert_eq!(
            find_row(&touched, &follow),
            Some(0),
            "a trigger rewrote the note"
        );
        touched.rows[1][3] = Some("t".to_string());
        assert_eq!(
            find_row(&touched, &follow),
            None,
            "the core values are not unique"
        );
        assert_eq!(
            find_row(&touched, &Follow::default()),
            None,
            "nothing to tell rows apart by"
        );
    }

    #[test]
    fn only_exact_values_in_range_match_a_followed_row() {
        let page = page();
        assert!(
            !row_matches(&page, 1, &[(1, Some("xxx…".to_string()))]),
            "a cut value never matches, even as shown"
        );
        assert!(
            !row_matches(&page, 0, &[(2, Some("0xdead".to_string()))]),
            "nor does an opaque one"
        );
        assert!(row_matches(&page, 0, &[(3, None)]), "NULL matches NULL");
        assert!(
            !row_matches(&page, 0, &[(9, None)]),
            "a column that is gone"
        );
        assert!(!row_matches(&page, 2, &[]), "a row that is gone");
        assert_eq!(find_row(&page, &follow(&[(0, Some("8"))], &[])), Some(1));
    }

    #[test]
    fn the_update_carries_only_the_changed_columns() {
        let page = page();
        let target = target(&page, 0, TableKind::Table, None).unwrap();
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
                key: vec![("id".to_string(), Some("7".to_string()))],
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
