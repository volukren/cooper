use super::{ConsoleArea, GRID, RESULT, SQL, grid, syntax};
use crate::browser::{Main, Pane};
use crate::keys::Command;
use crate::test_support::{TestConfig, TestWorkspace};

const ENDLESS: &str =
    "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r) SELECT count(*) FROM r";

fn status(ws: &TestWorkspace) -> Option<(String, bool)> {
    ws.browser().status.clone()
}

fn hint(ws: &TestWorkspace) -> Option<String> {
    status(ws).map(|(text, _)| text)
}

fn text(ws: &TestWorkspace) -> String {
    ws.console().text().to_string()
}

fn outcome(ws: &TestWorkspace) -> Option<String> {
    ws.console().outcome_text()
}

fn record(ws: &TestWorkspace) -> Option<usize> {
    ws.console().record
}

fn selected_cell(ws: &TestWorkspace) -> Option<(usize, usize)> {
    ws.console().grid.selected_cell()
}

fn is_running(ws: &TestWorkspace) -> bool {
    ws.console().is_running()
}

fn ticker(ws: &TestWorkspace) -> Option<String> {
    ws.console().ticker()
}

fn running_status(ws: &TestWorkspace) -> Option<String> {
    ws.console().running_status()
}

fn set_sql(ws: &mut TestWorkspace, sql: &str) {
    let sql = sql.to_string();
    ws.update(move |s, _| {
        if let Some(b) = s.active_browser_mut() {
            let sql_edit = &mut b.console.sql;
            let focused = sql_edit.focused;
            sql_edit.replace_text(&sql);
            sql_edit.set_caret(sql_edit.text.len());
            sql_edit.focused = focused;
        }
    });
}

fn editor_focused(ws: &TestWorkspace) -> bool {
    ws.is_focused(SQL)
}

fn result_focused(ws: &TestWorkspace) -> bool {
    ws.focus_within(RESULT)
}

fn grid_focused(ws: &TestWorkspace) -> bool {
    ws.is_focused(GRID)
}

fn focused_pane(ws: &TestWorkspace) -> Option<Pane> {
    ws.browser().focused_pane(ws.focused().as_deref())
}

fn tree_focused(ws: &TestWorkspace) -> bool {
    focused_pane(ws) == Some(Pane::Tree)
}

fn press_now(ws: &mut TestWorkspace, keys: &str) {
    for key in keys.split_whitespace() {
        ws.rt.key(crate::test_support::key_event(key));
        ws.rt.frame(0.0);
    }
}

fn click_now(ws: &mut TestWorkspace, label: &str) {
    let element = ws.by_label(label).expect("a labelled element");
    let (x, y) = crate::test_support::center(element.visible);
    ws.rt.pointer_move(x, y);
    ws.rt.pointer_down(x, y);
    ws.rt.pointer_up(x, y);
    ws.rt.frame(0.0);
}

fn shop_console(config: &TestConfig) -> TestWorkspace {
    let mut ws = TestWorkspace::open(config);
    ws.connect("shop");
    ws.press("cmd-e");
    ws
}

#[test]
fn console_opens_empty_and_runs_with_every_run_key() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);

    assert_eq!(ws.browser().main, Main::Console);
    assert!(editor_focused(&ws), "⌘E focuses the editor");
    assert_eq!(outcome(&ws), None, "nothing has been run yet");

    ws.type_text("select 1 as q, 2 as rs");
    assert_eq!(text(&ws), "select 1 as q, 2 as rs");

    for keys in ["cmd-enter", "f5"] {
        press_now(&mut ws, keys);
        assert!(is_running(&ws), "{keys} runs the script");
        let done = ws.wait_script_done();
        assert!(done.starts_with("1 row · "), "{keys}: {done}");
        assert_eq!(
            text(&ws),
            "select 1 as q, 2 as rs",
            "{keys} leaves the text alone"
        );
        assert!(
            editor_focused(&ws),
            "{keys}: running does not move the focus"
        );
    }

    click_now(&mut ws, "Run");
    assert!(is_running(&ws));
    assert!(ws.wait_script_done().starts_with("1 row · "));

    ws.press("cmd-a backspace");
    ws.type_text("select 1 as a");
    ws.press("enter");
    ws.type_text("select 2 as b, 3 as c");
    ws.press("shift-home cmd-enter");
    ws.wait_script_done();
    let result = ws.console().result().unwrap().clone();
    let columns: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(columns, ["b", "c"]);
    assert_eq!(text(&ws), "select 1 as a\nselect 2 as b, 3 as c");

    ws.replace_text("  ");
    ws.press("cmd-enter");
    assert_eq!(
        hint(&ws).as_deref(),
        Some("Nothing to run: the editor is empty")
    );
    ws.type_text("x");
    assert_eq!(hint(&ws), None);
}

#[test]
fn f5_runs_in_the_console_and_refreshes_elsewhere() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    ws.type_text("select 42");
    ws.press("f5");
    assert!(ws.wait_script_done().starts_with("1 row"));

    ws.press("cmd-p");
    let before = ws.console().result().unwrap().clone();
    ws.press("f5");
    assert!(!is_running(&ws));
    let after = ws.console().result().unwrap().clone();
    assert!(
        std::rc::Rc::ptr_eq(&before, &after),
        "F5 in the tree must not rerun the script"
    );
    ws.wait_tree_loaded();
    assert_eq!(ws.browser().main, Main::Console);
}

#[test]
fn a_running_script_refuses_a_second_run_and_cancels() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    set_sql(&mut ws, ENDLESS);

    ws.press("cmd-enter");
    assert!(is_running(&ws));
    assert_eq!(ticker(&ws).as_deref(), Some("running… 0s"));
    assert_eq!(outcome(&ws), None, "a new run hides the previous outcome");

    ws.press("cmd-enter");
    assert_eq!(
        hint(&ws).as_deref(),
        Some("A script is already running · Esc cancels it")
    );
    ws.press("f5");
    assert_eq!(
        hint(&ws).as_deref(),
        Some("A script is already running · Esc cancels it")
    );
    assert!(is_running(&ws));

    ws.rt.key(crate::test_support::key_event("escape"));
    ws.rt.key(crate::test_support::key_event("escape"));
    ws.rt.frame(0.0);
    assert_eq!(ticker(&ws).as_deref(), Some("cancelling…"));
    assert!(
        editor_focused(&ws),
        "the second Esc, while cancelling, stays put"
    );
    assert_eq!(
        ws.wait_script_done(),
        "cancelled",
        "the db error text is not shown"
    );
    assert_eq!(text(&ws), ENDLESS);

    ws.press("cmd-enter");
    assert!(is_running(&ws));
    press_now(&mut ws, "escape");
    assert_eq!(ticker(&ws).as_deref(), Some("cancelling…"));
    assert_eq!(hint(&ws).as_deref(), Some("Cancelling the script…"));
    assert_eq!(ws.wait_script_done(), "cancelled");

    ws.press("cmd-enter");
    assert!(is_running(&ws));
    ws.press("cmd-.");
    assert_eq!(ws.wait_script_done(), "cancelled");

    ws.press("cmd-enter");
    ws.click_label("Cancel");
    assert_eq!(ws.wait_script_done(), "cancelled");

    set_sql(&mut ws, "select 1");
    ws.press("cmd-enter");
    assert!(ws.wait_script_done().starts_with("1 row"));
}

#[test]
fn stale_script_responses_are_ignored() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    set_sql(&mut ws, ENDLESS);
    ws.press("cmd-enter");
    let handled = ws.update(|s, _| {
        let b = s.active_browser_mut().expect("a browser");
        super::on_error(b, u64::MAX, "boom")
    });
    assert!(!handled, "an unknown id is not the console's");
    assert!(is_running(&ws), "the run keeps going");
    ws.press("cmd-.");
    assert_eq!(ws.wait_script_done(), "cancelled");
}

fn tree_has(ws: &TestWorkspace, table: &str) -> bool {
    ws.tree().model.nodes[0]
        .tables
        .iter()
        .flatten()
        .any(|t| t.name == table)
}

#[test]
fn scripts_report_failures_affected_rows_and_refresh_the_tree() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);

    set_sql(
        &mut ws,
        "CREATE TABLE made_here (id INTEGER, name TEXT);\n\
         INSERT INTO made_here VALUES (1, 'a'), (2, NULL);\n\
         SELECT * FROM made_here;\n",
    );
    ws.press("cmd-enter");
    let done = ws.wait_script_done();
    assert!(
        done.starts_with("2 rows · 3 statements · 2 rows affected · "),
        "{done}"
    );
    let result = ws.console().result().unwrap().clone();
    assert_eq!(result.rows[1][1], None, "NULL stays NULL");

    ws.wait_until("made_here in the tree", |ws| tree_has(ws, "made_here"));

    set_sql(
        &mut ws,
        "INSERT INTO made_here VALUES (3, 'c'); DELETE FROM made_here WHERE id < 3",
    );
    ws.press("cmd-enter");
    let done = ws.wait_script_done();
    assert!(
        done.starts_with("2 statements · 3 rows affected · "),
        "{done}"
    );
    assert_eq!(ws.console().result().unwrap().result_sets, 0);

    set_sql(&mut ws, "DROP TABLE made_here; SELECT * FROM nowhere");
    ws.press("cmd-enter");
    let done = ws.wait_script_done();
    assert!(
        done.starts_with("error: ") && done.contains("no such table: nowhere"),
        "{done}"
    );
    ws.wait_until("made_here gone from the tree", |ws| {
        ws.tree().model.schemas_request.is_none() && !tree_has(ws, "made_here")
    });

    set_sql(&mut ws, "select 1");
    ws.press("cmd-enter");
    assert!(ws.wait_script_done().starts_with("1 row"));
}

#[test]
fn result_grid_row_view_and_copy() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    set_sql(
        &mut ws,
        "SELECT id, name, country FROM customers ORDER BY id",
    );
    ws.press("cmd-enter");
    assert!(ws.wait_script_done().starts_with("8 rows · "));

    assert_eq!(
        selected_cell(&ws),
        Some((0, 1)),
        "the first cell is selected"
    );

    ws.click_label("Fatima Al-Sayed");
    assert!(result_focused(&ws));
    assert_eq!(selected_cell(&ws), Some((4, 2)));

    ws.press("cmd-c");
    assert_eq!(ws.clipboard().as_deref(), Some("Fatima Al-Sayed"));
    assert_eq!(hint(&ws).as_deref(), Some("Copied 1 cell"));
    ws.press("cmd-shift-c");
    assert_eq!(ws.clipboard().as_deref(), Some("5\tFatima Al-Sayed\tAE"));

    ws.press("enter");
    assert_eq!(record(&ws), Some(4));
    ws.press("right");
    assert_eq!(record(&ws), Some(5));
    ws.press("left left");
    assert_eq!(record(&ws), Some(3));
    ws.press("alt-down");
    assert_eq!(record(&ws), Some(4));
    ws.press("cmd-c");
    assert_eq!(ws.clipboard().as_deref(), Some("Fatima Al-Sayed"));
    ws.press("cmd-shift-c");
    assert_eq!(ws.clipboard().as_deref(), Some("5\tFatima Al-Sayed\tAE"));
    ws.press("enter");
    assert_eq!(record(&ws), None);
    assert!(
        result_focused(&ws),
        "closing the row view keeps the focus in the result"
    );

    ws.press("space");
    assert_eq!(record(&ws), Some(4));
    ws.press("escape");
    assert_eq!(record(&ws), None);

    ws.double_click_label("José García");
    assert_eq!(record(&ws), Some(1));

    ws.press("space");
    ws.press("space");
    assert_eq!(record(&ws), Some(1));
    ws.press("cmd-enter");
    assert!(ws.wait_script_done().starts_with("8 rows"));
    assert_eq!(record(&ws), None);
    assert_eq!(selected_cell(&ws), Some((0, 1)));

    ws.click_label("Bob");
    assert_eq!(selected_cell(&ws), Some((7, 2)));
    press_now(&mut ws, "f5");
    assert!(is_running(&ws));
    assert!(ws.wait_script_done().starts_with("8 rows"));
    assert!(result_focused(&ws));
    assert_eq!(selected_cell(&ws), Some((0, 1)));

    ws.press("cmd-e");
    set_sql(&mut ws, "SELECT id, name FROM customers WHERE id < 0");
    ws.press("cmd-enter");
    assert!(ws.wait_script_done().starts_with("0 rows · "));
    let result = ws.console().result().unwrap().clone();
    assert_eq!(result.columns.len(), 2);
    assert_eq!(selected_cell(&ws), None);
    ws.focus(GRID);
    ws.press("space enter");
    assert_eq!(record(&ws), None);
    ws.press("cmd-c cmd-shift-c");
}

#[test]
fn result_columns_fit_their_content() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    set_sql(
        &mut ws,
        "SELECT id, name, email, country FROM customers ORDER BY id",
    );
    ws.press("cmd-enter");
    assert!(ws.wait_script_done().starts_with("8 rows"));
    let result = ws.console().result().unwrap().clone();
    let t = ws.state().theme();
    let shape = |text: &str, style: rune::TextStyle| rune::text::measure(text, &[], style, None).0;
    for (index, column) in result.columns.iter().enumerate() {
        let mut needed =
            shape(&column.name, t.semibold(t.text_sm)).max(shape(&column.type_name, t.text_xs));
        for row in &result.rows {
            let text = row
                .get(index)
                .and_then(|cell| cell.as_deref())
                .unwrap_or(grid::NULL);
            if unicode_width::UnicodeWidthStr::width(text) <= grid::MAX_COLUMN_CHARS {
                needed = needed.max(shape(text, t.mono));
            }
        }
        let needed = needed + 2. * grid::CELL_PADDING;
        let width = ws
            .find(&format!("console-grid-col-{index}"))
            .expect("the column header is drawn")
            .rect[2];
        assert!(
            width >= needed,
            "column {index} is {width:?} wide but its text needs {needed:?}"
        );
    }
}

#[test]
fn unicode_values_do_not_break_anything() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    set_sql(
        &mut ws,
        "SELECT '日本語テキスト' || char(10) || '😀 é' AS \"名前\", NULL AS n, '' AS e, \
         replace(hex(zeroblob(400)), '0', 'ю') AS long",
    );
    ws.press("cmd-enter");
    let done = ws.wait_script_done();
    assert!(done.starts_with("1 row · "), "{done}");
    ws.focus(GRID);
    ws.press("space");
    assert_eq!(record(&ws), Some(0));
    ws.press("cmd-shift-c");
    let copied = ws.clipboard().unwrap();
    assert!(
        copied.starts_with("日本語テキスト\n😀 é\tNULL\t\t"),
        "{copied:?}"
    );
    ws.press("down pagedown end home right left escape");

    ws.resize((120., 90.));
    ws.resize((1280., 820.));
}

fn table_page_rows(ws: &TestWorkspace) -> Option<usize> {
    ws.table_view()
        .grid
        .as_ref()
        .and_then(|g| g.page())
        .map(|p| p.rows.len())
}

#[test]
fn opening_a_table_hides_the_console_and_keeps_it() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    ws.type_text("select 5 as five");
    ws.press("cmd-enter");
    let done = ws.wait_script_done();

    ws.open_table("customers");
    assert_eq!(ws.browser().main, Main::Table);
    let page = table_page_rows(&ws);

    ws.press("cmd-e");
    assert_eq!(ws.browser().main, Main::Console);
    assert_eq!(text(&ws), "select 5 as five");
    assert_eq!(outcome(&ws), Some(done));
    assert!(editor_focused(&ws));
    assert_eq!(
        ws.table_view().grid.as_ref().map(|g| g.is_loading()),
        Some(false)
    );
    assert_eq!(table_page_rows(&ws), page);
}

#[test]
fn paste_into_the_editor_is_normalized() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    ws.clip.set_text("SELECT\r\n\t1,\r2\u{0}\u{1b}[31m;\n");
    ws.press("cmd-v");
    assert_eq!(text(&ws), "SELECT\n    1,\n2[31m;\n");
    ws.clip.set_text("-- ok");
    ws.press("cmd-v");
    assert_eq!(text(&ws), "SELECT\n    1,\n2[31m;\n-- ok");
}

#[test]
fn home_and_end_move_along_the_row_of_the_result_grid() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    set_sql(
        &mut ws,
        "SELECT id, name, country FROM customers ORDER BY id",
    );
    ws.press("cmd-enter");
    assert!(ws.wait_script_done().starts_with("8 rows"));
    ws.press("cmd-j down");
    assert_eq!(selected_cell(&ws), Some((1, 1)));
    ws.press("end");
    assert_eq!(selected_cell(&ws), Some((1, 3)));
    ws.press("right");
    assert_eq!(selected_cell(&ws), Some((1, 3)));
    ws.press("home");
    assert_eq!(selected_cell(&ws), Some((1, 0)), "the row number cell");
    ws.press("left");
    assert_eq!(selected_cell(&ws), Some((1, 0)));
    ws.press("cmd-c");
    assert_eq!(ws.clipboard(), None, "the row number is not copied");
    ws.press("right");
    assert_eq!(selected_cell(&ws), Some((1, 1)));
    ws.press("home enter");
    assert_eq!(record(&ws), Some(1));
}

#[test]
fn a_click_in_the_result_pane_is_remembered_for_the_tab() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    set_sql(&mut ws, "SELECT * FROM no_such_table");
    ws.press("cmd-enter");
    assert!(ws.wait_script_done().starts_with("error:"));
    assert!(editor_focused(&ws));
    ws.click_text("Script failed");
    assert!(result_focused(&ws));
    assert_eq!(ws.console().area(), ConsoleArea::Message);
    ws.press("cmd-1 cmd-2");
    assert!(result_focused(&ws), "back in the result pane");
}

#[test]
fn tab_indents_and_return_keeps_the_indentation() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);

    ws.type_text("select 1,");
    ws.press("enter tab");
    ws.type_text("2");
    ws.press("enter");
    assert_eq!(text(&ws), "select 1,\n  2\n  ", "Return keeps the indent");
    ws.type_text("from t");
    ws.press("shift-tab");
    assert_eq!(text(&ws), "select 1,\n  2\nfrom t", "Shift+Tab outdents");
    assert!(editor_focused(&ws), "Shift+Tab stays in the editor");

    ws.press("cmd-a tab");
    assert_eq!(text(&ws), "  select 1,\n    2\n  from t");
    assert_eq!(
        ws.console().sql.selected(),
        "select 1,\n    2\n  from t",
        "the lines stay selected"
    );
    ws.press("shift-tab shift-tab");
    assert_eq!(text(&ws), "select 1,\n2\nfrom t");
    ws.press("pageup");
    assert_eq!(ws.console().sql.range(), (0, 0), "PageUp moves the caret");
    ws.press("pagedown");
    let end = text(&ws).len();
    assert_eq!(
        ws.console().sql.range(),
        (end, end),
        "PageDown moves the caret"
    );
    ws.press("tab");
    assert!(editor_focused(&ws), "Tab stays in the editor");
}

#[test]
fn escape_leaves_the_idle_console() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);

    ws.type_text("select 1");
    ws.press("escape");
    assert!(tree_focused(&ws));
    assert_eq!(text(&ws), "select 1");
    assert_eq!(ws.browser().main, Main::Console);
}

#[test]
fn f6_and_cmd_j_reach_the_result_grid() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);

    assert!(editor_focused(&ws));
    ws.press("f6");
    assert!(
        tree_focused(&ws),
        "no result: F6 from the editor goes to the tree"
    );
    ws.press("f6");
    assert!(editor_focused(&ws));
    ws.press("cmd-j");
    assert!(editor_focused(&ws));
    assert_eq!(
        hint(&ws).as_deref(),
        Some("No result yet · run the script with ⌘↩ or F5")
    );
    ws.press("shift-f6");
    assert!(tree_focused(&ws), "back from the editor: the tree");

    ws.press("cmd-e");
    set_sql(&mut ws, "SELECT id, name FROM customers ORDER BY id");
    ws.press("cmd-enter");
    assert!(ws.wait_script_done().starts_with("8 rows"));
    ws.press("f6");
    assert!(
        grid_focused(&ws),
        "F6 from the editor goes to the result grid"
    );
    ws.press("down");
    assert_eq!(selected_cell(&ws), Some((1, 1)), "arrows move in the grid");
    ws.press("f6");
    assert!(tree_focused(&ws));
    ws.press("shift-f6");
    assert!(
        grid_focused(&ws),
        "Shift+F6 from the tree comes back to the result"
    );
    ws.press("shift-f6");
    assert!(editor_focused(&ws));
    ws.press("shift-f6");
    assert!(tree_focused(&ws));
    ws.press("f6 f6");
    assert!(grid_focused(&ws));

    ws.press("cmd-j");
    assert!(editor_focused(&ws));
    assert_eq!(
        text(&ws),
        "SELECT id, name FROM customers ORDER BY id",
        "⌘J types nothing"
    );
    ws.press("cmd-j");
    assert!(grid_focused(&ws));
    assert_eq!(selected_cell(&ws), Some((1, 1)), "the selection stayed");

    ws.press("space");
    assert_eq!(record(&ws), Some(1));
    ws.press("cmd-j");
    assert!(editor_focused(&ws));
    ws.press("cmd-j");
    assert!(result_focused(&ws));
    assert_eq!(record(&ws), Some(1));
    ws.press("escape");
    assert_eq!(record(&ws), None, "Esc closes the row view first");
    assert!(grid_focused(&ws));

    ws.press("cmd-1");
    ws.press("cmd-2");
    assert!(
        grid_focused(&ws),
        "the grid had the focus when the tab was left"
    );
    ws.press("cmd-j cmd-1 cmd-2");
    assert!(editor_focused(&ws));
}

#[test]
fn escape_in_the_result_grid_goes_to_the_tree_and_keeps_the_selection() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    set_sql(&mut ws, "SELECT id, name FROM customers ORDER BY id");
    ws.press("cmd-enter");
    ws.wait_script_done();

    ws.click_label("Fatima Al-Sayed");
    assert_eq!(selected_cell(&ws), Some((4, 2)));
    ws.press("escape");
    assert!(
        tree_focused(&ws),
        "Esc in the grid goes to the tree, like the hint says"
    );
    assert_eq!(selected_cell(&ws), Some((4, 2)), "the selection is kept");
    ws.press("cmd-e cmd-j down");
    assert_eq!(selected_cell(&ws), Some((5, 2)), "arrows go on from it");
}

#[test]
fn a_running_script_is_cancelled_from_any_pane() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    set_sql(&mut ws, ENDLESS);
    let start = |ws: &mut TestWorkspace| {
        ws.press("cmd-e cmd-enter");
        assert!(ws.console().is_running());
    };

    start(&mut ws);
    assert_eq!(
        running_status(&ws).as_deref(),
        Some("Running the script… 0s · Esc or ⌘. cancels it")
    );

    ws.press("cmd-p");
    ws.type_text("cust");
    assert!(ws.tree().is_filtering());
    assert!(running_status(&ws).is_some(), "shown with the tree focused");
    press_now(&mut ws, "escape");
    assert_eq!(ticker(&ws).as_deref(), Some("cancelling…"));
    assert_eq!(
        running_status(&ws).as_deref(),
        Some("Cancelling the script…")
    );
    ws.settle();
    assert!(tree_focused(&ws));
    assert!(
        ws.tree().is_filtering(),
        "that Esc did not clear the search"
    );
    assert_eq!(ws.wait_script_done(), "cancelled");
    assert_eq!(running_status(&ws), None);
    ws.press("escape");
    assert!(!ws.tree().is_filtering());

    start(&mut ws);
    ws.press("cmd-p");
    assert!(tree_focused(&ws));
    ws.press("cmd-.");
    assert_eq!(ws.wait_script_done(), "cancelled");
    assert!(tree_focused(&ws));

    start(&mut ws);
    ws.press("cmd-enter");
    assert_eq!(
        hint(&ws).as_deref(),
        Some("A script is already running · Esc cancels it")
    );
    ws.press("escape");
    assert_eq!(ws.wait_script_done(), "cancelled");

    start(&mut ws);
    ws.dispatch(Command::FocusTree);
    ws.type_text("customers");
    ws.press("enter");
    ws.wait_table_loaded("customers");
    assert_eq!(ws.browser().main, Main::Table);
    assert!(
        running_status(&ws).is_some(),
        "shown with the table in the main area"
    );
    ws.press("cmd-p f6");
    assert_eq!(focused_pane(&ws), Some(Pane::Main));
    ws.press("escape");
    assert_eq!(ws.wait_script_done(), "cancelled");
    assert_eq!(ws.browser().main, Main::Table);
    assert_eq!(
        focused_pane(&ws),
        Some(Pane::Main),
        "that Esc only cancelled"
    );
}

#[test]
fn the_editor_highlights_with_the_connection_dialect() {
    use crate::highlight::Token;
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    let tokens = |ws: &TestWorkspace, row: usize| -> Vec<(String, Token)> {
        let text = text(ws);
        let line = text.split('\n').nth(row).unwrap_or_default().to_string();
        let lines = ws.console().syntax.borrow();
        let mut start = 0;
        let mut out = Vec::new();
        for &(end, token) in lines.tokens(row) {
            if token != Token::Plain {
                out.push((line[start..end].to_string(), token));
            }
            start = end;
        }
        out
    };
    let owned = |found: &[(&str, Token)]| -> Vec<(String, Token)> {
        found
            .iter()
            .map(|(text, token)| (text.to_string(), *token))
            .collect()
    };
    set_sql(&mut ws, "SELECT [order], `x` -- c\n/* a\nb */ 'it''s', 1.5");

    let first = owned(&[
        ("SELECT", Token::Keyword),
        ("[order]", Token::Identifier),
        ("`x`", Token::Identifier),
        ("-- c", Token::Comment),
    ]);
    assert_eq!(tokens(&ws, 0), first);
    assert_eq!(tokens(&ws, 1), owned(&[("/* a", Token::Comment)]));
    let last = owned(&[
        ("b */", Token::Comment),
        ("'it''s'", Token::String),
        ("1.5", Token::Number),
    ]);
    assert_eq!(tokens(&ws, 2), last);

    ws.press("cmd-up");
    ws.type_text("-- ");
    assert_eq!(
        tokens(&ws, 0),
        owned(&[("-- SELECT [order], `x` -- c", Token::Comment)])
    );
    assert_eq!(tokens(&ws, 2), last);
    ws.press("backspace backspace backspace");
    assert_eq!(tokens(&ws, 0), first);

    let t = ws.state().theme();
    let styles = {
        let text = text(&ws);
        ws.console()
            .syntax
            .borrow()
            .styles(&(0..text.len()), &syntax::Palette::new(t))
    };
    let keyword = syntax::token_style(Token::Keyword, t).unwrap();
    assert_eq!(styles.first(), Some(&(0..6, keyword)));
}

fn grid_with_a_result(ws: &mut TestWorkspace) {
    set_sql(ws, "SELECT id, name FROM customers ORDER BY id");
    ws.press("cmd-e cmd-enter");
    let outcome = ws.wait_script_done();
    assert!(outcome.starts_with("8 rows"), "{outcome}");
    ws.press("cmd-j");
    assert!(grid_focused(ws));
}

#[test]
fn a_rerun_from_the_grid_keeps_the_keyboard_and_esc_cancels() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    grid_with_a_result(&mut ws);

    set_sql(&mut ws, ENDLESS);
    ws.press("f5");
    assert!(is_running(&ws));
    assert!(
        result_focused(&ws),
        "the result pane (always drawn) has the keyboard"
    );
    assert_eq!(focused_pane(&ws), Some(Pane::Main));
    press_now(&mut ws, "escape");
    assert_eq!(
        ticker(&ws).as_deref(),
        Some("cancelling…"),
        "Esc cancels, as the status says"
    );
    assert_eq!(ws.wait_script_done(), "cancelled");
    assert!(result_focused(&ws));
    assert_eq!(ws.console().area(), ConsoleArea::Message);

    ws.press("cmd-enter");
    assert!(is_running(&ws));
    ws.press("cmd-.");
    assert_eq!(ws.wait_script_done(), "cancelled");

    set_sql(&mut ws, "SELECT id, name FROM customers ORDER BY id");
    ws.press("f5");
    assert!(ws.wait_script_done().starts_with("8 rows"));
    assert!(grid_focused(&ws), "the new grid takes the keyboard");
    ws.press("down");
    assert_eq!(selected_cell(&ws), Some((1, 1)));
}

#[test]
fn a_failed_rerun_from_the_grid_leaves_f6_and_cmd_j_working() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    grid_with_a_result(&mut ws);

    set_sql(&mut ws, "SELECT * FROM no_such_table");
    ws.press("f5");
    assert!(ws.wait_script_done().starts_with("error:"));
    assert!(result_focused(&ws));
    assert_eq!(ws.console().area(), ConsoleArea::Message);
    ws.press("cmd-j");
    assert!(editor_focused(&ws), "⌘J goes to the editor");
    ws.press("cmd-j");
    assert!(editor_focused(&ws), "no grid to go back to");

    grid_with_a_result(&mut ws);
    set_sql(&mut ws, "SELECT * FROM no_such_table");
    ws.press("f5");
    assert!(ws.wait_script_done().starts_with("error:"));
    ws.press("f6");
    assert!(tree_focused(&ws), "F6 leaves the result pane");

    grid_with_a_result(&mut ws);
    set_sql(&mut ws, "CREATE TABLE scratch (x INTEGER)");
    ws.press("f5");
    ws.wait_script_done();
    assert!(result_focused(&ws));
    ws.press("shift-f6");
    assert!(
        editor_focused(&ws),
        "Shift+F6 from the result pane: the editor"
    );
    ws.press("f6");
    assert!(
        tree_focused(&ws),
        "no result grid: F6 from the editor goes to the tree"
    );
}

#[test]
fn coming_back_to_the_tab_returns_to_the_result_pane() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    grid_with_a_result(&mut ws);
    set_sql(&mut ws, "SELECT * FROM no_such_table");
    ws.press("f5");
    ws.wait_script_done();
    ws.press("cmd-1 cmd-2");
    assert!(result_focused(&ws));
    ws.press("escape");
    assert!(tree_focused(&ws), "Esc in the result pane goes to the tree");
}

#[test]
fn a_focused_element_that_is_no_longer_drawn_gives_the_keyboard_back() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    assert!(editor_focused(&ws));
    ws.rt.update(|_, tx| tx.blur());
    let mut pane = Some(Pane::Tree);
    ws.rt.update(|s, tx| {
        pane = s
            .active_browser()
            .and_then(|b| b.focused_pane(tx.focused()))
    });
    assert_eq!(pane, None, "no pane has the keyboard");
    ws.wait_until("the editor to get the keyboard back", editor_focused);

    ws.update(|s, _| {
        if let Some(b) = s.active_browser_mut() {
            b.main = Main::Table;
        }
    });
    ws.focus("table-view");
    assert_eq!(focused_pane(&ws), Some(Pane::Main));
    assert!(!editor_focused(&ws));
    ws.update(|s, _| {
        if let Some(b) = s.active_browser_mut() {
            b.main = Main::Console;
        }
    });
    assert!(!ws.exists("table-view"), "the focused element is gone");
    ws.wait_until("the editor to get the keyboard back", editor_focused);

    set_sql(&mut ws, ENDLESS);
    ws.press("cmd-enter");
    press_now(&mut ws, "escape");
    assert_eq!(ticker(&ws).as_deref(), Some("cancelling…"));
    assert_eq!(ws.wait_script_done(), "cancelled");
    assert_eq!(focused_pane(&ws), Some(Pane::Main));
}

#[test]
fn snapshots_of_the_console() {
    let Some(dir) = std::env::var_os("CONSOLE_SNAPS") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    for dark in [true, false] {
        let theme = if dark { "dark" } else { "light" };
        let shot = |ws: &mut TestWorkspace, name: &str| {
            crate::test_support::snap_png(&mut ws.rt, &dir.join(format!("{name}-{theme}.png")));
        };
        let config = TestConfig::with_sample_dbs(&["shop"]);
        let mut ws = TestWorkspace::open_with(config.store(), dark);
        ws.connect("shop");
        ws.press("cmd-e");
        shot(&mut ws, "01-empty");
        set_sql(
            &mut ws,
            "-- customers with their orders\nSELECT c.id, c.name, c.email, c.country,\n       count(o.id) AS orders, NULL AS note\nFROM customers c\nLEFT JOIN orders o ON o.customer_id = c.id\nWHERE c.name <> 'x' /* skip */\nGROUP BY c.id\nORDER BY c.id;",
        );
        ws.press("cmd-enter");
        ws.wait_script_done();
        ws.press("cmd-j down right");
        shot(&mut ws, "02-grid");
        ws.press("home");
        shot(&mut ws, "02b-grid-gutter");
        ws.press("right");
        ws.press("enter");
        shot(&mut ws, "03-record");
        ws.press("escape cmd-e");
        set_sql(&mut ws, ENDLESS);
        ws.press("cmd-enter");
        shot(&mut ws, "04-running");
        ws.press("cmd-.");
        ws.wait_script_done();
        shot(&mut ws, "05-cancelled");
        set_sql(&mut ws, "SELECT * FROM nowhere");
        ws.press("cmd-enter");
        ws.wait_script_done();
        shot(&mut ws, "06-failed");
        set_sql(
            &mut ws,
            "CREATE TABLE t1 (x); INSERT INTO t1 VALUES (1), (2);",
        );
        ws.press("cmd-enter");
        ws.wait_script_done();
        shot(&mut ws, "07-completed");
        ws.press("cmd-e");
        set_sql(
            &mut ws,
            &format!(
                "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r WHERE n < 1000) SELECT {} , replace(hex(zeroblob(300)), '0', 'long ') AS long_value FROM r",
                (1..=100)
                    .map(|i| format!("n * {i} AS column_{i}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
        ws.press("cmd-enter");
        ws.wait_script_done();
        ws.press("cmd-j");
        shot(&mut ws, "11-wide");
        for _ in 0..40 {
            ws.press("right down");
        }
        ws.press("pagedown");
        shot(&mut ws, "12-wide-scrolled");
        ws.press("end");
        shot(&mut ws, "12b-wide-end");
        ws.press("space");
        shot(&mut ws, "13-wide-record");
        ws.press("escape cmd-e cmd-f");
        ws.type_text("column_4");
        shot(&mut ws, "14-find");
    }
}

fn drag_across(ws: &mut TestWorkspace, key: &str) {
    let rect = ws.find(key).expect("the text to select").visible;
    let y = rect[1] + rect[3] / 2.;
    let (from, to) = (rect[0] + 1., rect[0] + rect[2] - 1.);
    ws.rt.pointer_move(from, y);
    ws.rt.pointer_down(from, y);
    ws.rt.frame(0.0);
    ws.rt.pointer_move(to, y);
    ws.rt.frame(0.0);
    ws.rt.pointer_up(to, y);
    ws.settle();
}

#[test]
fn cmd_c_copies_text_selected_in_the_result_pane() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);

    set_sql(&mut ws, "select * from nope");
    ws.press("cmd-enter");
    assert!(ws.wait_script_done().starts_with("error: "));
    drag_across(&mut ws, "script-error-text");
    assert!(result_focused(&ws));
    let selected = ws.rt.selected_text().expect("the error text is selected");
    assert!(selected.contains("no such table"), "{selected}");
    ws.clip.set_text("sentinel");
    ws.press("cmd-c");
    assert_eq!(ws.clipboard(), Some(selected));

    set_sql(
        &mut ws,
        "SELECT id, name, country FROM customers ORDER BY id",
    );
    ws.press("cmd-e cmd-enter");
    assert!(ws.wait_script_done().starts_with("8 rows"));
    ws.click_label("Fatima Al-Sayed");
    ws.press("enter");
    assert_eq!(record(&ws), Some(4));
    ws.press("cmd-c");
    assert_eq!(
        ws.clipboard().as_deref(),
        Some("Fatima Al-Sayed"),
        "with nothing selected ⌘C copies the cell"
    );
    drag_across(&mut ws, "record-value-2");
    assert_eq!(ws.rt.selected_text().as_deref(), Some("AE"));
    ws.press("cmd-c");
    assert_eq!(
        ws.clipboard().as_deref(),
        Some("AE"),
        "⌘C copies the selection"
    );
    ws.press("cmd-shift-c");
    assert_eq!(
        ws.clipboard().as_deref(),
        Some("5\tFatima Al-Sayed\tAE"),
        "⌘⇧C still copies the row"
    );
}

#[test]
fn scrolling_the_editor_moves_the_highlight_window_once_it_stops() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    let script = (0..2000)
        .map(|i| format!("select {i};"))
        .collect::<Vec<_>>()
        .join("\n");
    set_sql(&mut ws, &script);
    ws.settle();
    assert_eq!(ws.console().editor.block, 0);

    for step in 1..=10 {
        let y = step as f32 * 600.;
        ws.rt
            .update(move |_, tx| tx.scroll_to_instant(super::EDITOR_SCROLL, 0., y));
        ws.rt.frame(crate::test_support::DT);
        assert_eq!(
            ws.console().editor.block,
            0,
            "the span window stays put while the editor scrolls"
        );
    }
    assert!(ws.console().editor.pending.is_some());
    for _ in 0..20 {
        ws.rt.frame(crate::test_support::DT);
    }
    let block = ws.console().editor.block;
    assert_eq!(ws.console().editor.pending, None);
    assert_eq!(
        block,
        6000 / super::LINE_HEIGHT as usize / super::WINDOW_BLOCK
    );
}

#[test]
fn a_narrow_result_keeps_its_header_and_stripes_across_the_pane() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    grid_with_a_result(&mut ws);
    ws.settle();
    let pane = ws.find(grid::SCROLL).expect("the grid viewport").rect;
    let total = ws.console().grid.total_width();
    assert!(total < pane[2], "the columns are narrower than the pane");
    for key in ["console-grid-header", &grid::row_key(1)] {
        let rect = ws.find(key).expect("a grid row").rect;
        assert!(
            (rect[2] - pane[2]).abs() < 1.,
            "{key} spans the pane: {rect:?} vs {pane:?}"
        );
    }
}

fn type_keys(ws: &mut TestWorkspace, text: &str) {
    for c in text.chars() {
        ws.type_text(&c.to_string());
    }
}

fn caret(ws: &TestWorkspace) -> usize {
    ws.console().sql.range().1
}

#[test]
fn the_editor_closes_brackets_and_quotes_and_splits_brackets_on_return() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);

    type_keys(&mut ws, "select count(");
    assert_eq!(text(&ws), "select count()");
    assert_eq!(caret(&ws), 13, "the caret sits between the pair");
    type_keys(&mut ws, "*)");
    assert_eq!(text(&ws), "select count(*)", "a typed closer steps over");
    assert_eq!(caret(&ws), text(&ws).len());

    type_keys(&mut ws, ", '");
    assert_eq!(text(&ws), "select count(*), ''");
    type_keys(&mut ws, "it'");
    assert_eq!(
        text(&ws),
        "select count(*), 'it'",
        "a typed quote steps over"
    );
    assert_eq!(caret(&ws), text(&ws).len());
    ws.replace_text("select 'a  '");
    ws.press("left left");
    type_keys(&mut ws, "(");
    assert_eq!(text(&ws), "select 'a ( '", "no pair inside a string");
    ws.replace_text("select x");
    type_keys(&mut ws, "'");
    assert_eq!(text(&ws), "select x'", "no pair right after a word");
    ws.replace_text("select (x");
    ws.press("left");
    type_keys(&mut ws, "(");
    assert_eq!(text(&ws), "select ((x", "no pair before a word");
    ws.replace_text("-- a");
    type_keys(&mut ws, "(");
    assert_eq!(text(&ws), "-- a(", "no pair in a comment");

    ws.replace_text("select ");
    type_keys(&mut ws, "[");
    assert_eq!(text(&ws), "select []");
    ws.press("backspace");
    assert_eq!(text(&ws), "select ", "Backspace deletes an empty pair");
    ws.press("backspace");
    assert_eq!(text(&ws), "select");

    ws.replace_text("  insert into t values ");
    type_keys(&mut ws, "(");
    ws.press("enter");
    assert_eq!(text(&ws), "  insert into t values (\n    \n  )");
    assert_eq!(caret(&ws), "  insert into t values (\n    ".len());
    type_keys(&mut ws, "1");
    ws.press("enter");
    assert_eq!(text(&ws), "  insert into t values (\n    1\n    \n  )");
    ws.press("cmd-z");
    assert!(text(&ws).len() < "  insert into t values (\n    1\n    \n  )".len());
}

fn find_open(ws: &TestWorkspace) -> bool {
    ws.console().finding
}

fn selection(ws: &TestWorkspace) -> String {
    ws.console().sql.selected().to_string()
}

#[test]
fn cmd_f_finds_in_the_script() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = shop_console(&config);
    let mut script = String::from("select 1;\n");
    for i in 0..300 {
        script.push_str(&format!("-- filler {i}\n"));
    }
    script.push_str("SELECT name FROM customers;\nselect 2;");
    set_sql(&mut ws, &script);
    ws.press("cmd-up");

    ws.press("cmd-f");
    assert!(find_open(&ws));
    assert!(ws.focus_within(super::FIND), "⌘F focuses the find field");
    ws.type_text("name");
    assert_eq!(selection(&ws), "name", "typing selects the first match");
    assert_eq!(ws.text_of("console-find-status").as_deref(), Some("1 of 1"));
    let match_start = ws.console().sql.range().0;
    assert_eq!(match_start, script.find("name").unwrap());
    let field = ws.find(SQL).unwrap().rect;
    let pane = ws.find("console-editor-scroll").unwrap().rect;
    let line = (script[..match_start].matches('\n').count()) as f32;
    let y = field[1] + 4. + line * super::LINE_HEIGHT;
    assert!(
        y >= pane[1] && y < pane[1] + pane[3],
        "the match is scrolled into view: {y} in {pane:?}"
    );

    ws.press("cmd-a backspace");
    ws.type_text("SELECT");
    assert_eq!(
        selection(&ws),
        "select",
        "the search ignores case and goes on from the caret"
    );
    assert_eq!(ws.console().sql.range().0, script.rfind("select").unwrap());
    assert_eq!(ws.text_of("console-find-status").as_deref(), Some("3 of 3"));
    ws.press("enter");
    assert_eq!(ws.console().sql.range().0, 0, "the search wraps");
    assert_eq!(ws.text_of("console-find-status").as_deref(), Some("1 of 3"));
    ws.press("cmd-g");
    assert_eq!(selection(&ws), "SELECT");
    assert_eq!(ws.text_of("console-find-status").as_deref(), Some("2 of 3"));
    ws.press("enter");
    assert_eq!(ws.text_of("console-find-status").as_deref(), Some("3 of 3"));
    ws.press("shift-enter");
    assert_eq!(ws.text_of("console-find-status").as_deref(), Some("2 of 3"));
    ws.press("cmd-shift-g cmd-shift-g");
    assert_eq!(
        ws.text_of("console-find-status").as_deref(),
        Some("3 of 3"),
        "backwards wraps too"
    );
    ws.press("cmd-g");
    assert_eq!(text(&ws), script, "finding never edits the script");

    ws.type_text("x");
    assert_eq!(
        ws.text_of("console-find-status").as_deref(),
        Some("No matches")
    );

    ws.press("escape");
    assert!(!find_open(&ws));
    assert!(editor_focused(&ws), "Esc returns to the editor");
    assert_eq!(selection(&ws), "select", "the last match stays selected");
    assert!(!ws.exists(super::FIND));

    ws.press("cmd-f");
    assert!(find_open(&ws));
    assert_eq!(
        ws.console().find.text,
        "select",
        "the selection seeds the search"
    );
    ws.press("escape");
    ws.press("cmd-down");
    ws.press("cmd-f");
    ws.press("escape");
    assert!(editor_focused(&ws));
    ws.press("cmd-f");
    ws.press("cmd-enter");
    assert!(is_running(&ws) || ws.console().outcome.is_some());
}
