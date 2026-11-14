use std::{path::Path, rc::Rc};

use super::{Focus, TabState, Workspace};
use crate::{
    browser::Browser,
    connection::{Connection, DbKind, PasswordStorage, Store, Tag},
    dialog::Dialog,
    form::{ConnectionForm, FormValues},
    keys::Command,
    settings::{Fonts, Page, ThemeMode},
    test_support::{TestConfig, TestWorkspace, sqlite_connection},
};

fn names(ws: &TestWorkspace) -> Vec<String> {
    ws.read(|w| w.test_connection_names())
}

fn selected(ws: &TestWorkspace) -> Option<String> {
    ws.read(|w| w.test_selected_connection())
}

fn focus(ws: &TestWorkspace) -> Focus {
    ws.read(|w| w.home.focus)
}

fn status(ws: &TestWorkspace) -> Option<String> {
    ws.read(|w| w.home.status.clone())
}

fn status_line(ws: &TestWorkspace) -> String {
    ws.read(|w| {
        w.home
            .status_line()
            .map(|(text, _)| text)
            .unwrap_or_default()
    })
}

fn tabs(ws: &TestWorkspace) -> Vec<(String, bool)> {
    ws.read(|w| w.test_tabs())
}

fn active(ws: &TestWorkspace) -> Option<usize> {
    ws.read(|w| w.active)
}

fn form_of(w: &Workspace) -> &ConnectionForm {
    match &w.dialog {
        Some(Dialog::Form(session)) => &session.form,
        _ => panic!("the connection form is open"),
    }
}

fn form_mut(w: &mut Workspace) -> &mut ConnectionForm {
    match &mut w.dialog {
        Some(Dialog::Form(session)) => &mut session.form,
        _ => panic!("the connection form is open"),
    }
}

fn values(ws: &TestWorkspace) -> FormValues {
    ws.read(|w| form_of(w).values())
}

fn form_original(ws: &TestWorkspace) -> Option<String> {
    ws.read(|w| form_of(w).original.as_ref().map(|c| c.name.clone()))
}

fn form_error(ws: &TestWorkspace) -> Option<String> {
    ws.read(|w| form_of(w).error.clone())
}

fn focused_field(ws: &TestWorkspace) -> Option<&'static str> {
    let field = ws.read(|w| form_of(w).focused_field());
    field.or_else(|| ws.focus_within("form-kind").then_some("kind"))
}

fn set_text(ws: &mut TestWorkspace, field: &'static str, text: &str) {
    let text = text.to_string();
    ws.update(move |w, _| form_mut(w).test_set_text(field, &text));
}

fn set_kind(ws: &mut TestWorkspace, kind: DbKind) {
    ws.update(move |w, _| form_mut(w).set_kind(kind));
}

fn focus_field(ws: &mut TestWorkspace, field: &'static str) {
    ws.update(move |w, _| form_mut(w).focus_field(field));
}

fn help_query(ws: &TestWorkspace) -> Option<String> {
    ws.read(|w| match &w.dialog {
        Some(Dialog::Help(help)) => Some(help.query().to_string()),
        _ => None,
    })
}

fn help_match_count(ws: &TestWorkspace) -> Option<usize> {
    ws.read(|w| match &w.dialog {
        Some(Dialog::Help(help)) => Some(help.match_count()),
        _ => None,
    })
}

fn tab_browser<'a>(ws: &'a TestWorkspace, name: &str) -> &'a Browser {
    ws.state()
        .tabs
        .iter()
        .find(|tab| tab.name() == name)
        .and_then(|tab| tab.browser())
        .unwrap_or_else(|| panic!("no connected tab for {name:?}"))
}

fn lose(ws: &mut TestWorkspace, name: &str, message: &str) {
    let (name, message) = (name.to_string(), message.to_string());
    ws.update(move |w, tx| {
        let index = w.tab_index(&name).expect("the tab is open");
        w.fail(index, message, tx);
    });
}

fn click_row(ws: &mut TestWorkspace, position: usize) {
    ws.click(&format!("connection-{position}"));
}

fn click_menu(ws: &mut TestWorkspace, label: &str) {
    ws.click_in("connection-menu-menu", label);
}

fn click_dialog_delete(ws: &mut TestWorkspace) {
    ws.click_in("delete-connection-dialog", "Delete");
}

fn wait_dialog_closed(ws: &mut TestWorkspace) {
    ws.wait_until("the dialog to close", |ws| {
        !ws.has_dialog() && ws.read(|w| w.dialog.is_none())
    });
}

fn hold_connecting(ws: &mut TestWorkspace, hold: bool) {
    ws.update(move |w, _| w.hold_connecting = hold);
}

fn postgres_plain(name: &str) -> Connection {
    Connection {
        name: name.into(),
        kind: DbKind::Postgres,
        tag: None,
        host: "db.invalid".into(),
        port: Some(5432),
        user: "app".into(),
        database: "orders".into(),
        password_storage: PasswordStorage::Plain,
        password: "pw".into(),
    }
}

fn three_connections() -> TestConfig {
    let config = TestConfig::empty();
    config.add_connection(postgres_plain("prod orders"));
    let local = config.add_sample_db("shop");
    let staging = config.add_sample_db("library");
    let store = config.store();
    store
        .update("shop", &sqlite_connection("local", &local))
        .unwrap();
    store
        .update("library", &sqlite_connection("Staging Orders", &staging))
        .unwrap();
    config
}

fn store_names(path: &Path) -> Vec<String> {
    Store::at(path.to_path_buf())
        .load()
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect()
}

#[test]
fn startup_focuses_the_list_and_selects_the_first_connection() {
    let config = three_connections();
    let ws = TestWorkspace::open(&config);
    assert_eq!(names(&ws), ["prod orders", "local", "Staging Orders"]);
    assert_eq!(selected(&ws).as_deref(), Some("prod orders"));
    assert_eq!(focus(&ws), Focus::Connections);
    assert_eq!(status_line(&ws), "", "no key hints at the bottom");
    assert!(
        ws.is_focused("home"),
        "the home screen starts in its connection list\n{}",
        ws.describe()
    );
    assert!(tabs(&ws).is_empty());
}

#[test]
fn empty_config_shows_the_new_button() {
    let config = TestConfig::empty();
    let mut ws = TestWorkspace::open(&config);
    assert!(names(&ws).is_empty());
    assert_eq!(focus(&ws), Focus::NewButton);
    assert_eq!(selected(&ws), None);
    assert_eq!(status_line(&ws), "", "no key hints at the bottom");
    assert!(!ws.has_dialog());
    ws.press("enter");
    assert!(
        ws.read(|w| w.test_form_open()),
        "Enter on the New button opens the form"
    );
    ws.press("escape");
    wait_dialog_closed(&mut ws);
    ws.click_label("New connection");
    assert!(ws.read(|w| w.test_form_open()));
    ws.press("escape");
    wait_dialog_closed(&mut ws);
}

#[test]
fn arrows_reach_the_new_button_and_return_to_the_list() {
    let config = three_connections();
    let mut ws = TestWorkspace::open(&config);
    ws.press("up");
    assert_eq!(focus(&ws), Focus::NewButton, "up from the first row");
    ws.press("up");
    assert_eq!(focus(&ws), Focus::NewButton);
    ws.press("cmd-e");
    ws.press("cmd-backspace");
    ws.dispatch(Command::DeleteConnection);
    assert!(!ws.has_dialog());
    assert_eq!(names(&ws).len(), 3);

    click_row(&mut ws, 2);
    assert_eq!(focus(&ws), Focus::Connections);
    assert_eq!(selected(&ws).as_deref(), Some("Staging Orders"));
    ws.press("up up up");
    assert_eq!(focus(&ws), Focus::NewButton);
    ws.press("down");
    assert_eq!(focus(&ws), Focus::Connections);
    assert_eq!(selected(&ws).as_deref(), Some("prod orders"));

    ws.press("up enter");
    assert!(ws.read(|w| w.test_form_open()));
    assert!(form_original(&ws).is_none());
    ws.press("escape");
    wait_dialog_closed(&mut ws);
}

#[test]
fn a_connections_context_menu_connects_edits_and_deletes() {
    let config = TestConfig::with_sample_dbs(&["shop", "library"]);
    let mut ws = TestWorkspace::open(&config);

    ws.right_click("connection-1");
    assert_eq!(
        selected(&ws).as_deref(),
        Some("library"),
        "a right click selects"
    );
    assert!(ws.has_label("Edit…"));
    ws.press("escape");
    assert!(!ws.has_label("Edit…"), "Esc closes the menu");
    assert_eq!(ws.read(|w| w.home.menu), None);

    ws.right_click("connection-1");
    click_menu(&mut ws, "Edit…");
    assert!(ws.has_dialog());
    assert_eq!(form_original(&ws), Some("library".into()));
    assert_eq!(ws.read(|w| w.home.menu), None);
    ws.press("escape");
    wait_dialog_closed(&mut ws);

    ws.right_click("connection-1");
    click_menu(&mut ws, "Delete…");
    assert!(ws.has_dialog());
    click_dialog_delete(&mut ws);
    wait_dialog_closed(&mut ws);
    assert_eq!(names(&ws), ["shop"]);

    ws.right_click("connection-0");
    click_menu(&mut ws, "Connect");
    ws.wait_connected("shop");
    assert_eq!(active(&ws), Some(0));

    ws.press("cmd-1");
    assert_eq!(active(&ws), None);
    ws.right_click("connection-0");
    assert!(
        ws.has_label("Switch to Tab"),
        "an open connection offers its tab"
    );
    click_menu(&mut ws, "Switch to Tab");
    assert_eq!(active(&ws), Some(0));
    assert_eq!(tabs(&ws).len(), 1);
}

#[test]
fn enter_connects_and_close_returns_to_the_list() {
    let config = TestConfig::with_sample_dbs(&["shop", "library"]);
    let mut ws = TestWorkspace::open(&config);
    hold_connecting(&mut ws, true);
    ws.press("enter");
    assert_eq!(tabs(&ws), [("shop".to_string(), true)]);
    assert_eq!(active(&ws), Some(0));
    hold_connecting(&mut ws, false);
    ws.wait_connected("shop");
    assert!(ws.browser().status.as_ref().is_none_or(|(_, error)| !error));

    ws.press("cmd-w");
    assert!(tabs(&ws).is_empty());
    assert_eq!(active(&ws), None);
    assert_eq!(status(&ws).as_deref(), Some("Disconnected: \"shop\""));
    click_row(&mut ws, 1);
    assert_eq!(selected(&ws).as_deref(), Some("library"));
    assert_eq!(status(&ws), None, "the next action clears the message");
}

#[test]
fn connecting_can_be_cancelled() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    hold_connecting(&mut ws, true);
    ws.press("enter");
    assert_eq!(tabs(&ws), [("shop".to_string(), true)]);
    ws.press("x");
    ws.type_text("x");
    assert_eq!(tabs(&ws), [("shop".to_string(), true)]);
    ws.press("escape");
    assert!(tabs(&ws).is_empty());
    assert_eq!(active(&ws), None);
    assert_eq!(
        status(&ws).as_deref(),
        Some("Connection cancelled: \"shop\"")
    );
    for _ in 0..10 {
        ws.step();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(tabs(&ws).is_empty());

    ws.press("enter");
    assert_eq!(tabs(&ws), [("shop".to_string(), true)]);
    ws.click_label("Cancel");
    assert!(tabs(&ws).is_empty());
}

#[test]
fn failed_connection_shows_the_error_in_the_list() {
    let config = TestConfig::empty();
    let missing = config.dir().join("data").join("missing.db");
    config.add_connection(sqlite_connection("missing", &missing));
    let mut ws = TestWorkspace::open(&config);
    hold_connecting(&mut ws, true);
    ws.press("enter");
    assert_eq!(tabs(&ws), [("missing".to_string(), true)]);
    hold_connecting(&mut ws, false);
    ws.wait_for_state("the connection to fail", |w| w.home.error.is_some());
    assert!(tabs(&ws).is_empty());
    assert_eq!(active(&ws), None);
    assert!(
        status_line(&ws).starts_with("Connection failed: \"missing\": "),
        "{}",
        status_line(&ws)
    );
    assert!(ws.read(|w| w.home.failure_of("missing").is_some()));
    assert!(
        !missing.exists(),
        "a failed connect must not create the file"
    );
    ws.press("down");
    assert_eq!(
        ws.read(|w| w.home.error.as_ref().map(|f| f.message.clone())),
        None
    );
}

#[test]
fn connections_open_in_parallel_tabs() {
    let config = TestConfig::with_sample_dbs(&["shop", "library"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    ws.connect("library");
    assert_eq!(
        tabs(&ws),
        [("shop".to_string(), false), ("library".to_string(), false)]
    );
    assert_eq!(active(&ws), Some(1));

    ws.press("cmd-1");
    assert_eq!(active(&ws), None);
    ws.press("cmd-2");
    assert_eq!(active(&ws), Some(0));
    ws.press("cmd-3");
    assert_eq!(active(&ws), Some(1));
    ws.press("cmd-9");
    assert_eq!(active(&ws), Some(1), "a missing tab is ignored");

    ws.dispatch(Command::NextTab);
    assert_eq!(active(&ws), None, "after the last tab comes home");
    ws.press("ctrl-tab");
    assert_eq!(active(&ws), Some(0));
    ws.dispatch(Command::PrevTab);
    assert_eq!(active(&ws), None);
    ws.press("ctrl-shift-tab");
    assert_eq!(active(&ws), Some(1));

    ws.press("cmd-1");
    click_row(&mut ws, 0);
    ws.press("enter");
    assert_eq!(active(&ws), Some(0));
    assert_eq!(tabs(&ws).len(), 2);

    ws.click_label("Connections");
    assert_eq!(active(&ws), None);
    ws.click_label("library");
    assert_eq!(active(&ws), Some(1));

    ws.click_label("Disconnect shop");
    assert_eq!(tabs(&ws), [("library".to_string(), false)]);
    assert_eq!(active(&ws), Some(0));
    ws.press("cmd-w");
    assert!(tabs(&ws).is_empty());
    assert_eq!(active(&ws), None);
    assert_eq!(status(&ws).as_deref(), Some("Disconnected: \"library\""));
}

#[test]
fn tabs_keep_their_own_state_and_background_work() {
    let config = TestConfig::with_sample_dbs(&["shop", "library"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    ws.press("cmd-e");
    ws.replace_text("SELECT count(*) AS n FROM customers");
    ws.press("cmd-enter");
    ws.connect("library");
    ws.wait_for_state("the background script", |w| {
        w.tabs
            .iter()
            .find(|tab| tab.name() == "shop")
            .and_then(|tab| tab.browser())
            .is_some_and(|b| !b.console.is_running() && b.console.outcome_text().is_some())
    });
    ws.press("cmd-2");
    assert_eq!(active(&ws), Some(0));
    let outcome = ws.console().outcome_text().unwrap();
    assert!(outcome.starts_with("1 row"), "{outcome}");
    assert_eq!(tab_browser(&ws, "library").console.outcome_text(), None);
}

#[test]
fn a_background_failure_closes_its_tab_and_is_reported() {
    let config = TestConfig::with_sample_dbs(&["shop", "library"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    ws.connect("library");
    lose(&mut ws, "shop", "server went away");
    assert_eq!(tabs(&ws), [("library".to_string(), false)]);
    assert_eq!(active(&ws), Some(0), "the visible tab stays");
    ws.press("cmd-1");
    assert_eq!(
        status_line(&ws),
        "Connection failed: \"shop\": server went away"
    );

    ws.press("cmd-2");
    lose(&mut ws, "library", "gone");
    assert!(tabs(&ws).is_empty());
    assert_eq!(active(&ws), None);
    assert_eq!(status_line(&ws), "Connection failed: \"library\": gone");
}

#[test]
fn the_console_text_survives_a_reconnect() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    let outcome = ws.run_sql("SELECT 1 AS one");
    assert!(outcome.starts_with("1 row"), "{outcome}");
    ws.press("cmd-w");
    assert_eq!(
        ws.read(|w| w.home.drafts.get("shop").cloned()).as_deref(),
        Some("SELECT 1 AS one")
    );

    ws.connect("shop");
    assert_eq!(ws.console().text(), "SELECT 1 AS one");
    assert_eq!(ws.console().outcome_text(), None, "nothing run yet");

    ws.press("cmd-e");
    ws.press("cmd-a backspace");
    assert_eq!(ws.console().text(), "");
    ws.press("cmd-w");
    assert!(ws.read(|w| w.home.drafts.is_empty()));
}

#[test]
fn open_connections_cannot_be_edited_or_deleted() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    ws.press("cmd-1");
    ws.press("cmd-e");
    assert!(!ws.has_dialog());
    assert_eq!(
        status(&ws).as_deref(),
        Some("\"shop\" is open in a tab, close it before editing")
    );
    ws.press("cmd-backspace");
    assert!(!ws.has_dialog());
    ws.press("cmd-backspace");
    assert!(!ws.has_dialog());
    assert_eq!(
        status(&ws).as_deref(),
        Some("\"shop\" is open in a tab, close it before deleting")
    );
    ws.right_click("connection-0");
    click_menu(&mut ws, "Delete…");
    assert!(!ws.has_dialog());
    assert!(status(&ws).unwrap().contains("close it before deleting"));
}

#[test]
fn ctrl_e_edits_the_selected_connection() {
    let config = TestConfig::with_sample_dbs(&["shop", "library"]);
    let new_path = config.dir().join("data").join("other.db");
    std::fs::copy(config.dir().join("data").join("shop.db"), &new_path).unwrap();
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-e");
    assert!(ws.has_dialog());
    assert_eq!(form_original(&ws), Some("shop".into()));
    assert!(
        ws.descendants("connection-dialog")
            .iter()
            .any(|e| e.key.as_deref() == Some("connection-form"))
    );
    assert_eq!(
        focused_field(&ws),
        Some("name"),
        "the name is focused first"
    );
    ws.press("tab tab");
    assert_eq!(focused_field(&ws), Some("database"));
    ws.replace_text(&new_path.to_string_lossy());
    ws.press("enter");
    wait_dialog_closed(&mut ws);
    assert_eq!(status(&ws).as_deref(), Some("Connection \"shop\" updated"));
    assert_eq!(selected(&ws).as_deref(), Some("shop"));
    let saved = config.store().load().unwrap();
    assert_eq!(saved[0].database, new_path.to_string_lossy());
    assert_eq!(names(&ws), ["shop", "library"]);
    ws.connect("shop");
}

#[test]
fn editing_can_rename_but_not_onto_another_connection() {
    let config = TestConfig::with_sample_dbs(&["shop", "library"]);
    let mut ws = TestWorkspace::open(&config);
    click_row(&mut ws, 1);
    ws.press("cmd-e");
    assert_eq!(focused_field(&ws), Some("name"));
    ws.replace_text("shop");
    ws.press("enter");
    assert_eq!(
        form_error(&ws).as_deref(),
        Some("Connection \"shop\" already exists")
    );
    assert!(ws.has_dialog(), "the form stays open");
    ws.type_text("2");
    assert_eq!(form_error(&ws), None);
    ws.replace_text("three");
    ws.press("enter");
    wait_dialog_closed(&mut ws);
    assert_eq!(names(&ws), ["shop", "three"], "a rename keeps the position");
    assert_eq!(selected(&ws).as_deref(), Some("three"));
    assert_eq!(status(&ws).as_deref(), Some("Connection \"three\" updated"));
    assert_eq!(store_names(&config.connections_path()), ["shop", "three"]);
}

#[test]
fn the_draft_follows_a_renamed_connection() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    ws.press("cmd-e");
    ws.replace_text("SELECT 2 AS two");
    ws.press("cmd-w");
    assert!(ws.read(|w| w.home.drafts.contains_key("shop")));

    ws.press("cmd-e");
    ws.replace_text("renamed");
    ws.press("enter");
    wait_dialog_closed(&mut ws);
    assert_eq!(names(&ws), ["renamed"]);
    let drafts = ws.read(|w| w.home.drafts.clone());
    assert_eq!(
        drafts.get("renamed").map(String::as_str),
        Some("SELECT 2 AS two")
    );
    assert!(!drafts.contains_key("shop"));

    ws.connect("renamed");
    assert_eq!(ws.console().text(), "SELECT 2 AS two");
}

#[test]
fn cancelling_an_edit_changes_nothing() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let before = config.connections_toml();
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-e");
    ws.type_text("-renamed");
    assert_eq!(values(&ws).name, "shop-renamed");
    ws.press("escape");
    wait_dialog_closed(&mut ws);
    assert_eq!(config.connections_toml(), before);
    assert_eq!(names(&ws), ["shop"]);
    assert_eq!(status(&ws), None);
}

#[test]
fn the_new_form_validates_before_saving() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let before = config.connections_toml();
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-n");
    ws.press("enter");
    assert_eq!(
        form_error(&ws).as_deref(),
        Some("Connection name is required")
    );
    ws.type_text("shop");
    ws.click_label("Save");
    assert_eq!(
        form_error(&ws).as_deref(),
        Some("Connection \"shop\" already exists")
    );
    ws.replace_text("fresh");
    set_text(&mut ws, "host", "");
    ws.press("enter");
    assert_eq!(form_error(&ws).as_deref(), Some("Host is required"));
    set_text(&mut ws, "host", "localhost");
    set_text(&mut ws, "port", "99999");
    ws.press("enter");
    assert_eq!(form_error(&ws).as_deref(), Some("Invalid port: 99999"));
    set_kind(&mut ws, DbKind::Sqlite);
    assert_eq!(values(&ws).port, "99999", "a custom port is kept");
    ws.press("enter");
    assert_eq!(
        form_error(&ws).as_deref(),
        Some("Database file path is required")
    );
    assert!(ws.has_dialog());
    ws.press("escape");
    wait_dialog_closed(&mut ws);
    assert_eq!(config.connections_toml(), before);
}

#[test]
fn a_new_sqlite_connection_is_saved_selected_and_connectable() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let path = config.dir().join("data").join("shop.db");
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-n");
    ws.type_text("copy of shop");
    set_kind(&mut ws, DbKind::Sqlite);
    assert_eq!(values(&ws).port, "", "the default port goes");
    focus_field(&mut ws, "name");
    ws.press("tab tab");
    assert_eq!(focused_field(&ws), Some("database"));
    ws.type_text(&path.to_string_lossy());
    ws.press("enter");
    wait_dialog_closed(&mut ws);
    assert_eq!(names(&ws), ["shop", "copy of shop"]);
    assert_eq!(selected(&ws).as_deref(), Some("copy of shop"));
    assert_eq!(
        status(&ws).as_deref(),
        Some("Connection \"copy of shop\" saved")
    );
    assert_eq!(
        store_names(&config.connections_path()),
        ["shop", "copy of shop"]
    );
    ws.press("enter");
    ws.wait_connected("copy of shop");
}

#[test]
fn a_connection_url_fills_in_the_new_form() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let path = config.dir().join("data").join("shop.db");
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-n");
    assert!(ws.exists("form-url"));
    assert_eq!(focused_field(&ws), Some("name"), "the name is still first");

    focus_field(&mut ws, "url");
    ws.type_text("mysql://root:p%40ss@db.example.com/app");
    let v = values(&ws);
    assert_eq!(v.kind, DbKind::MySql);
    assert_eq!(v.host, "db.example.com");
    assert_eq!(v.port, "3306", "a missing port becomes the default one");
    assert_eq!(v.user, "root");
    assert_eq!(v.password, "p@ss");
    assert_eq!(v.database, "app");

    ws.replace_text("redis://localhost");
    assert_eq!(
        values(&ws).host,
        "db.example.com",
        "a bad URL changes nothing"
    );
    focus_field(&mut ws, "name");
    ws.type_text("copy of shop");
    ws.press("enter");
    assert_eq!(
        form_error(&ws).as_deref(),
        Some(
            "Invalid connection URL: unsupported scheme \"redis\", \
             expected postgres://, mysql:// or sqlite://"
        )
    );

    focus_field(&mut ws, "url");
    ws.replace_text(&format!("sqlite://{}", path.to_string_lossy()));
    assert_eq!(form_error(&ws), None);
    assert_eq!(values(&ws).kind, DbKind::Sqlite);
    assert_eq!(values(&ws).database, path.to_string_lossy());
    ws.press("enter");
    wait_dialog_closed(&mut ws);
    assert_eq!(
        store_names(&config.connections_path()),
        ["shop", "copy of shop"]
    );
    ws.press("enter");
    ws.wait_connected("copy of shop");
}

#[test]
fn editing_a_connection_has_no_url_field() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-e");
    assert_eq!(form_original(&ws), Some("shop".into()));
    assert!(!ws.exists("form-url"));
}

#[test]
fn a_new_connection_can_keep_its_password_in_plain_text() {
    let config = TestConfig::empty();
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-n");
    ws.type_text("dev");
    ws.press("tab tab tab tab");
    assert_eq!(focused_field(&ws), Some("user"));
    ws.type_text("postgres");
    ws.press("tab");
    assert_eq!(focused_field(&ws), Some("password"));
    ws.type_text("s3cret");
    ws.click_label("As plain text in config");
    let v = values(&ws);
    assert_eq!(
        v.storage,
        PasswordStorage::Plain,
        "never touch the real keychain"
    );
    focus_field(&mut ws, "database");
    ws.type_text("app");
    let v = values(&ws);
    assert_eq!(
        (v.name.as_str(), v.user.as_str(), v.password.as_str()),
        ("dev", "postgres", "s3cret")
    );
    assert_eq!(v.database, "app");
    assert_eq!(v.storage, PasswordStorage::Plain);
    ws.press("enter");
    wait_dialog_closed(&mut ws);
    let toml = config.connections_toml();
    assert!(toml.contains("password_storage = \"plain\""), "{toml}");
    assert!(toml.contains("password = \"s3cret\""), "{toml}");
    assert_eq!(status(&ws).as_deref(), Some("Connection \"dev\" saved"));
    assert_eq!(focus(&ws), Focus::Connections);

    ws.press("cmd-e");
    let v = values(&ws);
    assert_eq!(v.password, "s3cret");
    assert_eq!(v.storage, PasswordStorage::Plain);
    assert_eq!(v.port, "5432");
    ws.press("escape");
    wait_dialog_closed(&mut ws);
}

#[test]
fn paste_strips_control_characters() {
    let config = three_connections();
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-n");
    ws.clip.set_text("pasted\tname\r\n");
    ws.press("cmd-v");
    assert_eq!(values(&ws).name, "pastedname");
    assert!(ws.has_dialog(), "a pasted newline does not submit");
    ws.press("escape");
    wait_dialog_closed(&mut ws);
}

#[test]
fn delete_asks_first_and_removes_drafts() {
    let config = three_connections();
    let mut ws = TestWorkspace::open(&config);
    click_row(&mut ws, 1);
    ws.update(|w, _| w.home.remember_draft("local", "SELECT 3".into()));

    ws.press("cmd-backspace");
    assert!(ws.has_dialog());
    ws.press("enter");
    assert!(ws.has_dialog(), "Enter does not confirm a delete");
    ws.press("escape");
    wait_dialog_closed(&mut ws);
    assert_eq!(names(&ws).len(), 3);
    ws.dispatch(Command::DeleteConnection);
    ws.click_label("Cancel");
    wait_dialog_closed(&mut ws);
    assert_eq!(names(&ws).len(), 3);

    click_row(&mut ws, 1);
    ws.press("delete");
    assert!(ws.has_dialog());
    click_dialog_delete(&mut ws);
    wait_dialog_closed(&mut ws);
    assert_eq!(names(&ws), ["prod orders", "Staging Orders"]);
    assert_eq!(status(&ws).as_deref(), Some("Connection \"local\" deleted"));
    assert_eq!(
        store_names(&config.connections_path()),
        ["prod orders", "Staging Orders"]
    );
    assert!(ws.read(|w| w.home.drafts.is_empty()));
    assert_eq!(selected(&ws).as_deref(), Some("Staging Orders"));

    ws.right_click("connection-1");
    click_menu(&mut ws, "Delete…");
    click_dialog_delete(&mut ws);
    wait_dialog_closed(&mut ws);
    assert_eq!(selected(&ws).as_deref(), Some("prod orders"));

    click_row(&mut ws, 0);
    ws.press("cmd-backspace");
    assert!(ws.has_dialog());
    click_dialog_delete(&mut ws);
    wait_dialog_closed(&mut ws);
    assert!(names(&ws).is_empty());
    assert_eq!(focus(&ws), Focus::NewButton);
    assert!(status_line(&ws).contains("deleted"));
    ws.press("enter");
    assert!(
        ws.read(|w| w.test_form_open()),
        "Enter now opens the new form"
    );
}

#[test]
fn help_opens_filters_and_closes() {
    let config = three_connections();
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-/");
    assert!(ws.has_dialog());
    assert!(
        ws.descendants("help-dialog")
            .iter()
            .any(|e| e.key.as_deref() == Some("help")),
        "the help view is in the dialog"
    );
    ws.type_text("quit");
    assert_eq!(help_query(&ws).as_deref(), Some("quit"));
    let matches = help_match_count(&ws).unwrap();
    assert!(matches >= 1, "quit matches the quit binding");
    ws.press("ctrl-u");
    assert_eq!(help_query(&ws).as_deref(), Some(""));
    ws.press("escape");
    wait_dialog_closed(&mut ws);
    ws.press("f1");
    assert!(ws.has_dialog());
    ws.press("cmd-/");
    wait_dialog_closed(&mut ws);
    ws.press("escape");
    ws.connect("local");
    ws.press("f1");
    assert!(ws.has_dialog());
    ws.press("escape");
    wait_dialog_closed(&mut ws);
    assert_eq!(active(&ws), Some(0));
}

#[test]
fn the_type_checkboxes_switch_fields_and_ports() {
    let config = TestConfig::empty();
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-n");
    ws.press("tab");
    assert_eq!(focused_field(&ws), Some("kind"));
    ws.click("form-kind-sqlite");
    assert_eq!(values(&ws).kind, DbKind::Sqlite);
    ws.click("form-kind-sqlite");
    assert_eq!(values(&ws).kind, DbKind::Sqlite, "the checked type stays checked");
    ws.click("form-kind-mysql");
    let v = values(&ws);
    assert_eq!(v.kind, DbKind::MySql, "{v:?}");
    assert_eq!(v.port, "3306", "the default port follows the type");
    assert!(ws.has_dialog(), "choosing a type does not submit the form");
    assert_eq!(form_error(&ws), None);

    focus_field(&mut ws, "port");
    ws.press("cmd-a backspace");
    ws.type_text("6a5x4");
    ws.type_text("3");
    assert_eq!(values(&ws).port, "6543");
    set_kind(&mut ws, DbKind::Postgres);
    assert_eq!(values(&ws).port, "6543");
    set_kind(&mut ws, DbKind::Sqlite);
    set_kind(&mut ws, DbKind::Postgres);
    let v = values(&ws);
    assert_eq!((v.host.as_str(), v.port.as_str()), ("localhost", "6543"));
    ws.press("escape");
    wait_dialog_closed(&mut ws);
    assert!(config.connections_toml().is_empty(), "nothing saved");
}

#[test]
fn the_environment_tag_is_saved_shown_and_cleared() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let path = config.dir().join("data").join("shop.db");
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-n");
    ws.type_text("live shop");
    set_kind(&mut ws, DbKind::Sqlite);
    set_text(&mut ws, "database", &path.to_string_lossy());
    assert!(!ws.exists("form-tag-hint"));

    ws.click("form-tag-trigger");
    assert!(ws.read(|w| form_of(w).tag_open), "the menu opens");
    ws.click_in("form-tag-content", "Production");
    assert_eq!(values(&ws).tag, Some(Tag::Prod));
    assert!(
        ws.read(|w| !form_of(w).tag_open),
        "choosing closes the menu"
    );
    assert!(ws.exists("form-tag-hint"));

    focus_field(&mut ws, "name");
    ws.press("enter");
    wait_dialog_closed(&mut ws);
    assert!(
        config.connections_toml().contains("tag = \"prod\""),
        "{}",
        config.connections_toml()
    );
    assert_eq!(selected(&ws).as_deref(), Some("live shop"));
    assert!(ws.exists("connection-1-tag"));
    assert!(!ws.exists("connection-0-tag"), "shop has no tag");
    assert!(ws.exists("details-tag"));
    assert_eq!(
        ws.find("connection-1-tag").and_then(|e| e.label).as_deref(),
        Some("Production")
    );

    ws.press("cmd-e");
    assert_eq!(values(&ws).tag, Some(Tag::Prod));
    ws.update(|w, _| form_mut(w).set_tag(None));
    assert!(!ws.exists("form-tag-hint"));
    focus_field(&mut ws, "name");
    ws.press("enter");
    wait_dialog_closed(&mut ws);
    let toml = config.connections_toml();
    assert!(!toml.lines().any(|line| line.starts_with("tag")), "{toml}");
    assert!(!ws.exists("connection-1-tag"));
    assert!(!ws.exists("details-tag"));
}

#[test]
fn double_click_connects_and_switches_to_an_open_tab() {
    let config = TestConfig::with_sample_dbs(&["shop", "library"]);
    let mut ws = TestWorkspace::open(&config);
    ws.double_click("connection-1");
    ws.wait_connected("library");
    assert_eq!(tabs(&ws), [("library".to_string(), false)]);

    ws.press("cmd-1");
    assert_eq!(selected(&ws).as_deref(), Some("library"));
    ws.double_click("connection-1");
    assert_eq!(active(&ws), Some(0));
    assert_eq!(tabs(&ws).len(), 1);
}

#[test]
fn an_unreadable_config_shows_the_error_and_can_be_retried() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let good = config.connections_toml();
    std::fs::write(config.connections_path(), "[[connection]\nname = ").unwrap();
    let mut ws = TestWorkspace::open(&config);
    assert!(ws.read(|w| w.load_error.is_some()));
    assert!(names(&ws).is_empty());
    ws.press("cmd-n");
    assert!(!ws.has_dialog());
    ws.click_label("Retry");
    assert!(ws.read(|w| w.load_error.is_some()), "still broken");
    std::fs::write(config.connections_path(), good).unwrap();
    ws.click_label("Retry");
    assert!(ws.read(|w| w.load_error.is_none()));
    assert_eq!(names(&ws), ["shop"]);
    assert_eq!(selected(&ws).as_deref(), Some("shop"));
    ws.connect("shop");
}

#[test]
fn unicode_and_control_characters_in_names() {
    let config = TestConfig::empty();
    let path = config.add_sample_db("shop");
    let name = "日本語\tデータベース with a rather long name";
    config.add_connection(sqlite_connection(name, &path));
    let mut ws = TestWorkspace::open(&config);
    click_row(&mut ws, 1);
    assert_eq!(selected(&ws).as_deref(), Some(name));
    ws.press("enter");
    ws.wait_connected(name);
    let title = super::model::tab_title(name);
    assert_eq!(title.chars().count(), 24);
    assert!(title.starts_with("日本語→データベース"), "{title}");
    ws.press("cmd-w");
    ws.press("escape");
    ws.press("cmd-backspace");
    assert!(ws.has_dialog());
    ws.press("escape");
    wait_dialog_closed(&mut ws);
    ws.press("cmd-e");
    assert_eq!(values(&ws).name, name);
    ws.press("enter");
    wait_dialog_closed(&mut ws);
    assert_eq!(names(&ws), ["shop", name]);
}

#[test]
fn a_lost_connection_does_not_steal_focus_from_a_dialog() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    ws.press("f1");
    assert!(ws.has_dialog());
    lose(&mut ws, "shop", "gone");
    assert_eq!(active(&ws), None);
    assert!(ws.has_dialog(), "the help stays open");
    ws.type_text("quit");
    assert_eq!(
        help_query(&ws).as_deref(),
        Some("quit"),
        "typing still goes to the help search"
    );
    ws.press("escape");
    wait_dialog_closed(&mut ws);
    assert!(status_line(&ws).contains("Connection failed: \"shop\": gone"));
    click_row(&mut ws, 0);
    assert_eq!(selected(&ws).as_deref(), Some("shop"));
}

#[test]
fn new_connection_from_a_browser_tab_goes_home() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    ws.press("cmd-n");
    assert_eq!(active(&ws), None);
    assert!(ws.read(|w| w.test_form_open()));
    assert_eq!(focused_field(&ws), Some("name"));
    ws.press("escape");
    wait_dialog_closed(&mut ws);
    ws.press("cmd-2");
    assert_eq!(active(&ws), Some(0));
}

#[test]
fn closing_a_busy_tab_releases_its_session_off_the_ui_thread() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let db = config.dir().join("data").join("shop.db");
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch(
            "CREATE VIEW slow AS WITH RECURSIVE c(x) AS
               (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 100000000000) SELECT x FROM c;",
        )
        .unwrap();
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    let session = Rc::downgrade(&ws.browser().session);
    assert!(
        session.upgrade().is_some_and(|s| s.drops_in_background()),
        "a browser's session never waits for its workers when it is dropped"
    );

    ws.press("cmd-e");
    ws.replace_text("SELECT x FROM slow ORDER BY x DESC LIMIT 1");
    ws.press("cmd-enter");
    assert!(ws.console().is_running());

    ws.press("cmd-w");
    assert!(tabs(&ws).is_empty());
    ws.settle();
    assert!(
        session.upgrade().is_none(),
        "the browser and its views let go of the session"
    );
}

#[test]
fn the_connecting_session_is_dropped_in_the_background_too() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    hold_connecting(&mut ws, true);
    ws.press("enter");
    let background = ws.read(|w| match &w.tabs[0].state {
        TabState::Connecting(session) => session.drops_in_background(),
        TabState::Browser(_) => panic!("connected already"),
    });
    assert!(background);
    ws.press("escape");
    assert!(tabs(&ws).is_empty());
}

#[test]
fn quitting_makes_the_session_drops_wait_for_the_first_cancel() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    let session = Rc::clone(&ws.browser().session);
    assert!(session.drops_in_background());

    ws.simulate_quit();
    assert!(
        !session.drops_in_background(),
        "on quit the drop waits until the first cancel was sent (the process exits next)"
    );
}

#[test]
fn closing_the_window_makes_the_session_drops_wait_too() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    let session = Rc::clone(&ws.browser().session);
    assert!(session.drops_in_background());

    ws.simulate_close();
    assert!(
        !session.drops_in_background(),
        "closing the window quits: the drop waits until the first cancel was sent"
    );
}

#[test]
fn quitting_also_covers_connecting_tabs() {
    let config = TestConfig::with_sample_dbs(&["shop", "library"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    ws.dispatch(Command::GoHome);
    click_row(&mut ws, 1);
    hold_connecting(&mut ws, true);
    ws.press("enter");
    let flags = |ws: &TestWorkspace| {
        ws.read(|w| {
            w.tabs
                .iter()
                .map(|tab| match &tab.state {
                    TabState::Connecting(session) => (true, session.drops_in_background()),
                    TabState::Browser(browser) => (false, browser.session.drops_in_background()),
                })
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(flags(&ws), [(false, true), (true, true)]);
    ws.read(|w| w.drop_sessions_in_foreground());
    assert_eq!(flags(&ws), [(false, false), (true, false)]);
}

#[test]
fn the_window_title_names_the_connection_and_what_it_shows() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    let title = |ws: &TestWorkspace| ws.read(|w| w.window_title.clone());
    assert_eq!(title(&ws), "Cooper");
    assert_eq!(ws.text_of("window-title").as_deref(), Some("Cooper"));
    ws.connect("shop");
    assert!(!ws.exists("window-title"), "the tabs take the titlebar");
    assert_eq!(title(&ws), "shop — Cooper");
    ws.open_table("customers");
    assert_eq!(title(&ws), "main.customers — shop — Cooper");
    ws.dispatch(Command::OpenConsole);
    assert_eq!(title(&ws), "Console — shop — Cooper");
    ws.dispatch(Command::GoHome);
    assert_eq!(title(&ws), "Cooper");
}

#[test]
fn browse_fills_the_sqlite_path() {
    let config = TestConfig::empty();
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-n");
    set_kind(&mut ws, DbKind::Sqlite);
    ws.click_label("Browse…");
    assert!(ws.rt.file_dialog().is_some(), "Browse… asks for a file");
    ws.rt
        .resolve_file_dialog(Some("/data/app.db".into()))
        .unwrap();
    ws.settle();
    assert_eq!(values(&ws).database, "/data/app.db");
    assert!(ws.has_dialog());
    assert_eq!(
        focused_field(&ws),
        Some("name"),
        "Browse… keeps the field focus"
    );
}

#[test]
#[ignore = "renders PNG snapshots into $COOPER_SHELL_SNAPS"]
fn zz_shell_snapshots() {
    let Some(dir) = std::env::var_os("COOPER_SHELL_SNAPS") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    for dark in [true, false] {
        let theme = if dark { "dark" } else { "light" };
        let shot = |ws: &mut TestWorkspace, name: &str| {
            ws.settle();
            crate::test_support::snap_png(&mut ws.rt, &dir.join(format!("{name}-{theme}.png")));
        };
        let config = three_connections();
        let mut ws = TestWorkspace::open_with(config.store(), dark);
        shot(&mut ws, "home");
        ws.press("up");
        shot(&mut ws, "home-new-button");
        ws.press("escape cmd-n");
        shot(&mut ws, "form-new");
        ws.click("form-kind-mysql");
        shot(&mut ws, "form-type-mysql");
        ws.click("form-kind-sqlite");
        shot(&mut ws, "form-type-sqlite");
        ws.press("enter");
        shot(&mut ws, "form-sqlite-error");
        ws.press("escape");
        ws.press("cmd-backspace");
        shot(&mut ws, "delete-dialog");
        ws.press("escape cmd-/");
        shot(&mut ws, "help");
        ws.type_text("tab");
        shot(&mut ws, "help-filtered");
        ws.press("ctrl-u");
        ws.type_text("nothing-matches");
        shot(&mut ws, "help-empty");
        ws.press("escape");
        click_row(&mut ws, 1);
        ws.update(|w, _| w.hold_connecting = true);
        ws.press("enter");
        shot(&mut ws, "connecting");
        ws.update(|w, _| w.hold_connecting = false);
        ws.wait_connected("local");
        ws.press("cmd-1");
        click_row(&mut ws, 2);
        ws.press("enter");
        ws.wait_connected("Staging Orders");
        ws.update(|w, tx| w.push_toast("Disconnected: \"something\"".into(), false, tx));
        ws.update(|w, tx| {
            w.push_toast(
                "Connection \"prod orders\" failed: server closed the connection unexpectedly; this probably means the server terminated abnormally before or while processing the request".into(),
                true,
                tx,
            )
        });
        shot(&mut ws, "tabs-toast");
        ws.press("cmd-1");
        shot(&mut ws, "home-with-tabs");
        let config = TestConfig::empty();
        let missing = config.dir().join("data").join("missing.db");
        config.add_connection(sqlite_connection("missing", &missing));
        let mut ws = TestWorkspace::open_with(config.store(), dark);
        ws.press("enter");
        ws.wait_for_state("failure", |w| w.home.error.is_some());
        shot(&mut ws, "home-failure");
        let config = TestConfig::empty();
        let mut ws = TestWorkspace::open_with(config.store(), dark);
        shot(&mut ws, "home-empty");
        std::fs::create_dir_all(config.connections_path().parent().unwrap()).unwrap();
        std::fs::write(config.connections_path(), "[[connection]\nname = ").unwrap();
        let mut ws = TestWorkspace::open_with(config.store(), dark);
        shot(&mut ws, "load-error");
    }
}

#[test]
fn closing_a_dialog_returns_the_keyboard_to_the_home_list() {
    let config = TestConfig::with_sample_dbs(&["shop", "library"]);
    let mut ws = TestWorkspace::open(&config);
    ws.right_click("connection-0");
    click_menu(&mut ws, "Edit…");
    assert!(ws.read(|w| w.test_form_open()));
    ws.press("escape");
    wait_dialog_closed(&mut ws);
    assert!(ws.is_focused("home"), "{}", ws.describe());

    click_row(&mut ws, 1);
    ws.press("cmd-backspace");
    assert!(ws.has_dialog());
    ws.click_in("delete-connection-dialog", "Close");
    wait_dialog_closed(&mut ws);
    assert!(ws.is_focused("home"), "{}", ws.describe());
    assert_eq!(names(&ws).len(), 2);

    click_row(&mut ws, 1);
    ws.press("cmd-e");
    assert!(ws.read(|w| w.test_form_open()));
    ws.press("escape");
    wait_dialog_closed(&mut ws);
    assert!(ws.is_focused("home"), "the list keeps the keyboard");
}

#[test]
fn a_saved_connection_is_scrolled_into_view() {
    let config = TestConfig::empty();
    let path = config.add_sample_db("shop");
    for n in 0..60 {
        config.add_connection(sqlite_connection(&format!("db {n:02}"), &path));
    }
    let mut ws = TestWorkspace::open(&config);
    assert!(!ws.is_visible("connection-59"));
    ws.press("cmd-n");
    ws.type_text("zz last");
    set_kind(&mut ws, DbKind::Sqlite);
    set_text(&mut ws, "database", &path.to_string_lossy());
    ws.press("enter");
    wait_dialog_closed(&mut ws);
    assert_eq!(selected(&ws).as_deref(), Some("zz last"));
    ws.settle();
    assert!(ws.is_visible("connection-60"), "{}", ws.describe());
}

#[test]
fn saving_or_deleting_returns_the_keyboard_to_where_it_was() {
    let config = three_connections();
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-e");
    ws.press("enter");
    wait_dialog_closed(&mut ws);
    assert!(ws.is_focused("home"), "{}", ws.describe());

    click_row(&mut ws, 1);
    assert!(ws.is_focused("home"));
    ws.press("cmd-e");
    assert!(ws.read(|w| w.test_form_open()));
    ws.click("save-connection");
    wait_dialog_closed(&mut ws);
    assert!(ws.is_focused("home"), "{}", ws.describe());

    ws.press("cmd-e");
    ws.click("form-cancel");
    wait_dialog_closed(&mut ws);
    assert!(ws.is_focused("home"), "{}", ws.describe());

    ws.press("cmd-backspace");
    assert!(ws.has_dialog());
    click_dialog_delete(&mut ws);
    wait_dialog_closed(&mut ws);
    assert_eq!(names(&ws).len(), 2);
    assert!(ws.is_focused("home"), "{}", ws.describe());
}

#[test]
fn the_delete_dialog_opens_on_cancel() {
    let config = three_connections();
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-backspace");
    assert!(ws.has_dialog());
    assert!(ws.is_focused("cancel-delete"), "{}", ws.describe());
    ws.press("enter");
    assert!(ws.has_dialog(), "Enter does not confirm a delete");
    ws.press("space");
    wait_dialog_closed(&mut ws);
    assert_eq!(names(&ws).len(), 3);
}

#[test]
fn close_tab_is_left_unhandled_on_home() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    assert!(
        !ws.dispatch(Command::CloseTab),
        "Window › Close may close the window"
    );
    let cmd_w = crate::test_support::key_event("cmd-w");
    assert!(!ws.update(move |w, tx| crate::keys::route(w, &cmd_w, tx)));
    ws.connect("shop");
    assert!(ws.dispatch(Command::CloseTab));
    assert!(tabs(&ws).is_empty());
}

#[test]
fn the_app_follows_the_os_until_a_theme_is_chosen() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    assert!(ws.read(|w| w.is_dark()));
    ws.rt.set_system_dark(false);
    ws.settle();
    assert!(!ws.read(|w| w.is_dark()), "the app follows the OS");

    ws.press("cmd-shift-t");
    assert_eq!(ws.read(|w| w.theme_mode), ThemeMode::Dark);
    ws.rt.set_system_dark(true);
    ws.settle();
    ws.rt.set_system_dark(false);
    ws.settle();
    assert!(ws.read(|w| w.is_dark()), "a chosen theme outlives the OS");
}

fn settings_open(ws: &TestWorkspace) -> bool {
    ws.read(|w| matches!(w.dialog, Some(Dialog::Settings(_))))
}

fn says(ws: &TestWorkspace, ancestor: &str, needle: &str) -> bool {
    ws.descendants(ancestor)
        .iter()
        .any(|e| e.text.as_deref().is_some_and(|text| text.contains(needle)))
}

fn settings_toml(config: &TestConfig) -> String {
    std::fs::read_to_string(config.dir().join("cooper").join("settings.toml")).unwrap_or_default()
}

#[test]
fn the_settings_open_from_the_title_bar_and_the_keyboard() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.click("open-settings");
    assert!(settings_open(&ws), "{}", ws.describe());
    ws.press("escape");
    assert!(!ws.has_dialog(), "{}", ws.describe());

    ws.press("cmd-,");
    assert!(settings_open(&ws), "{}", ws.describe());
    ws.press("cmd-,");
    assert!(!ws.has_dialog(), "{}", ws.describe());
}

#[test]
fn the_theme_is_picked_in_the_settings_and_remembered() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-,");
    ws.click_in("theme-mode", "Light");
    assert_eq!(ws.read(|w| w.theme_mode), ThemeMode::Light);
    assert!(!ws.read(|w| w.is_dark()));
    assert!(settings_toml(&config).contains("theme = \"light\""));

    let ws = TestWorkspace::open(&config);
    assert_eq!(ws.read(|w| w.theme_mode), ThemeMode::Light);
    assert!(!ws.read(|w| w.is_dark()));
}

fn families(ws: &TestWorkspace) -> (String, String) {
    let (sans, mono) = ws.rt.font_families();
    (sans.to_string(), mono.to_string())
}

#[test]
fn fonts_are_picked_in_the_settings_and_remembered() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    let defaults = Fonts::default();
    assert_eq!(
        families(&ws),
        (defaults.interface.clone(), defaults.code.clone())
    );
    ws.press("cmd-,");
    assert!(ws.exists("settings-fonts"), "{}", ws.describe());
    assert!(!ws.exists("reset-fonts"));

    ws.click("font-code-value-trigger");
    ws.type_text("Fira Code");
    assert!(
        says(&ws, "font-code-content", "Use “Fira Code”"),
        "{}",
        ws.describe()
    );
    ws.press("enter");
    assert!(settings_open(&ws), "Enter only closes the picker");
    assert_eq!(ws.read(|w| w.fonts.code.clone()), "Fira Code");
    assert_eq!(families(&ws).1, "Fira Code", "the app switches at once");
    assert!(settings_toml(&config).contains("code = \"Fira Code\""));
    assert!(ws.exists("reset-fonts"));

    ws.click("font-interface-value-trigger");
    assert!(ws.exists("font-interface-content"));
    ws.press("escape");
    assert!(!ws.exists("font-interface-content"));
    assert!(settings_open(&ws), "the first Esc only closes the picker");

    ws.click("font-interface-value-trigger");
    ws.type_text("Verdana");
    ws.press("enter");
    assert_eq!(ws.read(|w| w.fonts.interface.clone()), "Verdana");
    assert_eq!(families(&ws).0, "Verdana");

    let mut ws = TestWorkspace::open(&config);
    assert_eq!(
        families(&ws),
        ("Verdana".to_string(), "Fira Code".to_string()),
        "the first frame is drawn in the saved fonts"
    );
    ws.press("cmd-,");
    ws.click("reset-fonts");
    assert_eq!(ws.read(|w| w.fonts.clone()), defaults);
    assert_eq!(families(&ws), (defaults.interface, defaults.code));
    assert!(!settings_toml(&config).contains("fonts"));
}

fn settings_page(ws: &TestWorkspace) -> Option<Page> {
    ws.read(|w| match &w.dialog {
        Some(Dialog::Settings(settings)) => Some(settings.page()),
        _ => None,
    })
}

#[test]
fn the_settings_pages_switch_from_the_sidebar() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-,");
    assert_eq!(settings_page(&ws), Some(Page::General));
    assert!(ws.exists("theme-mode"));
    assert!(!ws.exists("settings-list"));

    ws.click("settings-nav-shortcuts");
    assert_eq!(settings_page(&ws), Some(Page::Shortcuts));
    assert!(ws.exists("settings-list"), "{}", ws.describe());
    assert!(!ws.exists("theme-mode"));

    ws.click("shortcut-show-help-0");
    ws.click("settings-nav-general");
    assert!(ws.exists("theme-mode"));
    ws.click("settings-nav-shortcuts");
    assert!(
        !says(&ws, "shortcut-show-help-0", "Press keys"),
        "leaving the page stops the recording"
    );

    ws.press("escape cmd-,");
    assert_eq!(settings_page(&ws), Some(Page::General));
}

fn help_open(ws: &TestWorkspace) -> bool {
    ws.read(|w| matches!(w.dialog, Some(Dialog::Help(_))))
}

#[test]
fn a_shortcut_is_recorded_in_the_settings_and_remembered() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-,");
    ws.click("settings-nav-shortcuts");
    ws.click("shortcut-show-help-0");
    assert!(
        says(&ws, "shortcut-show-help-0", "Press keys"),
        "{}",
        ws.describe()
    );

    ws.press("k");
    assert!(
        says(&ws, "shortcut-row-show-help", "needs a modifier"),
        "{}",
        ws.describe()
    );
    ws.press("escape");
    assert!(settings_open(&ws), "the first Esc only stops the recording");
    assert!(!ws.exists("reset-show-help"));

    ws.click("shortcut-show-help-0");
    ws.press("cmd-k");
    assert!(ws.exists("reset-show-help"), "{}", ws.describe());
    assert!(settings_toml(&config).contains("show-help = [\"mod+k\", \"f1\"]"));
    ws.press("escape");
    assert!(!ws.has_dialog());

    ws.press("cmd-/");
    assert!(!help_open(&ws), "the old shortcut is free");
    ws.press("cmd-k");
    assert!(help_open(&ws));
    ws.press("escape");

    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-k");
    assert!(help_open(&ws), "the shortcut was loaded");
    ws.press("escape");

    ws.press("cmd-,");
    ws.click("settings-nav-shortcuts");
    ws.click("reset-show-help");
    assert!(!ws.exists("reset-show-help"));
    assert!(!settings_toml(&config).contains("show-help"));
    ws.press("escape cmd-/");
    assert!(help_open(&ws));
}

#[test]
fn a_shortcut_taken_elsewhere_is_flagged_and_reset_all_clears_it() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-,");
    ws.click("settings-nav-shortcuts");
    assert!(!ws.exists("reset-all-shortcuts"));
    ws.click("shortcut-show-help-0");
    ws.press("cmd-w");
    let taken = "also used by: Disconnect and close the tab";
    assert!(
        says(&ws, "shortcut-row-show-help", taken),
        "{}",
        ws.describe()
    );

    ws.click("reset-all-shortcuts");
    assert!(!ws.exists("reset-all-shortcuts"));
    assert!(!ws.exists("reset-show-help"));
    assert!(!settings_toml(&config).contains("keys"));
}

#[test]
fn snapshots_of_the_settings() {
    let Some(dir) = std::env::var_os("SETTINGS_SNAPS") else {
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
        ws.press("cmd-,");
        shot(&mut ws, "01-general");
        ws.click("settings-nav-shortcuts");
        shot(&mut ws, "02-shortcuts");
        ws.click("shortcut-show-help-0");
        ws.press("k");
        shot(&mut ws, "03-recording");
        ws.press("cmd-w");
        shot(&mut ws, "04-conflict");
        ws.click("settings-nav-general");
        ws.click("font-code-value-trigger");
        ws.type_text("Mon");
        shot(&mut ws, "05-font-picker");
        ws.press("enter");
        shot(&mut ws, "06-font-chosen");
    }
}

#[test]
fn a_wide_font_keeps_the_connection_form_inside_its_dialog() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    let wide = "Menlo, Courier New";
    ws.update(move |s, tx| {
        s.set_fonts(
            Fonts {
                interface: wide.into(),
                code: wide.into(),
            },
            tx,
        )
    });
    ws.press("cmd-n");
    for size in [crate::WINDOW_SIZE, crate::MIN_SIZE] {
        ws.resize(size);
        let found: Vec<String> = ws
            .overflows()
            .into_iter()
            .filter(|found| found.contains("connection"))
            .collect();
        assert!(found.is_empty(), "at {size:?}: {found:#?}");
    }
}
