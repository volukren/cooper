use std::path::{Path, PathBuf};

use super::READ_ONLY_EDIT;
use crate::{
    browser::console::Outcome,
    connection::{Connection, Tag},
    dialog::Dialog,
    test_support::{TestConfig, TestWorkspace, build_sample_db, snap_png, sqlite_connection},
};

fn add_shop(config: &TestConfig, name: &str, tag: Option<Tag>, read_only: bool) -> PathBuf {
    let data = config.dir().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let path = data.join(format!("{name}.db"));
    build_sample_db("shop", &path);
    config.add_connection(Connection {
        tag,
        read_only,
        ..sqlite_connection(name, &path)
    });
    path
}

fn open_shop(tag: Option<Tag>) -> (TestConfig, PathBuf, TestWorkspace) {
    let config = TestConfig::empty();
    let path = add_shop(&config, "shop", tag, true);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    (config, path, ws)
}

fn ask(ws: &mut TestWorkspace, sql: &str) {
    ws.press("cmd-e");
    ws.replace_text(sql);
    ws.press("cmd-enter");
}

fn query_one(path: &Path, sql: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(sql, [], |row| row.get(0))
        .unwrap()
}

fn stock(path: &Path) -> i64 {
    query_one(path, "SELECT stock FROM products WHERE id = 1")
}

fn customer_name(path: &Path, id: i64) -> String {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row("SELECT name FROM customers WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .unwrap()
}

fn status(ws: &TestWorkspace) -> Option<String> {
    ws.browser().status.as_ref().map(|(text, _)| text.clone())
}

fn refused(ws: &TestWorkspace) -> Option<String> {
    match &ws.console().outcome {
        Some(Outcome::Refused(message)) => Some(message.clone()),
        _ => None,
    }
}

#[test]
fn reads_run_as_usual_on_a_read_only_connection() {
    let (_config, _path, mut ws) = open_shop(None);
    let done = ws.run_sql("SELECT count(*) FROM customers; SELECT * FROM products");
    assert!(refused(&ws).is_none(), "{done}");
    assert!(ws.console().result().is_some(), "{done}");
    assert!(!ws.exists("console-refused"));

    let done = ws.run_sql("-- DELETE FROM customers\nEXPLAIN QUERY PLAN SELECT 1");
    assert!(refused(&ws).is_none(), "{done}");

    ws.open_table("customers");
    ws.wait_table_loaded("customers");
}

#[test]
fn a_script_that_writes_is_not_run_on_a_read_only_connection() {
    let (_config, path, mut ws) = open_shop(None);
    let sql = "SELECT 1; UPDATE products SET stock = 0 WHERE id = 1";
    ask(&mut ws, sql);

    assert!(!ws.console().is_running(), "nothing was sent to run");
    assert!(!ws.has_dialog() && ws.state().dialog.is_none());
    let message = refused(&ws).expect("the script was refused");
    assert!(
        message.starts_with("Nothing was run: this connection is read-only"),
        "{message}"
    );
    assert!(
        message.ends_with("UPDATE products SET stock = 0 WHERE id = 1"),
        "{message}"
    );
    assert!(ws.is_visible("console-refused"));
    assert!(!ws.exists("console-failed"));
    assert_eq!(
        ws.text_of("script-error-text").as_deref(),
        Some(message.as_str())
    );
    assert_eq!(
        ws.console().outcome_text().as_deref(),
        Some(format!("refused: {message}").as_str())
    );
    assert_eq!(ws.console().text(), sql, "the editor keeps the SQL");
    assert_eq!(stock(&path), 12);

    ws.click("copy-error");
    assert_eq!(ws.clipboard().as_deref(), Some(message.as_str()));

    let done = ws.run_sql("SELECT stock FROM products WHERE id = 1");
    assert!(
        refused(&ws).is_none(),
        "a read replaces the refusal: {done}"
    );
    assert!(!ws.exists("console-refused"));
}

#[test]
fn every_kind_of_write_is_refused_and_changes_nothing() {
    let (_config, path, mut ws) = open_shop(None);
    let orders = || query_one(&path, "SELECT count(*) FROM order_items");
    let before = orders();
    for sql in [
        "DELETE FROM order_items",
        "INSERT INTO order_items SELECT * FROM order_items",
        "DROP TABLE order_items",
        "CREATE TABLE made (id INTEGER)",
        "WITH gone AS (SELECT 1) DELETE FROM order_items",
        "PRAGMA query_only = OFF; DELETE FROM order_items",
        "ATTACH DATABASE ':memory:' AS other",
        "SELECT 'unterminated",
    ] {
        ask(&mut ws, sql);
        assert!(refused(&ws).is_some(), "{sql}");
        assert!(!ws.console().is_running(), "{sql}");
    }
    assert_eq!(orders(), before);
    assert_eq!(
        query_one(
            &path,
            "SELECT count(*) FROM sqlite_master WHERE name = 'made'"
        ),
        0
    );
}

#[test]
fn only_the_selected_text_is_judged() {
    let (_config, path, mut ws) = open_shop(None);
    ws.press("cmd-e");
    ws.type_text("DELETE FROM order_items;");
    ws.press("enter");
    ws.type_text("SELECT 1");
    ws.press("shift-home");
    ws.press("cmd-enter");
    let done = ws.wait_script_done();
    assert!(refused(&ws).is_none(), "{done}");
    assert!(done.starts_with("1 row"), "{done}");

    ws.press("cmd-a cmd-enter");
    assert!(refused(&ws).is_some());
    assert!(query_one(&path, "SELECT count(*) FROM order_items") > 0);
}

#[test]
fn a_read_only_production_connection_refuses_instead_of_asking() {
    let (_config, path, mut ws) = open_shop(Some(Tag::Prod));
    assert!(ws.is_visible("read-only-banner"));
    assert!(!ws.exists("prod-banner"), "one banner: nothing to confirm");

    ask(&mut ws, "UPDATE products SET stock = 0 WHERE id = 1");
    assert!(ws.state().dialog.is_none(), "no confirmation prompt");
    assert!(refused(&ws).is_some());
    assert_eq!(stock(&path), 12);
}

#[test]
fn rows_cannot_be_edited_on_a_read_only_connection() {
    let (_config, path, mut ws) = open_shop(None);
    ws.open_table("customers");

    ws.press("down f2");
    assert!(ws.table_view().draft().is_none(), "F2 opens no editor");
    assert_eq!(status(&ws).as_deref(), Some(READ_ONLY_EDIT));

    ws.press("space");
    assert!(
        ws.by_label("Edit name").is_none(),
        "values of the row view are not editable"
    );
    ws.press("f2");
    assert!(ws.table_view().draft().is_none());
    assert_eq!(status(&ws).as_deref(), Some(READ_ONLY_EDIT));
    assert!(ws.table_view().save_request.is_none());
    assert_eq!(customer_name(&path, 2), "José García");
}

#[test]
fn rows_stay_editable_on_other_connections() {
    let config = TestConfig::empty();
    let path = add_shop(&config, "shop", None, false);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    assert!(!ws.exists("read-only-banner"));

    ws.open_table("customers");
    ws.press("down space");
    ws.click_label("Edit name");
    ws.replace_text("Renamed");
    ws.press("cmd-enter");
    ws.wait_for_state("the edit to be saved", |s| {
        s.active_browser()
            .is_some_and(|b| b.table.draft.is_none() && b.table.save_request.is_none())
    });
    assert_eq!(customer_name(&path, 2), "Renamed");

    let done = ws.run_sql("UPDATE products SET stock = 0 WHERE id = 1");
    assert!(refused(&ws).is_none(), "{done}");
    assert_eq!(stock(&path), 0);
}

#[test]
fn a_save_that_slips_through_is_refused_before_it_reaches_the_database() {
    let (_config, path, mut ws) = open_shop(None);
    let tab = ws.tab_id();
    let id = ws.update(move |w, _| {
        let b = w.browser_mut(tab).unwrap();
        b.session.update_row(crate::db::RowUpdate {
            schema: "main".into(),
            table: "customers".into(),
            key: vec![("id".into(), "2".into())],
            values: vec![("name".into(), Some("Renamed".into()))],
        })
    });
    let sql_id = ws.update(move |w, _| {
        let b = w.browser_mut(tab).unwrap();
        b.session
            .run_script("UPDATE products SET stock = 0 WHERE id = 1", 10)
    });
    assert!(sql_id > id);
    ws.settle();
    std::thread::sleep(std::time::Duration::from_millis(200));
    ws.settle();
    assert_eq!(customer_name(&path, 2), "José García");
    assert_eq!(stock(&path), 12);
}

#[test]
fn read_only_connections_show_a_banner_and_badges() {
    let config = TestConfig::empty();
    add_shop(&config, "shop", Some(Tag::Staging), true);
    add_shop(&config, "shop-rw", None, false);
    let mut ws = TestWorkspace::open(&config);
    assert_eq!(
        ws.find("connection-0-read-only")
            .and_then(|e| e.label)
            .as_deref(),
        Some("Read-only")
    );
    assert!(ws.exists("connection-0-tag"), "the tag stays next to it");
    assert!(!ws.exists("connection-1-read-only"));
    assert!(ws.exists("details-read-only"));
    assert!(ws.by_text("Access").is_some());

    let id = ws.connect("shop");
    assert!(ws.exists(&format!("tab-{}-read-only", id.0)));
    assert!(ws.exists(&format!("tab-{}-tag", id.0)));
    assert!(ws.is_visible("read-only-banner"));
    assert!(ws.overflows().is_empty(), "{:?}", ws.overflows());

    ws.press("cmd-1");
    ws.press("down");
    assert!(!ws.exists("details-read-only"));
    assert!(ws.by_text("Access").is_none());
    let id = ws.connect("shop-rw");
    assert!(!ws.exists(&format!("tab-{}-read-only", id.0)));
    assert!(!ws.exists("read-only-banner"));
}

fn tooltip_over(ws: &mut TestWorkspace, key: &str) -> Option<String> {
    let rect = ws
        .find(key)
        .unwrap_or_else(|| panic!("no element keyed {key:?}"))
        .visible;
    let (x, y) = crate::test_support::center(rect);
    ws.rt.pointer_move(x, y);
    ws.rt.frame(crate::test_support::DT);
    let delay = ws.rt.help_delay;
    ws.rt.frame(delay + 0.1);
    ws.rt.help_tooltip().map(|(text, _)| text.to_string())
}

#[test]
fn hovering_the_lock_says_read_only() {
    let config = TestConfig::empty();
    add_shop(&config, "shop", Some(Tag::Staging), true);
    add_shop(&config, "shop-rw", None, false);
    let mut ws = TestWorkspace::open(&config);
    assert_eq!(
        tooltip_over(&mut ws, "connection-0-read-only").as_deref(),
        Some("Read-only")
    );
    assert_eq!(
        tooltip_over(&mut ws, "connection-1").as_deref(),
        None,
        "{}",
        ws.describe()
    );

    let id = ws.connect("shop");
    assert_eq!(
        tooltip_over(&mut ws, &format!("tab-{}-read-only", id.0)).as_deref(),
        Some("shop · Read-only")
    );
    assert_eq!(
        tooltip_over(&mut ws, &format!("tab-{}", id.0)).as_deref(),
        Some("shop · Read-only"),
        "the whole tab says it"
    );

    ws.press("cmd-1");
    let id = ws.connect("shop-rw");
    assert_eq!(
        tooltip_over(&mut ws, &format!("tab-{}", id.0)).as_deref(),
        Some("shop-rw")
    );
}

#[test]
#[ignore = "renders PNG snapshots into $COOPER_READ_ONLY_SNAPS"]
fn read_only_snapshots() {
    let Some(out) = std::env::var_os("COOPER_READ_ONLY_SNAPS").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let theme = if dark { "dark" } else { "light" };
        let snap = |ws: &mut TestWorkspace, name: &str| {
            ws.settle();
            snap_png(&mut ws.rt, &out.join(format!("{name}-{theme}.png")));
        };
        let config = TestConfig::empty();
        add_shop(&config, "shop", Some(Tag::Prod), true);
        add_shop(&config, "shop-staging", Some(Tag::Staging), false);
        add_shop(&config, "replica", None, true);
        add_shop(&config, "scratch", None, false);

        let mut ws = TestWorkspace::open_with(config.store(), dark);
        ws.rt.set_viewport((1280.0, 820.0), 2.0);
        snap(&mut ws, "01-home");

        ws.press("cmd-n");
        ws.type_text("billing replica");
        ws.update(|w, _| {
            if let Some(Dialog::Form(session)) = &mut w.dialog {
                session
                    .form
                    .test_set_text("host", "replica.billing.internal");
                session.form.test_set_text("user", "report");
                session.form.test_set_text("database", "billing");
            }
        });
        ws.click("form-read-only");
        snap(&mut ws, "02-form");
        ws.press("escape");
        ws.wait_until("the form to close", |ws| !ws.has_dialog());

        ws.connect("shop");
        ask(
            &mut ws,
            "SELECT count(*) FROM orders;\n\
             UPDATE products SET stock = stock - 1 WHERE id = 3;\n\
             DELETE FROM order_items;",
        );
        assert!(refused(&ws).is_some());
        snap(&mut ws, "03-refused");

        ws.open_table("customers");
        ws.press("down space f2");
        assert_eq!(status(&ws).as_deref(), Some(READ_ONLY_EDIT));
        snap(&mut ws, "04-row");
    }
}
