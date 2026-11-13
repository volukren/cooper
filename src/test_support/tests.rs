use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use super::{TestConfig, TestWorkspace, key_event};
use crate::{
    browser::{Main, Pane},
    dialog::Dialog,
    keys::Command,
    workspace::TabState,
};
use rune::{Key, Mods};

#[test]
fn key_specs_parse_into_key_events() {
    let ev = key_event("cmd-shift-]");
    assert_eq!(ev.key, Key::Command(']'));
    assert!(ev.mods.cmd && ev.mods.shift);
    let ev = key_event("ctrl-tab");
    assert_eq!(ev.key, Key::Tab);
    assert!(ev.mods.ctrl && !ev.mods.cmd);
    assert_eq!(key_event("space").key, Key::Text(" ".into()));
    assert_eq!(key_event("f6").key, Key::Function(6));
    assert_eq!(key_event("cmd--").key, Key::Command('-'));
    assert_eq!(key_event("a").mods, Mods::default());
}

#[test]
fn the_app_boots_on_the_connections_list() {
    let config = TestConfig::with_sample_dbs(&["shop", "library"]);
    let ws = TestWorkspace::open(&config);
    assert_eq!(ws.read(|w| w.test_connection_names()), ["shop", "library"]);
    assert_eq!(ws.read(|w| w.active), None);
    assert!(ws.read(|w| w.tabs.is_empty()));
    assert_eq!(ws.read(|w| w.window_title.clone()), "Cooper");
    assert!(ws.exists("home"), "{}", ws.describe());
    assert!(ws.exists("titlebar"));
    assert!(!ws.has_dialog());
    assert!(ws.is_focused("home"), "{}", ws.describe());
    assert_eq!(
        ws.read(|w| w.test_selected_connection()),
        Some("shop".to_string())
    );
}

#[test]
fn cmd_shift_t_toggles_the_theme() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    assert!(ws.read(|w| w.is_dark()), "the test window starts dark");
    ws.press("cmd-shift-t");
    assert_eq!(ws.read(|w| w.dark), Some(false));
    ws.press("cmd-shift-t");
    assert_eq!(ws.read(|w| w.dark), Some(true));
}

#[test]
fn help_opens_with_cmd_slash_and_f1_and_closes() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.press("cmd-/");
    assert!(ws.has_dialog(), "{}", ws.describe());
    assert!(ws.read(|w| matches!(w.dialog, Some(Dialog::Help(_)))));
    assert_eq!(ws.dialog_label().as_deref(), Some("Keyboard shortcuts"));
    ws.press("cmd-/");
    assert!(!ws.has_dialog(), "⌘/ toggles the help\n{}", ws.describe());
    ws.press("f1");
    assert!(ws.has_dialog());
    ws.press("escape");
    assert!(!ws.has_dialog(), "Esc closes the help\n{}", ws.describe());
    assert!(ws.read(|w| w.dialog.is_none()));
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
        ws.read(|w| w.dark),
        None,
        "workspace keys stay off while a dialog is open"
    );
    ws.press("escape");
    assert!(!ws.has_dialog(), "{}", ws.describe());
    assert!(!ws.read(|w| w.test_form_open()));
}

#[test]
fn commands_run_through_dispatch_and_the_title_follows() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    assert!(ws.dispatch(Command::ToggleTheme));
    assert!(!ws.dispatch(Command::CopyCell), "no browser to copy from");
    ws.connect("shop");
    assert_eq!(ws.read(|w| w.window_title.clone()), "shop — Cooper");
    ws.press("cmd-1");
    assert_eq!(ws.read(|w| w.active), None);
    assert_eq!(ws.read(|w| w.window_title.clone()), "Cooper");
    ws.press("cmd-2");
    assert_eq!(ws.read(|w| w.active), Some(0));
    ws.press("cmd-w");
    assert!(ws.read(|w| w.tabs.is_empty()));
    assert_eq!(
        ws.read(|w| w.home.status.clone()).as_deref(),
        Some("Disconnected: \"shop\"")
    );
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

#[test]
fn f6_cycles_the_panes_from_wherever_focus_is() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    assert!(ws.is_focused("schema-tree"), "{}", ws.describe());
    ws.press("f6");
    assert!(ws.is_focused("saved-queries"), "{}", ws.describe());
    assert_eq!(ws.browser().pane, Pane::Saved);
    ws.press("f6");
    assert!(ws.is_focused("table-view"), "{}", ws.describe());
    ws.press("shift-f6");
    assert!(ws.is_focused("saved-queries"), "{}", ws.describe());
    ws.focus("schema-tree");
    assert_eq!(ws.browser().pane, Pane::Tree, "the pane follows focus");
    ws.press("shift-f6");
    assert!(ws.is_focused("table-view"), "{}", ws.describe());
    assert_eq!(ws.browser().pane, Pane::Main);
}

#[test]
fn connect_shows_schemas_and_tables() {
    let config = TestConfig::with_sample_dbs(&["shop", "library"]);
    let mut ws = TestWorkspace::open(&config);
    assert_eq!(ws.read(|w| w.test_connection_names()), ["shop", "library"]);

    ws.connect("shop");
    assert_eq!(ws.read(|w| w.test_tabs()), [("shop".to_string(), false)]);
    assert_eq!(ws.browser().main, Main::Table);

    let model = &ws.tree().model;
    let schemas: Vec<String> = model.nodes.iter().map(|n| n.name.clone()).collect();
    let tables: Vec<String> = model.nodes[0]
        .tables
        .iter()
        .flatten()
        .map(|t| t.name.clone())
        .collect();
    assert_eq!(schemas, ["main"]);
    for table in [
        "customers",
        "orders",
        "order_items",
        "products",
        "v_order_totals",
    ] {
        assert!(
            tables.iter().any(|t| t == table),
            "{table} missing from {tables:?}"
        );
    }
    assert_eq!(model.rows().len(), 1 + tables.len());
}

#[test]
fn open_table_shows_page_rows() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");

    ws.open_table("customers");

    let view = ws.table_view();
    let grid = view.grid.as_ref().unwrap();
    let page = grid.page().unwrap();
    let columns: Vec<String> = page.columns.iter().map(|c| c.name.clone()).collect();
    assert_eq!(
        view.current(),
        Some(("main".to_string(), "customers".to_string()))
    );
    assert_eq!(
        columns,
        ["id", "name", "email", "country", "created_at", "is_active"]
    );
    assert_eq!(page.rows.len(), 8);
    assert_eq!(page.offset, 0);
    assert_eq!(page.rows[0][0].as_deref(), Some("1"));
    assert_eq!(
        ws.tree().open_table,
        Some(("main".to_string(), "customers".to_string()))
    );
    assert!(ws.is_visible("table-view"));
    ws.wait_for_state("the row count", |s| {
        s.active_browser()
            .and_then(|b| b.table.grid.as_ref())
            .and_then(|grid| grid.total())
            == Some(8)
    });
}

#[test]
fn console_runs_a_script_and_shows_the_summary() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");

    let outcome = ws.run_sql("SELECT id, name FROM customers WHERE id <= 3 ORDER BY id");
    assert_eq!(ws.browser().main, Main::Console);
    assert!(outcome.starts_with("3 rows · "), "summary: {outcome}");

    let result = ws.console().result().unwrap().clone();
    let columns: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(columns, ["id", "name"]);
    assert_eq!(result.rows.len(), 3);

    let outcome = ws.run_sql("SELECT * FROM no_such_table");
    assert!(
        outcome.starts_with("error: ") && outcome.contains("no_such_table"),
        "{outcome}"
    );
}

#[test]
fn new_connection_dialog_cancels() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let before = config.connections_toml();
    let mut ws = TestWorkspace::open(&config);

    ws.press("cmd-n");
    assert!(ws.has_dialog(), "⌘N opens the new connection dialog");
    assert!(ws.read(|w| w.test_form_open()));
    assert!(ws.exists("connection-form"));
    assert!(!ws.descendants("connection-dialog").is_empty());

    ws.type_text("scratch");
    let name = ws.read(|w| match &w.dialog {
        Some(Dialog::Form(session)) => session.form.values().name,
        _ => String::new(),
    });
    assert_eq!(name, "scratch");
    ws.click_in("connection-dialog", "Cancel");
    ws.wait_until("the dialog to close", |ws| !ws.has_dialog());
    assert!(!ws.read(|w| w.test_form_open()));

    ws.press("cmd-n");
    assert!(ws.has_dialog());
    ws.press("escape");
    ws.wait_until("the dialog to close on Esc", |ws| !ws.has_dialog());
    assert!(!ws.read(|w| w.test_form_open()));

    assert_eq!(ws.read(|w| w.test_connection_names()), ["shop"]);
    assert_eq!(
        config.connections_toml(),
        before,
        "cancel must not write the config"
    );
}

#[test]
fn a_connected_tab_draws_the_browser_layout() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    for key in [
        "tab-strip",
        "browser",
        "browser-sidebar",
        "browser-tree",
        "saved-pane",
        "browser-main",
        "browser-status-bar",
        "browser-split-handle",
    ] {
        assert!(ws.is_visible(key), "{key} is not drawn\n{}", ws.describe());
    }
    assert!(ws.has_label("Disconnect shop"));
    ws.snap("browser");
    ws.click_label("Disconnect shop");
    assert!(ws.read(|w| w.tabs.is_empty()), "{}", ws.describe());
}

#[test]
fn the_table_row_view_scrolls_with_a_scroll_bar() {
    let config = TestConfig::with_sample_dbs(&["shop"]);
    let mut ws = TestWorkspace::open(&config);
    ws.connect("shop");
    ws.open_table("customers");
    ws.double_click("table-row-0");
    assert!(
        ws.table_view()
            .grid
            .as_ref()
            .is_some_and(|grid| grid.record_open())
    );
    assert!(ws.is_visible("record-view"));
    assert!(
        ws.exists("table-record-scroll-scroll-area"),
        "the row view's scroll area carries the kit scroll bar, as the console's record view does"
    );
}

#[test]
fn dialogs_sit_above_the_dimmed_page_in_both_themes() {
    for dark in [false, true] {
        let t = crate::theme::theme(dark);
        let scrim = t.k.c.scrim;
        let dimmed = |c: f32| c * (1.0 - scrim.a);
        let lum = |c: rune::Color| c.r + c.g + c.b;
        let backdrop = dimmed(t.background.r) + dimmed(t.background.g) + dimmed(t.background.b);
        assert_eq!(t.modal.c.layer_01, t.background);
        assert!(lum(t.modal.c.layer_01) > backdrop, "dark: {dark}");
        let ring = t
            .modal
            .elevation
            .overlay
            .iter()
            .find(|s| s.blur == 0.0)
            .expect("the overlay elevation has a ring");
        assert_eq!(ring.color, t.modal_border);
        assert!(ring.spread >= 1.0);
        assert!(t.modal_border.a >= t.border.a, "dark: {dark}");
    }
    let dark = crate::theme::theme(true);
    assert!(dark.modal_border.a > 2.0 * dark.border.a);
}
