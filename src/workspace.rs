mod home;
pub mod model;

#[cfg(test)]
mod ui_tests;

use std::{cell::Cell, time::Duration};

use rune::{Cx, El, KeyEvent, PointerButton, TextEdit, Tx, Waker, col, row, text};

use crate::{
    browser::{self, Browser, Polled},
    connection::{self, Connection, Store},
    db::{Response, Session},
    dialog::{self, Dialog},
    form::{ConnectionForm, FormDialog},
    icons,
    keys::{Command, Ctx},
    theme::{Theme, theme},
    ui,
};

pub use self::home::delete_dialog;
use self::model::{Failure, Focus, Home, Original};

pub const TOAST_FOR: Duration = Duration::from_secs(5);
pub const TITLEBAR_HEIGHT: f32 = 38.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u64);

pub enum TabState {
    Connecting(Session),
    Browser(Box<Browser>),
}

pub struct Tab {
    pub id: TabId,
    pub connection: Connection,
    pub state: TabState,
}

impl Tab {
    pub fn name(&self) -> &str {
        &self.connection.name
    }

    pub fn is_connecting(&self) -> bool {
        matches!(self.state, TabState::Connecting(_))
    }

    pub fn browser(&self) -> Option<&Browser> {
        match &self.state {
            TabState::Browser(browser) => Some(browser),
            TabState::Connecting(_) => None,
        }
    }

    pub fn browser_mut(&mut self) -> Option<&mut Browser> {
        match &mut self.state {
            TabState::Browser(browser) => Some(browser),
            TabState::Connecting(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    pub error: bool,
    pub due: Option<f64>,
}

pub struct Workspace {
    pub home: Home,
    pub load_error: Option<String>,
    pub scratch: TextEdit,
    pub tabs: Vec<Tab>,
    pub active: Option<usize>,
    pub dialog: Option<Dialog>,
    pub toasts: Vec<Toast>,
    pub dark: Option<bool>,
    pub dark_system: bool,
    pub system_dark: Cell<bool>,
    pub window_title: String,
    pub waker: Option<Waker>,
    pub polls: usize,
    pub refocus: bool,
    reveal: bool,
    #[cfg(test)]
    pub hold_connecting: bool,
    next_tab: u64,
    next_toast: u64,
}

impl Workspace {
    pub fn new(store: Store) -> Self {
        let (connections, load_error) = match store.load() {
            Ok(connections) => (connections, None),
            Err(err) => (Vec::new(), Some(format!("{err:#}"))),
        };
        Self {
            home: Home::new(store, connections),
            load_error,
            scratch: TextEdit::new(""),
            tabs: Vec::new(),
            active: None,
            dialog: None,
            toasts: Vec::new(),
            dark: None,
            dark_system: true,
            system_dark: Cell::new(true),
            window_title: model::APP_TITLE.to_string(),
            waker: None,
            polls: 0,
            refocus: false,
            reveal: false,
            #[cfg(test)]
            hold_connecting: false,
            next_tab: 1,
            next_toast: 1,
        }
    }

    pub fn is_dark(&self) -> bool {
        let system = self.system_dark.get();
        match self.dark {
            Some(dark) if self.dark_system == system => dark,
            _ => system,
        }
    }

    pub fn theme(&self) -> &'static Theme {
        theme(self.is_dark())
    }

    pub fn tab_index(&self, name: &str) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.name() == name)
    }

    pub fn tab_position(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id == id)
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active?)
    }

    pub fn active_id(&self) -> Option<TabId> {
        self.active_tab().map(|tab| tab.id)
    }

    pub fn browser(&self, id: TabId) -> Option<&Browser> {
        self.tab(id)?.browser()
    }

    pub fn browser_mut(&mut self, id: TabId) -> Option<&mut Browser> {
        self.tabs.iter_mut().find(|tab| tab.id == id)?.browser_mut()
    }

    pub fn active_browser(&self) -> Option<&Browser> {
        self.active_tab()?.browser()
    }

    pub fn active_browser_mut(&mut self) -> Option<&mut Browser> {
        let index = self.active?;
        self.tabs.get_mut(index)?.browser_mut()
    }

    pub fn browser_and_dialog(&mut self, id: TabId) -> Option<(&mut Browser, &mut Option<Dialog>)> {
        let browser = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == id)?
            .browser_mut()?;
        Some((browser, &mut self.dialog))
    }

    pub fn open_names(&self) -> Vec<String> {
        self.tabs.iter().map(|tab| tab.name().to_string()).collect()
    }

    fn new_tab_id(&mut self) -> TabId {
        let id = TabId(self.next_tab);
        self.next_tab += 1;
        id
    }

    pub fn wire_session(&self, session: &Session) {
        if let Some(waker) = self.waker.clone() {
            session.set_waker(move || waker());
        }
    }

    pub fn switch_to(&mut self, target: Option<usize>, tx: &mut Tx) {
        if target.is_some_and(|index| index >= self.tabs.len()) {
            return;
        }
        self.active = target;
        if let Some(browser) = self.active_browser() {
            browser.activate(tx);
        }
        let key = match target.map(|index| self.tabs[index].id) {
            Some(id) => format!("tab-{}", id.0),
            None => "tab-home".to_string(),
        };
        tx.scroll_into_view(key);
        self.focus_active(tx);
        self.sync_title(tx);
    }

    pub fn focus_active(&mut self, tx: &mut Tx) {
        if self.dialog.is_some() {
            self.refocus = true;
            return;
        }
        self.refocus = false;
        let Some(index) = self.active else {
            tx.focus("home", false);
            return;
        };
        match &mut self.tabs[index].state {
            TabState::Connecting(_) => tx.focus("connecting", false),
            TabState::Browser(browser) => browser.focus(tx),
        }
    }

    pub fn close_tab(&mut self, index: usize) -> Connection {
        let Tab {
            connection, state, ..
        } = self.tabs.remove(index);
        self.active = model::active_after_close(self.active, index);
        if let TabState::Browser(browser) = &state {
            self.home
                .remember_draft(&connection.name, browser.draft().unwrap_or_default());
        }
        if self
            .dialog
            .as_ref()
            .and_then(Dialog::tab)
            .is_some_and(|tab| self.tab_position(tab).is_none())
        {
            self.dialog = None;
        }
        drop(state);
        connection
    }

    pub fn drop_sessions_in_foreground(&self) {
        for tab in &self.tabs {
            match &tab.state {
                TabState::Connecting(session) => session.drop_in_foreground(),
                TabState::Browser(browser) => browser.drop_session_in_foreground(),
            }
        }
    }

    pub fn release_sessions(&mut self) {
        self.drop_sessions_in_foreground();
        self.dialog = None;
        self.active = None;
        drop(std::mem::take(&mut self.tabs));
    }

    pub fn disconnect(&mut self, index: usize, tx: &mut Tx) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let verb = if tab.is_connecting() {
            "Connection cancelled"
        } else {
            "Disconnected"
        };
        let was_active = self.active == Some(index);
        let connection = self.close_tab(index);
        self.notify(format!("{verb}: \"{}\"", connection.name), tx);
        if was_active {
            self.focus_active(tx);
        }
        self.sync_title(tx);
    }

    pub fn notify(&mut self, text: String, tx: &mut Tx) {
        if self.active_browser().is_some() {
            self.push_toast(text, false, tx);
        } else {
            self.home.error = None;
            self.home.status = Some(text);
        }
    }

    pub fn push_toast(&mut self, text: String, error: bool, tx: &mut Tx) {
        let due = (!error).then(|| tx.clock() + TOAST_FOR.as_secs_f64());
        if !error {
            tx.wake_after(TOAST_FOR);
        }
        let id = self.next_toast;
        self.next_toast += 1;
        self.toasts.push(Toast {
            id,
            text,
            error,
            due,
        });
    }

    pub fn close_toast(&mut self, id: u64) {
        self.toasts.retain(|toast| toast.id != id);
    }

    pub fn fail(&mut self, index: usize, message: String, tx: &mut Tx) {
        let was_active = self.active == Some(index);
        let connection = self.close_tab(index);
        let name = connection.name;
        if self.active_browser().is_some() {
            self.push_toast(format!("Connection \"{name}\" failed: {message}"), true, tx);
        }
        self.home.status = None;
        self.home.error = Some(Failure { name, message });
        if was_active {
            self.focus_active(tx);
        }
        self.sync_title(tx);
    }

    pub fn connect(&mut self, tx: &mut Tx) -> bool {
        self.home.clear_messages();
        if self.home.focus == Focus::NewButton {
            self.open_form(None, tx);
            return true;
        }
        let Some(conn) = self.home.selected().cloned() else {
            return true;
        };
        if let Some(index) = self.tab_index(&conn.name) {
            self.switch_to(Some(index), tx);
            return true;
        }
        let password = match connection::password(&conn) {
            Ok(password) => password,
            Err(err) => {
                self.home.error = Some(Failure {
                    name: conn.name,
                    message: format!("reading the password from the keychain: {err}"),
                });
                return true;
            }
        };
        let session = Session::spawn(conn.clone(), password);
        session.drop_in_background();
        self.wire_session(&session);
        let id = self.new_tab_id();
        self.tabs.push(Tab {
            id,
            connection: conn,
            state: TabState::Connecting(session),
        });
        self.switch_to(Some(self.tabs.len() - 1), tx);
        true
    }

    fn open_browser(&mut self, index: usize, server_info: String, tx: &mut Tx) {
        let Tab {
            id,
            connection,
            state,
        } = self.tabs.remove(index);
        let session = match state {
            TabState::Connecting(session) => session,
            state => {
                self.tabs.insert(
                    index,
                    Tab {
                        id,
                        connection,
                        state,
                    },
                );
                return;
            }
        };
        let draft =
            browser::smoke_sql().or_else(|| self.home.drafts.get(&connection.name).cloned());
        let mut browser = Browser::new(id, connection.clone(), session, draft);
        browser.set_server_info(server_info);
        if let Some(waker) = self.waker.clone() {
            browser.set_waker(waker);
        }
        self.tabs.insert(
            index,
            Tab {
                id,
                connection,
                state: TabState::Browser(Box::new(browser)),
            },
        );
        browser::smoke(self, id, tx);
        if self.active == Some(index) {
            self.focus_active(tx);
        }
        self.sync_title(tx);
    }

    pub fn sync_title(&mut self, tx: &mut Tx) {
        let tab = self.active_tab();
        let what = tab.and_then(|tab| match &tab.state {
            TabState::Connecting(_) => Some("Connecting…".to_string()),
            TabState::Browser(browser) => browser.title_context(),
        });
        let title = model::window_title(tab.map(Tab::name), what.as_deref());
        if title != self.window_title {
            tx.set_title(title.clone());
            self.window_title = title;
        }
    }

    pub fn poll(&mut self, tx: &mut Tx) -> bool {
        self.polls += 1;
        if cfg!(debug_assertions) && self.polls == 1 {
            self.smoke_connect(tx);
        }
        let mut changed = false;
        let mut index = 0;
        while index < self.tabs.len() {
            let id = self.tabs[index].id;
            match &self.tabs[index].state {
                TabState::Connecting(_) if self.holds_connecting() => {}
                TabState::Connecting(session) => match session.try_recv() {
                    Some(Response::Connected { server_info }) => {
                        self.open_browser(index, server_info, tx);
                        changed = true;
                    }
                    Some(Response::Error { message, .. }) => {
                        self.fail(index, message, tx);
                        changed = true;
                        continue;
                    }
                    _ => {}
                },
                TabState::Browser(_) => match browser::poll(self, id, tx) {
                    Polled::Idle => {}
                    Polled::Changed => changed = true,
                    Polled::Lost(message) => {
                        if let Some(index) = self.tab_position(id) {
                            self.fail(index, message, tx);
                        }
                        changed = true;
                        continue;
                    }
                },
            }
            index += 1;
        }
        changed |= self.expire_toasts(tx);
        if self.dark.is_some() && self.dark_system != self.system_dark.get() {
            self.dark = None;
        }
        if self.dialog.is_none() {
            let on_list = self.active.is_none() && tx.focused() == Some("home");
            if self.refocus && on_list {
                self.refocus = false;
            } else if self.refocus || tx.focused().is_none() {
                self.focus_active(tx);
                changed = true;
            }
        }
        if std::mem::take(&mut self.reveal) {
            self.reveal_selection(tx);
        }
        self.sync_title(tx);
        changed
    }

    fn smoke_connect(&mut self, tx: &mut Tx) {
        if !cfg!(debug_assertions) {
            return;
        }
        let Ok(name) = std::env::var("COOPER_SMOKE_CONNECT") else {
            return;
        };
        if let Some(position) = self
            .home
            .connections
            .iter()
            .position(|connection| connection.name == name)
        {
            self.home.select(position);
            self.connect(tx);
        }
    }

    #[cfg(test)]
    fn holds_connecting(&self) -> bool {
        self.hold_connecting
    }

    #[cfg(not(test))]
    fn holds_connecting(&self) -> bool {
        false
    }

    fn expire_toasts(&mut self, tx: &mut Tx) -> bool {
        if self.toasts.is_empty() {
            return false;
        }
        let now = tx.clock();
        let before = self.toasts.len();
        self.toasts
            .retain(|toast| toast.due.is_none_or(|due| due > now));
        if let Some(next) = self
            .toasts
            .iter()
            .filter_map(|toast| toast.due)
            .reduce(f64::min)
        {
            tx.wake_at(next);
        }
        self.toasts.len() != before
    }

    pub fn contexts(&self, focused: Option<&str>, out: &mut Vec<Ctx>) {
        if let Some(dialog) = &self.dialog {
            dialog.contexts(focused, out);
            return;
        }
        match self.active_tab() {
            None => {
                out.push(Ctx::Home);
            }
            Some(tab) => match &tab.state {
                TabState::Connecting(_) => out.push(Ctx::Connecting),
                TabState::Browser(browser) => browser.contexts(focused, out),
            },
        }
        out.push(Ctx::Workspace);
    }

    pub fn before_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.dialog.is_some() {
            return false;
        }
        let Some(id) = self.active_id() else {
            return false;
        };
        self.browser(id).is_some() && browser::before_key(self, id, ev, tx)
    }

    pub fn run(&mut self, cmd: Command, tx: &mut Tx) -> bool {
        let done = self.dispatch(cmd, tx);
        if done {
            self.sync_title(tx);
        }
        done
    }

    pub fn run_from_menu(&mut self, cmd: Command, tx: &mut Tx) {
        let global = matches!(cmd, Command::Quit | Command::ShowHelp);
        if global || self.dialog.is_none() {
            self.run(cmd, tx);
        }
    }

    fn dispatch(&mut self, cmd: Command, tx: &mut Tx) -> bool {
        use Command::*;
        if let Some(done) = dialog::run(self, cmd, tx) {
            return done;
        }
        match cmd {
            Quit => {
                self.drop_sessions_in_foreground();
                tx.quit();
                true
            }
            ShowHelp => self.toggle_help(tx),
            CloseTab => match self.active {
                Some(index) => {
                    self.disconnect(index, tx);
                    true
                }
                None => false,
            },
            NextTab | PrevTab => {
                let target = model::cycle_tab(self.active, self.tabs.len(), cmd == NextTab);
                self.switch_to(target, tx);
                true
            }
            GoHome => {
                self.switch_to(None, tx);
                true
            }
            ActivateTab(position) => {
                if let Some(target) = model::tab_at_position(position, self.tabs.len()) {
                    self.switch_to(target, tx);
                }
                true
            }
            NewConnection => {
                if self.active.is_some() {
                    self.switch_to(None, tx);
                }
                self.home.clear_messages();
                self.open_form(None, tx);
                true
            }
            ToggleTheme => {
                self.dark = Some(!self.is_dark());
                self.dark_system = self.system_dark.get();
                tx.color_motion(rune::tok::motion::CALM);
                true
            }
            CancelConnect => match self.active {
                Some(index) if self.tabs[index].is_connecting() => {
                    self.disconnect(index, tx);
                    true
                }
                _ => false,
            },
            Connect | EditConnection | DeleteConnection | DeleteForward | SelectNext
            | SelectPrev => self.active.is_none() && home::run(self, cmd, tx),
            _ => match self.active_id() {
                Some(id) if self.browser(id).is_some() => browser::run(self, id, cmd, tx),
                _ => false,
            },
        }
    }

    fn toggle_help(&mut self, tx: &mut Tx) -> bool {
        match &self.dialog {
            Some(Dialog::Help(_)) => self.close_dialog(tx),
            None => {
                self.dialog = Some(Dialog::Help(Box::default()));
            }
            Some(_) => return false,
        }
        true
    }

    pub fn open_form(&mut self, edit: Option<(Connection, String)>, _tx: &mut Tx) {
        if self.load_error.is_some() {
            return;
        }
        let original = edit.as_ref().map(|(conn, password)| Original {
            name: conn.name.clone(),
            password: password.clone(),
        });
        let form_original = edit.map(|(mut conn, password)| {
            conn.password = password;
            conn
        });
        self.dialog = Some(Dialog::Form(Box::new(FormDialog {
            form: ConnectionForm::new(form_original),
            original,
        })));
    }

    pub fn submit_form(&mut self) {
        let Some(Dialog::Form(session)) = &self.dialog else {
            return;
        };
        let original = session.original.clone();
        let saved = session
            .form
            .build(&self.home.connections)
            .and_then(|(conn, password)| match &original {
                None => self.home.save_new(conn, &password),
                Some(original) => self.home.save_edit(original, conn, &password),
            });
        match saved {
            Ok((index, status)) => {
                self.dialog = None;
                self.home.saved(index, status);
                self.reveal = true;
                self.refocus = true;
            }
            Err(message) => {
                if let Some(Dialog::Form(session)) = &mut self.dialog {
                    session.form.set_error(message);
                }
            }
        }
    }

    pub fn close_dialog(&mut self, _tx: &mut Tx) {
        self.dialog = None;
        self.refocus = true;
    }

    pub fn reveal_selection(&self, tx: &mut Tx) {
        if self.active.is_none()
            && let Some(position) = self.home.selected_position()
        {
            tx.scroll_into_view_in("connection-list", format!("connection-{position}"));
        }
    }

    pub fn retry_load(&mut self, tx: &mut Tx) {
        match self.home.reload() {
            Ok(()) => {
                self.load_error = None;
                self.focus_active(tx);
            }
            Err(message) => self.load_error = Some(message),
        }
    }

    pub fn delete_connection(&mut self, name: &str) {
        self.home.delete_connection(name);
        self.reveal = true;
        self.dialog = None;
        self.refocus = true;
    }
}

pub fn browser_edit<'a>(
    s: &'a mut Workspace,
    tab: TabId,
    f: impl FnOnce(&'a mut Browser) -> &'a mut TextEdit,
) -> &'a mut TextEdit {
    browser_field(s, tab, |b| Some(f(b)))
}

pub fn browser_field<'a>(
    s: &'a mut Workspace,
    tab: TabId,
    f: impl FnOnce(&'a mut Browser) -> Option<&'a mut TextEdit>,
) -> &'a mut TextEdit {
    let Workspace { tabs, scratch, .. } = s;
    match tabs
        .iter_mut()
        .find(|t| t.id == tab)
        .and_then(Tab::browser_mut)
        .and_then(f)
    {
        Some(edit) => edit,
        None => scratch,
    }
}

pub fn dialog_edit<'a>(
    s: &'a mut Workspace,
    f: impl FnOnce(&'a mut Dialog) -> Option<&'a mut TextEdit>,
) -> &'a mut TextEdit {
    let Workspace {
        dialog, scratch, ..
    } = s;
    match dialog.as_mut().and_then(f) {
        Some(edit) => edit,
        None => scratch,
    }
}

pub fn default_workspace() -> Workspace {
    match Store::default_location() {
        Ok(store) => Workspace::new(store),
        Err(err) => {
            eprintln!("cooper: cannot locate config directory: {err:?}");
            std::process::exit(1);
        }
    }
}

pub fn setup(s: &mut Workspace, waker: Waker) {
    for tab in &mut s.tabs {
        match &mut tab.state {
            TabState::Connecting(session) => {
                let waker = waker.clone();
                session.set_waker(move || waker());
            }
            TabState::Browser(browser) => browser.set_waker(waker.clone()),
        }
    }
    s.waker = Some(waker);
}

pub fn poll(s: &mut Workspace, _clock: f64, tx: &mut Tx) -> bool {
    s.poll(tx)
}

pub fn on_close(s: &mut Workspace, _tx: &mut Tx) {
    s.release_sessions();
}

pub fn on_quit(s: &mut Workspace, _tx: &mut Tx) {
    s.release_sessions();
}

pub fn view(s: &Workspace, cx: &mut Cx<Workspace>) -> El {
    s.system_dark.set(cx.system_dark());
    let t = s.theme();
    let mut layer = Vec::new();
    let body = match s.active_tab() {
        None => home::view(s, t, cx, &mut layer),
        Some(tab) => match &tab.state {
            TabState::Connecting(_) => home::connecting(tab, t),
            TabState::Browser(browser) => browser::view(s, browser.id(), t, cx, &mut layer),
        },
    };
    dialog::view(s, t, cx, &mut layer);
    if !s.toasts.is_empty() {
        layer.push(toasts(s, t));
    }
    let root = col()
        .key("workspace")
        .w_full()
        .h_full()
        .fill(t.background)
        .on_key(crate::keys::route)
        .child(title_bar(s, t, cx))
        .child(col().key("body").grow(1.0).min_h(0.0).w_full().child(body));
    rune_kit::root(root, layer)
}

fn title_bar(s: &Workspace, t: &Theme, cx: &mut Cx<Workspace>) -> El {
    let insets = cx.chrome_insets();
    let bar = row()
        .key("titlebar")
        .w_full()
        .h(TITLEBAR_HEIGHT.max(insets.height))
        .pr(insets.trailing + 12.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
        .window_drag();
    if s.tabs.is_empty() {
        return bar.pl(insets.leading + 12.0).fill(t.background).child(
            text(s.window_title.clone())
                .key("window-title")
                .font(t.text_sm)
                .color(t.muted)
                .truncate()
                .grow(1.0)
                .min_w(0.0)
                .align(rune::Align::Center),
        );
    }
    bar.pl(insets.leading + 8.0)
        .fill(t.sidebar)
        .child(tab_strip(s, t))
}

fn tab_strip(s: &Workspace, t: &Theme) -> El {
    let home_hint = crate::keys::label(Command::GoHome).unwrap_or_default();
    let home_tab = tab_button(
        "tab-home",
        "Connections",
        s.active.is_none(),
        ui::icon_el(&icons::SERVER, 14.0, t.muted),
        t,
    )
    .help(format!("Connections  {home_hint}"))
    .child(ui::label(home_hint, t.text_xs, t.muted))
    .on_press_tx(|s: &mut Workspace, tx: &mut Tx| {
        s.switch_to(None, tx);
    });
    let tabs = s.tabs.iter().enumerate().map(|(index, tab)| {
        let id = tab.id;
        let lead = if tab.is_connecting() {
            ui::spinner(12.0, t.foreground, t)
        } else {
            let color = tab.connection.tag.map_or(t.muted, |tag| t.tag_color(tag));
            ui::icon_el(&icons::DATABASE, 14.0, color)
        };
        let close = rune_kit::IconButton::new(&icons::CLOSE)
            .variant(rune_kit::IconButtonVariant::Ghost)
            .control_size(rune_kit::ControlSize::Mini)
            .label(format!("Disconnect {}", tab.name()))
            .help("Disconnect")
            .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
                if let Some(index) = s.tab_position(id) {
                    s.disconnect(index, tx);
                }
            })
            .build(&t.k)
            .key(format!("close-tab-{}", id.0));
        tab_button(
            format!("tab-{}", id.0),
            &model::tab_title(tab.name()),
            s.active == Some(index),
            lead,
            t,
        )
        .help(crate::db::render_text(tab.name()))
        .children(
            tab.connection
                .tag
                .map(|tag| ui::tag_badge(tag, t).key(format!("tab-{}-tag", id.0))),
        )
        .children(model::tab_shortcut(index).map(|hint| ui::label(hint, t.text_xs, t.muted)))
        .child(close)
        .pointer_buttons([PointerButton::Primary, PointerButton::Middle])
        .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| {
            let Some(index) = s.tab_position(id) else {
                return;
            };
            if tx.button() == PointerButton::Middle {
                s.disconnect(index, tx);
            } else {
                s.switch_to(Some(index), tx);
            }
        })
    });
    row()
        .key("tab-strip")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .center_y()
        .child(
            row()
                .key("tab-scroll")
                .grow(1.0)
                .min_w(0.0)
                .h_full()
                .scroll_x()
                .window_drag()
                .child(
                    row()
                        .key("tab-list")
                        .role("tablist")
                        .label("Connection tabs")
                        .h_full()
                        .child(home_tab)
                        .children(tabs),
                ),
        )
}

fn tab_button(key: impl Into<String>, title: &str, active: bool, lead: El, t: &Theme) -> El {
    let mut tab = row()
        .key(key)
        .role("tab")
        .label(title.to_string())
        .focus_selected(active)
        .h_full()
        .pl(10.0)
        .pr(6.0)
        .gap(6.0)
        .center_y()
        .border_sides(
            [0.0, 1.0, 0.0, 1.0],
            if active {
                t.border
            } else {
                t.border.alpha(0.0)
            },
        )
        .fill(if active { t.background } else { t.sidebar });
    if !active {
        tab = tab.hover(t.list_hover);
    }
    tab.child(lead).child(
        text(title.to_string())
            .font(if active {
                t.medium(t.text_sm)
            } else {
                t.text_sm
            })
            .color(if active { t.foreground } else { t.muted }),
    )
}

fn toasts(s: &Workspace, t: &Theme) -> El {
    col()
        .key("toast-region")
        .role("region")
        .label("Notifications")
        .pin(None, Some(TITLEBAR_HEIGHT + 16.0), Some(16.0), None)
        .gap(8.0)
        .children(s.toasts.iter().map(|toast| toast_card(toast, t)))
}

fn toast_card(toast: &Toast, t: &Theme) -> El {
    let id = toast.id;
    let (glyph, tint) = if toast.error {
        (&icons::CIRCLE_X, t.danger)
    } else {
        (&icons::INFO, t.blue)
    };
    let close = rune_kit::IconButton::new(&icons::CLOSE)
        .variant(rune_kit::IconButtonVariant::Ghost)
        .control_size(rune_kit::ControlSize::Mini)
        .label("Close")
        .on_press(move |s: &mut Workspace| s.close_toast(id))
        .build(&t.k)
        .key("close");
    let card = row()
        .key(format!("toast-{id}"))
        .role("status")
        .label(toast.text.clone())
        .w(382.0)
        .padding(12.0)
        .gap(10.0)
        .items_start()
        .radius(8.0)
        .fill(t.background)
        .border(1.0, t.modal_border)
        .child(col().pt(2.0).child(ui::icon_el(glyph, 16.0, tint)))
        .child(
            text(toast.text.clone())
                .font(t.text_sm)
                .color(t.foreground)
                .wrap_anywhere()
                .grow(1.0)
                .min_w(0.0),
        )
        .child(close);
    t.k.elevate(card, &t.k.elevation.floating)
}

#[cfg(test)]
impl Workspace {
    pub fn test_tabs(&self) -> Vec<(String, bool)> {
        self.tabs
            .iter()
            .map(|tab| (tab.name().to_string(), tab.is_connecting()))
            .collect()
    }

    pub fn test_connection_names(&self) -> Vec<String> {
        self.home
            .connections
            .iter()
            .map(|c| c.name.clone())
            .collect()
    }

    pub fn test_selected_connection(&self) -> Option<String> {
        self.home.selected().map(|c| c.name.clone())
    }

    pub fn test_form_open(&self) -> bool {
        matches!(self.dialog, Some(Dialog::Form(_)))
    }

    pub fn test_home_messages(&self) -> (Option<String>, Option<String>) {
        let error = self
            .home
            .error
            .as_ref()
            .map(|failure| format!("{}: {}", failure.name, failure.message));
        (self.home.status.clone(), error)
    }
}
