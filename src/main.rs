#![allow(dead_code)]
mod browser;
mod connection;
mod db;
mod dialog;
mod form;
mod help;
mod highlight;
mod icons;
mod keys;
mod queries;
#[cfg(test)]
mod test_support;
mod theme;
mod ui;
mod workspace;

use rune::{App, AppSpec, MenuItem, Shortcut, Titlebar, Tx};

use crate::{
    connection::Store,
    keys::Command,
    workspace::{TITLEBAR_HEIGHT, Workspace},
};

pub const WINDOW_SIZE: (f32, f32) = (1280.0, 820.0);
pub const MIN_SIZE: (f32, f32) = (760.0, 480.0);
const APP_ICON: &[u8] = include_bytes!("../assets/icons/icon.png");

pub fn spec() -> AppSpec<Workspace> {
    AppSpec {
        min_size: Some(MIN_SIZE),
        families: theme::FAMILIES,
        setup: Some(workspace::setup),
        poll: Some(workspace::poll),
        on_key: Some(keys::app_on_key),
        ..AppSpec::new(
            "Cooper",
            WINDOW_SIZE,
            workspace::default_workspace,
            workspace::view,
        )
    }
}

fn menu_item(
    title: &str,
    menu: Option<&str>,
    keys: Option<&str>,
    action: fn(&mut Workspace, &mut Tx),
) -> MenuItem<Workspace> {
    let mut item = MenuItem::new(title, action).keys_first();
    if let Some(menu) = menu {
        item = item.menu(menu);
    }
    if let Some(shortcut) = keys.and_then(|keys| keys.parse::<Shortcut>().ok()) {
        item = item.key_equivalent(shortcut);
    }
    item
}

pub fn app(store: Store) -> App<Workspace> {
    App::new(spec())
        .init(move || Workspace::new(store))
        .maximized(true)
        .titlebar(Titlebar::new(TITLEBAR_HEIGHT))
        .tab_visits_controls(true)
        .on_ready(|_| apply_app_icon())
        .on_close(workspace::on_close)
        .on_quit(workspace::on_quit)
        .menu_item(menu_item(
            "New Connection…",
            Some("File"),
            Some("cmd+n"),
            |s, tx| s.run_from_menu(Command::NewConnection, tx),
        ))
        .menu_item(MenuItem::separator().menu("File"))
        .menu_item(menu_item(
            "Close Tab",
            Some("File"),
            Some("cmd+w"),
            |s, tx| s.run_from_menu(Command::CloseTab, tx),
        ))
        .menu_item(menu_item(
            "Connections",
            Some("View"),
            Some("cmd+1"),
            |s, tx| s.run_from_menu(Command::GoHome, tx),
        ))
        .menu_item(menu_item(
            "Next Tab",
            Some("View"),
            Some("cmd+shift+]"),
            |s, tx| s.run_from_menu(Command::NextTab, tx),
        ))
        .menu_item(menu_item(
            "Previous Tab",
            Some("View"),
            Some("cmd+shift+["),
            |s, tx| s.run_from_menu(Command::PrevTab, tx),
        ))
        .menu_item(MenuItem::separator().menu("View"))
        .menu_item(menu_item(
            "Toggle Light/Dark",
            Some("View"),
            Some("cmd+shift+t"),
            |s, tx| s.run_from_menu(Command::ToggleTheme, tx),
        ))
        .menu_item(menu_item(
            "Keyboard Shortcuts",
            Some("Help"),
            Some("cmd+/"),
            |s, tx| s.run_from_menu(Command::ShowHelp, tx),
        ))
}

fn apply_app_icon() {
    if let Err(err) = rune::set_app_icon(APP_ICON) {
        eprintln!("cooper: {err}");
    }
}

fn main() {
    let store = match Store::default_location() {
        Ok(store) => store,
        Err(err) => {
            eprintln!("cooper: cannot locate config directory: {err:?}");
            std::process::exit(1);
        }
    };
    apply_app_icon();
    rune::prewarm_gpu();
    app(store).run();
}
