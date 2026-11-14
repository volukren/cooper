use std::collections::HashMap;

use rune::PointerPress;

use crate::{
    connection::{self, Connection, Store},
    db,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Connections,
    NewButton,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub name: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Original {
    pub name: String,
    pub password: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    Error,
    Info,
}

const TAB_TITLE_MAX: usize = 24;

pub struct Home {
    store: Store,
    pub connections: Vec<Connection>,
    selected: Option<usize>,
    pub focus: Focus,
    pub drafts: HashMap<String, String>,
    pub status: Option<String>,
    pub error: Option<Failure>,
    pub menu: Option<PointerPress>,
}

impl Home {
    pub fn new(store: Store, connections: Vec<Connection>) -> Self {
        let mut home = Self {
            store,
            connections: Vec::new(),
            selected: None,
            focus: Focus::NewButton,
            drafts: HashMap::new(),
            status: None,
            error: None,
            menu: None,
        };
        home.set_connections(connections);
        home
    }

    pub fn set_connections(&mut self, connections: Vec<Connection>) {
        self.connections = connections;
        if self.connections.is_empty() {
            self.focus = Focus::NewButton;
            self.selected = None;
        } else {
            self.focus = Focus::Connections;
            self.selected = Some(0);
        }
    }

    pub fn reload(&mut self) -> Result<(), String> {
        let connections = self.store.load().map_err(|err| format!("{err:#}"))?;
        self.set_connections(connections);
        Ok(())
    }

    pub fn selected_position(&self) -> Option<usize> {
        self.selected
    }

    pub fn selected(&self) -> Option<&Connection> {
        self.connections.get(self.selected?)
    }

    pub fn select(&mut self, position: usize) {
        let last = self.connections.len().checked_sub(1);
        self.selected = last.map(|last| position.min(last));
    }

    pub fn clear_messages(&mut self) {
        self.status = None;
        self.error = None;
    }

    pub fn select_next(&mut self) {
        self.clear_messages();
        match self.focus {
            Focus::NewButton if !self.connections.is_empty() => {
                self.focus = Focus::Connections;
                self.select(0);
            }
            Focus::Connections => {
                if let Some(position) = self.selected {
                    self.select(position + 1);
                }
            }
            Focus::NewButton => {}
        }
    }

    pub fn select_prev(&mut self) {
        self.clear_messages();
        if self.focus != Focus::Connections {
            return;
        }
        match self.selected {
            Some(position) if position > 0 => self.selected = Some(position - 1),
            _ => self.focus = Focus::NewButton,
        }
    }

    pub fn click_row(&mut self, position: usize) {
        self.clear_messages();
        self.focus = Focus::Connections;
        self.select(position);
    }

    pub fn target_for(&mut self, verb: &str, open: &[&str]) -> Option<Connection> {
        if self.focus != Focus::Connections {
            return None;
        }
        let conn = self.selected()?.clone();
        if open.contains(&conn.name.as_str()) {
            self.status = Some(format!(
                "\"{}\" is open in a tab, close it before {verb}",
                conn.name
            ));
            return None;
        }
        Some(conn)
    }

    pub fn status_line(&self) -> Option<(String, StatusKind)> {
        if let Some(Failure { name, message }) = &self.error {
            Some((
                format!("Connection failed: \"{name}\": {message}"),
                StatusKind::Error,
            ))
        } else {
            self.status
                .as_ref()
                .map(|status| (status.clone(), StatusKind::Info))
        }
    }

    pub fn failure_of(&self, name: &str) -> Option<&str> {
        self.error
            .as_ref()
            .filter(|failure| failure.name == name)
            .map(|failure| failure.message.as_str())
    }

    pub fn remember_draft(&mut self, name: &str, draft: String) {
        if draft.trim().is_empty() {
            self.drafts.remove(name);
        } else {
            self.drafts.insert(name.to_string(), draft);
        }
    }

    pub fn saved(&mut self, index: usize, status: String) {
        self.error = None;
        self.status = Some(status);
        self.focus = Focus::Connections;
        self.select(index);
    }

    pub fn save_new(
        &mut self,
        conn: Connection,
        password: &str,
    ) -> Result<(usize, String), String> {
        let uses_keychain = conn.uses_keychain();
        if uses_keychain {
            store_password(&conn.name, password)?;
        }
        if let Err(err) = self.store.append(&conn) {
            if uses_keychain {
                let _ = connection::delete_password(&conn.name);
            }
            return Err(format!("Failed to save: {err}"));
        }
        let status = format!("Connection \"{}\" saved", conn.name);
        self.connections.push(conn);
        Ok((self.connections.len() - 1, status))
    }

    pub fn save_edit(
        &mut self,
        original: &Original,
        conn: Connection,
        password: &str,
    ) -> Result<(usize, String), String> {
        let index = self
            .connections
            .iter()
            .position(|c| c.name == original.name)
            .ok_or_else(|| format!("Connection \"{}\" no longer exists", original.name))?;
        let renamed = conn.name != original.name;
        let had_keychain = self.connections[index].uses_keychain();
        let uses_keychain = conn.uses_keychain();

        if uses_keychain {
            store_password(&conn.name, password)?;
        }
        if let Err(err) = self.store.update(&original.name, &conn) {
            if uses_keychain {
                let restore = if had_keychain && !renamed {
                    original.password.as_str()
                } else {
                    ""
                };
                let _ = store_password(&conn.name, restore);
            }
            return Err(format!("Failed to save: {err}"));
        }

        let mut status = format!("Connection \"{}\" updated", conn.name);
        if had_keychain
            && (renamed || !uses_keychain)
            && let Err(err) = connection::delete_password(&original.name)
        {
            status.push_str(&format!(
                " (could not remove the old keychain entry \"{}\": {err})",
                original.name
            ));
        }
        if renamed && let Some(draft) = self.drafts.remove(&original.name) {
            self.drafts.insert(conn.name.clone(), draft);
        }
        self.connections[index] = conn;
        Ok((index, status))
    }

    pub fn delete_connection(&mut self, name: &str) {
        if let Err(err) = self.store.remove(name) {
            self.status = Some(format!("Failed to delete \"{name}\": {err}"));
            return;
        }
        let had_keychain = self
            .connections
            .iter()
            .find(|c| c.name == name)
            .is_some_and(Connection::uses_keychain);
        self.connections.retain(|c| c.name != name);
        self.drafts.remove(name);
        let mut status = format!("Connection \"{name}\" deleted");
        if had_keychain && let Err(err) = connection::delete_password(name) {
            status.push_str(&format!(", but its keychain password was not: {err}"));
        }
        self.error = None;
        self.status = Some(status);

        if self.connections.is_empty() {
            self.focus = Focus::NewButton;
        }
        self.select(self.selected.unwrap_or(0));
    }
}

fn store_password(name: &str, password: &str) -> Result<(), String> {
    let stored = if password.is_empty() {
        connection::delete_password(name)
    } else {
        connection::set_password(name, password)
    };
    stored.map_err(|err| format!("Failed to save password to keychain: {err}"))
}

pub fn cycle_tab(active: Option<usize>, tabs: usize, forward: bool) -> Option<usize> {
    let count = tabs + 1;
    let current = active.map_or(0, |index| index + 1);
    let next = if forward {
        (current + 1) % count
    } else {
        (current + count - 1) % count
    };
    next.checked_sub(1)
}

pub fn tab_at_position(position: usize, tabs: usize) -> Option<Option<usize>> {
    match position.checked_sub(2) {
        None => Some(None),
        Some(index) if index < tabs => Some(Some(index)),
        Some(_) => None,
    }
}

pub fn active_after_close(active: Option<usize>, closed: usize) -> Option<usize> {
    match active {
        Some(active) if active == closed => None,
        Some(active) if active > closed => Some(active - 1),
        active => active,
    }
}

pub fn window_title(connection: Option<&str>, what: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(3);
    if let Some(connection) = connection {
        if let Some(what) = what {
            parts.push(db::render_text(what));
        }
        parts.push(db::render_text(connection));
    }
    parts.push(APP_TITLE.to_string());
    parts.join(" — ")
}

pub const APP_TITLE: &str = "Cooper";

pub fn tab_title(name: &str) -> String {
    let text = db::render_text(name);
    if text.chars().count() > TAB_TITLE_MAX {
        let mut short: String = text.chars().take(TAB_TITLE_MAX - 1).collect();
        short.push('…');
        short
    } else {
        text
    }
}

pub fn tab_shortcut(index: usize) -> Option<String> {
    let position = index + 2;
    (position <= 9).then(|| format!("⌘{position}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn window_title_names_the_connection_and_what_it_shows() {
        assert_eq!(super::window_title(None, None), "Cooper");
        assert_eq!(super::window_title(None, Some("main.users")), "Cooper");
        assert_eq!(super::window_title(Some("local"), None), "local — Cooper");
        assert_eq!(
            super::window_title(Some("local"), Some("main.users")),
            "main.users — local — Cooper"
        );
        assert!(!super::window_title(Some("a\nb"), None).contains('\n'));
    }

    use std::path::Path;

    use super::{
        Focus, Home, Original, StatusKind, active_after_close, cycle_tab, tab_at_position,
        tab_shortcut, tab_title,
    };
    use crate::connection::{Connection, DbKind, PasswordStorage, Store};

    fn home_from(dir: &Path, toml: &str) -> Home {
        let config = dir.join("connections.toml");
        std::fs::write(&config, toml).unwrap();
        let store = Store::at(config);
        let connections = store.load().unwrap();
        Home::new(store, connections)
    }

    fn home_with_three(dir: &Path) -> Home {
        home_from(
            dir,
            "[[connection]]\nname = \"prod orders\"\nkind = \"postgres\"\nhost = \"db\"\n\
             port = 5432\nuser = \"app\"\ndatabase = \"orders\"\n\
             password_storage = \"plain\"\n\
             [[connection]]\nname = \"local\"\nkind = \"sqlite\"\ndatabase = \"1.db\"\n\
             [[connection]]\nname = \"Staging Orders\"\nkind = \"sqlite\"\ndatabase = \"2.db\"\n",
        )
    }

    fn sqlite(name: &str, database: &str) -> Connection {
        Connection {
            name: name.to_string(),
            kind: DbKind::Sqlite,
            tag: None,
            host: String::new(),
            port: None,
            user: String::new(),
            database: database.to_string(),
            password_storage: PasswordStorage::Keychain,
            password: String::new(),
        }
    }

    fn selected(home: &Home) -> Option<&str> {
        home.selected().map(|conn| conn.name.as_str())
    }

    fn names(home: &Home) -> Vec<&str> {
        home.connections.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn startup_selects_the_first_connection_or_the_new_button() {
        let dir = tempfile::tempdir().unwrap();
        let home = home_with_three(dir.path());
        assert_eq!(home.focus, Focus::Connections);
        assert_eq!(selected(&home), Some("prod orders"));
        assert_eq!(home.status_line(), None);

        let empty = home_from(dir.path(), "");
        assert_eq!(empty.focus, Focus::NewButton);
        assert_eq!(empty.selected_position(), None);
        assert_eq!(empty.status_line(), None);
    }

    #[test]
    fn arrows_move_between_the_list_and_the_new_button() {
        let dir = tempfile::tempdir().unwrap();
        let mut home = home_with_three(dir.path());
        home.click_row(2);
        assert_eq!(selected(&home), Some("Staging Orders"));
        for _ in 0..3 {
            home.select_prev();
        }
        assert_eq!(home.focus, Focus::NewButton, "up from the first row");
        home.select_prev();
        assert_eq!(home.focus, Focus::NewButton);
        assert!(
            home.target_for("editing", &[]).is_none(),
            "nothing on the button"
        );
        home.select_next();
        assert_eq!(home.focus, Focus::Connections);
        assert_eq!(selected(&home), Some("prod orders"));
        home.select_prev();
        assert_eq!(home.focus, Focus::NewButton);
        home.select_next();
        assert_eq!(home.focus, Focus::Connections);
        assert_eq!(selected(&home), Some("prod orders"));
        home.click_row(2);
        assert_eq!(selected(&home), Some("Staging Orders"));
    }

    #[test]
    fn open_connections_cannot_be_edited_or_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let mut home = home_with_three(dir.path());
        assert!(home.target_for("editing", &["prod orders"]).is_none());
        assert_eq!(
            home.status.as_deref(),
            Some("\"prod orders\" is open in a tab, close it before editing")
        );
        assert!(home.target_for("deleting", &["prod orders"]).is_none());
        assert!(
            home.status_line()
                .unwrap()
                .0
                .contains("close it before deleting")
        );
        let target = home.target_for("editing", &["local"]).unwrap();
        assert_eq!(target.name, "prod orders");
    }

    #[test]
    fn deleting_keeps_the_selection_in_range() {
        let dir = tempfile::tempdir().unwrap();
        let mut home = home_with_three(dir.path());
        let (index, status) = home
            .save_edit(
                &Original {
                    name: "Staging Orders".into(),
                    password: String::new(),
                },
                sqlite("other", "2.db"),
                "",
            )
            .unwrap();
        home.saved(index, status);
        assert_eq!(selected(&home), Some("other"));
        assert_eq!(home.status.as_deref(), Some("Connection \"other\" updated"));

        home.delete_connection("other");
        assert_eq!(home.connections.len(), 2);
        assert_eq!(home.status.as_deref(), Some("Connection \"other\" deleted"));
        assert_eq!(selected(&home), Some("local"));

        home.delete_connection("prod orders");
        home.delete_connection("local");
        assert!(home.connections.is_empty());
        assert_eq!(home.focus, Focus::NewButton);
        assert_eq!(home.selected_position(), None);
        let reloaded = Store::at(dir.path().join("connections.toml"))
            .load()
            .unwrap();
        assert!(reloaded.is_empty());
    }

    #[test]
    fn editing_can_rename_but_keeps_the_position() {
        let dir = tempfile::tempdir().unwrap();
        let mut home = home_from(
            dir.path(),
            "[[connection]]\nname = \"one\"\nkind = \"sqlite\"\ndatabase = \"1.db\"\n\
             [[connection]]\nname = \"two\"\nkind = \"sqlite\"\ndatabase = \"2.db\"\n",
        );
        let original = Original {
            name: "two".into(),
            password: String::new(),
        };
        let (index, status) = home
            .save_edit(&original, sqlite("three", "3.db"), "")
            .unwrap();
        home.saved(index, status);
        assert_eq!(home.selected_position(), Some(1));
        assert_eq!(names(&home), ["one", "three"]);
        let reloaded = Store::at(dir.path().join("connections.toml"))
            .load()
            .unwrap();
        let reloaded: Vec<_> = reloaded.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(reloaded, ["one", "three"]);

        let missing = Original {
            name: "gone".into(),
            password: String::new(),
        };
        assert_eq!(
            home.save_edit(&missing, sqlite("gone", "x.db"), ""),
            Err("Connection \"gone\" no longer exists".to_string())
        );
    }

    #[test]
    fn drafts_follow_a_renamed_connection() {
        let dir = tempfile::tempdir().unwrap();
        let mut home = home_from(
            dir.path(),
            "[[connection]]\nname = \"local\"\nkind = \"sqlite\"\ndatabase = \"1.db\"\n",
        );
        home.remember_draft("local", "draft".into());

        let original = Original {
            name: "local".into(),
            password: String::new(),
        };
        let (_, status) = home
            .save_edit(&original, sqlite("renamed", "1.db"), "")
            .unwrap();
        assert_eq!(status, "Connection \"renamed\" updated");
        assert_eq!(
            home.drafts.get("renamed").map(String::as_str),
            Some("draft")
        );
        assert!(!home.drafts.contains_key("local"));

        home.delete_connection("renamed");
        assert!(home.drafts.is_empty());
    }

    #[test]
    fn a_new_connection_can_keep_its_password_in_plain_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut home = home_from(dir.path(), "");
        let conn = Connection {
            name: "dev".into(),
            kind: DbKind::Postgres,
            tag: None,
            host: "localhost".into(),
            port: Some(5432),
            user: "postgres".into(),
            database: "app".into(),
            password_storage: PasswordStorage::Plain,
            password: "s3cret".into(),
        };
        let (index, status) = home.save_new(conn, "s3cret").unwrap();
        assert_eq!((index, status.as_str()), (0, "Connection \"dev\" saved"));
        home.saved(index, status);
        assert_eq!(home.focus, Focus::Connections);
        assert_eq!(selected(&home), Some("dev"));
        let data = std::fs::read_to_string(dir.path().join("connections.toml")).unwrap();
        assert!(data.contains("password_storage = \"plain\""), "{data}");
        assert!(data.contains("password = \"s3cret\""), "{data}");
    }

    #[test]
    fn drafts_are_forgotten_when_blank() {
        let dir = tempfile::tempdir().unwrap();
        let mut home = home_from(dir.path(), "");
        home.remember_draft("a", "SELECT 1;".into());
        assert_eq!(home.drafts.get("a").map(String::as_str), Some("SELECT 1;"));
        home.remember_draft("a", " \n ".into());
        assert!(home.drafts.is_empty());
    }

    #[test]
    fn failures_take_over_the_status_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut home = home_with_three(dir.path());
        home.status = Some("Disconnected: \"local\"".into());
        assert_eq!(home.status_line().unwrap().1, StatusKind::Info);
        home.status = None;
        home.error = Some(super::Failure {
            name: "local".into(),
            message: "boom".into(),
        });
        assert_eq!(
            home.status_line(),
            Some((
                "Connection failed: \"local\": boom".to_string(),
                StatusKind::Error
            ))
        );
        assert_eq!(home.failure_of("local"), Some("boom"));
        assert_eq!(home.failure_of("prod orders"), None);
        home.select_next();
        assert_eq!(home.error, None, "the next action clears it");
    }

    #[test]
    fn tabs_cycle_and_wrap_around() {
        assert_eq!(cycle_tab(None, 2, true), Some(0));
        assert_eq!(
            cycle_tab(Some(1), 2, true),
            None,
            "after the last tab comes home"
        );
        assert_eq!(cycle_tab(None, 2, false), Some(1));
        assert_eq!(cycle_tab(Some(0), 2, false), None);
        assert_eq!(cycle_tab(None, 0, true), None);

        assert_eq!(tab_at_position(1, 2), Some(None));
        assert_eq!(tab_at_position(3, 2), Some(Some(1)));
        assert_eq!(tab_at_position(9, 2), None, "a missing tab is ignored");

        assert_eq!(active_after_close(Some(1), 1), None);
        assert_eq!(active_after_close(Some(2), 0), Some(1));
        assert_eq!(active_after_close(Some(0), 1), Some(0));
        assert_eq!(active_after_close(None, 0), None);
    }

    #[test]
    fn tab_titles_are_sanitized_and_truncated() {
        assert_eq!(tab_title("first"), "first");
        assert_eq!(tab_title("a\nb\tc"), "a↵b→c");
        assert_eq!(tab_title(&"x".repeat(24)), "x".repeat(24));
        assert_eq!(tab_title(&"x".repeat(25)), format!("{}…", "x".repeat(23)));
        assert_eq!(tab_shortcut(0).as_deref(), Some("⌘2"));
        assert_eq!(tab_shortcut(7).as_deref(), Some("⌘9"));
        assert_eq!(tab_shortcut(8), None);
    }
}
