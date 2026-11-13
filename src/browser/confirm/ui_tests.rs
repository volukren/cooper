use std::path::{Path, PathBuf};

use super::{Confirm, MAX_LISTED, Pending};
use crate::{
    connection::{DbKind, Tag},
    dialog::Dialog,
    test_support::{TestConfig, TestWorkspace, snap_png},
};

fn tagged_shop(tag: Option<Tag>) -> (TestConfig, PathBuf) {
    let config = TestConfig::empty();
    let path = config.add_sample_db_tagged("shop", "shop", tag);
    (config, path)
}

fn open_shop(tag: Option<Tag>) -> (TestConfig, PathBuf, TestWorkspace) {
    let (config, path) = tagged_shop(tag);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    (config, path, ws)
}

fn ask(ws: &mut TestWorkspace, sql: &str) {
    ws.press("cmd-e");
    ws.replace_text(sql);
    ws.press("cmd-enter");
}

fn prompt(ws: &TestWorkspace) -> Option<&Confirm> {
    match &ws.state().dialog {
        Some(Dialog::Confirm(c)) => Some(c),
        _ => None,
    }
}

fn query_one(path: &Path, sql: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(sql, [], |row| row.get(0))
        .unwrap()
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

fn wait_closed(ws: &mut TestWorkspace) {
    ws.wait_until("the prompt to close", |ws| {
        !ws.has_dialog() && prompt(ws).is_none()
    });
}

fn stock(path: &Path) -> i64 {
    query_one(path, "SELECT stock FROM products WHERE id = 1")
}

#[test]
fn an_update_on_production_asks_and_cancel_changes_nothing() {
    let (_config, path, mut ws) = open_shop(Some(Tag::Prod));
    let sql = "UPDATE products SET stock = 0 WHERE id = 1";
    for how in ["escape", "space"] {
        ask(&mut ws, sql);
        assert!(prompt(&ws).is_some(), "{how}: the prompt is open");
        assert!(
            ws.is_focused("confirm-cancel"),
            "{how}: Cancel has the focus"
        );
        assert!(!ws.console().is_running());
        assert!(ws.console().outcome.is_none());
        assert_eq!(
            ws.text_of("confirm-statement-0-verb").as_deref(),
            Some("UPDATE")
        );
        assert!(!ws.exists("confirm-unbounded"));

        ws.press(how);
        wait_closed(&mut ws);
        assert_eq!(stock(&path), 12, "{how}");
        assert_eq!(
            status(&ws).as_deref(),
            Some("Cancelled: nothing was run"),
            "{how}"
        );
        assert_eq!(ws.console().text(), sql, "{how}: the editor keeps the SQL");
        assert!(ws.console().outcome.is_none());
    }
}

#[test]
fn confirming_runs_exactly_the_pending_script() {
    let (_config, path, mut ws) = open_shop(Some(Tag::Prod));
    let count = || query_one(&path, "SELECT count(*) FROM order_items");
    let before = count();
    ask(&mut ws, "DELETE FROM order_items WHERE id = 1");
    assert!(prompt(&ws).is_some());

    ws.press("enter");
    assert!(prompt(&ws).is_some(), "Enter does not confirm");
    ws.press("cmd-enter");
    assert!(prompt(&ws).is_some(), "⌘⏎ does not confirm");
    assert!(!ws.console().is_running());
    assert_eq!(count(), before);

    let tab = ws.tab_id();
    ws.update(move |w, _| {
        let b = w.browser_mut(tab).unwrap();
        b.console
            .sql
            .replace_text("DELETE FROM order_items".to_string());
    });
    ws.click("confirm-run");
    wait_closed(&mut ws);
    ws.wait_script_done();
    assert_eq!(count(), before - 1, "only the confirmed statement ran");
}

#[test]
fn delete_without_where_is_called_out() {
    let (_config, _path, mut ws) = open_shop(Some(Tag::Prod));
    ask(&mut ws, "DELETE FROM order_items");
    assert!(ws.exists("confirm-unbounded"));
    assert!(ws.exists("confirm-statement-0-unbounded"));
    ws.click("confirm-cancel");
    wait_closed(&mut ws);

    ask(&mut ws, "DELETE FROM order_items WHERE id = 2");
    assert!(prompt(&ws).is_some());
    assert!(!ws.exists("confirm-unbounded"));
    assert!(!ws.exists("confirm-statement-0-unbounded"));
}

#[test]
fn many_statements_are_capped() {
    let (_config, _path, mut ws) = open_shop(Some(Tag::Prod));
    let script: Vec<String> = (1..=8)
        .map(|id| format!("UPDATE products SET stock = stock WHERE id = {id};"))
        .collect();
    ask(&mut ws, &script.join("\n"));
    assert_eq!(prompt(&ws).map(|c| c.statements.len()), Some(8));
    for i in 0..MAX_LISTED {
        assert!(ws.exists(&format!("confirm-statement-{i}")), "row {i}");
    }
    assert!(!ws.exists(&format!("confirm-statement-{MAX_LISTED}")));
    assert_eq!(ws.text_of("confirm-more").as_deref(), Some("…and 3 more"));
}

#[test]
fn read_only_scripts_run_without_asking_on_production() {
    let (_config, _path, mut ws) = open_shop(Some(Tag::Prod));
    let done = ws.run_sql(
        "SELECT count(*) FROM customers; EXPLAIN QUERY PLAN SELECT 1; \
         PRAGMA table_info(customers)",
    );
    assert!(prompt(&ws).is_none(), "{done}");
    let done = ws.run_sql("-- DELETE FROM customers\nSELECT 1");
    assert!(prompt(&ws).is_none(), "{done}");
    assert!(done.starts_with("1 row"), "{done}");
}

#[test]
fn untagged_and_dev_connections_never_ask() {
    for tag in [None, Some(Tag::Dev)] {
        let (_config, path, mut ws) = open_shop(tag);
        ws.run_sql("DELETE FROM order_items WHERE id = 3");
        assert!(prompt(&ws).is_none(), "{tag:?}");
        assert_eq!(
            query_one(&path, "SELECT count(*) FROM order_items WHERE id = 3"),
            0,
            "{tag:?}"
        );
    }
}

#[test]
fn a_row_edit_on_production_asks_before_saving() {
    let (_config, path, mut ws) = open_shop(Some(Tag::Prod));
    ws.open_table("customers");
    ws.press("down space");
    ws.click_label("Edit name");
    ws.replace_text("Renamed");
    ws.press("cmd-enter");

    let c = prompt(&ws).expect("the prompt is open");
    assert!(matches!(&c.pending, Pending::Row { row, .. } if row == "row 2"));
    let intro = ws.text_of("confirm-intro").unwrap_or_default();
    assert!(
        intro.ends_with("This change to row 2 of main.customers will be saved:"),
        "{intro}"
    );
    assert_eq!(
        ws.text_of("confirm-statement-0-verb").as_deref(),
        Some("UPDATE")
    );
    let label = ws
        .find("confirm-statement-0")
        .and_then(|e| e.label)
        .unwrap_or_default();
    assert!(
        label.contains("customers") && label.contains("Renamed"),
        "{label}"
    );

    ws.press("escape");
    wait_closed(&mut ws);
    let draft = ws.table_view().draft().expect("the edit is kept");
    assert!(!draft.saving);
    assert!(draft.update().is_some(), "the change is kept");
    assert_eq!(customer_name(&path, 2), "José García");
    assert_eq!(status(&ws).as_deref(), Some("Cancelled: nothing was saved"));

    ws.press("cmd-enter");
    assert!(prompt(&ws).is_some());
    ws.press("tab space");
    wait_closed(&mut ws);
    ws.wait_for_state("the save", |s| {
        s.active_browser()
            .is_some_and(|b| b.table.draft.is_none() && b.table.save_request.is_none())
    });
    assert_eq!(customer_name(&path, 2), "Renamed");
    assert_eq!(status(&ws).as_deref(), Some("Saved row 2"));
}

#[test]
fn losing_the_tab_drops_the_prompt() {
    let (_config, path, mut ws) = open_shop(Some(Tag::Prod));
    ask(&mut ws, "UPDATE products SET stock = 0 WHERE id = 1");
    assert!(prompt(&ws).is_some());
    ws.update(|w, tx| {
        let index = w.tab_index("shop").unwrap();
        w.fail(index, "gone".into(), tx);
    });
    assert!(ws.state().dialog.is_none());
    assert!(!ws.has_dialog());
    assert_eq!(stock(&path), 12);
}

#[test]
fn production_tabs_show_a_banner_and_badges() {
    let (config, _path) = tagged_shop(Some(Tag::Prod));
    let mut ws = TestWorkspace::open(&config);
    assert_eq!(
        ws.find("connection-0-tag").and_then(|e| e.label).as_deref(),
        Some("Production")
    );
    assert!(ws.exists("details-tag"));
    let id = ws.connect("shop");
    assert!(ws.exists(&format!("tab-{}-tag", id.0)));
    assert!(ws.is_visible("prod-banner"));

    let (config, _path) = tagged_shop(Some(Tag::Dev));
    let mut ws = TestWorkspace::open(&config);
    assert_eq!(
        ws.find("connection-0-tag").and_then(|e| e.label).as_deref(),
        Some("Development")
    );
    let id = ws.connect("shop");
    assert!(ws.exists(&format!("tab-{}-tag", id.0)));
    assert!(!ws.exists("prod-banner"));
}

#[test]
fn a_row_prompt_does_not_save_an_edit_that_went_away() {
    let (_config, path, mut ws) = open_shop(Some(Tag::Prod));
    ws.open_table("customers");
    ws.press("down space");
    ws.click_label("Edit name");
    ws.replace_text("Renamed");
    ws.press("cmd-enter");
    assert!(prompt(&ws).is_some());

    let tab = ws.tab_id();
    ws.update(move |w, _| w.browser_mut(tab).unwrap().table.draft = None);
    ws.click("confirm-run");
    wait_closed(&mut ws);
    assert_eq!(
        status(&ws).as_deref(),
        Some("Not saved: the row is no longer being edited")
    );
    assert!(ws.table_view().save_request.is_none());
    assert_eq!(customer_name(&path, 2), "José García");
}

fn select_connection(ws: &mut TestWorkspace, name: &str) {
    let index = ws
        .read(|w| w.home.connections.iter().position(|c| c.name == name))
        .expect("the connection is listed");
    ws.update(move |w, _| w.home.click_row(index));
}

#[test]
#[ignore = "renders PNG snapshots into $COOPER_GUARD_SNAPS"]
fn guard_snapshots() {
    let Some(out) = std::env::var_os("COOPER_GUARD_SNAPS").map(PathBuf::from) else {
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
        config.add_sample_db_tagged("shop", "shop", Some(Tag::Prod));
        config.add_sample_db_tagged("shop-staging", "shop", Some(Tag::Staging));
        config.add_sample_db_tagged("analytics", "analytics", Some(Tag::Test));
        config.add_sample_db_tagged("library", "library", Some(Tag::Dev));
        config.add_sample_db_tagged("scratch", "shop", None);

        let mut ws = TestWorkspace::open_with(config.store(), dark);
        ws.rt.set_viewport((1280.0, 820.0), 2.0);
        ws.settle();
        select_connection(&mut ws, "shop");
        snap(&mut ws, "home-tags");

        ws.press("cmd-n");
        ws.type_text("billing");
        ws.update(|w, _| {
            if let Some(Dialog::Form(session)) = &mut w.dialog {
                session.form.set_kind(DbKind::Postgres);
                session.form.test_set_text("host", "db.billing.internal");
                session.form.test_set_text("user", "app");
                session.form.test_set_text("database", "billing");
            }
        });
        ws.click("form-tag-trigger");
        snap(&mut ws, "form-environment");
        ws.click_in("form-tag-content", "Production");
        snap(&mut ws, "form-production");
        ws.press("escape");
        ws.wait_until("the form to close", |ws| !ws.has_dialog());

        ws.connect("library");
        ws.connect("shop");
        ws.run_sql("SELECT id, name, country FROM customers ORDER BY id");
        snap(&mut ws, "prod-console");

        ask(
            &mut ws,
            "UPDATE products SET stock = stock - 1 WHERE id = 3;\n\
             DELETE FROM order_items;\n\
             SELECT count(*) FROM orders;",
        );
        assert!(prompt(&ws).is_some());
        snap(&mut ws, "confirm-script");
        ws.press("escape");
        wait_closed(&mut ws);

        ws.open_table("customers");
        ws.press("down space");
        ws.click_label("Edit name");
        ws.replace_text("José García-López");
        ws.press("cmd-enter");
        assert!(prompt(&ws).is_some());
        snap(&mut ws, "confirm-row");
        ws.press("escape");
        wait_closed(&mut ws);
    }
}
