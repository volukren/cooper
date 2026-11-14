use rune::{Cx, El, PointerPress, ScrollSpec, TextStyle, Tx, col, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Dialog as KitDialog, MenuItem, context_menu};

use super::{
    Tab, Workspace,
    model::{self, Focus},
};
use crate::{
    connection, db,
    dialog::Dialog,
    icons,
    keys::{self, Command, Ctx},
    theme::{RADIUS, Theme},
    ui,
};

const SIDEBAR_WIDTH: f32 = 290.0;
const ROW_HEIGHT: f32 = 30.0;

pub fn run(s: &mut Workspace, cmd: Command, tx: &mut Tx) -> bool {
    match cmd {
        Command::Connect => s.connect(tx),
        Command::SelectNext => {
            s.home.select_next();
            s.reveal_selection(tx);
            true
        }
        Command::SelectPrev => {
            s.home.select_prev();
            s.reveal_selection(tx);
            true
        }
        Command::EditConnection => {
            edit_connection(s, tx);
            true
        }
        Command::DeleteConnection => {
            ask_to_delete(s);
            true
        }
        Command::DeleteForward => {
            ask_to_delete(s);
            true
        }
        _ => false,
    }
}

fn edit_connection(s: &mut Workspace, tx: &mut Tx) {
    s.home.clear_messages();
    let open = s.open_names();
    let open: Vec<&str> = open.iter().map(String::as_str).collect();
    let Some(conn) = s.home.target_for("editing", &open) else {
        return;
    };
    match connection::password(&conn) {
        Ok(password) => s.open_form(Some((conn, password.unwrap_or_default())), tx),
        Err(err) => {
            s.home.status = Some(format!(
                "Cannot edit: reading the password from the keychain: {err}"
            ))
        }
    }
}

fn ask_to_delete(s: &mut Workspace) {
    s.home.clear_messages();
    let open = s.open_names();
    let open: Vec<&str> = open.iter().map(String::as_str).collect();
    if let Some(conn) = s.home.target_for("deleting", &open) {
        s.dialog = Some(Dialog::DeleteConnection(conn.name));
    }
}

pub fn view(s: &Workspace, t: &Theme, _cx: &mut Cx<Workspace>, layer: &mut Vec<El>) -> El {
    let body = match &s.load_error {
        Some(message) => load_error(message, t),
        None => row()
            .grow(1.0)
            .min_h(0.0)
            .w_full()
            .child(sidebar(s, t, layer))
            .child(details(s, t)),
    };
    let status = match (&s.load_error, s.home.status_line()) {
        (None, Some((message, kind))) => Some(ui::status_line(
            "home-status",
            &message,
            kind == super::model::StatusKind::Error,
            t,
        )),
        _ => None,
    };
    col()
        .key("home")
        .focusable(true)
        .no_focus_ring()
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .on_key(keys::route)
        .child(body)
        .children(status)
}

fn sidebar(s: &Workspace, t: &Theme, layer: &mut Vec<El>) -> El {
    let count = s.home.connections.len();
    let new_focused = s.home.focus == Focus::NewButton;
    let list: El = if count == 0 {
        list_placeholder("No connections yet", t)
    } else {
        col()
            .key("connection-list")
            .grow(1.0)
            .min_h(0.0)
            .w_full()
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(
                col()
                    .w_full()
                    .px(8.0)
                    .pb(8.0)
                    .gap(1.0)
                    .children((0..count).map(|index| connection_row(s, index, t))),
            )
    };
    let new_button = Button::new("New connection")
        .icon(&icons::PLUS)
        .control_size(ControlSize::Large)
        .variant(if new_focused {
            ButtonVariant::Submit
        } else {
            ButtonVariant::Outline
        })
        .help("New connection")
        .shortcut(keys::label(Command::NewConnection).unwrap_or_default())
        .on_press_tx(|s: &mut Workspace, tx: &mut Tx| {
            s.run(Command::NewConnection, tx);
        })
        .build(&t.k)
        .key("new-connection")
        .w_full();
    col()
        .key("home-sidebar")
        .w(SIDEBAR_WIDTH)
        .h_full()
        .fill(t.sidebar)
        .border_sides([0.0, 1.0, 0.0, 0.0], t.border)
        .child(
            row()
                .w_full()
                .px(12.0)
                .pt(12.0)
                .pb(8.0)
                .justify_between()
                .center_y()
                .child(ui::label("CONNECTIONS", t.semibold(t.text_xs), t.muted))
                .child(ui::label(count.to_string(), t.text_xs, t.muted)),
        )
        .child(
            col()
                .key("new-connection-frame")
                .mx(5.0)
                .mb(8.0)
                .padding(2.0)
                .radius(RADIUS + 2.0)
                .border(
                    1.0,
                    if new_focused {
                        t.ring
                    } else {
                        t.ring.alpha(0.0)
                    },
                )
                .child(new_button),
        )
        .child(
            connection_menu(s, list, t, layer)
                .grow(1.0)
                .min_h(0.0)
                .w_full(),
        )
}

fn connection_menu(s: &Workspace, list: El, t: &Theme, layer: &mut Vec<El>) -> El {
    let open = s
        .home
        .selected()
        .is_some_and(|conn| s.tab_index(&conn.name).is_some());
    let area = col()
        .key("connection-area")
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .child(list);
    context_menu("connection-menu", area, s.home.menu)
        .item(
            MenuItem::new(if open { "Switch to Tab" } else { "Connect" })
                .icon(&icons::PLUG)
                .shortcut(keys::label_in(Command::Connect, Ctx::Home).unwrap_or_default())
                .on_select_tx(|s: &mut Workspace, tx: &mut Tx| {
                    s.home.menu = None;
                    s.connect(tx);
                }),
        )
        .item(
            MenuItem::new("Edit…")
                .icon(&icons::SETTINGS)
                .shortcut(keys::label_in(Command::EditConnection, Ctx::Home).unwrap_or_default())
                .on_select_tx(|s: &mut Workspace, tx: &mut Tx| {
                    s.home.menu = None;
                    edit_connection(s, tx);
                }),
        )
        .separator()
        .item(
            MenuItem::new("Delete…")
                .icon(&icons::DELETE)
                .danger(true)
                .shortcut(keys::label_in(Command::DeleteConnection, Ctx::Home).unwrap_or_default())
                .on_select(|s: &mut Workspace| {
                    s.home.menu = None;
                    ask_to_delete(s);
                }),
        )
        .on_close(|s: &mut Workspace| s.home.menu = None)
        .build(&t.k, layer)
}

fn list_placeholder(message: &str, t: &Theme) -> El {
    col()
        .px(16.0)
        .py(12.0)
        .child(ui::label(message.to_string(), t.text, t.muted))
}

fn connection_row(s: &Workspace, index: usize, t: &Theme) -> El {
    let conn = &s.home.connections[index];
    let selected = s.home.selected_position() == Some(index);
    let active = selected && s.home.focus == Focus::Connections;
    let failed = s.home.failure_of(&conn.name).is_some();
    let open = s.tab_index(&conn.name).is_some();
    let clear = t.background.alpha(0.0);
    let name = db::render_text(&conn.name);
    let mut row = row()
        .key(format!("connection-{index}"))
        .role("option")
        .label(name.clone())
        .h(ROW_HEIGHT)
        .w_full()
        .px(8.0)
        .gap(8.0)
        .center_y()
        .radius(RADIUS)
        .border(1.0, if active { t.list_active_border } else { clear })
        .fill(if active {
            t.list_active
        } else if selected {
            t.list_hover
        } else {
            clear
        });
    if !selected {
        row = row.hover(t.list_hover);
    }
    row.child(if failed {
        ui::icon_el(icons::db_kind_mono(conn.kind), 14.0, t.danger)
    } else {
        ui::icon_el(icons::db_kind(conn.kind), 14.0, t.muted)
    })
    .child(
        text(name)
            .font(if selected { t.medium(t.text) } else { t.text })
            .color(t.foreground)
            .truncate()
            .grow(1.0)
            .min_w(0.0),
    )
    .children(
        conn.tag
            .map(|tag| ui::tag_badge(tag, t).key(format!("connection-{index}-tag"))),
    )
    .children(open.then(|| {
        rune::el()
            .key("open-dot")
            .size(6.0, 6.0)
            .radius(3.0)
            .fill(t.success)
    }))
    .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
        s.home.click_row(index);
        tx.focus("home", false);
    })
    .on_double_press_tx(|s: &mut Workspace, tx: &mut Tx| {
        s.connect(tx);
    })
    .on_secondary_press(
        move |s: &mut Workspace, press: PointerPress, _tx: &mut Tx| {
            s.home.click_row(index);
            s.home.menu = Some(press);
        },
    )
}

fn details(s: &Workspace, t: &Theme) -> El {
    let Some(conn) = s.home.selected() else {
        return col()
            .key("details")
            .grow(1.0)
            .min_w(0.0)
            .h_full()
            .center()
            .gap(8.0)
            .child(ui::icon_el(&icons::DATABASE, 32.0, t.muted))
            .child(ui::label(
                model::APP_TITLE,
                t.bold(TextStyle::new(17.0, 700.0).lh(24.0)),
                t.foreground,
            ))
            .child(ui::label(
                "No connection selected",
                t.semibold(t.text),
                t.foreground,
            ))
            .child(ui::label(
                "Press ⌘N to create a new connection",
                t.text_sm,
                t.muted,
            ));
    };
    let mut fields: Vec<(&str, String)> = vec![("Type", conn.kind.label().to_string())];
    if conn.kind.is_file_based() {
        fields.push(("File", conn.database.clone()));
    } else {
        fields.push(("Host", conn.host.clone()));
        fields.push((
            "Port",
            conn.port.map(|port| port.to_string()).unwrap_or_default(),
        ));
        fields.push(("User", conn.user.clone()));
        fields.push(("Database", conn.database.clone()));
    }
    fields.push(("URL", conn.summary()));
    let header = row()
        .key("details-header")
        .w_full()
        .gap(16.0)
        .pb(16.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
        .child(
            row()
                .gap(12.0)
                .min_w(0.0)
                .shrink()
                .center_y()
                .child(ui::icon_el(icons::db_kind(conn.kind), 24.0, t.foreground))
                .child(
                    col()
                        .min_w(0.0)
                        .shrink()
                        .child(
                            row()
                                .gap(8.0)
                                .center_y()
                                .shrink()
                                .child(
                                    text(db::render_text(&conn.name))
                                        .key("details-name")
                                        .font(t.bold(TextStyle::new(18.0, 700.0).lh(24.0)))
                                        .color(t.foreground)
                                        .truncate()
                                        .shrink(),
                                )
                                .children(
                                    conn.tag.map(|tag| ui::tag_badge(tag, t).key("details-tag")),
                                ),
                        )
                        .child(ui::label(conn.kind.label(), t.text_sm, t.muted)),
                ),
        );
    let rows = col()
        .key("details-fields")
        .w_full()
        .gap(8.0)
        .children(fields.into_iter().map(|(label, value)| {
            row()
                .w_full()
                .gap(16.0)
                .items_start()
                .child(ui::label(label, t.text_sm, t.muted).w(96.0))
                .child(
                    text(db::render_text(&value))
                        .font(t.mono)
                        .color(t.foreground)
                        .selectable(true)
                        .wrap_anywhere()
                        .grow(1.0)
                        .min_w(0.0),
                )
        }));
    let failure = s.home.failure_of(&conn.name).map(|message| {
        col()
            .key("details-failure")
            .w_full()
            .gap(4.0)
            .padding(12.0)
            .radius(RADIUS)
            .border(1.0, t.danger)
            .child(
                row()
                    .gap(8.0)
                    .center_y()
                    .child(ui::icon_el(&icons::CIRCLE_X, 14.0, t.danger))
                    .child(ui::label("Connection failed", t.bold(t.text), t.danger)),
            )
            .child(
                text(message.to_string())
                    .font(t.text_sm)
                    .color(t.danger)
                    .selectable(true)
                    .wrap_anywhere()
                    .w_full()
                    .min_w(0.0),
            )
    });
    col()
        .key("details")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(
            col()
                .w_full()
                .padding(24.0)
                .gap(16.0)
                .child(header)
                .child(rows)
                .children(failure),
        )
}

fn load_error(message: &str, t: &Theme) -> El {
    col().grow(1.0).w_full().padding(24.0).center().child(
        col()
            .key("load-error")
            .w_full()
            .max_w(640.0)
            .gap(12.0)
            .padding(24.0)
            .radius(RADIUS)
            .border(1.0, t.border)
            .child(
                row()
                    .gap(8.0)
                    .center_y()
                    .child(ui::icon_el(&icons::TRIANGLE_ALERT, 16.0, t.danger))
                    .child(ui::label(
                        "Cannot load connections",
                        t.bold(t.text),
                        t.danger,
                    )),
            )
            .child(
                text(message.to_string())
                    .font(t.mono)
                    .color(t.muted)
                    .selectable(true)
                    .wrap_anywhere()
                    .w_full()
                    .min_w(0.0),
            )
            .child(
                row().w_full().justify_end().child(
                    Button::new("Retry")
                        .icon(&icons::REFRESH_CW)
                        .control_size(ControlSize::Small)
                        .variant(ButtonVariant::Outline)
                        .on_press_tx(|s: &mut Workspace, tx: &mut Tx| s.retry_load(tx))
                        .build(&t.k)
                        .key("retry-load"),
                ),
            ),
    )
}

pub fn connecting(tab: &Tab, t: &Theme) -> El {
    let id = tab.id;
    col()
        .key("connecting")
        .focusable(true)
        .no_focus_ring()
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .child(
            col().grow(1.0).w_full().center().child(
                col()
                    .key("connecting-card")
                    .w(380.0)
                    .gap(12.0)
                    .padding(24.0)
                    .items_center()
                    .radius(12.0)
                    .border(1.0, t.warning)
                    .fill(t.background)
                    .child(ui::spinner(28.0, t.warning, t))
                    .child(ui::label("Connecting…", t.bold(t.text), t.foreground))
                    .child(
                        text(db::render_text(tab.name()))
                            .font(t.text)
                            .color(t.muted)
                            .truncate()
                            .max_w(332.0),
                    )
                    .child(
                        Button::new("Cancel")
                            .control_size(ControlSize::Small)
                            .variant(ButtonVariant::Outline)
                            .help("Esc cancel")
                            .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                                if let Some(index) = s.tab_position(id) {
                                    s.disconnect(index, tx);
                                }
                            })
                            .build(&t.k)
                            .key("cancel-connect"),
                    ),
            ),
        )
        .child(
            row()
                .key("connecting-hints")
                .h(26.0)
                .w_full()
                .px(12.0)
                .center_y()
                .border_sides([1.0, 0.0, 0.0, 0.0], t.border)
                .child(ui::label(
                    "Esc cancel · ⌘1 connections · ⌘Q quit",
                    t.text_xs,
                    t.muted,
                )),
        )
}

pub fn delete_dialog(_s: &Workspace, name: &str, t: &Theme, layer: &mut Vec<El>) {
    let quoted = format!("\"{}\"", db::render_text(name));
    let question = format!("Delete {quoted}?");
    let bold = "Delete ".len().."Delete ".len() + quoted.len();
    let target = name.to_string();
    KitDialog::new("delete-connection", "Delete connection")
        .width(440.0)
        .fit()
        .open(true)
        .on_close(|s: &mut Workspace| {
            s.dialog = None;
            s.refocus = true;
        })
        .child(
            col()
                .px(16.0)
                .gap(8.0)
                .child(
                    rune::rich(question, vec![rune::Span::new(bold.start, bold.end).bold()])
                        .font(t.text)
                        .color(t.foreground),
                )
                .child(ui::label(
                    "Its saved password will be removed too.",
                    t.text_sm,
                    t.muted,
                )),
        )
        .child(
            row()
                .w_full()
                .padding(16.0)
                .gap(8.0)
                .justify_end()
                .child(
                    Button::new("Cancel")
                        .variant(ButtonVariant::Outline)
                        .on_press_tx(|s: &mut Workspace, tx: &mut Tx| s.close_dialog(tx))
                        .build(&t.k)
                        .key("cancel-delete")
                        .autofocus(),
                )
                .child(
                    Button::new("Delete")
                        .icon(&icons::DELETE)
                        .variant(ButtonVariant::Danger)
                        .on_press(move |s: &mut Workspace| s.delete_connection(&target))
                        .build(&t.k)
                        .key("confirm-delete"),
                ),
        )
        .build(&t.modal, layer);
}
