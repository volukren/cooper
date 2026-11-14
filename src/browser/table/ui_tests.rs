use std::path::{Path, PathBuf};

use rune::PointerButton;

use crate::{
    browser::{
        Browser, Pane,
        table::{self, TableView, grid::Grid},
        tree,
    },
    db::TableKind,
    dialog::Dialog,
    keys::Command,
    test_support::{TestConfig, TestWorkspace, snap_png, sqlite_connection},
    workspace::Workspace,
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

fn sql(path: &Path, script: &str) {
    rusqlite::Connection::open(path)
        .unwrap()
        .execute_batch(script)
        .unwrap();
}

fn held(ws: &mut TestWorkspace, f: impl FnOnce(&mut TestWorkspace)) {
    let poll = ws.rt.poll.take();
    f(ws);
    ws.rt.poll = poll;
}

fn browser_mut(s: &mut Workspace) -> &mut Browser {
    s.active_browser_mut().expect("a connected browser")
}

fn status(ws: &TestWorkspace) -> Option<(String, bool)> {
    ws.browser().status.clone()
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

fn view(ws: &TestWorkspace) -> &TableView {
    ws.table_view()
}

fn grid(ws: &TestWorkspace) -> &Grid {
    view(ws).grid.as_ref().expect("a table is open")
}

fn footer(ws: &TestWorkspace) -> String {
    view(ws).footer().unwrap_or_default()
}

fn grid_state(ws: &TestWorkspace) -> (u64, bool, usize, bool) {
    let grid = grid(ws);
    (
        grid.offset(),
        grid.is_loading(),
        grid.row(),
        grid.record_open(),
    )
}

fn selected_cell(ws: &TestWorkspace) -> Option<(usize, Option<usize>)> {
    view(ws).selected_cell()
}

fn grid_focused(ws: &TestWorkspace) -> bool {
    ws.is_focused("table-view") && view(ws).grid_shown()
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

fn wait_view(ws: &mut TestWorkspace, what: &str, done: impl Fn(&TableView) -> bool) {
    ws.wait_for_state(what, move |s| {
        s.active_browser().is_some_and(|b| done(&b.table))
    });
}

#[test]
fn opening_a_table_shows_the_page_and_the_count() {
    let (_config, _path, mut ws) = connect_fixture();

    ws.press("shift-f6");
    assert_eq!(view(&ws).current(), None);

    ws.dispatch(Command::FocusTree);
    ws.type_text("people");
    held(&mut ws, |ws| ws.press("enter"));
    let (page, count, ddl) = grid(&ws).test_requests();
    assert!(
        page.is_some() && count.is_some(),
        "a page and a count are requested"
    );
    assert_eq!(ddl, None, "the DDL waits for the row view");
    assert_eq!(footer(&ws), "", "nothing is shown yet");
    wait_page(&mut ws, 0);
    wait_total(&mut ws, PEOPLE);
    assert_eq!(footer(&ws), "rows 1–100 of 250 · page 1/3");
    {
        let grid = grid(&ws);
        let page = grid.page().unwrap();
        let columns: Vec<_> = page
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.type_name.as_str()))
            .collect();
        assert_eq!(
            columns,
            [("id", "integer"), ("name", "text"), ("note", "text")]
        );
        assert_eq!(page.rows[0][1].as_deref(), Some("person 1"));
        assert_eq!(page.rows[9][2], None, "NULL");
        assert_eq!(grid.kind(), TableKind::Table);
    }
    assert_eq!(ws.tree().open_table, Some(("main".into(), "people".into())));
    assert!(ws.is_visible("table-view"));

    ws.press("down down down");
    assert_eq!(grid_state(&ws).2, 3);
    ws.dispatch(Command::FocusTree);
    ws.press("enter");
    let (offset, loading, row, _) = grid_state(&ws);
    assert_eq!((offset, loading, row), (0, false, 3), "no reload");
}

#[test]
fn paging_with_keys_and_buttons() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.open_table("people");
    wait_total(&mut ws, PEOPLE);

    ws.press("cmd-[");
    assert_eq!(
        status(&ws),
        Some(("Already on the first page".into(), false))
    );
    held(&mut ws, |ws| ws.press("cmd-]"));
    assert_eq!(status(&ws), None, "a key press clears the message");
    let (offset, loading, ..) = grid_state(&ws);
    assert_eq!((offset, loading), (100, true));
    assert_eq!(
        footer(&ws),
        "rows 1–100 of 250 · page 1/3",
        "the old page stays while loading"
    );
    wait_page(&mut ws, 100);
    assert_eq!(footer(&ws), "rows 101–200 of 250 · page 2/3");

    ws.click_label("Next page");
    wait_page(&mut ws, 200);
    assert_eq!(footer(&ws), "rows 201–250 of 250 · page 3/3");
    ws.press("cmd-]");
    assert_eq!(
        status(&ws),
        Some(("Already on the last page".into(), false))
    );
    assert!(!grid_state(&ws).1);

    ws.click_label("Previous page");
    wait_page(&mut ws, 100);
    ws.press("cmd-alt-[");
    wait_page(&mut ws, 0);
    ws.press("cmd-alt-]");
    wait_page(&mut ws, 200);
    ws.press("cmd-alt-]");
    assert_eq!(grid_state(&ws).2, 49);
    ws.click_label("First page");
    wait_page(&mut ws, 0);
    ws.press("down down");
    ws.press("cmd-alt-[");
    assert_eq!(grid_state(&ws).2, 0);
    ws.click_label("Last page");
    wait_page(&mut ws, 200);
}

#[test]
fn next_and_last_wait_for_what_they_need() {
    let (_config, _path, mut ws) = connect_fixture();

    ws.dispatch(Command::FocusTree);
    ws.type_text("people");
    held(&mut ws, |ws| {
        ws.press("enter");
        ws.press("cmd-]");
    });
    assert_eq!(grid_state(&ws).0, 0);

    held(&mut ws, |ws| ws.press("cmd-alt-]"));
    assert_eq!(
        status(&ws),
        Some(("Counting rows to find the last page…".into(), false))
    );
    assert!(grid(&ws).is_counting_for_last());
    wait_page(&mut ws, 200);
    assert_eq!(status(&ws), None, "the hint is gone");
    assert_eq!(footer(&ws), "rows 201–250 of 250 · page 3/3");
}

#[test]
fn the_awaited_count_takes_back_only_its_own_hint() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.dispatch(Command::FocusTree);
    ws.type_text("people");
    held(&mut ws, |ws| {
        ws.press("enter");
        ws.press("cmd-alt-]");
        assert_eq!(
            status(ws),
            Some(("Counting rows to find the last page…".into(), false))
        );
        ws.update(|s, _| tree::refresh(browser_mut(s)));
        let id = ws.tree().model.schemas_request.unwrap();
        ws.update(move |s, _| assert!(tree::on_error(browser_mut(s), id, "boom")));
    });
    ws.settle();
    wait_page(&mut ws, 200);
    assert_eq!(
        status(&ws),
        Some(("Failed to refresh schemas: boom".into(), true))
    );
}

#[test]
fn a_count_error_is_shown_and_last_retries_it() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.dispatch(Command::FocusTree);
    ws.type_text("people");
    held(&mut ws, |ws| ws.press("enter"));
    let count = grid(&ws).test_requests().1.unwrap();
    held(&mut ws, |ws| {
        ws.press("cmd-alt-]");
        ws.update(move |s, _| assert!(table::on_error(browser_mut(s), count, "too slow")));
    });
    ws.settle();
    assert_eq!(
        status(&ws),
        Some(("Failed to count rows: too slow".into(), true))
    );
    wait_page(&mut ws, 0);
    for _ in 0..5 {
        ws.step();
    }
    assert_eq!(footer(&ws), "rows 1–100 of ? · page 1/?");
    assert_eq!(grid_state(&ws).0, 0, "the failed count did not jump");

    held(&mut ws, |ws| ws.press("cmd-alt-]"));
    let again = grid(&ws).test_requests().1;
    assert!(again.is_some_and(|id| id != count));
    wait_page(&mut ws, 200);
    assert_eq!(footer(&ws), "rows 201–250 of 250 · page 3/3");
}

#[test]
fn a_small_table_knows_its_total_and_stale_answers_are_ignored() {
    let config = TestConfig::with_sample_dbs(&["analytics"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("analytics");

    held(&mut ws, |ws| {
        ws.dispatch(Command::FocusTree);
        ws.type_text("events");
        ws.press("enter");
        ws.dispatch(Command::FocusTree);
        ws.press("escape");
        ws.type_text("users");
        ws.press("enter");
    });
    ws.wait_table_loaded("users");
    wait_total(&mut ws, 10);
    for _ in 0..5 {
        ws.step();
    }
    {
        let grid = grid(&ws);
        assert_eq!(grid.table(), "users");
        assert_eq!(grid.page().unwrap().rows.len(), 10);
        assert_eq!(grid.total(), Some(10));
    }
    assert_eq!(footer(&ws), "rows 1–10 of 10 · page 1/1");
    ws.press("cmd-alt-]");
    assert_eq!(grid_state(&ws).2, 9);
    assert!(!grid_state(&ws).1, "no page request");
    ws.press("cmd-alt-[");
    assert_eq!(grid_state(&ws).2, 0);

    ws.dispatch(Command::FocusTree);
    ws.press("escape");
    ws.type_text("events");
    ws.press("enter");
    ws.wait_table_loaded("events");
    wait_total(&mut ws, 3000);
    assert_eq!(footer(&ws), "rows 1–100 of 3,000 · page 1/30");
    ws.press("cmd-alt-]");
    wait_page(&mut ws, 2900);
    assert_eq!(footer(&ws), "rows 2,901–3,000 of 3,000 · page 30/30");
}

#[test]
fn a_page_error_offers_a_retry() {
    let (_config, path, mut ws) = connect_fixture();
    ws.open_table("people");
    ws.press("space");
    assert!(grid_state(&ws).3, "the row view is open");

    sql(&path, "ALTER TABLE people RENAME TO folks;");
    ws.press("cmd-r");
    wait_view(&mut ws, "the page error", |v| {
        v.grid.as_ref().is_some_and(|g| g.error().is_some())
    });
    let error = grid(&ws).error().map(str::to_string);
    assert!(
        error.as_deref().is_some_and(|e| e.contains("people")),
        "{error:?}"
    );
    assert!(!grid_state(&ws).3, "a page error closes the row view");
    ws.press("space");
    assert!(!grid_state(&ws).3);

    sql(&path, "ALTER TABLE folks RENAME TO people;");
    ws.click_label("Retry");
    wait_page(&mut ws, 0);
    assert!(grid(&ws).error().is_none());
    assert_eq!(status(&ws), None);
    assert!(grid_focused(&ws), "{}", ws.describe());

    sql(&path, "ALTER TABLE people RENAME TO folks;");
    ws.press("cmd-r");
    wait_view(&mut ws, "the page error again", |v| {
        v.grid.as_ref().is_some_and(|g| g.error().is_some())
    });
    assert_eq!(focused_pane(&ws), Some(Pane::Main));
    sql(&path, "ALTER TABLE folks RENAME TO people;");
    ws.press("cmd-r");
    wait_page(&mut ws, 0);
    assert!(grid(&ws).error().is_none());
    assert!(grid_focused(&ws), "the grid gets the focus back");
}

#[test]
fn an_empty_table_shows_its_columns_without_a_definition() {
    let (_config, path, mut ws) = connect_fixture();

    ws.open_table("blank");
    assert_eq!(footer(&ws), "no rows · page 1/1");
    let (_, _, ddl_request) = grid(&ws).test_requests();
    assert_eq!(ddl_request, None, "an empty table does not fetch its DDL");
    ws.press("down up right left cmd-] cmd-[ cmd-alt-] cmd-alt-[ home end pagedown space cmd-d");
    let (offset, _, row, record) = grid_state(&ws);
    assert_eq!(
        (offset, row, record),
        (0, 0, false),
        "no row view without rows"
    );
    assert!(!ws.has_dialog(), "⌘D in the table opens nothing");
    assert!(ws.exists("table-grid-empty"));
    assert!(
        !ws.exists("table-grid-gutter"),
        "no row-number column beside 'No rows'"
    );
    let empty = ws.find("table-grid-empty").unwrap().visible;
    let grid_rect = ws.find("table-grid").unwrap().visible;
    assert_eq!(empty[0], grid_rect[0], "'No rows' spans the whole body");

    sql(&path, "INSERT INTO blank (label) VALUES ('a'), ('b');");
    ws.press("cmd-r");
    wait_view(&mut ws, "the rows", |v| {
        v.grid
            .as_ref()
            .and_then(Grid::page)
            .is_some_and(|p| p.rows.len() == 2)
    });
    let (_, _, ddl_request) = grid(&ws).test_requests();
    assert_eq!(ddl_request, None);
    assert!(ws.exists("table-grid-gutter"));
}

#[test]
fn the_row_view_shows_full_values_and_steps_through_rows() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.open_table("people");

    ws.press("down");
    ws.press("space");
    assert!(grid_state(&ws).3);
    assert_eq!(footer(&ws), "row 2");
    wait_view(&mut ws, "the DDL", |v| {
        v.grid.as_ref().is_some_and(|g| g.ddl().is_some())
    });
    let definition = grid(&ws)
        .ddl()
        .unwrap()
        .columns
        .iter()
        .find(|c| c.name == "id")
        .map(|c| c.definition.clone());
    assert_eq!(definition.as_deref(), Some("INTEGER PRIMARY KEY"));

    let (shown, full) = {
        let page = grid(&ws).page().unwrap();
        (
            page.rows[1][2].clone(),
            page.full.get(&(1, 2)).map(|f| f.text.clone()),
        )
    };
    assert_eq!(shown.as_deref(), Some("first line↵→second line"));
    assert_eq!(full.as_deref(), Some("first line\n\tsecond line"));

    ws.press("right right");
    assert_eq!(footer(&ws), "row 4");
    ws.press("alt-up alt-up alt-up alt-up");
    assert_eq!(footer(&ws), "row 1");
    ws.press("left");
    assert_eq!(footer(&ws), "row 1", "clamped at the first row");
    ws.press("alt-down down up pagedown pageup end home");
    assert_eq!(footer(&ws), "row 2");

    ws.press("cmd-]");
    assert_eq!(grid_state(&ws).0, 0);

    ws.click_label("Next row");
    assert_eq!(footer(&ws), "row 3");
    ws.click_label("Previous row");
    assert_eq!(footer(&ws), "row 2");

    ws.press("escape");
    let (_, _, row, record) = grid_state(&ws);
    assert_eq!((row, record), (1, false));
    assert_eq!(selected_cell(&ws).map(|c| c.0), Some(1));
    assert!(grid_focused(&ws));
    assert_eq!(footer(&ws), "rows 1–100 of 250 · page 1/3");

    ws.press("enter");
    assert!(grid_state(&ws).3, "Enter opens the row view");
    ws.press("enter");
    assert!(!grid_state(&ws).3, "Enter closes it");

    ws.press("space");
    ws.click_label("Back to the table");
    assert!(!grid_state(&ws).3);

    ws.press("space");
    assert!(ws.exists("table-title"));
    ws.click_label("Back");
    assert!(!grid_state(&ws).3, "the title's back arrow");
    ws.press("space");
    ws.click("table-title");
    assert!(!grid_state(&ws).3, "the table name in the title");

    ws.press("space");
    ws.press("cmd-[");
    let (offset, _, row, record) = grid_state(&ws);
    assert_eq!((offset, row, record), (0, 1, false));

    ws.press("space");
    ws.button_click("table-title", PointerButton::Back);
    assert!(!grid_state(&ws).3, "the mouse's back button");

    let truncated = grid(&ws)
        .page()
        .unwrap()
        .full
        .get(&(3, 2))
        .map(|f| f.truncated);
    assert_eq!(truncated, Some(true));
    ws.press("down down");
    ws.press("space");
    assert_eq!(footer(&ws), "row 4");
}

#[test]
fn the_grid_follows_the_selection_and_the_mouse() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.open_table("people");

    for _ in 0..30 {
        ws.press("down");
    }
    assert_eq!(grid_state(&ws).2, 30);
    assert!(ws.is_visible("table-row-30"), "the selected row is in view");
    assert!(!ws.exists("table-row-0") || !ws.is_visible("table-row-0"));
    ws.press("pagedown pagedown pagedown pagedown pagedown pagedown down down");
    assert_eq!(grid_state(&ws).2, 99, "clamped to the page");
    assert_eq!(grid_state(&ws).0, 0, "↓ does not turn the page");
    ws.press("cmd-alt-[");
    assert_eq!(grid_state(&ws).2, 0);

    ws.click("table-row-2");
    assert_eq!(grid_state(&ws).2, 2);
    ws.double_click("table-row-4");
    let (_, _, row, record) = grid_state(&ws);
    assert_eq!((row, record), (4, true));
    assert_eq!(footer(&ws), "row 5");
    assert_eq!(focused_pane(&ws), Some(Pane::Main));
    ws.press("escape");
    assert_eq!(grid_state(&ws).2, 4);
}

#[test]
fn the_row_view_survives_a_refresh_and_closes_on_another_page() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.open_table("people");
    wait_total(&mut ws, PEOPLE);
    ws.press("down down space");
    assert!(grid_state(&ws).3);

    ws.press("cmd-r");
    wait_page(&mut ws, 0);
    let (_, _, row, record) = grid_state(&ws);
    assert_eq!((row, record), (2, true));

    ws.press("escape cmd-]");
    wait_page(&mut ws, 100);
    ws.press("space");
    assert!(grid_state(&ws).3);
    assert_eq!(footer(&ws), "row 101");
    ws.press("escape cmd-alt-[");
    wait_page(&mut ws, 0);
    assert!(!grid_state(&ws).3);
}

#[test]
fn copying_cells_and_rows_uses_full_values() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.open_table("people");

    assert_eq!(selected_cell(&ws), Some((0, Some(0))));
    ws.press("cmd-c");
    assert_eq!(ws.clipboard().as_deref(), Some("1"));
    assert_eq!(status(&ws), Some(("Copied 1 character".into(), false)));

    ws.press("down right right");
    assert_eq!(selected_cell(&ws), Some((1, Some(2))));
    ws.press("cmd-c");
    assert_eq!(ws.clipboard().as_deref(), Some("first line\n\tsecond line"));
    assert_eq!(status(&ws), Some(("Copied 2 lines".into(), false)));

    for _ in 0..8 {
        ws.press("down");
    }
    ws.press("cmd-shift-c");
    assert_eq!(ws.clipboard().as_deref(), Some("10\tperson 10\tNULL"));
    ws.press("cmd-c");
    assert_eq!(ws.clipboard().as_deref(), Some("NULL"));

    ws.press("up up up up up up up");
    ws.press("cmd-c");
    assert_eq!(ws.clipboard().map(|t| t.len()), Some(600));

    ws.press("space cmd-shift-c");
    assert_eq!(
        ws.clipboard().map(|t| t.starts_with("3\tperson 3\txxx")),
        Some(true)
    );
}

#[test]
fn views_are_marked_in_the_title_and_unicode_tables_open() {
    let (_config, _path, mut ws) = connect_fixture();

    ws.open_table("adults");
    {
        let grid = grid(&ws);
        assert_eq!(grid.kind(), TableKind::View);
        assert_eq!(table::format::kind_label(grid.kind()), Some("view"));
    }
    wait_total(&mut ws, PEOPLE - 18);
    assert_eq!(footer(&ws), "rows 1–100 of 232 · page 1/3");
    assert_eq!(ws.tree().open_table, Some(("main".into(), "adults".into())));

    ws.dispatch(Command::FocusTree);
    ws.press("escape");
    ws.type_text("таб");
    ws.press("enter");
    ws.wait_table_loaded("таблица");
    assert_eq!(
        ws.tree().open_table,
        Some(("main".into(), "таблица".into()))
    );
    {
        let page = grid(&ws).page().unwrap();
        assert_eq!(page.columns[0].name, "имя");
        assert_eq!(page.rows[0][1].as_deref(), Some("日本語テキスト"));
        assert_eq!(page.rows[2][0], None);
        assert_eq!(grid(&ws).kind(), TableKind::Table);
    }

    ws.update(|s, _| table::open(browser_mut(s), "main", "people", TableKind::Table));
    assert_eq!(ws.tree().open_table, Some(("main".into(), "people".into())));
    ws.wait_table_loaded("people");

    ws.resize((320.0, 200.0));
    ws.press("down down space");
    ws.resize((120.0, 90.0));
    ws.press("right escape");
    ws.resize((1280.0, 820.0));
}

#[test]
fn escape_in_the_grid_keeps_the_selection() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.open_table("people");
    ws.press("down down escape");
    assert_eq!(selected_cell(&ws), Some((2, Some(0))));
    ws.press("down");
    assert_eq!(grid_state(&ws).2, 3, "↓ still moves");
    assert_eq!(selected_cell(&ws), Some((3, Some(0))));
    assert_eq!(view(&ws).highlighted_row(), Some(3));
    assert!(grid_focused(&ws));
    ws.press("cmd-c");
    assert_eq!(ws.clipboard().as_deref(), Some("4"));
    assert_eq!(status(&ws), Some(("Copied 1 character".into(), false)));

    ws.update(|s, _| table::test_clear_kit_selection(browser_mut(s)));
    assert_eq!(selected_cell(&ws), Some((3, Some(0))));
    assert_eq!(view(&ws).highlighted_row(), Some(3));
    ws.press("space");
    assert_eq!(footer(&ws), "row 4");
}

#[test]
fn the_row_view_footer_follows_the_displayed_page() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.open_table("people");
    wait_total(&mut ws, PEOPLE);
    held(&mut ws, |ws| ws.press("cmd-] down down space"));
    let (offset, loading, row, record) = grid_state(&ws);
    assert_eq!((offset, loading, row, record), (100, true, 2, true));
    assert_eq!(
        footer(&ws),
        "row 3",
        "the displayed page's row, not the requested one's"
    );
    wait_page(&mut ws, 100);
    assert!(!grid_state(&ws).3);
    assert_eq!(footer(&ws), "rows 101–200 of 250 · page 2/3");
}

fn field_text(ws: &TestWorkspace, index: usize) -> Option<String> {
    view(ws).draft()?.field(index).map(|f| f.input.text.clone())
}

fn focused_field(ws: &TestWorkspace) -> Option<usize> {
    view(ws).draft()?.focused()
}

fn has_draft(ws: &TestWorkspace) -> bool {
    view(ws).draft().is_some()
}

fn draft_error(ws: &TestWorkspace) -> Option<String> {
    view(ws).draft()?.error.clone()
}

fn set_null(ws: &mut TestWorkspace, index: usize, null: bool) {
    ws.update(move |s, _| table::set_null(browser_mut(s), index, null));
}

fn click_beside(ws: &mut TestWorkspace, label: &str) {
    let element = ws
        .by_label(label)
        .unwrap_or_else(|| panic!("no element labelled {label:?}\n{}", ws.describe()));
    let rect = element.visible;
    ws.click_at(rect[0] - 30.0, rect[1] + rect[3] / 2.0);
}

fn open_field(ws: &TestWorkspace) -> Option<usize> {
    view(ws).draft()?.open_field()
}

fn pane_focused(ws: &TestWorkspace) -> bool {
    ws.is_focused("table-view")
}

fn person(path: &Path, id: i64) -> Option<(String, Option<String>)> {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row("SELECT name, note FROM people WHERE id = ?1", [id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .ok()
}

#[test]
fn a_click_on_a_value_edits_it_in_place_and_saves_only_what_changed() {
    let (_config, path, mut ws) = connect_fixture();
    ws.open_table("people");

    ws.press("down space");
    assert!(!has_draft(&ws));
    ws.click_label("Edit name");
    assert!(!ws.has_dialog(), "no dialog: the value is edited in place");
    assert_eq!(field_text(&ws, 1).as_deref(), Some("person 2"));
    assert_eq!(
        field_text(&ws, 2),
        None,
        "only the clicked value has a field"
    );
    assert_eq!(focused_field(&ws), Some(1));
    assert!(has_draft(&ws));

    ws.press("tab");
    assert_eq!(focused_field(&ws), Some(2));
    assert_eq!(
        field_text(&ws, 2).as_deref(),
        Some("first line\n\tsecond line")
    );
    ws.press("tab");
    assert_eq!(focused_field(&ws), Some(0), "round");
    ws.press("shift-tab");
    assert_eq!(focused_field(&ws), Some(2));
    assert_eq!(
        field_text(&ws, 2).as_deref(),
        Some("first line\n\tsecond line"),
        "no tab typed"
    );
    ws.press("shift-tab");
    assert_eq!(focused_field(&ws), Some(1));

    ws.replace_text("it's");
    ws.press("space enter");
    ws.type_text("renamed");
    assert!(
        grid_state(&ws).3,
        "Space and Enter were typed, the row view stays"
    );
    ws.press("tab");
    assert_eq!(
        field_text(&ws, 1).as_deref(),
        Some("it's \nrenamed"),
        "the change stays"
    );
    set_null(&mut ws, 2, true);
    assert_eq!(
        field_text(&ws, 2).as_deref(),
        Some(""),
        "a NULL field is empty"
    );
    let update = view(&ws).draft().unwrap().update().unwrap();
    assert_eq!(update.key, [("id".to_string(), Some("2".to_string()))]);
    let changed: Vec<&str> = update.values.iter().map(|(c, _)| c.as_str()).collect();
    assert_eq!(
        changed,
        ["name", "note"],
        "the untouched key column is not sent"
    );

    ws.click_label("Next row");
    assert_eq!(grid_state(&ws).2, 1, "still on row 2");
    assert!(
        status(&ws)
            .unwrap()
            .0
            .starts_with("Unsaved changes in row 2")
    );

    ws.press("cmd-enter");
    wait_view(&mut ws, "the save", |v| v.draft().is_none());
    assert_eq!(person(&path, 2), Some(("it's \nrenamed".to_string(), None)));
    assert_eq!(status(&ws), Some(("Saved row 2".to_string(), false)));
    wait_view(&mut ws, "the reloaded page", |v| {
        v.grid
            .as_ref()
            .and_then(Grid::page)
            .is_some_and(|p| p.rows[1][1].as_deref() == Some("it's ↵renamed"))
    });
    let (_, _, row, record) = grid_state(&ws);
    assert!(record && row == 1, "still the row view of row 2");
    assert!(pane_focused(&ws));

    ws.press("escape");
    assert!(!grid_state(&ws).3);
    ws.press("right right f2");
    assert!(grid_state(&ws).3);
    assert_eq!(focused_field(&ws), Some(2));
    assert_eq!(field_text(&ws, 2).as_deref(), Some(""));
    ws.type_text("back");
    let values = view(&ws).draft().unwrap().values();
    assert_eq!(values[2].as_deref(), Some("back"));
    ws.press("escape");
    assert!(!has_draft(&ws));
    assert!(grid_state(&ws).3, "the first Esc only ends the editing");
    assert!(pane_focused(&ws));
    assert_eq!(person(&path, 2), Some(("it's \nrenamed".to_string(), None)));
    ws.press("escape");
    assert!(!grid_state(&ws).3);
}

#[test]
fn a_failed_save_keeps_the_edits_with_the_reason() {
    let (_config, path, mut ws) = connect_fixture();
    ws.open_table("people");

    ws.press("f2");
    assert_eq!(focused_field(&ws), Some(0), "F2 on the id column");
    ws.press("cmd-enter");
    assert_eq!(draft_error(&ws).as_deref(), Some("Nothing changed"));

    ws.click_label("Edit name");
    set_null(&mut ws, 1, true);
    ws.press("cmd-s");
    wait_view(&mut ws, "the error", |v| {
        v.draft().is_some_and(|d| d.error.is_some())
    });
    assert!(
        draft_error(&ws).unwrap().contains("NOT NULL"),
        "{:?}",
        draft_error(&ws)
    );
    assert!(!view(&ws).draft().unwrap().saving);
    assert_eq!(person(&path, 1).map(|p| p.0).as_deref(), Some("person 1"));

    set_null(&mut ws, 1, false);
    ws.replace_text("ghost");
    sql(&path, "DELETE FROM people WHERE id = 1;");
    ws.click_label("Save");
    wait_view(&mut ws, "the error", |v| {
        v.draft().is_some_and(|d| d.error.is_some())
    });
    assert!(
        draft_error(&ws).unwrap().contains("not found"),
        "{:?}",
        draft_error(&ws)
    );
    assert_eq!(person(&path, 1), None);
    ws.click_label("Cancel");
    assert!(!has_draft(&ws));
}

#[test]
fn edits_do_not_outlive_the_row_they_were_made_on() {
    let (_config, path, mut ws) = connect_fixture();
    ws.open_table("people");
    wait_total(&mut ws, PEOPLE);
    ws.press("down down down");

    held(&mut ws, |ws| {
        ws.press("cmd-]");
        ws.press("space");
        ws.click_label("Edit name");
        ws.type_text("hijacked");
        assert!(view(ws).typing());
    });
    wait_page(&mut ws, 100);
    assert!(!grid_state(&ws).3, "the new page closes the row view");
    assert!(!has_draft(&ws), "and the edits made on the old page");
    assert!(!view(&ws).typing());
    assert_eq!(
        status(&ws),
        Some((
            "Unsaved changes in row 4 were thrown away: the row is no longer shown".to_string(),
            true
        ))
    );
    ws.step();
    assert!(pane_focused(&ws), "{}", ws.describe());
    ws.press("cmd-enter");
    assert_eq!(person(&path, 4).map(|p| p.0).as_deref(), Some("person 4"));
    ws.press("cmd-]");
    wait_page(&mut ws, 200);

    ws.press("down down space");
    ws.click_label("Edit name");
    ws.type_text(" kept");
    ws.update(|s, _| table::reload(browser_mut(s)));
    wait_page(&mut ws, 200);
    assert!(grid_state(&ws).3);
    assert_eq!(
        field_text(&ws, 1).as_deref(),
        Some("person 203 kept"),
        "a refresh that shows the same row keeps the edits"
    );

    sql(&path, "DELETE FROM people WHERE id = 201;");
    ws.update(|s, _| table::reload(browser_mut(s)));
    wait_view(&mut ws, "the refreshed rows", |v| {
        v.grid
            .as_ref()
            .and_then(Grid::page)
            .is_some_and(|p| p.rows.len() == 49)
    });
    let (offset, _, row, record) = grid_state(&ws);
    assert_eq!((offset, row, record), (200, 2, true));
    assert!(
        !has_draft(&ws),
        "another row now sits at the same place: the edits go"
    );
    assert!(!view(&ws).typing());
    assert!(
        status(&ws)
            .unwrap()
            .0
            .starts_with("Unsaved changes in row 203")
    );
    ws.press("cmd-enter");
    assert_eq!(
        person(&path, 203).map(|p| p.0).as_deref(),
        Some("person 203")
    );
    assert_eq!(
        person(&path, 204).map(|p| p.0).as_deref(),
        Some("person 204")
    );
}

#[test]
fn a_click_anywhere_in_the_value_cell_edits_it() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.open_table("people");
    ws.press("down space");

    let field = ws.find("table-field-1").expect("the name row").visible;
    let value = ws.by_label("Edit name").expect("the name value").visible;
    let text_height = ws.by_text("person 2").expect("the value text").visible[3];
    assert!(
        field[3] > text_height * 1.5,
        "the row is taller than its value: {field:?} against {text_height}"
    );
    ws.click_at(value[0] + value[2] / 2.0, field[1] + field[3] - 3.0);
    assert_eq!(open_field(&ws), Some(1), "the bottom of the cell edits too");
}

#[test]
fn values_that_cannot_be_edited_do_not_react() {
    let (_config, _path, mut ws) = connect_fixture();

    ws.open_table("people");
    ws.press("down down down space");
    wait_view(&mut ws, "the row view", |v| {
        v.grid.as_ref().is_some_and(Grid::record_open)
    });
    assert!(ws.has_label("Edit name"));
    assert!(!ws.has_label("Edit note"));

    ws.open_table("adults");
    ws.press("f2");
    assert!(!has_draft(&ws));
    assert!(
        !grid_state(&ws).3,
        "F2 does not open the row view of a view"
    );
    assert_eq!(
        status(&ws),
        Some(("Rows of a view can't be edited".to_string(), true))
    );
}

fn words(path: &Path) -> Vec<(Option<String>, Option<String>)> {
    let db = rusqlite::Connection::open(path).unwrap();
    let mut statement = db
        .prepare("SELECT имя, значение FROM таблица ORDER BY rowid")
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn a_table_without_a_key_is_edited_by_its_row_id() {
    let (_config, path, mut ws) = connect_fixture();
    let word = |text: &str| Some(text.to_string());

    ws.open_table("таблица");
    ws.press("down space");
    assert!(ws.has_label("Edit имя"));
    ws.press("f2");
    assert_eq!(focused_field(&ws), Some(0));
    ws.replace_text("три");
    ws.press("cmd-s");
    wait_view(&mut ws, "the save", |v| v.draft().is_none());
    assert_eq!(status(&ws), Some(("Saved row 2".to_string(), false)));
    assert_eq!(
        words(&path),
        [
            (word("один"), word("日本語テキスト")),
            (word("три"), word("ü")),
            (None, None)
        ]
    );
    wait_view(&mut ws, "the reloaded page", |v| {
        v.grid
            .as_ref()
            .and_then(Grid::page)
            .is_some_and(|p| p.rows[1][0].as_deref() == Some("три"))
    });

    ws.click_label("Next row");
    ws.click_label("Edit значение");
    ws.type_text("пусто");
    ws.press("cmd-s");
    wait_view(&mut ws, "the save", |v| v.draft().is_none());
    assert_eq!(status(&ws), Some(("Saved row 3".to_string(), false)));
    assert_eq!(words(&path)[2], (None, word("пусто")), "found by IS NULL");
    assert_eq!(words(&path)[1], (word("три"), word("ü")));
}

fn open_row(ws: &mut TestWorkspace) {
    ws.press("space");
    wait_view(ws, "the row view", |v| {
        v.grid.as_ref().is_some_and(Grid::record_open)
    });
}

#[test]
fn the_header_names_how_rows_are_found() {
    let config = TestConfig::empty();
    let path = fixture(&config);
    sql(
        &path,
        "CREATE TABLE codes (code TEXT NOT NULL UNIQUE, v TEXT);
         INSERT INTO codes VALUES ('a', 'x');
         CREATE TABLE shadow (\"rowid\" TEXT, \"_rowid_\" TEXT, oid TEXT, v TEXT);
         INSERT INTO shadow VALUES ('1', '2', '3', 'x');",
    );
    let mut ws = TestWorkspace::open(&config);
    ws.connect("fixture");
    let chip = |ws: &TestWorkspace| ws.text_of("table-row-key");

    ws.open_table("people");
    assert_eq!(chip(&ws), None, "a primary key needs no chip");
    open_row(&mut ws);
    assert!(ws.exists("table-key-0"), "the key icon marks id");
    assert!(!ws.exists("table-key-1"));

    ws.open_table("таблица");
    assert_eq!(chip(&ws).as_deref(), Some("key: rowid"));
    open_row(&mut ws);
    assert!(
        !(0..2).any(|i| ws.exists(&format!("table-key-{i}"))),
        "the row id is not a column"
    );

    ws.open_table("codes");
    assert_eq!(chip(&ws).as_deref(), Some("key: code (unique)"));
    open_row(&mut ws);
    assert!(ws.exists("table-key-0") && !ws.exists("table-key-1"));

    ws.open_table("adults");
    assert_eq!(chip(&ws), None, "rows of a view are not edited");

    ws.open_table("shadow");
    assert_eq!(chip(&ws).as_deref(), Some("no key"));
    ws.press("f2");
    assert!(!has_draft(&ws));
    assert!(picker(&ws).is_some(), "F2 asks for the key columns");
    ws.press("escape");

    ws.open_table("codes");
    ws.dispatch(Command::ChooseKey);
    assert!(picker(&ws).is_none());
    assert_eq!(
        status(&ws),
        Some(("codes already has a key: code (unique)".to_string(), false))
    );
    ws.open_table("people");
    ws.dispatch(Command::ChooseKey);
    assert_eq!(
        status(&ws).map(|s| s.0).as_deref(),
        Some("people already has a key: id (primary)")
    );
    ws.open_table("adults");
    ws.dispatch(Command::ChooseKey);
    assert!(picker(&ws).is_none());
    assert_eq!(
        status(&ws),
        Some(("Rows of a view can't be edited".to_string(), true))
    );
}

fn picker(ws: &TestWorkspace) -> Option<(Vec<String>, Option<usize>, bool)> {
    match &ws.state().dialog {
        Some(Dialog::KeyPicker(picker)) => Some(picker.test_state()),
        _ => None,
    }
}

fn shadow_fixture() -> (TestConfig, PathBuf) {
    let config = TestConfig::empty();
    let path = fixture(&config);
    sql(
        &path,
        "CREATE TABLE shadow (\"rowid\" TEXT, \"_rowid_\" TEXT, oid TEXT, v TEXT, b BLOB);
         INSERT INTO shadow VALUES ('1', '2', '3', 'x', NULL), ('a', 'b', 'c', NULL, NULL),
           ('d', 'd', 'd', 'dup', NULL), ('d', 'd', 'd', 'dup', NULL);",
    );
    (config, path)
}

fn shadows(path: &Path) -> Vec<Vec<Option<String>>> {
    let db = rusqlite::Connection::open(path).unwrap();
    let mut statement = db
        .prepare("SELECT \"rowid\", \"_rowid_\", oid, v FROM shadow")
        .unwrap();
    statement
        .query_map([], |row| (0..4).map(|i| row.get(i)).collect())
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn owned(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

#[test]
fn a_table_without_any_key_asks_for_key_columns_and_remembers_them() {
    let (config, path) = shadow_fixture();
    let before = shadows(&path);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("fixture");
    let chip = |ws: &TestWorkspace| ws.text_of("table-row-key");

    ws.open_table("shadow");
    ws.press("f2");
    assert_eq!(
        picker(&ws),
        Some((owned(&["rowid", "_rowid_", "oid", "v"]), Some(0), false)),
        "every comparable column is checked; the blob is not"
    );
    assert_eq!(
        ws.text_of("key-picker-intro").as_deref(),
        Some(
            "shadow has no primary key, unique index or row id. Choose the columns that together \
             identify one row. A change is saved only when exactly one row matches them."
        )
    );
    assert!(!ws.exists("key-picker-forget"), "nothing to forget yet");
    assert!(!ws.exists("key-picker-stale"));
    assert!(ws.is_focused("key-picker-cancel"), "{}", ws.describe());
    ws.press("escape");
    assert!(picker(&ws).is_none());
    assert!(!has_draft(&ws));
    ws.step();
    assert!(pane_focused(&ws), "{}", ws.describe());

    ws.press("down");
    open_row(&mut ws);
    ws.click_label("Edit v");
    assert_eq!(picker(&ws).map(|p| p.1), Some(Some(3)), "resumes at v");
    ws.click("key-picker-column-2");
    ws.click("key-picker-column-4");
    assert_eq!(
        picker(&ws).map(|p| p.0),
        Some(owned(&["rowid", "_rowid_", "v"])),
        "the blob can't be checked"
    );
    ws.click("key-picker-use");
    assert!(picker(&ws).is_none());
    assert_eq!(
        status(&ws),
        Some((
            "Rows of shadow are now found by rowid, _rowid_, v".to_string(),
            false
        ))
    );
    assert_eq!(open_field(&ws), Some(3), "the edit goes on at once");
    ws.step();
    assert_eq!(focused_field(&ws), Some(3), "{}", ws.describe());
    assert!(ws.is_focused("table-field-input-3"), "{}", ws.describe());
    let update = view(&ws).draft().unwrap().update();
    assert!(update.is_none());
    ws.type_text("filled");
    let key = view(&ws).draft().unwrap().update().unwrap().key;
    assert_eq!(
        key,
        [
            ("rowid".to_string(), Some("a".to_string())),
            ("_rowid_".to_string(), Some("b".to_string())),
            ("v".to_string(), None)
        ]
    );
    ws.press("cmd-s");
    wait_view(&mut ws, "the save", |v| v.draft().is_none());
    assert_eq!(status(&ws), Some(("Saved row 2".to_string(), false)));
    let mut expected = before.clone();
    expected[1][3] = Some("filled".to_string());
    assert_eq!(shadows(&path), expected, "exactly one row changed");

    let toml = config.connections_toml();
    assert!(toml.contains("[[connection.row_key]]"), "{toml}");
    assert!(toml.contains("table = \"shadow\""), "{toml}");
    assert_eq!(
        chip(&ws).as_deref(),
        Some("key: rowid, _rowid_, v (chosen)")
    );
    assert!(
        ws.exists("table-key-0") && ws.exists("table-key-3") && !ws.exists("table-key-2"),
        "the chosen columns are marked"
    );

    ws.click_label("Next row");
    ws.click_label("Edit v");
    assert!(picker(&ws).is_none(), "the chosen key is used");
    ws.replace_text("one");
    ws.press("cmd-s");
    wait_view(&mut ws, "the error", |v| {
        v.draft().is_some_and(|d| d.error.is_some())
    });
    assert_eq!(
        draft_error(&ws).as_deref(),
        Some(
            "Not saved: the row's key matches 2 rows (nothing was saved). Cancel the edit, \
             then choose more key columns to tell these rows apart."
        )
    );
    assert_eq!(shadows(&path), expected, "rolled back");

    ws.click("table-row-key");
    assert!(picker(&ws).is_none());
    assert_eq!(
        status(&ws),
        Some((
            "Save or cancel the changes in row 3 first".to_string(),
            true
        ))
    );
    ws.click_label("Cancel");
    assert!(!has_draft(&ws));
    drop(ws);

    let mut ws = TestWorkspace::open(&config);
    ws.connect("fixture");
    ws.open_table("shadow");
    ws.press("f2");
    assert!(picker(&ws).is_none(), "the choice was remembered");
    assert!(has_draft(&ws));
    ws.press("escape escape");
    assert!(!has_draft(&ws));

    ws.click("table-row-key");
    assert_eq!(
        picker(&ws).map(|p| p.0),
        Some(owned(&["rowid", "_rowid_", "v"])),
        "the saved choice is checked"
    );
    ws.click("key-picker-forget");
    assert!(picker(&ws).is_none());
    assert_eq!(
        status(&ws),
        Some((
            "The key columns of shadow were forgotten".to_string(),
            false
        ))
    );
    assert!(
        !config.connections_toml().contains("row_key"),
        "{}",
        config.connections_toml()
    );
    assert_eq!(chip(&ws).as_deref(), Some("no key"));
    ws.press("f2");
    assert!(picker(&ws).is_some(), "asks again");
}

#[test]
fn a_chosen_column_that_is_gone_asks_again() {
    let (config, _path) = shadow_fixture();
    config
        .store()
        .set_row_key("fixture", "main", "shadow", Some(&owned(&["gone", "v"])))
        .unwrap();
    let mut ws = TestWorkspace::open(&config);
    ws.connect("fixture");
    ws.open_table("shadow");
    assert_eq!(
        ws.text_of("table-row-key").as_deref(),
        Some("no key"),
        "a choice that can't be used is not shown as the key"
    );
    ws.press("cmd-shift-k");
    assert_eq!(
        picker(&ws),
        Some((owned(&["v"]), None, true)),
        "what is left of the choice"
    );
    assert!(ws.exists("key-picker-stale"));
    assert!(ws.exists("key-picker-forget"));
    ws.press("escape");

    ws.press("f2");
    assert_eq!(picker(&ws), Some((owned(&["v"]), Some(0), true)));
    ws.click("key-picker-column-0");
    ws.click("key-picker-use");
    assert_eq!(open_field(&ws), Some(0));
    assert_eq!(
        ws.text_of("table-row-key").as_deref(),
        Some("key: rowid, v (chosen)")
    );
}

#[test]
fn the_key_shortcut_is_left_to_a_wider_binding_on_tables_with_a_key() {
    let (config, _path) = shadow_fixture();
    let mut ws = TestWorkspace::open(&config);
    let chord = rune::Chord::parse("mod+shift+k").unwrap();
    crate::keys::update(|keymap| keymap.set(Command::ShowHelp, 0, chord));
    let help = |ws: &TestWorkspace| ws.read(|w| matches!(w.dialog, Some(Dialog::Help(_))));
    ws.connect("fixture");

    ws.open_table("people");
    ws.press("cmd-shift-k");
    assert!(help(&ws), "the table has its own key");
    ws.press("escape");
    assert!(!ws.has_dialog());

    ws.open_table("shadow");
    ws.press("cmd-shift-k");
    assert!(!help(&ws));
    assert!(picker(&ws).is_some(), "a table without a key takes it");
    ws.press("escape");
    crate::keys::update(|keymap| keymap.reset_all());
}

fn drift(ws: &TestWorkspace) -> (usize, bool, Vec<Option<String>>) {
    let grid = grid(ws);
    let ids = grid
        .page()
        .map(|page| page.rows.iter().map(|row| row[0].clone()).collect())
        .unwrap_or_default();
    (grid.row(), grid.record_open(), ids)
}

#[test]
fn the_row_view_follows_a_saved_row_that_moved_and_closes_when_it_left() {
    let (config, _path) = shadow_fixture();
    let path = config.dir().join("data").join("fixture.db");
    sql(
        &path,
        "CREATE TABLE drift (\"rowid\" TEXT, \"_rowid_\" TEXT, oid TEXT, v TEXT);
         INSERT INTO drift VALUES ('1', '1', '1', 'a'), ('2', '2', '2', 'b'), ('3', '3', '3', 'c');
         CREATE TRIGGER drift_moves AFTER UPDATE OF v ON drift WHEN NEW.v = 'moved' BEGIN
           DELETE FROM drift WHERE \"rowid\" = NEW.\"rowid\";
           INSERT INTO drift VALUES (NEW.\"rowid\", NEW.\"_rowid_\", NEW.oid, NEW.v);
         END;
         CREATE TRIGGER drift_goes AFTER UPDATE OF v ON drift WHEN NEW.v = 'gone' BEGIN
           DELETE FROM drift WHERE \"rowid\" = NEW.\"rowid\";
         END;",
    );
    config
        .store()
        .set_row_key("fixture", "main", "drift", Some(&owned(&["rowid"])))
        .unwrap();
    let id = |text: &str| Some(text.to_string());
    let mut ws = TestWorkspace::open(&config);
    ws.connect("fixture");
    ws.open_table("drift");
    open_row(&mut ws);
    ws.click_label("Edit v");
    ws.replace_text("moved");
    ws.press("cmd-s");
    wait_view(&mut ws, "the save", |v| v.draft().is_none());
    wait_view(&mut ws, "the moved row", |v| {
        v.grid
            .as_ref()
            .and_then(Grid::page)
            .is_some_and(|p| p.rows[2][3].as_deref() == Some("moved"))
    });
    assert_eq!(
        drift(&ws),
        (2, true, vec![id("2"), id("3"), id("1")]),
        "the row view went with the row"
    );
    assert_eq!(footer(&ws), "row 3");
    assert_eq!(selected_cell(&ws).map(|cell| cell.0), Some(2));
    assert_eq!(status(&ws), Some(("Saved row 1".to_string(), false)));
    assert!(view(&ws).follow.is_none());

    ws.click_label("Edit v");
    ws.replace_text("gone");
    ws.press("cmd-s");
    wait_view(&mut ws, "the save", |v| v.draft().is_none());
    wait_view(&mut ws, "the row left", |v| {
        v.grid
            .as_ref()
            .and_then(Grid::page)
            .is_some_and(|p| p.rows.len() == 2)
    });
    assert_eq!(drift(&ws), (1, false, vec![id("2"), id("3")]));
    assert_eq!(
        status(&ws),
        Some((
            "Saved row 3; it is no longer on this page".to_string(),
            false
        ))
    );
    ws.step();
    assert!(grid_focused(&ws), "{}", ws.describe());
    assert!(!view(&ws).reveal_grid);
}

#[test]
fn a_click_outside_turns_the_field_back_into_text() {
    let (_config, path, mut ws) = connect_fixture();
    ws.open_table("people");
    ws.press("down space");

    ws.click_label("Edit name");
    assert_eq!(open_field(&ws), Some(1));
    click_beside(&mut ws, "Edit note");
    assert!(!has_draft(&ws));
    assert!(!ws.has_label("Save"), "no save bar");
    assert!(pane_focused(&ws), "the row view has the keyboard");

    ws.click_label("Edit name");
    ws.replace_text("changed");
    click_beside(&mut ws, "Edit note");
    assert_eq!(open_field(&ws), None);
    assert_eq!(field_text(&ws, 1).as_deref(), Some("changed"));
    assert!(
        ws.has_label("Edit name"),
        "shown as text, a click edits it again"
    );
    assert!(ws.has_label("Save"));
    assert!(pane_focused(&ws));
    ws.click_label("Edit name");
    assert_eq!(focused_field(&ws), Some(1));
    assert_eq!(field_text(&ws, 1).as_deref(), Some("changed"));

    ws.click_label("Edit note");
    assert_eq!(open_field(&ws), Some(2));
    assert_eq!(focused_field(&ws), Some(2));
    assert_eq!(field_text(&ws, 1).as_deref(), Some("changed"));

    click_beside(&mut ws, "Edit name");
    assert_eq!(field_text(&ws, 2), None);
    ws.press("cmd-s");
    wait_view(&mut ws, "the save", |v| v.draft().is_none());
    assert_eq!(person(&path, 2).map(|p| p.0).as_deref(), Some("changed"));
}

#[test]
fn a_wide_table_builds_only_the_columns_in_view() {
    let (_config, _path, mut ws) = connect_with_wide_table();
    ws.open_table("wide");
    assert_eq!(grid(&ws).page().unwrap().columns.len(), 80);
    let (start, end) = view(&ws).cells.window;
    assert_eq!(start, 0);
    assert!(end < 80, "{end} columns built");
    assert!(ws.exists("table-resize-0"));
    assert!(!ws.exists("table-resize-79"));

    let before = view(&ws).cells.widths[0];
    let handle = ws.find("table-resize-0").unwrap().visible;
    let (x, y) = (handle[0] + handle[2] / 2.0, handle[1] + handle[3] / 2.0);
    ws.rt.pointer_move(x, y);
    ws.rt.pointer_down(x, y);
    for step in 1..=5 {
        ws.rt.pointer_move(x + step as f32 * 12.0, y);
        ws.rt.frame(1.0 / 60.0);
    }
    ws.rt.pointer_up(x + 60.0, y);
    ws.settle();
    let after = view(&ws).cells.widths[0];
    assert!((after - before - 60.0).abs() < 1.0, "{before} → {after}");
}

#[test]
#[ignore = "renders PNG snapshots into $COOPER_TABLE_SNAPS"]
fn table_snapshots() {
    let Some(dir) = std::env::var_os("COOPER_TABLE_SNAPS").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    for dark in [true, false] {
        let theme = if dark { "dark" } else { "light" };
        let config = TestConfig::empty();
        let path = fixture(&config);
        let columns: Vec<String> = (1..=80).map(|i| format!("  column_{i} TEXT")).collect();
        sql(
            &path,
            &format!("CREATE TABLE wide (\n{}\n);", columns.join(",\n")),
        );
        sql(
            &path,
            "INSERT INTO wide (column_1, column_2, column_40, column_80) VALUES ('a', 'b', 'forty', 'last');",
        );
        let mut ws = TestWorkspace::open_with(config.store(), dark);
        ws.connect("fixture");
        let shot = |ws: &mut TestWorkspace, name: &str| {
            ws.settle();
            snap_png(&mut ws.rt, &dir.join(format!("{name}-{theme}.png")));
        };
        shot(&mut ws, "empty");
        ws.open_table("people");
        wait_total(&mut ws, PEOPLE);
        ws.press("down right right");
        shot(&mut ws, "grid");
        for _ in 0..40 {
            ws.press("down");
        }
        shot(&mut ws, "grid-scrolled");
        ws.press("cmd-alt-[ down down down space");
        wait_view(&mut ws, "the DDL", |v| {
            v.grid.as_ref().is_some_and(|g| g.ddl().is_some())
        });
        shot(&mut ws, "record-long");
        ws.press("escape up up space");
        ws.click_label("Edit name");
        ws.replace_text("renamed");
        ws.press("tab");
        shot(&mut ws, "record-edit");
        set_null(&mut ws, 2, true);
        ws.press("cmd-s");
        wait_view(&mut ws, "the save", |v| v.draft().is_none_or(|d| !d.saving));
        shot(&mut ws, "record-saved");
        ws.press("escape escape");
        ws.open_table("adults");
        shot(&mut ws, "view");
        ws.open_table("blank");
        shot(&mut ws, "blank");
        ws.open_table("wide");
        ws.press("end");
        shot(&mut ws, "wide-end");
        sql(&path, "ALTER TABLE people RENAME TO folks;");
        ws.open_table("people");
        shot(&mut ws, "error");
        sql(&path, "ALTER TABLE folks RENAME TO people;");
    }
}

#[test]
#[ignore = "prints frame times for a wide, full page"]
fn wide_grid_frame_times() {
    let (_config, path, mut ws) = connect_with_wide_table();
    let values: Vec<String> = (1..=80)
        .map(|i| format!("'value {i} {}'", "w".repeat(i % 50)))
        .collect();
    let insert = format!("INSERT INTO wide VALUES ({});", values.join(", "));
    sql(&path, &insert.repeat(100));
    ws.open_table("wide");
    let time = |ws: &mut TestWorkspace, what: &str, f: &dyn Fn(&mut TestWorkspace)| {
        let started = std::time::Instant::now();
        f(ws);
        println!("{what}: {:?}", started.elapsed());
    };
    time(&mut ws, "down x10", &|ws| {
        for _ in 0..10 {
            ws.rt.key(crate::test_support::key_event("down"));
            ws.rt.frame(1.0 / 60.0);
        }
    });
    time(&mut ws, "right x40", &|ws| {
        for _ in 0..40 {
            ws.rt.key(crate::test_support::key_event("right"));
            ws.rt.frame(1.0 / 60.0);
        }
    });
    time(&mut ws, "wheel x20", &|ws| {
        for _ in 0..20 {
            ws.rt.wheel(800.0, 400.0, 40.0);
            ws.rt.frame(1.0 / 60.0);
        }
    });
    time(&mut ws, "wheel x back x20", &|ws| {
        for _ in 0..20 {
            ws.rt.wheel_xy(800.0, 400.0, 300.0, 0.0);
            ws.rt.frame(1.0 / 60.0);
        }
    });
    time(&mut ws, "record of row", &|ws| ws.press("space"));
}

#[test]
fn the_row_number_column_is_a_cell_that_stands_for_the_row() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.open_table("people");
    ws.press("down right home");
    assert_eq!(
        selected_cell(&ws),
        Some((1, None)),
        "Home reaches the # column"
    );
    ws.press("cmd-c");
    assert_eq!(
        ws.clipboard().as_deref(),
        Some("2\tperson 2\tfirst line\n\tsecond line")
    );
    ws.press("right");
    assert_eq!(selected_cell(&ws), Some((1, Some(0))));
    ws.press("left left");
    assert_eq!(
        selected_cell(&ws),
        Some((1, None)),
        "← stops at the # column"
    );
    ws.press("end");
    assert_eq!(selected_cell(&ws), Some((1, Some(2))));

    ws.click("table-row-3");
    ws.press("home down");
    assert_eq!(selected_cell(&ws).map(|c| c.1), Some(None));
    let row = grid_state(&ws).2;
    ws.press("f2");
    assert_eq!(focused_field(&ws), Some(0), "F2 on # edits the first value");
    assert_eq!(grid_state(&ws).2, row);
}

#[test]
fn a_keyboard_focus_move_leaves_the_field_open() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.open_table("people");
    ws.press("down space");
    ws.click_label("Edit name");
    assert_eq!(focused_field(&ws), Some(1));
    ws.press("f6");
    assert!(ws.is_focused("schema-tree"));
    assert_eq!(
        open_field(&ws),
        Some(1),
        "only a click outside closes the field"
    );
    assert!(has_draft(&ws));
    ws.press("shift-f6");
    assert!(pane_focused(&ws));
    assert_eq!(open_field(&ws), Some(1));
    ws.press("escape");
    assert!(!has_draft(&ws));
    assert!(grid_state(&ws).3);
}

#[test]
fn closing_the_row_view_shows_the_row_it_stepped_to() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.open_table("people");
    ws.press("space");
    for _ in 0..60 {
        ws.press("right");
    }
    assert_eq!(footer(&ws), "row 61");
    ws.press("escape");
    assert_eq!(grid_state(&ws).2, 60);
    assert!(ws.is_visible("table-row-60"), "the selected row is in view");
    assert!(!ws.exists("table-row-0") || !ws.is_visible("table-row-0"));
}

#[test]
fn data_columns_follow_a_font_change_and_keep_dragged_widths() {
    let (_config, _path, mut ws) = connect_fixture();
    ws.open_table("people");
    let digit = || table::grid::mono_cell_width(crate::theme::theme(true));
    let before = digit();
    assert_eq!(view(&ws).cells.cell_width, before);
    ws.update(|s, _| browser_mut(s).table.cells.widths[1] += 40.0);
    let dragged = view(&ws).cells.widths[1];

    ws.update(|s, tx| {
        let mut fonts = s.fonts.clone();
        fonts.code = "Courier New".into();
        s.set_fonts(fonts, tx);
    });
    let cells = &view(&ws).cells;
    assert_eq!(cells.cell_width, digit(), "sized in the new font");
    assert_eq!(cells.widths[0], cells.natural[0]);
    assert_eq!(cells.widths[2], cells.natural[2]);
    assert!(
        (cells.widths[1] - dragged * digit() / before).abs() < 0.01,
        "a dragged column keeps its digits"
    );
}
