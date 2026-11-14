use std::cell::Cell;

use rune::{
    Cx, El, FileDialogOptions, FileDialogResult, Key, KeyEvent, TextEdit, Tx, col, row, text,
};
use rune_kit::{
    Button, ButtonVariant, ControlSize, Dialog as KitDialog, IconButton, IconButtonVariant,
    Placement, RadioItem, Select, SelectAppearance, TextInput,
};

use crate::{
    connection::{self, Connection, DbKind, PasswordStorage, Tag},
    dialog::Dialog,
    icons, keys,
    theme::Theme,
    ui,
    workspace::{self, Workspace, model::Original},
};

const KINDS: [DbKind; 3] = [DbKind::Postgres, DbKind::MySql, DbKind::Sqlite];
const STORAGES: [PasswordStorage; 2] = [PasswordStorage::Keychain, PasswordStorage::Plain];
const TAGS: [Option<Tag>; 5] = [
    None,
    Some(Tag::Dev),
    Some(Tag::Test),
    Some(Tag::Staging),
    Some(Tag::Prod),
];
const PROD_HINT: &str =
    "Scripts that change data or schema, and row edits, will ask for confirmation.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormValues {
    pub kind: DbKind,
    pub name: String,
    pub host: String,
    pub port: String,
    pub user: String,
    pub password: String,
    pub storage: PasswordStorage,
    pub database: String,
    pub tag: Option<Tag>,
}

impl Default for FormValues {
    fn default() -> Self {
        let kind = DbKind::Postgres;
        Self {
            kind,
            name: String::new(),
            host: "localhost".into(),
            port: port_text(kind),
            user: String::new(),
            password: String::new(),
            storage: PasswordStorage::default(),
            database: String::new(),
            tag: None,
        }
    }
}

impl FormValues {
    pub fn from_connection(conn: &Connection, password: &str) -> Self {
        Self {
            kind: conn.kind,
            name: conn.name.clone(),
            host: conn.host.clone(),
            port: conn.port.map(|p| p.to_string()).unwrap_or_default(),
            user: conn.user.clone(),
            password: password.to_string(),
            storage: conn.password_storage,
            database: conn.database.clone(),
            tag: conn.tag,
        }
    }
}

fn port_text(kind: DbKind) -> String {
    kind.default_port()
        .map(|p| p.to_string())
        .unwrap_or_default()
}

pub fn port_after_kind_change(port: &str, old: DbKind, new: DbKind) -> Option<String> {
    (port.is_empty() || port == port_text(old)).then(|| port_text(new))
}

pub fn database_label(kind: DbKind) -> &'static str {
    if kind.is_file_based() {
        "File path"
    } else {
        "Database"
    }
}

pub fn validate(
    values: &FormValues,
    original_name: Option<&str>,
    existing: &[Connection],
) -> Result<(Connection, String), String> {
    let name = values.name.trim();
    if name.is_empty() {
        return Err("Connection name is required".into());
    }
    let new_name = original_name.is_none_or(|original| original != name);
    if new_name && existing.iter().any(|c| c.name == name) {
        return Err(format!("Connection \"{name}\" already exists"));
    }
    let database = values.database.trim().to_string();

    if values.kind.is_file_based() {
        if database.is_empty() {
            return Err("Database file path is required".into());
        }
        let conn = Connection {
            name: name.into(),
            kind: values.kind,
            tag: values.tag,
            host: String::new(),
            port: None,
            user: String::new(),
            database,
            password_storage: PasswordStorage::default(),
            password: String::new(),
        };
        return Ok((conn, String::new()));
    }

    let host = values.host.trim();
    if host.is_empty() {
        return Err("Host is required".into());
    }
    let port = match values.port.trim() {
        "" => None,
        p => Some(p.parse::<u16>().map_err(|_| format!("Invalid port: {p}"))?),
    };
    let conn = Connection {
        name: name.into(),
        kind: values.kind,
        tag: values.tag,
        host: host.into(),
        port,
        user: values.user.trim().into(),
        database,
        password_storage: values.storage,
        password: match values.storage {
            PasswordStorage::Plain => values.password.clone(),
            PasswordStorage::Keychain => String::new(),
        },
    };
    Ok((conn, values.password.clone()))
}

pub fn sanitize_paste(text: &str, digits_only: bool) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .filter(|c| !digits_only || c.is_ascii_digit())
        .collect()
}

pub fn is_port_text(text: &str) -> bool {
    text.chars().all(|c| c.is_ascii_digit())
}

pub fn load_password(conn: &Connection) -> Result<Connection, String> {
    let password = connection::password(conn)
        .map_err(|err| format!("Cannot edit: reading the password from the keychain: {err}"))?;
    Ok(Connection {
        password: password.unwrap_or_default(),
        ..conn.clone()
    })
}

fn storage_label(storage: PasswordStorage) -> &'static str {
    match storage {
        PasswordStorage::Keychain => "In system keychain",
        PasswordStorage::Plain => "As plain text in config",
    }
}

pub fn database_placeholder(kind: DbKind) -> &'static str {
    if kind.is_file_based() {
        "/path/to/database.sqlite"
    } else {
        "optional"
    }
}

const LABEL_WIDTH: f32 = 112.0;
/// The space between a field's label and its control.
const FIELD_GAP: f32 = 12.0;
const CONTROL_WIDTH: f32 = 344.0;
const FIELDS: [&str; 6] = ["name", "host", "port", "user", "password", "database"];

pub struct FormDialog {
    pub form: ConnectionForm,
    pub original: Option<Original>,
}

pub struct ConnectionForm {
    pub original: Option<Connection>,
    pub kind: DbKind,
    pub storage: PasswordStorage,
    pub tag: Option<Tag>,
    pub name: TextEdit,
    pub host: TextEdit,
    pub port: TextEdit,
    pub user: TextEdit,
    pub password: TextEdit,
    pub database: TextEdit,
    pub show_password: bool,
    pub kind_open: bool,
    pub tag_open: bool,
    pub error: Option<String>,
    pub last_field: Cell<Option<&'static str>>,
}

impl ConnectionForm {
    pub fn new(original: Option<Connection>) -> Self {
        let mut original = original;
        let mut error = None;
        if let Some(conn) = original
            .as_mut()
            .filter(|c| c.uses_keychain() && c.password.is_empty())
        {
            match load_password(conn) {
                Ok(loaded) => *conn = loaded,
                Err(msg) => error = Some(msg),
            }
        }
        let values = match &original {
            Some(conn) => FormValues::from_connection(conn, &conn.password),
            None => FormValues::default(),
        };
        let mut name = TextEdit::new(values.name.clone());
        name.set_caret(name.text.len());
        name.focused = true;
        let mut password = TextEdit::new("").secure();
        password.replace_text(values.password.clone());
        Self {
            original,
            kind: values.kind,
            storage: values.storage,
            tag: values.tag,
            name,
            host: TextEdit::new(values.host.clone()),
            port: TextEdit::new(values.port.clone()),
            user: TextEdit::new(values.user.clone()),
            password,
            database: TextEdit::new(values.database.clone()),
            show_password: false,
            kind_open: false,
            tag_open: false,
            error,
            last_field: Cell::new(Some("name")),
        }
    }

    pub fn values(&self) -> FormValues {
        FormValues {
            kind: self.kind,
            name: self.name.text.clone(),
            host: self.host.text.clone(),
            port: self.port.text.clone(),
            user: self.user.text.clone(),
            password: self.password.text.clone(),
            storage: self.storage,
            database: self.database.text.clone(),
            tag: self.tag,
        }
    }

    pub fn build(&self, existing: &[Connection]) -> Result<(Connection, String), String> {
        let original = self.original.as_ref().map(|c| c.name.as_str());
        validate(&self.values(), original, existing)
    }

    pub fn set_error(&mut self, message: String) {
        self.error = Some(message);
    }

    pub fn set_kind(&mut self, kind: DbKind) {
        if kind == self.kind {
            return;
        }
        if let Some(port) = port_after_kind_change(&self.port.text, self.kind, kind) {
            let focused = self.port.focused;
            self.port.replace_text(port);
            self.port.set_caret(self.port.text.len());
            self.port.focused = focused;
        }
        self.kind = kind;
        self.error = None;
    }

    pub fn set_storage(&mut self, storage: PasswordStorage) {
        self.storage = storage;
        self.error = None;
    }

    pub fn set_tag(&mut self, tag: Option<Tag>) {
        self.tag = tag;
        self.error = None;
    }

    pub fn title(&self) -> &'static str {
        if self.original.is_some() {
            "Edit connection"
        } else {
            "New connection"
        }
    }

    pub fn field(&self, field: &str) -> &TextEdit {
        match field {
            "name" => &self.name,
            "host" => &self.host,
            "port" => &self.port,
            "user" => &self.user,
            "password" => &self.password,
            "database" => &self.database,
            other => panic!("no text field {other:?}"),
        }
    }

    pub fn field_mut(&mut self, field: &str) -> &mut TextEdit {
        match field {
            "name" => &mut self.name,
            "host" => &mut self.host,
            "port" => &mut self.port,
            "user" => &mut self.user,
            "password" => &mut self.password,
            "database" => &mut self.database,
            other => panic!("no text field {other:?}"),
        }
    }

    pub fn focused_field(&self) -> Option<&'static str> {
        FIELDS.into_iter().find(|field| self.field(field).focused)
    }

    pub fn focus_field(&mut self, field: &str) {
        for other in FIELDS {
            self.field_mut(other).focused = false;
        }
        self.field_mut(field).focused = true;
    }

    #[cfg(test)]
    pub fn test_set_text(&mut self, field: &str, text: &str) {
        let edit = self.field_mut(field);
        let focused = edit.focused;
        edit.replace_text(text.to_string());
        edit.set_caret(edit.text.len());
        edit.focused = focused;
    }
}

pub fn kind_labels() -> [&'static str; 3] {
    KINDS.map(DbKind::label)
}

fn kind_of(label: &str) -> Option<DbKind> {
    KINDS.into_iter().find(|kind| kind.label() == label)
}

pub fn tag_label(tag: Option<Tag>) -> &'static str {
    tag.map_or("None", Tag::label)
}

pub fn tag_labels() -> [&'static str; 5] {
    TAGS.map(tag_label)
}

fn tag_of(label: &str) -> Option<Option<Tag>> {
    TAGS.into_iter().find(|&tag| tag_label(tag) == label)
}

fn with_form(s: &mut Workspace, f: impl FnOnce(&mut ConnectionForm)) {
    if let Some(Dialog::Form(session)) = &mut s.dialog {
        f(&mut session.form);
    }
}

type Pick = for<'a> fn(&'a mut ConnectionForm) -> &'a mut TextEdit;

fn form_edit(s: &mut Workspace, pick: Pick) -> &mut TextEdit {
    workspace::dialog_edit(s, |d| match d {
        Dialog::Form(session) => Some(pick(&mut session.form)),
        _ => None,
    })
}

fn refocus(s: &mut Workspace) {
    with_form(s, |f| {
        if f.focused_field().is_none()
            && let Some(field) = f.last_field.get()
        {
            f.focus_field(field);
        }
    });
}

fn press_save(s: &mut Workspace, _tx: &mut Tx) {
    s.submit_form();
    refocus(s);
}

fn clear_error(s: &mut Workspace, _tx: &mut Tx) {
    with_form(s, |f| f.error = None);
}

fn submit(s: &mut Workspace, _tx: &mut Tx) {
    s.submit_form();
}

fn port_key(s: &mut Workspace, ev: &KeyEvent, _tx: &mut Tx) -> bool {
    let mods = ev.mods;
    if mods.cmd || mods.ctrl || mods.alt || mods.logo {
        return false;
    }
    let Key::Text(typed) = &ev.key else {
        return false;
    };
    if is_port_text(typed) {
        return false;
    }
    let digits = sanitize_paste(typed, true);
    with_form(s, |f| {
        if !digits.is_empty() {
            f.port.insert(&digits);
            f.error = None;
        }
    });
    true
}

fn port_change(s: &mut Workspace, _tx: &mut Tx) {
    with_form(s, |f| {
        f.error = None;
        if is_port_text(&f.port.text) {
            return;
        }
        let caret = f.port.range().1;
        let before = f.port.text[..caret]
            .chars()
            .filter(char::is_ascii_digit)
            .count();
        let clean = sanitize_paste(&f.port.text, true);
        let focused = f.port.focused;
        f.port.replace_text(clean);
        f.port.set_caret(before);
        f.port.focused = focused;
    });
}

fn input(
    edit: &TextEdit,
    key: &'static str,
    placeholder: &str,
    label: &str,
    pick: Pick,
) -> TextInput {
    TextInput::new(edit)
        .placeholder(placeholder)
        .label(label)
        .width(None)
        .on_edit(move |s: &mut Workspace| form_edit(s, pick))
        .on_submit(submit)
        .on_change(if key == "form-port" {
            port_change
        } else {
            clear_error
        })
}

fn field_row(label: &str, control: El, t: &Theme) -> El {
    row()
        .w_full()
        .gap(FIELD_GAP)
        .center_y()
        .child(
            text(label.to_string())
                .font(t.text_sm)
                .color(t.foreground)
                .w(LABEL_WIDTH),
        )
        .child(col().grow(1.0).min_w(0.0).child(control))
}

fn select_row(key: &'static str, select: El) -> El {
    row()
        .key(key)
        .w_full()
        .on_key(|s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| {
            if keys::route(s, ev, tx) {
                return true;
            }
            if ev.key == Key::Enter && ev.mods == rune::Mods::default() {
                s.submit_form();
                return true;
            }
            false
        })
        .child(select)
}

fn browse(s: &mut Workspace, tx: &mut Tx) {
    refocus(s);
    let options = FileDialogOptions {
        title: "Choose".into(),
        ..Default::default()
    };
    tx.open_file(
        options,
        |s: &mut Workspace, result: FileDialogResult, _tx: &mut Tx| {
            if let Ok(Some(path)) = result {
                let path = path.to_string_lossy().into_owned();
                with_form(s, |f| {
                    let focused = f.database.focused;
                    f.database.replace_text(path);
                    f.database.set_caret(f.database.text.len());
                    f.database.focused = focused;
                    f.error = None;
                });
            }
        },
    );
}

pub fn dialog(
    _s: &Workspace,
    session: &FormDialog,
    t: &Theme,
    _cx: &mut Cx<Workspace>,
    layer: &mut Vec<El>,
) {
    let form = &session.form;
    if let Some(field) = form.focused_field() {
        form.last_field.set(Some(field));
    }
    let server = !form.kind.is_file_based();
    let mut popups = Vec::new();
    let field_key = |s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| keys::field_key(s, ev, tx);

    let name = input(&form.name, "form-name", "e.g. production", "Name", |f| {
        &mut f.name
    })
    .on_key(field_key)
    .build(&t.k)
    .key("form-name");

    let kind = Select::new("form-kind", &kind_labels())
        .appearance(SelectAppearance::Field)
        .width(CONTROL_WIDTH)
        .label("Type")
        .current(Some(form.kind.label()))
        .open(form.kind_open)
        .placement(Placement::BottomStart)
        .on_toggle(|s: &mut Workspace| {
            with_form(s, |f| {
                f.kind_open = !f.kind_open;
                f.tag_open = false;
            })
        })
        .on_close(|s: &mut Workspace| with_form(s, |f| f.kind_open = false))
        .on_choose(|s: &mut Workspace, label: &str, _tx: &mut Tx| {
            let kind = kind_of(label);
            with_form(s, |f| {
                f.kind_open = false;
                if let Some(kind) = kind {
                    f.set_kind(kind);
                }
            });
        })
        .build(&t.k, &mut popups);
    let kind = select_row("form-kind", kind);

    let tag = Select::new("form-tag", &tag_labels())
        .appearance(SelectAppearance::Field)
        .width(CONTROL_WIDTH)
        .label("Environment")
        .current(Some(tag_label(form.tag)))
        .open(form.tag_open)
        .placement(Placement::BottomStart)
        .on_toggle(|s: &mut Workspace| {
            with_form(s, |f| {
                f.tag_open = !f.tag_open;
                f.kind_open = false;
            })
        })
        .on_close(|s: &mut Workspace| with_form(s, |f| f.tag_open = false))
        .on_choose(|s: &mut Workspace, label: &str, _tx: &mut Tx| {
            let tag = tag_of(label);
            with_form(s, |f| {
                f.tag_open = false;
                if let Some(tag) = tag {
                    f.set_tag(tag);
                }
            });
        })
        .build(&t.k, &mut popups);
    let tag = select_row("form-tag", tag);

    let mut body = col()
        .key("connection-form")
        .w_full()
        .px(16.0)
        .pb(4.0)
        .gap(10.0)
        .child(field_row("Name", name, t))
        .child(field_row("Type", kind, t));

    if server {
        let host = input(&form.host, "form-host", "localhost", "Host", |f| {
            &mut f.host
        })
        .on_key(field_key)
        .build(&t.k)
        .key("form-host");
        let port = input(&form.port, "form-port", "default", "Port", |f| &mut f.port)
            .on_key(|s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx| {
                keys::field_key(s, ev, tx) || port_key(s, ev, tx)
            })
            .build(&t.k)
            .key("form-port");
        let user = input(&form.user, "form-user", "user name", "User", |f| {
            &mut f.user
        })
        .on_key(field_key)
        .build(&t.k)
        .key("form-user");
        let shown = form.show_password;
        let reveal = IconButton::new(if shown { &icons::EYE_OFF } else { &icons::EYE })
            .variant(IconButtonVariant::Ghost)
            .control_size(ControlSize::Small)
            .label(if shown {
                "Hide password"
            } else {
                "Show password"
            })
            .on_press(|s: &mut Workspace| {
                with_form(s, |f| f.show_password = !f.show_password);
                refocus(s);
            })
            .build(&t.k)
            .key("form-reveal");
        let password = input(
            &form.password,
            "form-password",
            "password",
            "Password",
            |f| &mut f.password,
        )
        .secure()
        .revealed(shown)
        .trailing(reveal)
        .on_key(field_key)
        .build(&t.k)
        .key("form-password");
        let storage = row()
            .key("form-storage")
            .role("radiogroup")
            .label("Store password")
            .focus_group()
            .gap(16.0)
            .center_y()
            .children(STORAGES.into_iter().map(|storage| {
                RadioItem::new(storage_label(storage), form.storage == storage)
                    .on_select(move |s: &mut Workspace| with_form(s, |f| f.set_storage(storage)))
                    .build(&t.k)
            }));
        body = body
            .child(field_row("Host", host, t))
            .child(field_row("Port", port, t))
            .child(field_row("User", user, t))
            .child(field_row("Password", password, t))
            .child(field_row("Store password", storage, t));
    }

    let database = input(
        &form.database,
        "form-database",
        database_placeholder(form.kind),
        database_label(form.kind),
        |f| &mut f.database,
    )
    .on_key(field_key)
    .build(&t.k)
    .key("form-database");
    let database = if server {
        database
    } else {
        row()
            .w_full()
            .gap(8.0)
            .center_y()
            .child(col().grow(1.0).min_w(0.0).child(database))
            .child(
                Button::new("Browse…")
                    .icon(&icons::FOLDER_OPEN)
                    .variant(ButtonVariant::Outline)
                    .help("Choose the SQLite database file")
                    .on_press_tx(browse)
                    .build(&t.k)
                    .key("browse"),
            )
    };
    body = body
        .child(field_row(database_label(form.kind), database, t))
        .child(field_row("Environment", tag, t));
    if form.tag == Some(Tag::Prod) {
        body = body.child(
            row()
                .w_full()
                .child(col().w(LABEL_WIDTH + FIELD_GAP))
                .child(
                    text(PROD_HINT)
                        .key("form-tag-hint")
                        .font(t.text_xs)
                        .color(t.muted)
                        .wrap()
                        .grow(1.0)
                        .min_w(0.0),
                ),
        );
    }

    if let Some(error) = &form.error {
        body = body.child(
            row()
                .key("form-error")
                .w_full()
                .gap(8.0)
                .items_start()
                .child(
                    col()
                        .pt(2.0)
                        .child(ui::icon_el(&icons::CIRCLE_X, 14.0, t.danger)),
                )
                .child(
                    text(error.clone())
                        .font(t.text_sm)
                        .color(t.danger)
                        .wrap_anywhere()
                        .grow(1.0)
                        .min_w(0.0),
                ),
        );
    }

    let footer = row()
        .key("form-footer")
        .w_full()
        .padding(16.0)
        .gap(8.0)
        .justify_end()
        .child(
            Button::new("Cancel")
                .variant(ButtonVariant::Outline)
                .on_press_tx(|s: &mut Workspace, tx: &mut Tx| s.close_dialog(tx))
                .build(&t.k)
                .key("form-cancel"),
        )
        .child(
            Button::new("Save")
                .icon(&icons::CHECK)
                .variant(ButtonVariant::Submit)
                .on_press_tx(press_save)
                .build(&t.k)
                .key("save-connection"),
        );
    KitDialog::new("connection", form.title())
        .width(500.0)
        .fit()
        .open(true)
        .on_close(|s: &mut Workspace| {
            s.dialog = None;
            s.refocus = true;
        })
        .child(body)
        .child(footer)
        .build(&t.modal, layer);
    layer.extend(popups);
}

#[cfg(test)]
mod tests {
    use super::{
        FormValues, TAGS, is_port_text, port_after_kind_change, sanitize_paste, tag_label,
        tag_labels, tag_of, validate,
    };
    use crate::connection::{Connection, DbKind, PasswordStorage, Tag};

    fn conn(name: &str, kind: DbKind) -> Connection {
        Connection {
            name: name.into(),
            kind,
            tag: None,
            host: if kind.is_file_based() {
                String::new()
            } else {
                "localhost".into()
            },
            port: kind.default_port(),
            user: String::new(),
            database: if kind.is_file_based() {
                "/tmp/x.db".into()
            } else {
                String::new()
            },
            password_storage: PasswordStorage::default(),
            password: String::new(),
        }
    }

    fn values(name: &str) -> FormValues {
        FormValues {
            name: name.into(),
            ..FormValues::default()
        }
    }

    #[test]
    fn validation_runs_in_order() {
        let existing = [conn("one", DbKind::Postgres)];
        assert_eq!(
            validate(&values("   "), None, &existing).unwrap_err(),
            "Connection name is required"
        );
        assert_eq!(
            validate(&values(" one "), None, &existing).unwrap_err(),
            "Connection \"one\" already exists"
        );
        let no_host = FormValues {
            host: "  ".into(),
            ..values("two")
        };
        assert_eq!(
            validate(&no_host, None, &existing).unwrap_err(),
            "Host is required"
        );
        let bad_port = FormValues {
            port: "99999".into(),
            ..values("two")
        };
        assert_eq!(
            validate(&bad_port, None, &existing).unwrap_err(),
            "Invalid port: 99999"
        );
        let sqlite = FormValues {
            kind: DbKind::Sqlite,
            database: "  ".into(),
            ..values("two")
        };
        assert_eq!(
            validate(&sqlite, None, &existing).unwrap_err(),
            "Database file path is required"
        );
    }

    #[test]
    fn duplicate_names_are_case_sensitive() {
        let existing = [conn("one", DbKind::Postgres)];
        assert!(validate(&values("One"), None, &existing).is_ok());
    }

    #[test]
    fn editing_can_rename_but_not_onto_another_connection() {
        let existing = [conn("one", DbKind::Postgres), conn("two", DbKind::Postgres)];
        assert!(validate(&values("two"), Some("two"), &existing).is_ok());
        let (renamed, _) = validate(&values("three"), Some("two"), &existing).unwrap();
        assert_eq!(renamed.name, "three");
        assert_eq!(
            validate(&values("one"), Some("two"), &existing).unwrap_err(),
            "Connection \"one\" already exists"
        );
    }

    #[test]
    fn sqlite_discards_server_fields() {
        let v = FormValues {
            kind: DbKind::Sqlite,
            name: " local ".into(),
            host: "db.example.com".into(),
            port: "6543".into(),
            user: "admin".into(),
            password: "secret".into(),
            storage: PasswordStorage::Plain,
            database: "  /data/app.db ".into(),
            tag: Some(Tag::Staging),
        };
        let (conn, password) = validate(&v, None, &[]).unwrap();
        assert_eq!(conn.name, "local");
        assert_eq!(conn.database, "/data/app.db");
        assert!(conn.host.is_empty() && conn.user.is_empty() && conn.password.is_empty());
        assert_eq!(conn.port, None);
        assert_eq!(conn.password_storage, PasswordStorage::Keychain);
        assert_eq!(conn.tag, Some(Tag::Staging));
        assert_eq!(password, "");
    }

    #[test]
    fn the_tag_round_trips_through_validation_and_edit() {
        for kind in [DbKind::Postgres, DbKind::Sqlite] {
            let v = FormValues {
                kind,
                name: "shop".into(),
                database: "/tmp/shop.db".into(),
                tag: Some(Tag::Prod),
                ..FormValues::default()
            };
            let (conn, _) = validate(&v, None, &[]).unwrap();
            assert_eq!(conn.tag, Some(Tag::Prod), "{kind:?}");
            assert!(conn.asks_to_confirm());

            let reopened = FormValues::from_connection(&conn, "");
            assert_eq!(reopened.tag, Some(Tag::Prod));
            let (again, _) =
                validate(&reopened, Some("shop"), std::slice::from_ref(&conn)).unwrap();
            assert_eq!(again.tag, Some(Tag::Prod), "{kind:?}");
        }
    }

    #[test]
    fn tag_labels_map_back_to_tags() {
        for (label, tag) in tag_labels().into_iter().zip(TAGS) {
            assert_eq!(tag_label(tag), label);
            assert_eq!(tag_of(label), Some(tag));
        }
        assert_eq!(tag_of("Production"), Some(Some(Tag::Prod)));
        assert_eq!(tag_of("None"), Some(None));
        assert_eq!(tag_of("x"), None);
    }

    #[test]
    fn server_connection_fields_are_trimmed_but_the_password_is_not() {
        let v = FormValues {
            name: "dev".into(),
            host: " db ".into(),
            port: " ".into(),
            user: " postgres ".into(),
            password: " s3cret ".into(),
            database: " app ".into(),
            ..FormValues::default()
        };
        let (conn, password) = validate(&v, None, &[]).unwrap();
        assert_eq!(conn.host, "db");
        assert_eq!(conn.port, None);
        assert_eq!(conn.user, "postgres");
        assert_eq!(conn.database, "app");
        assert_eq!(conn.password, "");
        assert_eq!(password, " s3cret ");
    }

    #[test]
    fn a_new_connection_can_keep_its_password_in_plain_text() {
        let v = FormValues {
            name: "dev".into(),
            user: "postgres".into(),
            password: "s3cret".into(),
            storage: PasswordStorage::Plain,
            database: "app".into(),
            ..FormValues::default()
        };
        let (conn, password) = validate(&v, None, &[]).unwrap();
        assert_eq!(conn.password_storage, PasswordStorage::Plain);
        assert_eq!(conn.password, "s3cret");
        assert_eq!(conn.port, Some(5432));
        assert_eq!(password, "s3cret");

        let reopened = FormValues::from_connection(&conn, &conn.password);
        let (again, password) =
            validate(&reopened, Some("dev"), std::slice::from_ref(&conn)).unwrap();
        assert_eq!(again.password_storage, PasswordStorage::Plain);
        assert_eq!(again.password, "s3cret");
        assert_eq!(password, "s3cret");
    }

    #[test]
    fn edit_prefill_copies_the_connection() {
        let mut c = conn("pg", DbKind::MySql);
        c.port = None;
        c.user = "root".into();
        let v = FormValues::from_connection(&c, "pw");
        assert_eq!(v.kind, DbKind::MySql);
        assert_eq!(v.port, "");
        assert_eq!(v.user, "root");
        assert_eq!(v.password, "pw");
    }

    #[test]
    fn kind_change_moves_only_default_ports() {
        use DbKind::*;
        assert_eq!(
            port_after_kind_change("5432", Postgres, MySql).as_deref(),
            Some("3306")
        );
        assert_eq!(port_after_kind_change("6543", Postgres, MySql), None);
        assert_eq!(
            port_after_kind_change("3306", MySql, Sqlite).as_deref(),
            Some("")
        );
        assert_eq!(
            port_after_kind_change("", Sqlite, Postgres).as_deref(),
            Some("5432")
        );
        assert_eq!(
            port_after_kind_change("", Postgres, MySql).as_deref(),
            Some("3306")
        );
    }

    #[test]
    fn paste_drops_control_characters() {
        assert_eq!(sanitize_paste("pasted\tname\r\n", false), "pastedname");
        assert_eq!(sanitize_paste("54a3\n2", true), "5432");
        assert_eq!(sanitize_paste("\n", false), "");
        assert!(is_port_text("") && is_port_text("5432"));
        assert!(!is_port_text("54x") && !is_port_text("-1"));
    }
}
