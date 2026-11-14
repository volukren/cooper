use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    db::{self, ColumnDef, ColumnInfo, KeySource, Page, RowKey, TableKind},
    ui::group,
};

pub const PAGE_SIZE: u32 = 100;
pub const MAX_COLUMN_WIDTH: usize = 40;
pub const MAX_LABEL_WIDTH: usize = 30;
pub const TAB_WIDTH: usize = 4;
pub const NULL: &str = "NULL";
pub const CUT_NOTE: &str = "… the value is longer, only its first 64 KB are loaded";

pub fn column_widths(columns: &[ColumnInfo], rows: &[Vec<Option<String>>]) -> Vec<usize> {
    columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let cells = rows.iter().map(|row| {
                row.get(index)
                    .map_or(0, |cell| cell.as_deref().unwrap_or(NULL).width())
            });
            cells
                .chain([column.name.width(), column.type_name.width()])
                .max()
                .unwrap_or(0)
                .clamp(1, MAX_COLUMN_WIDTH)
        })
        .collect()
}

pub fn gutter_width(offset: u64, rows: usize) -> usize {
    group(offset + rows as u64).width().max(1)
}

pub fn definition<'a>(defs: &'a [ColumnDef], column: &'a ColumnInfo) -> &'a str {
    defs.iter()
        .find(|def| def.name == column.name)
        .map_or(column.type_name.as_str(), |def| def.definition.as_str())
}

pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let (c, count) = match c {
            '\r' if chars.peek() == Some(&'\n') => continue,
            '\n' | '\r' | '\u{2028}' | '\u{2029}' => {
                lines.push(std::mem::take(&mut line));
                used = 0;
                continue;
            }
            '\t' => {
                let stop = TAB_WIDTH - used % TAB_WIDTH;
                (' ', stop.min(width.saturating_sub(used)).max(1))
            }
            c if c.is_control() => ('·', 1),
            c => (c, 1),
        };
        let char_width = c.width().unwrap_or(0);
        for _ in 0..count {
            if used > 0 && used + char_width > width {
                if c == ' ' && !line.ends_with(' ') {
                    line.push(c);
                    used += char_width;
                    continue;
                }
                let word = line.rfind(' ').map_or(line.len(), |space| space + 1);
                let rest = line.split_off(if c == ' ' { line.len() } else { word });
                used = rest.width();
                lines.push(std::mem::replace(&mut line, rest));
                if used > 0 && used + char_width > width {
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                }
            }
            line.push(c);
            used += char_width;
        }
    }
    lines.push(line);
    lines
}

pub fn normalize(text: &str) -> String {
    wrap(text, usize::MAX).join("\n")
}

pub fn page_footer(page: &Page, total: Option<u64>, count_failed: bool) -> String {
    let unknown = if count_failed { "?" } else { "…" };
    let rows = if page.rows.is_empty() {
        "no rows".to_string()
    } else {
        format!(
            "rows {}–{} of {}",
            group(page.offset + 1),
            group(page.offset + page.rows.len() as u64),
            total.map_or(unknown.to_string(), group)
        )
    };
    let number = db::page_number(page.offset, PAGE_SIZE);
    let pages = total.map_or(unknown.to_string(), |total| {
        group(db::page_count(total, PAGE_SIZE).max(number))
    });
    format!("{rows} · page {}/{pages}", group(number))
}

pub fn kind_label(kind: TableKind) -> Option<&'static str> {
    match kind {
        TableKind::Table => None,
        TableKind::View => Some("view"),
        TableKind::MaterializedView => Some("materialized view"),
    }
}

pub fn column_list(columns: &[String]) -> String {
    columns
        .iter()
        .map(|column| db::render_text(column))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn key_chip(key: &RowKey, chosen: Option<&[String]>) -> Option<(String, String)> {
    let columns = || column_list(&key.columns);
    let (label, help) = match &key.source {
        KeySource::Primary => return None,
        KeySource::Unique(index) => (
            format!("key: {} (unique)", columns()),
            format!(
                "Rows are found by the unique index {}",
                db::render_text(index)
            ),
        ),
        KeySource::RowId => (
            format!("key: {}", columns()),
            "Rows are found by SQLite's row id".to_string(),
        ),
        KeySource::Ctid => (
            "key: ctid".to_string(),
            "Rows are found by their place on disk (ctid) and version (xmin), which change \
             when a row is saved"
                .to_string(),
        ),
        KeySource::None => match chosen {
            Some(chosen) => (
                format!("key: {} (chosen)", column_list(chosen)),
                "Rows are found by the columns you chose. Click to change them".to_string(),
            ),
            None => (
                "no key".to_string(),
                "No primary key, unique index or row id. Click to choose key columns".to_string(),
            ),
        },
    };
    Some((label, help))
}

pub fn record_footer(offset: u64, row: usize) -> String {
    format!("row {}", group(offset + row as u64 + 1))
}

pub fn full_value(page: &Page, row: usize, column: usize) -> Option<&str> {
    match page.full.get(&(row, column)) {
        Some(full) => Some(full.text.as_str()),
        None => page.rows.get(row)?.get(column)?.as_deref(),
    }
}

pub fn cell_copy_text(page: &Page, row: usize, column: usize) -> String {
    full_value(page, row, column).unwrap_or(NULL).to_string()
}

pub fn row_copy_text(page: &Page, row: usize) -> String {
    (0..page.columns.len())
        .map(|column| full_value(page, row, column).unwrap_or(NULL))
        .collect::<Vec<_>>()
        .join("\t")
}

pub fn copied(text: &str) -> String {
    let lines = text.lines().count();
    if lines > 1 {
        format!("Copied {lines} lines")
    } else {
        let chars = text.chars().count();
        format!(
            "Copied {chars} character{}",
            if chars == 1 { "" } else { "s" }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_COLUMN_WIDTH, column_widths, copied, definition, gutter_width, key_chip, normalize,
        page_footer, record_footer, row_copy_text, wrap,
    };
    use crate::db::{ColumnDef, ColumnInfo, FullValue, FullValues, KeySource, Page, RowKey};

    fn column(name: &str, type_name: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            type_name: type_name.to_string(),
        }
    }

    fn page_of(offset: u64, rows: usize, has_more: bool) -> Page {
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
            offset,
            has_more,
            ..Page::default()
        }
    }

    #[test]
    fn column_widths_follow_content_with_a_cap() {
        let mut page = page_of(0, 2, false);
        page.rows[1][1] = Some("x".repeat(200));
        page.rows[0][2] = Some("日本".to_string());
        assert_eq!(
            column_widths(&page.columns, &page.rows),
            [7, MAX_COLUMN_WIDTH, 4]
        );
        page.rows.clear();
        assert_eq!(column_widths(&page.columns, &page.rows), [7, 4, 4]);
    }

    #[test]
    fn gutter_fits_the_last_row_number() {
        assert_eq!(gutter_width(0, 0), 1);
        assert_eq!(gutter_width(0, 9), 1);
        assert_eq!(gutter_width(0, 100), 3);
        assert_eq!(gutter_width(900, 100), 5);
    }

    #[test]
    fn labels_prefer_the_ddl_definition() {
        let columns = [
            column("id", "INTEGER"),
            column("name", "TEXT"),
            column("note", ""),
        ];
        assert_eq!(definition(&[], &columns[0]), "INTEGER");
        let defs = vec![
            ColumnDef {
                name: "id".to_string(),
                definition: "INTEGER PRIMARY KEY".to_string(),
            },
            ColumnDef {
                name: "name".to_string(),
                definition: "TEXT NOT NULL".to_string(),
            },
        ];
        assert_eq!(definition(&defs, &columns[0]), "INTEGER PRIMARY KEY");
        assert_eq!(definition(&defs, &columns[2]), "");
    }

    #[test]
    fn wraps_values_by_width_and_line_breaks() {
        assert_eq!(wrap("", 10), [""]);
        assert_eq!(wrap("abcdef", 3), ["abc", "def"]);
        assert_eq!(wrap("abc", 3), ["abc"]);
        assert_eq!(
            wrap("a\nb\r\nc\rd\u{2028}e\n", 10),
            ["a", "b", "c", "d", "e", ""]
        );
        assert_eq!(wrap("a\tb\t\tc", 10), ["a   b     ", "c"]);
        assert_eq!(wrap("ab\tc", 3), ["ab ", "c"]);
        assert_eq!(wrap("a\u{1b}[31mb\u{7}", 10), ["a·[31mb·"]);
        assert_eq!(wrap("日本語", 4), ["日本", "語"]);
        assert_eq!(wrap("日本", 1), ["日", "本"]);
        assert_eq!(wrap("abc", 0), ["a", "b", "c"]);
        assert_eq!(wrap("one two three", 8), ["one two ", "three"]);
        assert_eq!(wrap("one two three", 7), ["one two ", "three"]);
        assert_eq!(wrap("one two three", 6), ["one ", "two ", "three"]);
        assert_eq!(wrap("one  two", 4), ["one ", " two"]);
        assert_eq!(wrap("a bcdefgh", 4), ["a ", "bcde", "fgh"]);
        assert_eq!(wrap("ab 日本語", 5), ["ab ", "日本", "語"]);
    }

    #[test]
    fn normalizes_breaks_tabs_and_controls() {
        assert_eq!(normalize("SELECT   1,\r\n\tx"), "SELECT   1,\n    x");
        assert_eq!(normalize("a\tb\u{7}\rc"), "a   b·\nc");
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("end\n"), "end\n");
    }

    #[test]
    fn footer_describes_the_displayed_page() {
        let page = page_of(0, 100, true);
        assert_eq!(
            page_footer(&page, None, false),
            "rows 1–100 of … · page 1/…"
        );
        assert_eq!(
            page_footer(&page, Some(1250), false),
            "rows 1–100 of 1,250 · page 1/13"
        );
        assert_eq!(page_footer(&page, None, true), "rows 1–100 of ? · page 1/?");
        assert_eq!(
            page_footer(&page_of(100, 100, true), Some(250), false),
            "rows 101–200 of 250 · page 2/3"
        );
        assert_eq!(
            page_footer(&page_of(0, 42, false), Some(42), false),
            "rows 1–42 of 42 · page 1/1"
        );
        assert_eq!(
            page_footer(&page_of(0, 0, false), Some(0), false),
            "no rows · page 1/1"
        );
        assert_eq!(
            page_footer(&page_of(300, 5, false), Some(10), false),
            "rows 301–305 of 10 · page 4/4"
        );
        assert_eq!(record_footer(1200, 33), "row 1,234");
    }

    #[test]
    fn copies_full_values_and_reports_the_size() {
        let mut page = page_of(0, 2, false);
        page.full.insert(
            (1, 1),
            FullValue {
                text: "a\nb".to_string(),
                truncated: false,
            },
        );
        assert_eq!(row_copy_text(&page, 0), "1\tuser 1\tNULL");
        assert_eq!(row_copy_text(&page, 1), "2\ta\nb\tNULL");
        assert_eq!(copied("x"), "Copied 1 character");
        assert_eq!(copied("hello"), "Copied 5 characters");
        assert_eq!(copied(""), "Copied 0 characters");
        assert_eq!(copied("a\nb\nc"), "Copied 3 lines");
    }

    #[test]
    fn the_key_chip_names_how_rows_are_found() {
        let key = |source: KeySource, columns: &[&str]| RowKey {
            source,
            columns: columns.iter().map(|c| c.to_string()).collect(),
        };
        assert_eq!(key_chip(&key(KeySource::Primary, &["id"]), None), None);
        assert_eq!(
            key_chip(
                &key(KeySource::Unique("t_code".into()), &["code", "at"]),
                None
            ),
            Some((
                "key: code, at (unique)".to_string(),
                "Rows are found by the unique index t_code".to_string()
            ))
        );
        assert_eq!(
            key_chip(&key(KeySource::RowId, &["_rowid_"]), None).map(|chip| chip.0),
            Some("key: _rowid_".to_string())
        );
        let (label, help) =
            key_chip(&key(KeySource::Ctid, &["tableoid", "ctid", "xmin"]), None).unwrap();
        assert_eq!(label, "key: ctid");
        assert!(help.contains("(xmin)"), "{help}");
        let (label, help) = key_chip(&RowKey::default(), None).unwrap();
        assert_eq!(label, "no key");
        assert!(help.ends_with("Click to choose key columns"), "{help}");
        let chosen = ["at".to_string(), "msg".to_string()];
        let (label, help) = key_chip(&RowKey::default(), Some(&chosen)).unwrap();
        assert_eq!(label, "key: at, msg (chosen)");
        assert!(help.ends_with("Click to change them"), "{help}");
        assert_eq!(
            key_chip(&key(KeySource::RowId, &["rowid"]), Some(&chosen)).map(|chip| chip.0),
            Some("key: rowid".to_string()),
            "a key of its own wins"
        );
    }
}
