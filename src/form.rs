use std::cell::Cell;

use rune::{
    Cx, El, FileDialogOptions, FileDialogResult, Key, KeyEvent, ScrollSpec, TextEdit, Tx, col, row,
    text,
};
use rune_kit::{
    Button, ButtonVariant, Checkbox, ControlSize, Dialog as KitDialog, IconButton,
    IconButtonVariant, Placement, RadioItem, Select, SelectAppearance, TextInput,
};

use crate::{
    connection::{self, Connection, DbKind, PasswordStorage, Tag},
    dialog::Dialog,
    icons, keys,
    theme::Theme,
    ui,
    workspace::{self, Workspace, model::Original},
};

mod url;

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
const READ_ONLY_LABEL: &str = "Read-only";
const READ_ONLY_HINT: &str =
    "Scripts that change data or schema will not be run, and rows can't be edited.";

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
    pub read_only: bool,
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
            read_only: false,
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
            read_only: conn.read_only,
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
            read_only: values.read_only,
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
        read_only: values.read_only,
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
const FORM_CHROME: f32 = 150.0;
const MAX_LABEL_WIDTH: f32 = 168.0;
const FIELD_GAP: f32 = 12.0;
const ROW_WIDTH: f32 = 468.0;
const LABELS: [&str; 12] = [
    "URL",
    "Name",
    "Type",
    "Host",
    "Port",
    "User",
    "Password",
    "Store password",
    "Database",
    "File path",
    "Environment",
    "Access",
];

fn label_width(t: &Theme) -> f32 {
    LABELS
        .iter()
        .map(|label| rune::text::measure(label, &[], t.text_sm, None).0.ceil())
        .fold(LABEL_WIDTH, f32::max)
        .min(MAX_LABEL_WIDTH)
}
const FIELDS: [&str; 7] = [
    "url", "name", "host", "port", "user", "password", "database",
];

pub struct FormDialog {
    pub form: ConnectionForm,
    pub original: Option<Original>,
}

pub struct ConnectionForm {
    pub original: Option<Connection>,
    pub kind: DbKind,
    pub storage: PasswordStorage,
    pub tag: Option<Tag>,
    pub read_only: bool,
    pub url: TextEdit,
    pub name: TextEdit,
    pub host: TextEdit,
    pub port: TextEdit,
    pub user: TextEdit,
    pub password: TextEdit,
    pub database: TextEdit,
    pub show_password: bool,
    pub tag_open: bool,
    pub error: Option<String>,
    pub last_field: Cell<Option<&'static str>>,
    pub scroll: ui::ScrollY,
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
            read_only: values.read_only,
            url: TextEdit::new(""),
            name,
            host: TextEdit::new(values.host.clone()),
            port: TextEdit::new(values.port.clone()),
            user: TextEdit::new(values.user.clone()),
            password,
            database: TextEdit::new(values.database.clone()),
            show_password: false,
            tag_open: false,
            error,
            last_field: Cell::new(Some("name")),
            scroll: ui::ScrollY::default(),
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
            read_only: self.read_only,
        }
    }

    pub fn build(&self, existing: &[Connection]) -> Result<(Connection, String), String> {
        let url = self.url.text.trim();
        if !url.is_empty()
            && let Err(reason) = url::parse(url)
        {
            return Err(format!("Invalid connection URL: {reason}"));
        }
        let original = self.original.as_ref().map(|c| c.name.as_str());
        validate(&self.values(), original, existing)
    }

    pub fn shows_url(&self) -> bool {
        self.original.is_none()
    }

    pub fn apply_url(&mut self) {
        let Ok(parsed) = url::parse(&self.url.text) else {
            return;
        };
        self.set_kind(parsed.kind);
        if !parsed.kind.is_file_based() {
            let port = parsed
                .port
                .map_or_else(|| port_text(parsed.kind), |p| p.to_string());
            replace_keeping_focus(&mut self.host, parsed.host);
            replace_keeping_focus(&mut self.port, port);
            replace_keeping_focus(&mut self.user, parsed.user);
            replace_keeping_focus(&mut self.password, parsed.password);
        }
        replace_keeping_focus(&mut self.database, parsed.database);
        self.error = None;
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

    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
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
            "url" => &self.url,
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
            "url" => &mut self.url,
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
        replace_keeping_focus(self.field_mut(field), text.to_string());
    }
}

fn replace_keeping_focus(edit: &mut TextEdit, text: String) {
    if edit.text == text {
        return;
    }
    let focused = edit.focused;
    edit.replace_text(text);
    edit.set_caret(edit.text.len());
    edit.focused = focused;
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

fn url_change(s: &mut Workspace, _tx: &mut Tx) {
    with_form(s, |f| {
        f.error = None;
        f.apply_url();
    });
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
        .on_change(match key {
            "form-port" => port_change,
            "form-url" => url_change,
            _ => clear_error,
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
                .wrap()
                .w(label_width(t)),
        )
        .child(col().grow(1.0).min_w(0.0).child(control))
}

fn hint_row(key: &'static str, hint: &'static str, t: &Theme) -> El {
    row()
        .w_full()
        .child(col().w(label_width(t) + FIELD_GAP))
        .child(
            text(hint)
                .key(key)
                .font(t.text_xs)
                .color(t.muted)
                .wrap()
                .grow(1.0)
                .min_w(0.0),
        )
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
    cx: &mut Cx<Workspace>,
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

    let kind = row()
        .role("group")
        .label("Type")
        .focus_group()
        .w_full()
        .flex_wrap()
        .gaps(6.0, 16.0)
        .center_y()
        .children(KINDS.into_iter().map(|kind| {
            Checkbox::new(form.kind == kind)
                .label(kind.label())
                .on_toggle(move |s: &mut Workspace| {
                    with_form(s, |f| {
                        f.tag_open = false;
                        f.set_kind(kind);
                    })
                })
                .build(&t.k)
                .key(format!("form-kind-{}", kind.label().to_lowercase()))
        }));
    let kind = select_row("form-kind", kind);

    let tag = Select::new("form-tag", &tag_labels())
        .appearance(SelectAppearance::Field)
        .width(ROW_WIDTH - FIELD_GAP - label_width(t))
        .label("Environment")
        .current(Some(tag_label(form.tag)))
        .open(form.tag_open)
        .placement(Placement::BottomStart)
        .on_toggle(|s: &mut Workspace| with_form(s, |f| f.tag_open = !f.tag_open))
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
        .gap(10.0);
    if form.shows_url() {
        let url = input(
            &form.url,
            "form-url",
            url::placeholder(form.kind),
            "URL",
            |f| &mut f.url,
        )
        .on_key(field_key)
        .build(&t.k)
        .key("form-url");
        body = body.child(field_row("URL", url, t));
    }
    body = body
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
            .w_full()
            .flex_wrap()
            .gaps(6.0, 16.0)
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
    let read_only = form.read_only;
    if form.tag == Some(Tag::Prod) && !read_only {
        body = body.child(hint_row("form-tag-hint", PROD_HINT, t));
    }
    let access = Checkbox::new(read_only)
        .label(READ_ONLY_LABEL)
        .on_toggle(move |s: &mut Workspace| with_form(s, |f| f.set_read_only(!read_only)))
        .build(&t.k)
        .key("form-read-only");
    body = body.child(field_row("Access", row().w_full().child(access), t));
    if read_only {
        body = body.child(hint_row("form-read-only-hint", READ_ONLY_HINT, t));
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
    let room = cx
        .layout_size("workspace")
        .map_or(f32::INFINITY, |[_, height]| {
            (height - FORM_CHROME).max(160.0)
        });
    let body = form.scroll.watch(
        col()
            .key("connection-form-scroll")
            .w_full()
            .max_h(room)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(body),
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
            read_only: false,
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
            read_only: false,
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
    fn read_only_round_trips_through_validation_and_edit() {
        for kind in [DbKind::Postgres, DbKind::MySql, DbKind::Sqlite] {
            let v = FormValues {
                kind,
                name: "shop".into(),
                database: "/tmp/shop.db".into(),
                read_only: true,
                ..FormValues::default()
            };
            let (conn, _) = validate(&v, None, &[]).unwrap();
            assert!(conn.read_only, "{kind:?}");
            assert!(!conn.asks_to_confirm(), "read-only is not a tag");

            let reopened = FormValues::from_connection(&conn, "");
            assert!(reopened.read_only);
            let (again, _) =
                validate(&reopened, Some("shop"), std::slice::from_ref(&conn)).unwrap();
            assert!(again.read_only, "{kind:?}");

            let cleared = FormValues {
                read_only: false,
                ..reopened
            };
            let (writable, _) =
                validate(&cleared, Some("shop"), std::slice::from_ref(&conn)).unwrap();
            assert!(!writable.read_only, "{kind:?}");
        }
        assert!(
            !FormValues::default().read_only,
            "new connections can write"
        );
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
