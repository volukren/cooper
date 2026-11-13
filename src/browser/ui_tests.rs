use std::path::PathBuf;

use crate::{
    browser::{
        Main, Pane,
        tree::{self, TreeRow, row_key},
    },
    db::TableKind,
    keys::Command,
    test_support::{TestConfig, TestWorkspace, center, key_event, sqlite_connection},
};

const PEOPLE: u64 = 250;

fn fixture(config: &TestConfig) -> PathBuf {
    let dir = config.dir().join("data");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fixture.db");
    let mut db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT NOT NULL, note TEXT);
         CREATE TABLE blank (id INTEGER PRIMARY KEY, label TEXT NOT NULL DEFAULT 'x');
         CREATE INDEX blank_label ON blank(label);
         CREATE VIEW adults AS SELECT id, name FROM people WHERE id > 18;
         CREATE TABLE таблица (имя TEXT, значение TEXT);
         INSERT INTO таблица VALUES ('один', '日本語テキスト'), ('два', 'ü'), (NULL, NULL);",
    )
    .unwrap();
    let tx = db.transaction().unwrap();
    for id in 1..=PEOPLE {
        let note = match id {
            2 => Some("first line\n\tsecond line".to_string()),
            3 => Some("x".repeat(600)),
            4 => Some("y".repeat(70_000)),
            id if id % 10 == 0 => None,
            id => Some(format!("note {id}")),
        };
        tx.execute(
            "INSERT INTO people (id, name, note) VALUES (?1, ?2, ?3)",
            rusqlite::params![id as i64, format!("person {id}"), note],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    config.add_connection(sqlite_connection("fixture", &path));
    path
}

fn connect_fixture() -> (TestConfig, PathBuf, TestWorkspace) {
    let config = TestConfig::empty();
    let path = fixture(&config);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("fixture");
    (config, path, ws)
}

fn sql(path: &PathBuf, script: &str) {
    rusqlite::Connection::open(path)
        .unwrap()
        .execute_batch(script)
        .unwrap();
}

fn tree_lines(ws: &TestWorkspace) -> (Vec<String>, usize) {
    let model = &ws.tree().model;
    let lines = model
        .rows()
        .iter()
        .map(|row| match *row {
            TreeRow::Schema(i) => {
                let node = &model.nodes[i];
                format!("{} {}", if node.expanded { "▾" } else { "▸" }, node.name)
            }
            TreeRow::Table(i, j) => {
                let table = &model.nodes[i].tables.as_ref().unwrap()[j];
                match table.kind {
                    TableKind::Table => format!("    {}", table.name),
                    TableKind::View => format!("    {}  view", table.name),
                    TableKind::MaterializedView => format!("    {}  mview", table.name),
                }
            }
            TreeRow::Note(i) => {
                let node = &model.nodes[i];
                if node.error.is_some() {
                    "    failed".to_string()
                } else if node.tables.is_some() {
                    "    no tables".to_string()
                } else {
                    "    loading…".to_string()
                }
            }
        })
        .collect();
    (lines, model.cursor)
}

fn cursor_line(ws: &TestWorkspace) -> String {
    let (lines, cursor) = tree_lines(ws);
    lines.get(cursor).cloned().unwrap_or_default()
}

fn filter(ws: &TestWorkspace) -> String {
    ws.tree().model.filter().to_string()
}

fn status(ws: &TestWorkspace) -> Option<(String, bool)> {
    ws.browser().status.clone()
}

fn main(ws: &TestWorkspace) -> Main {
    ws.browser().main
}

fn focused_pane(ws: &TestWorkspace) -> Option<Pane> {
    let browser = ws.browser();
    let focused = browser.focused_pane(ws.focused().as_deref());
    if let Some(pane) = focused {
        assert_eq!(
            browser.pane, pane,
            "the remembered pane (F6) follows the focus"
        );
    }
    focused
}

fn grid_state(ws: &TestWorkspace) -> (u64, bool, usize, bool) {
    let grid = ws.table_view().grid.as_ref().expect("a table is open");
    (
        grid.offset(),
        grid.is_loading(),
        grid.row(),
        grid.record_open(),
    )
}

fn wait_page(ws: &mut TestWorkspace, offset: u64) {
    ws.wait_for_state(&format!("the page at offset {offset}"), move |s| {
        s.active_browser()
            .and_then(|b| b.table.grid.as_ref())
            .is_some_and(|grid| {
                !grid.is_loading() && grid.page().is_some_and(|page| page.offset == offset)
            })
    });
}

fn wait_total(ws: &mut TestWorkspace, total: u64) {
    ws.wait_for_state(&format!("the row count {total}"), move |s| {
        s.active_browser()
            .and_then(|b| b.table.grid.as_ref())
            .and_then(|grid| grid.total())
            == Some(total)
    });
}

fn press_unsettled(ws: &mut TestWorkspace, key: &str) {
    ws.rt.key(key_event(key));
    ws.rt.frame(0.0);
}

fn click_label_unsettled(ws: &mut TestWorkspace, label: &str) {
    let element = ws
        .by_label(label)
        .unwrap_or_else(|| panic!("no element labelled {label:?}\n{}", ws.describe()));
    let (x, y) = center(element.visible);
    ws.rt.pointer_move(x, y);
    ws.rt.pointer_down(x, y);
    ws.rt.pointer_up(x, y);
    ws.rt.frame(0.0);
}

fn click_row(ws: &mut TestWorkspace, row: usize) {
    ws.step();
    ws.click(&row_key(row));
}

#[test]
fn tree_lists_tables_and_views_and_folds_with_the_keyboard() {
    let (_config, _path, mut ws) = connect_fixture();

    let (lines, cursor) = tree_lines(&ws);
    assert_eq!(
        lines,
        [
            "▾ main",
            "    adults  view",
            "    blank",
            "    people",
            "    таблица"
        ]
    );
    assert_eq!(cursor, 0);
    assert_eq!(focused_pane(&ws), Some(Pane::Tree));

    ws.press("down down left");
    assert_eq!(cursor_line(&ws), "▾ main");
    ws.press("left");
    assert_eq!(tree_lines(&ws).0, ["▸ main"]);
    ws.press("enter");
    assert_eq!(tree_lines(&ws).0.len(), 5);
    let pending = ws.tree().model.nodes[0].request;
    assert_eq!(pending, None, "tables are cached");
    ws.press("right");
    assert_eq!(
        tree_lines(&ws).0.len(),
        5,
        "→ on an expanded schema keeps it open"
    );
    for _ in 0..8 {
        ws.press("down");
    }
    assert_eq!(cursor_line(&ws), "    таблица");
    ws.press("left");
    assert_eq!(
        cursor_line(&ws),
        "▾ main",
        "← on a table goes to its schema"
    );
    for _ in 0..3 {
        ws.press("up");
    }
    assert_eq!(tree_lines(&ws).1, 0);

    assert!(ws.exists("schema-tree"));
    click_row(&mut ws, 0);
    assert_eq!(tree_lines(&ws).0, ["▸ main"]);
    click_row(&mut ws, 0);
    assert_eq!(tree_lines(&ws).0.len(), 5);
    click_row(&mut ws, 3);
    assert_eq!(cursor_line(&ws), "    people");
    ws.wait_table_loaded("people");
    assert_eq!(focused_pane(&ws), Some(Pane::Tree));
}

#[test]
fn tree_search_filters_opens_and_clears() {
    let (_config, _path, mut ws) = connect_fixture();

    ws.dispatch(Command::FocusTree);
    ws.type_text(" ");
    ws.type_text("PEO");
    assert_eq!(filter(&ws), "PEO");
    assert_eq!(tree_lines(&ws).0, ["▾ main", "    people"]);
    assert_eq!(cursor_line(&ws), "    people");
    ws.press("enter");
    ws.wait_table_loaded("people");
    assert_eq!(focused_pane(&ws), Some(Pane::Main));
    assert_eq!(filter(&ws), "PEO", "the search persists when a table opens");

    ws.dispatch(Command::FocusTree);
    ws.press("escape");
    assert_eq!(filter(&ws), "");
    assert_eq!(cursor_line(&ws), "    people");
    assert_eq!(tree_lines(&ws).0.len(), 5);

    ws.type_text("l p");
    assert_eq!(
        tree_lines(&ws).0,
        ["▾ main", "    people"],
        "only people has l and p"
    );
    ws.press("backspace backspace");
    assert_eq!(filter(&ws), "l");
    assert_eq!(
        tree_lines(&ws).0,
        ["▾ main", "    adults  view", "    blank", "    people"]
    );
    ws.type_text("sqrojkhl");
    assert_eq!(filter(&ws), "lsqrojkhl");
    assert_eq!(tree_lines(&ws).0, Vec::<String>::new(), "No matches");
    ws.press("down enter left up");
    assert_eq!(
        ws.table_view().current(),
        Some(("main".into(), "people".into()))
    );
    ws.press("escape");
    assert_eq!(filter(&ws), "");
    ws.press("escape");
    assert_eq!(focused_pane(&ws), Some(Pane::Tree));
    assert_eq!(
        ws.read(|w| w.test_tabs().len()),
        1,
        "Esc never closes the tab"
    );

    ws.press("backspace");
    assert_eq!(filter(&ws), "");

    ws.type_text("табл");
    assert_eq!(tree_lines(&ws).0, ["▾ main", "    таблица"]);
    ws.press("backspace");
    assert_eq!(filter(&ws), "таб");
    ws.press("escape");
    ws.type_text(&"ы".repeat(200));
    assert!(tree_lines(&ws).0.is_empty());
    ws.press("escape");

    ws.type_text("p");
    ws.clip.set_text("eo\tp\r\nle");
    ws.press("cmd-v");
    assert_eq!(filter(&ws), "people");
    assert_eq!(tree_lines(&ws).0, ["▾ main", "    people"]);
}

#[test]
fn tree_search_follows_folding() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.dispatch(Command::FocusTree);
    ws.type_text("l");
    assert_eq!(
        tree_lines(&ws).0,
        ["▾ main", "    adults  view", "    blank", "    people"]
    );
    assert_eq!(
        cursor_line(&ws),
        "    adults  view",
        "the first match is selected"
    );
    ws.press("up left");
    assert_eq!(cursor_line(&ws), "▾ main");
    assert_eq!(
        tree_lines(&ws).0.len(),
        4,
        "← edits the search, it does not fold"
    );
    ws.dispatch(Command::FocusTree);
    ws.press("left");
    assert_eq!(tree_lines(&ws).0, ["▸ main"]);
    ws.press("right");
    assert_eq!(tree_lines(&ws).0.len(), 4);
    assert_eq!(filter(&ws), "l");
    ws.type_text("a");
    assert_eq!(filter(&ws), "la");
    assert_eq!(tree_lines(&ws).0, ["▾ main", "    blank"]);
}

#[test]
fn tree_refresh_keeps_expansion_and_the_cursor() {
    let (_config, path, mut ws) = connect_fixture();
    ws.dispatch(Command::FocusTree);
    ws.press("down down down");
    assert_eq!(cursor_line(&ws), "    people");

    sql(&path, "CREATE TABLE aaa (id INTEGER);");
    ws.press("cmd-r");
    ws.wait_tree_loaded();
    ws.wait_until("the new table", |_| true);
    ws.wait_for_state("aaa in the tree", |s| {
        s.active_browser().is_some_and(|b| {
            b.tree.model.nodes[0]
                .tables
                .as_ref()
                .is_some_and(|t| t.iter().any(|t| t.name == "aaa"))
        })
    });
    assert_eq!(tree_lines(&ws).0[1], "    aaa");
    assert_eq!(cursor_line(&ws), "    people");
    assert_eq!(status(&ws), None);

    sql(&path, "DROP TABLE aaa;");
    ws.click_label("Refresh schemas");
    ws.wait_for_state("aaa gone", |s| {
        s.active_browser().is_some_and(|b| {
            let model = &b.tree.model;
            model.schemas_request.is_none()
                && model.nodes[0].request.is_none()
                && model.nodes[0].tables.as_ref().is_some_and(|t| t.len() == 4)
        })
    });
    assert_eq!(cursor_line(&ws), "    people");
}

#[test]
fn tree_errors_are_reported() {
    let (_config, _path, mut ws) = connect_fixture();
    let tab = ws.tab_id();

    ws.dispatch(Command::FocusTree);
    press_unsettled(&mut ws, "cmd-r");
    let id = ws.tree().model.schemas_request.expect("a request");
    assert!(ws.update(move |s, _| tree::on_error(s.browser_mut(tab).unwrap(), id, "boom")));
    ws.settle();
    assert_eq!(
        status(&ws),
        Some(("Failed to refresh schemas: boom".into(), true))
    );
    assert_eq!(tree_lines(&ws).0.len(), 5);
    ws.press("down");
    assert_eq!(status(&ws), None);

    ws.press("up left");
    ws.update(move |s, _| {
        let node = &mut s.browser_mut(tab).unwrap().tree.model.nodes[0];
        node.tables = None;
        node.expanded = false;
    });
    press_unsettled(&mut ws, "enter");
    let id = ws.tree().model.nodes[0].request.expect("tables requested");
    assert!(ws.update(move |s, _| tree::on_error(s.browser_mut(tab).unwrap(), id, "nope")));
    ws.settle();
    assert_eq!(tree_lines(&ws).0, ["▾ main", "    failed"]);
    assert_eq!(
        status(&ws),
        Some(("Failed to list tables of main: nope".into(), true))
    );
    ws.press("down enter");
    ws.wait_tree_loaded();
    assert_eq!(tree_lines(&ws).0.len(), 5, "the retry listed the tables");
}

#[test]
fn refresh_behind_the_console_waits_for_the_table() {
    let (_config, path, mut ws) = connect_fixture();
    ws.open_table("people");
    ws.press("down down");

    ws.press("cmd-e");
    assert_eq!(main(&ws), Main::Console);
    sql(&path, "UPDATE people SET name = 'renamed' WHERE id = 1;");
    press_unsettled(&mut ws, "cmd-r");
    assert!(
        ws.tree().model.schemas_request.is_some(),
        "the tree reloads"
    );
    assert!(!grid_state(&ws).1, "the hidden table does not reload");
    assert!(ws.browser().table_stale);
    ws.wait_tree_loaded();

    click_label_unsettled(&mut ws, "Data");
    assert_eq!(main(&ws), Main::Table);
    assert!(grid_state(&ws).1, "the table reloads when it shows again");
    wait_page(&mut ws, 0);
    let name = ws.table_view().grid.as_ref().unwrap().page().unwrap().rows[0][1].clone();
    assert_eq!(name.as_deref(), Some("renamed"));
    assert_eq!(grid_state(&ws).2, 2);
    assert!(!ws.browser().table_stale);

    press_unsettled(&mut ws, "cmd-r");
    assert!(grid_state(&ws).1);
    assert!(ws.tree().model.schemas_request.is_some());
    wait_page(&mut ws, 0);

    ws.dispatch(Command::OpenConsole);
    ws.press("cmd-r");
    ws.dispatch(Command::FocusTree);
    press_unsettled(&mut ws, "enter");
    assert_eq!(main(&ws), Main::Table);
    assert!(grid_state(&ws).1);
    wait_page(&mut ws, 0);
}

#[test]
fn the_data_sql_switch_shows_which_side_is_active() {
    let (_config, _path, mut ws) = connect_fixture();
    let selected = |ws: &TestWorkspace| {
        let state = |label: &str| ws.by_label(label).map(|button| button.item_selected);
        (state("Data"), state("SQL"))
    };
    assert_eq!(selected(&ws), (Some(true), Some(false)));
    ws.click_label("SQL");
    assert_eq!(main(&ws), Main::Console);
    assert_eq!(selected(&ws), (Some(false), Some(true)));
    assert_eq!(
        focused_pane(&ws),
        Some(Pane::Main),
        "the console has the focus"
    );
    ws.click_label("Data");
    assert_eq!(main(&ws), Main::Table);
    assert_eq!(selected(&ws), (Some(true), Some(false)));
    ws.press("cmd-e");
    assert_eq!(selected(&ws), (Some(false), Some(true)));
    ws.open_table("people");
    assert_eq!(selected(&ws), (Some(true), Some(false)));
}

#[test]
fn f6_cycles_the_panes() {
    let (_config, _path, mut ws) = connect_fixture();
    assert_eq!(focused_pane(&ws), Some(Pane::Tree));
    ws.press("f6");
    assert_eq!(focused_pane(&ws), Some(Pane::Saved));
    ws.press("f6");
    assert_eq!(focused_pane(&ws), Some(Pane::Main));
    ws.press("f6");
    assert_eq!(focused_pane(&ws), Some(Pane::Tree));
    ws.press("shift-f6");
    assert_eq!(focused_pane(&ws), Some(Pane::Main));
    ws.dispatch(Command::FocusPrevPane);
    assert_eq!(focused_pane(&ws), Some(Pane::Saved));

    ws.open_table("people");
    assert_eq!(focused_pane(&ws), Some(Pane::Main));
    ws.press("space f6");
    assert_eq!(focused_pane(&ws), Some(Pane::Tree));
    ws.type_text("x");
    ws.dispatch(Command::FocusNextPane);
    assert_eq!(focused_pane(&ws), Some(Pane::Saved));
    ws.press("f6");
    assert_eq!(focused_pane(&ws), Some(Pane::Main));
    assert!(grid_state(&ws).3, "the row view stayed open");

    ws.press("cmd-e f6 f6 f6");
    assert_eq!(focused_pane(&ws), Some(Pane::Main));
    assert_eq!(main(&ws), Main::Console);
    ws.dispatch(Command::Refresh);
}

#[test]
fn an_idle_browser_is_not_woken() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.open_table("people");
    wait_total(&mut ws, PEOPLE);
    ws.settle();
    let drains = ws.browser().drains;
    let deadline = ws.rt.wake_deadline();
    for _ in 0..40 {
        ws.step();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(ws.browser().drains, drains, "woken while idle");
    assert_eq!(ws.rt.wake_deadline(), deadline, "a timer while idle");
    ws.press("cmd-]");
    wait_page(&mut ws, 100);
    assert!(ws.browser().drains > drains);
}

fn connect_with_wide_table() -> (TestConfig, PathBuf, TestWorkspace) {
    let config = TestConfig::empty();
    let path = fixture(&config);
    let columns: Vec<String> = (1..=80).map(|i| format!("  column_{i} TEXT")).collect();
    sql(
        &path,
        &format!("CREATE TABLE wide (\n{}\n);", columns.join(",\n")),
    );
    let mut ws = TestWorkspace::open(&config);
    ws.connect("fixture");
    ws.wait_tree_loaded();
    (config, path, ws)
}

fn tree_row(ws: &TestWorkspace, table: &str) -> usize {
    let line = format!("    {table}");
    let (lines, _) = tree_lines(ws);
    lines
        .iter()
        .position(|l| *l == line || l.starts_with(&format!("{line}  ")))
        .unwrap_or_else(|| panic!("no {table:?} in the tree: {lines:?}"))
}

fn right_click_tree_row(ws: &mut TestWorkspace, row: usize) {
    ws.right_click(&row_key(row));
}

fn ddl_menu_open(ws: &TestWorkspace) -> bool {
    ws.has_label("Show DDL")
}

fn dialog_sql(ws: &TestWorkspace) -> Option<String> {
    ws.browser()
        .ddl
        .as_ref()
        .and_then(|view| view.sql().map(str::to_string))
}

fn wait_dialog_sql(ws: &mut TestWorkspace) -> String {
    ws.wait_for_state("the DDL in the dialog", |s| {
        s.active_browser()
            .and_then(|b| b.ddl.as_ref())
            .is_some_and(|view| view.sql().is_some())
    });
    dialog_sql(ws).unwrap()
}

#[test]
fn a_tables_context_menu_shows_its_ddl_in_a_dialog() {
    let (_config, _path, mut ws) = connect_with_wide_table();

    let blank = tree_row(&ws, "blank");
    right_click_tree_row(&mut ws, blank);
    assert!(
        ddl_menu_open(&ws),
        "the right click opened the table's menu"
    );
    assert_eq!(cursor_line(&ws), "    blank");
    assert!(
        ws.table_view().current().is_none(),
        "the table is not opened"
    );

    ws.click_label("Show DDL");
    assert!(!ddl_menu_open(&ws));
    assert!(ws.has_dialog());
    let ddl = wait_dialog_sql(&mut ws);
    assert!(
        ddl.contains("CREATE TABLE blank") && ddl.contains("CREATE INDEX blank_label"),
        "{ddl}"
    );
    assert!(ws.table_view().current().is_none(), "still no table open");

    let has_label =
        |ws: &TestWorkspace, label: &str| ws.within_by_label("ddl-dialog-body", label).is_some();
    let wait_copied_reset = |ws: &mut TestWorkspace| {
        ws.rt.frame(2.0);
        ws.settle();
    };
    assert!(has_label(&ws, "Copy") && !has_label(&ws, "Copied"));
    ws.clip.set_text("");
    ws.press("cmd-c");
    assert_eq!(ws.clipboard().as_deref(), Some(ddl.as_str()));
    assert!(status(&ws).is_some_and(|(text, error)| text.starts_with("Copied") && !error));
    assert!(
        has_label(&ws, "Copied") && !has_label(&ws, "Copy"),
        "⌘C shows it too"
    );
    wait_copied_reset(&mut ws);
    assert!(
        has_label(&ws, "Copy") && !has_label(&ws, "Copied"),
        "and it goes back"
    );
    ws.clip.set_text("");
    ws.click_label("Copy");
    assert_eq!(ws.clipboard().as_deref(), Some(ddl.as_str()));
    assert!(has_label(&ws, "Copied"));
    ws.click_label("Copied");
    assert!(has_label(&ws, "Copied"), "copying again keeps it");
    wait_copied_reset(&mut ws);
    assert!(has_label(&ws, "Copy"));
    ws.press("escape");
    assert!(!ws.has_dialog());
    assert!(ws.browser().ddl.is_none(), "the view went with it");

    let adults = tree_row(&ws, "adults");
    right_click_tree_row(&mut ws, adults);
    ws.click_label("Show DDL");
    assert!(wait_dialog_sql(&mut ws).contains("CREATE VIEW adults"));
    ws.click_in("ddl-dialog-body", "Close");
    assert!(!ws.has_dialog());
}

#[test]
fn schema_rows_have_no_menu_and_cmd_d_shows_the_selected_tables_ddl() {
    let (_config, _path, mut ws) = connect_with_wide_table();
    right_click_tree_row(&mut ws, 0);
    assert!(!ddl_menu_open(&ws), "a schema has no menu");

    ws.dispatch(Command::FocusTree);
    assert_eq!(cursor_line(&ws), "▾ main");
    ws.press("cmd-d");
    assert!(!ws.has_dialog());
    let wide = tree_row(&ws, "wide");
    for _ in 0..wide {
        ws.press("down");
    }
    ws.press("cmd-d");
    assert!(ws.has_dialog());
    let ddl = wait_dialog_sql(&mut ws);
    assert!(ddl.contains("column_80"), "{ddl}");
}

#[test]
fn the_ddl_dialog_scrolls_with_the_keys() {
    let (_config, _path, mut ws) = connect_with_wide_table();
    let wide = tree_row(&ws, "wide");
    right_click_tree_row(&mut ws, wide);
    ws.click_label("Show DDL");
    wait_dialog_sql(&mut ws);
    ws.settle();
    let scroll = |ws: &TestWorkspace| {
        let view = ws.browser().ddl.as_ref().unwrap();
        (view.scroll.offset(), view.scroll.max())
    };
    let (top, max) = scroll(&ws);
    assert_eq!(top, 0.);
    assert!(
        max > 100.,
        "the definition is taller than the dialog ({max})"
    );

    ws.press("down");
    let line = scroll(&ws).0;
    assert!(line > 0., "↓ scrolls");
    ws.press("pagedown");
    assert!(scroll(&ws).0 > line + 50., "PgDn scrolls a page");
    ws.press("end");
    assert_eq!(scroll(&ws).0, max);
    ws.press("up");
    assert!(scroll(&ws).0 < max);
    ws.press("home");
    assert_eq!(scroll(&ws).0, 0.);
    ws.press("space");
    assert!(scroll(&ws).0 > 50., "Space pages down");
    assert!(ws.has_dialog(), "the keys do not close it");
}

#[test]
fn the_ddl_dialog_shows_why_it_failed() {
    let (_config, path, mut ws) = connect_with_wide_table();
    sql(&path, "DROP TABLE wide;");
    let wide = tree_row(&ws, "wide");
    right_click_tree_row(&mut ws, wide);
    ws.click_label("Show DDL");
    ws.wait_for_state("the error in the dialog", |s| {
        s.active_browser()
            .and_then(|b| b.ddl.as_ref())
            .is_some_and(|view| view.error().is_some())
    });
    assert!(ws.has_dialog());
    assert!(dialog_sql(&ws).is_none());
}

#[test]
fn escape_closes_the_tree_menu_before_it_cancels_the_script() {
    let (_config, _path, mut ws) = connect_with_wide_table();
    ws.press("cmd-e");
    ws.update(|s, _| {
        if let Some(b) = s.active_browser_mut() {
            b.console.sql.replace_text(
                "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r) SELECT count(*) FROM r",
            );
        }
    });
    ws.press("cmd-enter");
    assert!(ws.console().is_running());

    ws.press("cmd-p");
    assert_eq!(ws.browser().focused, Some(Pane::Tree));
    let blank = tree_row(&ws, "blank");
    right_click_tree_row(&mut ws, blank);
    assert!(ddl_menu_open(&ws));
    assert_eq!(
        ws.browser().focused,
        None,
        "the tree does not show the focused selection under its menu"
    );
    ws.press("escape");
    assert!(!ddl_menu_open(&ws), "the first Esc closes the menu");
    assert!(ws.console().is_running(), "and does not cancel the script");
    assert_eq!(focused_pane(&ws), Some(Pane::Tree));
    assert_eq!(ws.browser().focused, Some(Pane::Tree));

    press_unsettled(&mut ws, "escape");
    assert_eq!(
        ws.console().running_status().as_deref(),
        Some("Cancelling the script…"),
        "the next Esc cancels"
    );
    ws.wait_script_done();
}

#[test]
fn the_tree_header_belongs_to_the_tree_pane() {
    let (_config, _path, mut ws) = connect_with_wide_table();
    let wide = tree_row(&ws, "wide");
    for _ in 0..wide {
        ws.press("down");
    }
    ws.click("refresh-tree");
    ws.wait_tree_loaded();
    assert_eq!(focused_pane(&ws), Some(Pane::Tree));
    ws.press("f6");
    assert_eq!(focused_pane(&ws), Some(Pane::Saved));
    ws.press("shift-f6");
    ws.click("refresh-tree");
    ws.press("cmd-d");
    assert!(ws.has_dialog());
    assert!(wait_dialog_sql(&mut ws).contains("column_80"));
}

fn press_like_a_window(ws: &mut TestWorkspace, key: &str) {
    ws.rt.key(key_event(key));
    for _ in 0..10 {
        let animating = ws.rt.frame(crate::test_support::DT);
        if !animating && ws.rt.wake_deadline().is_none() {
            break;
        }
    }
}

#[test]
fn keyboard_focus_moves_show_without_another_event() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.open_table("people");
    assert_eq!(ws.browser().focused, Some(Pane::Main));
    press_like_a_window(&mut ws, "f6");
    assert!(ws.is_focused(tree::ROOT_KEY));
    assert_eq!(ws.browser().focused, Some(Pane::Tree));
    press_like_a_window(&mut ws, "f6");
    assert_eq!(ws.browser().focused, Some(Pane::Saved));
    press_like_a_window(&mut ws, "cmd-p");
    assert_eq!(ws.browser().focused, Some(Pane::Tree));
    press_like_a_window(&mut ws, "shift-f6");
    assert_eq!(ws.browser().focused, Some(Pane::Main));
}

fn two_people_tabs() -> (
    TestConfig,
    TestWorkspace,
    crate::workspace::TabId,
    crate::workspace::TabId,
) {
    let config = TestConfig::empty();
    let path = fixture(&config);
    config.add_connection(sqlite_connection("second", &path));
    let mut ws = TestWorkspace::open(&config);
    let first = ws.connect("fixture");
    ws.open_table("people");
    let second = ws.connect("second");
    ws.open_table("people");
    (config, ws, first, second)
}

fn grid_y(ws: &TestWorkspace, tab: crate::workspace::TabId) -> f32 {
    ws.read(|s| s.browser(tab).unwrap().table.cells.scroll.get().offset[1])
}

fn scroll_grid(ws: &mut TestWorkspace, y: f32) {
    ws.update(move |_, tx| tx.scroll_to_instant("table-grid-scroll", 0.0, y));
}

#[test]
fn every_tab_keeps_its_own_scroll_positions() {
    let (_config, mut ws, first, second) = two_people_tabs();
    scroll_grid(&mut ws, 1200.0);
    assert_eq!(grid_y(&ws, second), 1200.0);
    ws.press("cmd-2");
    assert_eq!(ws.tab_id(), first);
    assert_eq!(
        grid_y(&ws, first),
        0.0,
        "the first tab did not take the second's offset"
    );
    scroll_grid(&mut ws, 500.0);
    assert_eq!(grid_y(&ws, second), 1200.0);

    ws.press("cmd-3");
    assert_eq!(ws.tab_id(), second);
    assert_eq!(grid_y(&ws, second), 1200.0, "it came back where it was");
    ws.press("cmd-1 cmd-2");
    assert_eq!(grid_y(&ws, first), 500.0);

    let lines: String = (1..=300).map(|i| format!("select {i};\n")).collect();
    ws.press("cmd-e");
    ws.update(move |s, _| {
        s.active_browser_mut()
            .unwrap()
            .console
            .sql
            .replace_text(&lines)
    });
    ws.update(|_, tx| tx.scroll_to_instant("console-editor-scroll", 0.0, 2000.0));
    let editor_y = |ws: &TestWorkspace, tab| {
        ws.read(|s| s.browser(tab).unwrap().scrolls.get("console-editor-scroll"))
            .map(|[_, y]| y)
    };
    assert_eq!(editor_y(&ws, first), Some(2000.0));
    ws.press("cmd-3");
    ws.press("cmd-e");
    assert_eq!(editor_y(&ws, second), Some(0.0));
    ws.press("cmd-2");
    assert_eq!(editor_y(&ws, first), Some(2000.0));
}

#[test]
fn a_background_tab_does_not_scroll_or_focus_the_shown_one() {
    let (_config, mut ws, first, second) = two_people_tabs();
    scroll_grid(&mut ws, 1200.0);
    ws.press("cmd-2");
    assert_eq!(ws.tab_id(), first);
    ws.update(|s, tx| {
        assert!(s.run(Command::NextPage, tx));
        assert!(s.run(Command::ActivateTab(3), tx));
    });
    assert_eq!(ws.tab_id(), second);
    ws.wait_for_state("the first tab's next page", move |s| {
        s.browser(first)
            .and_then(|b| b.table.grid.as_ref())
            .is_some_and(|grid| {
                !grid.is_loading() && grid.page().is_some_and(|page| page.offset == 100)
            })
    });
    for _ in 0..10 {
        ws.step();
    }
    assert_eq!(grid_y(&ws, second), 1200.0, "the shown grid stayed put");
    assert!(ws.focus_within("table-view"));

    ws.press("cmd-2");
    assert_eq!(grid_y(&ws, first), 0.0, "the new page shows from its top");
}

#[test]
fn the_tree_menu_closes_when_anything_else_happens() {
    let (_config, _path, mut ws) = connect_with_wide_table();
    let blank = tree_row(&ws, "blank");
    let open_menu = |ws: &mut TestWorkspace| {
        right_click_tree_row(ws, blank);
        assert!(ddl_menu_open(ws));
    };

    open_menu(&mut ws);
    ws.press("cmd-1 cmd-2");
    assert!(!ddl_menu_open(&ws), "switching tabs closes it for good");
    assert!(ws.tree().menu.is_none());
    assert_eq!(focused_pane(&ws), Some(Pane::Tree));

    open_menu(&mut ws);
    ws.press("cmd-e");
    assert!(!ddl_menu_open(&ws));
    assert_eq!(main(&ws), Main::Console);
    assert_eq!(focused_pane(&ws), Some(Pane::Main));

    ws.press("cmd-p");
    open_menu(&mut ws);
    ws.press("cmd-o");
    assert!(!ddl_menu_open(&ws));
    assert_eq!(focused_pane(&ws), Some(Pane::Saved));

    ws.press("cmd-p");
    open_menu(&mut ws);
    ws.press("cmd-/");
    assert!(ws.has_dialog());
    ws.press("escape");
    assert!(!ws.has_dialog());
    assert!(
        !ddl_menu_open(&ws),
        "closing Help does not bring the menu back"
    );
    assert!(ws.tree().menu.is_none());
}

#[test]
fn a_click_into_a_field_clears_the_status() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.update(|s, _| {
        s.active_browser_mut()
            .unwrap()
            .set_status("Copied 3 characters", false)
    });
    ws.click_label("Filter tables");
    assert!(ws.tree().filter.focused);
    assert_eq!(status(&ws), None);

    ws.click("show-sql");
    ws.press("f6");
    assert!(!ws.console().sql.focused);
    ws.update(|s, _| s.active_browser_mut().unwrap().set_status("Failed", true));
    ws.click(crate::browser::console::SQL);
    assert!(ws.console().sql.focused);
    assert_eq!(status(&ws), None);

    ws.update(|s, tx| {
        let b = s.active_browser_mut().unwrap();
        b.tree.filter.focused = false;
        tx.focus("schema-tree", false);
    });
    ws.update(|s, _| {
        let b = s.active_browser_mut().unwrap();
        b.tree.filter.focused = true;
        b.set_status("Set with the focus", false);
    });
    assert_eq!(
        status(&ws),
        Some(("Set with the focus".to_string(), false)),
        "a status set by the same change stays"
    );
}

#[test]
fn cmd_backspace_in_the_saved_search_edits_the_text() {
    let config = TestConfig::empty();
    fixture(&config);
    config
        .store()
        .queries()
        .save(
            "fixture",
            &crate::queries::SavedQuery {
                name: "orders report".to_string(),
                sql: "select 1".to_string(),
            },
        )
        .unwrap();
    let mut ws = TestWorkspace::open(&config);
    ws.connect("fixture");
    ws.press("cmd-o");
    ws.type_text("ord");
    assert!(ws.browser().saved.filter.focused);
    ws.press("cmd-backspace");
    assert!(!ws.has_dialog(), "{:?}", ws.dialog_label());
    assert_eq!(ws.browser().saved.filter.text, "");
    ws.press("enter space");
    assert_eq!(ws.browser().saved.names(), ["orders report"]);

    ws.press("cmd-o");
    assert!(!ws.browser().saved.filter.focused);
    ws.press("cmd-backspace");
    assert!(ws.has_dialog(), "the list still asks to delete");
    ws.press("escape");
    assert_eq!(ws.browser().saved.names(), ["orders report"]);
}

#[test]
fn smoke_hooks_open_a_table_and_run_the_draft() {
    let (_config, _path, mut ws) = connect_fixture();
    let tab = ws.tab_id();
    ws.update(move |s, tx| super::smoke_with(s, tab, Some("main.people"), false, tx));
    ws.wait_table_loaded("people");
    assert_eq!(main(&ws), Main::Table);

    ws.update(|s, _| {
        s.active_browser_mut()
            .unwrap()
            .console
            .sql
            .replace_text("select 41 + 1")
    });
    ws.update(move |s, tx| super::smoke_with(s, tab, None, true, tx));
    assert_eq!(main(&ws), Main::Console);
    let done = ws.wait_script_done();
    assert!(done.starts_with("1 row"), "{done}");
}

#[test]
#[ignore = "renders PNG snapshots into $COOPER_BROWSER_SNAPS"]
fn browser_snapshots() {
    use crate::test_support::snap_png;
    let Some(out) = std::env::var_os("COOPER_BROWSER_SNAPS").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let mode = if dark { "dark" } else { "light" };
        let config = TestConfig::empty();
        let path = fixture(&config);
        let columns: Vec<String> = (1..=80).map(|i| format!("  column_{i} TEXT")).collect();
        sql(
            &path,
            &format!("CREATE TABLE wide (\n{}\n);", columns.join(",\n")),
        );
        for i in 0..60 {
            sql(
                &path,
                &format!("CREATE TABLE extra_table_with_a_rather_long_name_{i:02} (id INTEGER);"),
            );
        }
        let mut ws = TestWorkspace::open_with(config.store(), dark);
        ws.rt.set_viewport((1280.0, 820.0), 2.0);
        ws.settle();
        ws.connect("fixture");
        ws.wait_tree_loaded();
        ws.press("down down");
        ws.settle();
        snap_png(&mut ws.rt, &out.join(format!("{mode}-01-tree.png")));
        ws.type_text("l");
        ws.settle();
        snap_png(&mut ws.rt, &out.join(format!("{mode}-02-filter.png")));
        ws.press("escape escape");
        ws.press("down enter");
        ws.wait_table_loaded("blank");
        snap_png(
            &mut ws.rt,
            &out.join(format!("{mode}-03-table-focused.png")),
        );
        ws.dispatch(Command::FocusTree);
        snap_png(&mut ws.rt, &out.join(format!("{mode}-03-open-table.png")));
        let wide = tree_row(&ws, "wide");
        let tab = ws.tab_id();
        ws.update(move |s, _| {
            let b = s.browser_mut(tab).unwrap();
            b.tree.model.cursor = wide;
            b.tree.reveal = true;
        });
        ws.settle();
        right_click_tree_row(&mut ws, wide);
        snap_png(&mut ws.rt, &out.join(format!("{mode}-04-menu.png")));
        ws.click_label("Show DDL");
        wait_dialog_sql(&mut ws);
        ws.settle();
        snap_png(&mut ws.rt, &out.join(format!("{mode}-05-ddl.png")));
        ws.press("cmd-c");
        ws.press("pagedown");
        snap_png(&mut ws.rt, &out.join(format!("{mode}-06-ddl-copied.png")));
        ws.press("escape");
        ws.dispatch(Command::FocusTree);
        press_unsettled(&mut ws, "cmd-r");
        let id = ws.tree().model.schemas_request.unwrap();
        ws.update(move |s, _| {
            tree::on_error(
                s.browser_mut(tab).unwrap(),
                id,
                "boom: the server went away while listing schemas",
            )
        });
        ws.settle();
        snap_png(&mut ws.rt, &out.join(format!("{mode}-07-status-error.png")));
        ws.update(move |s, _| {
            let b = s.browser_mut(tab).unwrap();
            b.tree.model.schemas_loaded = false;
            b.tree.model.schemas_error = Some("permission denied for schema main".into());
        });
        snap_png(
            &mut ws.rt,
            &out.join(format!("{mode}-08-schemas-error.png")),
        );
        ws.update(move |s, _| {
            let b = s.browser_mut(tab).unwrap();
            b.tree.model.schemas_error = None;
            b.tree.model.schemas_loaded = true;
            b.tree.model.nodes[0].tables = None;
            b.tree.model.nodes[0].error = Some("x".into());
        });
        snap_png(&mut ws.rt, &out.join(format!("{mode}-09-note-failed.png")));
        ws.press("cmd-e");
        snap_png(&mut ws.rt, &out.join(format!("{mode}-10-sql.png")));
    }
}
