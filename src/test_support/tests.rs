use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use super::{TestConfig, TestWorkspace};
use crate::{settings::ThemeMode, workspace::TabState};

#[test]
fn cmd_shift_t_toggles_the_theme() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    assert!(ws.read(|w| w.is_dark()), "the test window starts dark");
    ws.press("cmd-shift-t");
    assert_eq!(ws.read(|w| w.theme_mode), ThemeMode::Light);
    ws.press("cmd-shift-t");
    assert_eq!(ws.read(|w| w.theme_mode), ThemeMode::Dark);
}

#[test]
fn cmd_n_opens_the_connection_form_and_escape_closes_it() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-n");
    assert!(ws.read(|w| w.test_form_open()));
    assert!(ws.has_dialog());
    ws.press("cmd-shift-t");
    assert_eq!(
        ws.read(|w| w.theme_mode),
        ThemeMode::System,
        "workspace keys stay off while a dialog is open"
    );
    ws.press("escape");
    assert!(!ws.has_dialog(), "{}", ws.describe());
    assert!(!ws.read(|w| w.test_form_open()));
}

#[test]
fn closing_the_window_drops_the_sessions_before_the_process_ends() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    let session = ws.browser().test_session();
    ws.simulate_close();
    assert!(ws.read(|w| w.tabs.is_empty() && w.active.is_none()));
    assert!(
        session.upgrade().is_none(),
        "the session is dropped in the hook: a fast exit never runs its destructor"
    );
    ws.simulate_quit();
    assert!(ws.read(|w| w.tabs.is_empty()));
}

#[test]
fn quitting_cancels_a_running_script_by_dropping_its_session() {
    let config = TestConfig::with_sample_dbs(&["shop", "library"]);
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
    let shop = ws.browser().test_session();
    ws.press("cmd-e");
    ws.replace_text("SELECT x FROM slow ORDER BY x DESC LIMIT 1");
    ws.press("cmd-enter");
    assert!(ws.console().is_running());
    ws.connect("library");
    let library = ws.browser().test_session();
    assert_eq!(ws.read(|w| w.tabs.len()), 2);

    ws.simulate_quit();
    assert!(ws.read(|w| w.tabs.is_empty() && w.dialog.is_none()));
    assert!(shop.upgrade().is_none() && library.upgrade().is_none());
}

#[test]
fn enter_and_double_click_connect_through_the_background_session() {
    let config = TestConfig::with_sample_dbs(&["shop", "library"]);
    let mut ws = TestWorkspace::open(&config);
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&wakes);
    ws.update(move |s, _| {
        s.waker = Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }))
    });
    assert_eq!(
        ws.read(|w| w.test_selected_connection()).as_deref(),
        Some("shop")
    );

    ws.press("enter");
    let shop = ws.read(|w| {
        assert_eq!(w.tabs.len(), 1, "Enter opens a tab");
        assert_eq!(w.active, Some(0));
        w.tabs[0].id
    });
    ws.wait_for_state("shop to connect", move |s| s.browser(shop).is_some());
    let info = ws.browser().server_info.clone().unwrap_or_default();
    assert!(info.starts_with("SQLite "), "server info: {info:?}");
    assert_eq!(ws.read(|w| w.window_title.clone()), "shop — Cooper");
    ws.wait_tree_loaded();
    assert_eq!(ws.tree().model.nodes.len(), 1);
    assert!(
        wakes.load(Ordering::SeqCst) > 0,
        "the session's worker threads wake the window"
    );

    ws.press("cmd-1");
    assert!(ws.exists("connection-1"), "{}", ws.describe());
    assert_eq!(
        ws.find("connection-1").and_then(|e| e.label).as_deref(),
        Some("library")
    );
    ws.double_click("connection-1");
    let library = ws.read(|w| {
        assert_eq!(w.tabs.len(), 2, "a double click opens a second tab");
        assert_eq!(w.active, Some(1));
        assert_eq!(w.tabs[1].name(), "library");
        w.tabs[1].id
    });
    ws.wait_for_state("library to connect", move |s| {
        s.tab(library)
            .is_some_and(|tab| matches!(tab.state, TabState::Browser(_)))
    });
    ws.wait_tree_loaded();
    assert_eq!(ws.browser().connection.name, "library");
    assert_eq!(ws.read(|w| w.window_title.clone()), "library — Cooper");
    assert_eq!(
        ws.read(|w| w.test_tabs()),
        [("shop".to_string(), false), ("library".to_string(), false)]
    );
    assert!(ws.is_visible("browser-tree"), "{}", ws.describe());
}
