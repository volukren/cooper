use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use rune::{ElementInfo, Env, Key, KeyEvent, Mods, PointerButton, Runtime, Tx, clipboard};
use tempfile::TempDir;

use crate::{
    browser::{Browser, console::Console, table::TableView, tree::SchemaTree},
    connection::{Connection, DbKind, PasswordStorage, Store, Tag},
    keys::Command,
    workspace::{self, TabId, Workspace},
};

pub const TIMEOUT: Duration = Duration::from_secs(20);
pub const DT: f64 = 1.0 / 60.0;
pub const CLOCK_STEP: f64 = 0.05;
const REAL_STEP: Duration = Duration::from_millis(5);
const SETTLE_FRAMES: usize = 30;
pub const WINDOW_SIZE: (f32, f32) = crate::WINDOW_SIZE;

pub type Rt = Runtime<Workspace>;

pub struct TestConfig {
    dir: TempDir,
}

impl TestConfig {
    pub fn empty() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("cooper-ui-test-")
            .tempdir()
            .expect("create temp config dir");
        Self { dir }
    }

    pub fn with_sample_dbs(names: &[&str]) -> Self {
        let config = Self::empty();
        for name in names {
            config.add_sample_db(name);
        }
        config
    }

    pub fn add_sample_db(&self, name: &str) -> PathBuf {
        self.add_sample_db_tagged(name, name, None)
    }

    pub fn add_sample_db_tagged(&self, name: &str, sample: &str, tag: Option<Tag>) -> PathBuf {
        let data = self.dir.path().join("data");
        std::fs::create_dir_all(&data).expect("create temp data dir");
        let path = data.join(format!("{name}.db"));
        build_sample_db(sample, &path);
        self.add_connection(Connection {
            tag,
            ..sqlite_connection(name, &path)
        });
        path
    }

    pub fn add_connection(&self, connection: Connection) {
        assert!(
            !connection.uses_keychain(),
            "test connections must not use the system keychain ({})",
            connection.name
        );
        self.store()
            .append(&connection)
            .expect("write test connections.toml");
    }

    pub fn store(&self) -> Store {
        Store::at(self.connections_path())
    }

    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    pub fn connections_path(&self) -> PathBuf {
        self.dir.path().join("cooper").join("connections.toml")
    }

    pub fn connections_toml(&self) -> String {
        std::fs::read_to_string(self.connections_path()).unwrap_or_default()
    }
}

pub fn sample_sql(name: &str) -> &'static str {
    match name {
        "shop" => include_str!("test_support/fixtures/shop.sql"),
        "library" => include_str!("test_support/fixtures/library.sql"),
        "analytics" => include_str!("test_support/fixtures/analytics.sql"),
        other => panic!("no SQL fixture for the sample database {other:?}"),
    }
}

pub fn build_sample_db(name: &str, path: &Path) {
    let db = rusqlite::Connection::open(path)
        .unwrap_or_else(|err| panic!("create {}: {err}", path.display()));
    db.execute_batch(sample_sql(name))
        .unwrap_or_else(|err| panic!("run the {name} fixture: {err}"));
}

pub fn sqlite_connection(name: &str, path: &Path) -> Connection {
    Connection {
        name: name.to_string(),
        kind: DbKind::Sqlite,
        tag: None,
        read_only: false,
        host: String::new(),
        port: None,
        user: String::new(),
        database: path.to_string_lossy().into_owned(),
        password_storage: PasswordStorage::Plain,
        password: String::new(),
    }
}

pub fn key_event(spec: &str) -> KeyEvent {
    let (body, minus) = match spec.strip_suffix("--") {
        Some(body) => (body, true),
        None => (spec, spec == "-"),
    };
    let mut parts: Vec<&str> = if minus && body == "-" {
        Vec::new()
    } else {
        body.split('-').collect()
    };
    if minus {
        parts.push("-");
    }
    let (name, mods_part) = parts
        .split_last()
        .unwrap_or_else(|| panic!("empty key spec {spec:?}"));
    let mut mods = Mods::default();
    for m in mods_part {
        match *m {
            "cmd" | "mod" => mods.cmd = true,
            "ctrl" => mods.ctrl = true,
            "alt" | "opt" => mods.alt = true,
            "shift" => mods.shift = true,
            other => panic!("unknown modifier {other} in {spec}"),
        }
    }
    let key = match *name {
        "enter" => Key::Enter,
        "escape" => Key::Escape,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "space" if mods.cmd => Key::Command(' '),
        "space" => Key::Text(" ".into()),
        f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok() => {
            Key::Function(f[1..].parse().unwrap_or(1))
        }
        c if c.chars().count() == 1 && mods.cmd => {
            Key::Command(c.chars().next().unwrap_or(' ').to_ascii_lowercase())
        }
        c if c.chars().count() == 1 => Key::Text(c.to_owned()),
        other => panic!("unknown key {other} in {spec}"),
    };
    KeyEvent { key, mods }
}

pub fn center(r: [f32; 4]) -> (f32, f32) {
    (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
}

pub struct TestWorkspace {
    pub rt: Rt,
    pub clip: clipboard::MemoryClipboard,
}

impl TestWorkspace {
    pub fn open(config: &TestConfig) -> Self {
        Self::open_with_store(config.store())
    }

    pub fn open_with_store(store: Store) -> Self {
        Self::open_with(store, true)
    }

    pub fn open_with(store: Store, dark: bool) -> Self {
        let size = std::env::var("COOPER_TEST_SIZE")
            .ok()
            .and_then(|size| {
                let (w, h) = size.split_once('x')?;
                Some((w.parse().ok()?, h.parse().ok()?))
            })
            .unwrap_or(WINDOW_SIZE);
        let mut rt = crate::app(store).headless_runtime(size, Env::new(dark, 1.0));
        rt.set_system_reduced_motion(true);
        let clip = clipboard::memory();
        rt.use_clipboard(&clip);
        rt.frame(0.0);
        if let Ok(fonts) = std::env::var("COOPER_TEST_FONTS")
            && let Some((sans, mono)) = fonts.split_once('|')
        {
            let (sans, mono) = (sans.to_string(), mono.to_string());
            rt.update(move |_, tx| tx.set_font_families(sans, mono));
        }
        let mut ws = Self { rt, clip };
        ws.settle();
        ws
    }

    pub fn state(&self) -> &Workspace {
        &self.rt.state
    }

    pub fn read<R>(&self, f: impl FnOnce(&Workspace) -> R) -> R {
        f(&self.rt.state)
    }

    pub fn update<R: 'static>(&mut self, f: impl FnOnce(&mut Workspace, &mut Tx) -> R) -> R {
        let mut out = None;
        self.rt.update(|s, tx| out = Some(f(s, tx)));
        self.settle();
        out.expect("update ran")
    }

    pub fn dispatch(&mut self, cmd: Command) -> bool {
        self.update(move |s, tx| s.run(cmd, tx))
    }

    pub fn settle(&mut self) {
        self.rt.frame(0.0);
        for _ in 0..SETTLE_FRAMES {
            self.rt.frame(DT);
        }
        if let Ok(log) = std::env::var("COOPER_OVERFLOW_LOG") {
            let found = self.overflows();
            if !found.is_empty() {
                use std::io::Write;
                let test = std::thread::current().name().unwrap_or("?").to_string();
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(log)
                {
                    for line in found {
                        let _ = writeln!(file, "{test}\t{line}");
                    }
                }
            }
        }
    }

    pub fn overflows(&self) -> Vec<String> {
        const SLACK: f32 = 1.0;
        let all = self.rt.elements();
        let name = |i: usize| {
            let e = &all[i];
            e.key
                .clone()
                .unwrap_or_else(|| format!("{}#{}", e.role, e.path))
        };
        let mut found = Vec::new();
        for (i, e) in all.iter().enumerate() {
            let Some(p) = e.parent else { continue };
            let parent = &all[p];
            if e.exiting || e.opacity <= 0.0 || e.moving || e.rect[2] <= 0.0 || e.rect[3] <= 0.0 {
                continue;
            }
            if e.visible != e.rect {
                let cut = e.rect[2] - e.visible[2];
                if e.role == "text" && e.visible[2] > 0.0 && e.visible[3] > 0.0 && cut > SLACK {
                    found.push(format!(
                        "CLIPPED {} text={:?} by {cut:.0} | {}",
                        name(i),
                        e.text.as_deref().unwrap_or(""),
                        e.path,
                    ));
                }
                continue;
            }
            let [x, y, w, h] = e.rect;
            let [px, py, pw, ph] = parent.rect;
            let left = px - x;
            let right = (x + w) - (px + pw);
            let top = py - y;
            let bottom = (y + h) - (py + ph);
            if [left, right, top, bottom].iter().all(|d| *d <= SLACK) {
                continue;
            }
            let mut chain = Vec::new();
            let mut up = Some(i);
            while let Some(j) = up {
                if all[j].key.is_some() {
                    chain.push(name(j));
                }
                up = all[j].parent;
            }
            chain.reverse();
            found.push(format!(
                "{} [{}] text={:?} out of {} by l{:.0} r{:.0} t{:.0} b{:.0} | {}",
                name(i),
                e.role,
                e.text.as_deref().or(e.label.as_deref()).unwrap_or(""),
                name(p),
                left.max(0.0),
                right.max(0.0),
                top.max(0.0),
                bottom.max(0.0),
                chain.join(" > "),
            ));
        }
        found
    }

    pub fn step(&mut self) {
        self.rt.frame(CLOCK_STEP);
    }

    pub fn press(&mut self, keys: &str) {
        for key in keys.split_whitespace() {
            if key == "cmd-v" {
                let text = self.clip.text().map(|t| t.to_string()).unwrap_or_default();
                self.rt.paste(text);
            } else {
                self.rt.key(key_event(key));
            }
            self.settle();
        }
    }

    pub fn type_text(&mut self, text: &str) {
        self.rt.key(KeyEvent {
            key: Key::Text(text.into()),
            mods: Mods::default(),
        });
        self.settle();
    }

    pub fn replace_text(&mut self, text: &str) {
        self.press("cmd-a backspace");
        self.type_text(text);
    }

    pub fn clipboard(&self) -> Option<String> {
        self.clip.text().map(|t| t.to_string())
    }

    pub fn elements(&self) -> Vec<ElementInfo> {
        self.rt
            .elements()
            .into_iter()
            .filter(|e| !e.exiting)
            .collect()
    }

    pub fn find(&self, key: &str) -> Option<ElementInfo> {
        self.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
    }

    pub fn exists(&self, key: &str) -> bool {
        self.find(key).is_some()
    }

    pub fn is_visible(&self, key: &str) -> bool {
        self.find(key)
            .is_some_and(|e| e.visible[2] > 0.0 && e.visible[3] > 0.0 && e.opacity > 0.0)
    }

    pub fn by_label(&self, label: &str) -> Option<ElementInfo> {
        self.elements()
            .into_iter()
            .find(|e| e.label.as_deref() == Some(label))
    }

    pub fn has_label(&self, label: &str) -> bool {
        self.by_label(label).is_some()
    }

    pub fn by_text(&self, text: &str) -> Option<ElementInfo> {
        self.elements()
            .into_iter()
            .find(|e| e.text.as_deref() == Some(text))
    }

    pub fn text_of(&self, key: &str) -> Option<String> {
        self.find(key).and_then(|e| e.text.or(e.label))
    }

    pub fn descendants(&self, ancestor: &str) -> Vec<ElementInfo> {
        let all = self.rt.elements();
        let inside = |i: usize| {
            std::iter::successors(all[i].parent, |&j| all[j].parent)
                .any(|j| all[j].key.as_deref() == Some(ancestor))
        };
        (0..all.len())
            .filter(|&i| !all[i].exiting && inside(i))
            .map(|i| all[i].clone())
            .collect()
    }

    pub fn within_by_label(&self, ancestor: &str, label: &str) -> Option<ElementInfo> {
        self.descendants(ancestor)
            .into_iter()
            .find(|e| e.label.as_deref() == Some(label))
    }

    pub fn has_dialog(&self) -> bool {
        self.elements().iter().any(|e| e.role == "dialog")
    }

    pub fn focused(&self) -> Option<String> {
        self.rt
            .elements()
            .into_iter()
            .find(|e| e.focused)
            .map(|e| e.key.unwrap_or(e.path))
    }

    pub fn is_focused(&self, key: &str) -> bool {
        self.focused().as_deref() == Some(key)
    }

    pub fn focus_within(&self, key: &str) -> bool {
        let all = self.rt.elements();
        let Some(focused) = all.iter().position(|e| e.focused) else {
            return false;
        };
        std::iter::successors(Some(focused), |&i| all[i].parent)
            .any(|i| all[i].key.as_deref() == Some(key))
    }

    pub fn focus(&mut self, key: &str) {
        let key = key.to_string();
        self.rt.update(move |_, tx| tx.focus(key, false));
        self.settle();
    }

    fn point_of(&self, element: Option<ElementInfo>, what: &str) -> (f32, f32) {
        let element = element.unwrap_or_else(|| panic!("no element {what}\n{}", self.describe()));
        center(element.visible)
    }

    pub fn click_at(&mut self, x: f32, y: f32) {
        self.rt.pointer_move(x, y);
        self.rt.pointer_down(x, y);
        self.rt.pointer_up(x, y);
        self.settle();
    }

    pub fn click(&mut self, key: &str) {
        let (x, y) = self.point_of(self.find(key), &format!("keyed {key:?}"));
        self.click_at(x, y);
    }

    pub fn click_label(&mut self, label: &str) {
        let (x, y) = self.point_of(self.by_label(label), &format!("labelled {label:?}"));
        self.click_at(x, y);
    }

    pub fn click_text(&mut self, text: &str) {
        let (x, y) = self.point_of(self.by_text(text), &format!("with text {text:?}"));
        self.click_at(x, y);
    }

    pub fn click_in(&mut self, ancestor: &str, label: &str) {
        let (x, y) = self.point_of(
            self.within_by_label(ancestor, label),
            &format!("labelled {label:?} inside {ancestor:?}"),
        );
        self.click_at(x, y);
    }

    pub fn double_click_at(&mut self, x: f32, y: f32) {
        for _ in 0..2 {
            self.rt.pointer_move(x, y);
            self.rt.pointer_down(x, y);
            self.rt.pointer_up(x, y);
            self.rt.frame(DT);
        }
        self.settle();
    }

    pub fn double_click(&mut self, key: &str) {
        let (x, y) = self.point_of(self.find(key), &format!("keyed {key:?}"));
        self.double_click_at(x, y);
    }

    pub fn double_click_label(&mut self, label: &str) {
        let (x, y) = self.point_of(self.by_label(label), &format!("labelled {label:?}"));
        self.double_click_at(x, y);
    }

    pub fn right_click_at(&mut self, x: f32, y: f32) {
        self.rt.pointer_move(x, y);
        self.rt.secondary_down(x, y);
        self.rt.secondary_up(x, y);
        self.settle();
    }

    pub fn right_click(&mut self, key: &str) {
        let (x, y) = self.point_of(self.find(key), &format!("keyed {key:?}"));
        self.right_click_at(x, y);
    }

    pub fn button_click(&mut self, key: &str, button: PointerButton) {
        let (x, y) = self.point_of(self.find(key), &format!("keyed {key:?}"));
        self.rt.pointer_move(x, y);
        self.rt.button_down(x, y, button);
        self.rt.button_up(x, y, button);
        self.settle();
    }

    pub fn resize(&mut self, size: (f32, f32)) {
        let scale = self.rt.env.scale;
        self.rt.set_viewport(size, scale);
        self.settle();
    }

    pub fn wait_until(&mut self, what: &str, done: impl FnMut(&TestWorkspace) -> bool) {
        self.wait_until_for(what, TIMEOUT, done)
    }

    pub fn wait_until_for(
        &mut self,
        what: &str,
        timeout: Duration,
        mut done: impl FnMut(&TestWorkspace) -> bool,
    ) {
        let deadline = Instant::now() + timeout;
        loop {
            self.step();
            if done(self) {
                self.settle();
                return;
            }
            if Instant::now() >= deadline {
                panic!(
                    "timed out after {timeout:?} waiting for {what}\n{}",
                    self.describe()
                );
            }
            std::thread::sleep(REAL_STEP);
        }
    }

    pub fn wait_for_state(&mut self, what: &str, mut done: impl FnMut(&Workspace) -> bool) {
        self.wait_until(what, move |ws| done(&ws.rt.state))
    }

    pub fn simulate_close(&mut self) {
        self.rt.update(workspace::on_close);
        self.settle();
    }

    pub fn simulate_quit(&mut self) {
        self.rt.update(workspace::on_quit);
        self.settle();
    }

    pub fn active_browser(&self) -> Option<&Browser> {
        self.rt.state.active_browser()
    }

    pub fn browser(&self) -> &Browser {
        self.active_browser().unwrap_or_else(|| {
            panic!(
                "the active tab is not a connected browser\n{}",
                self.describe()
            )
        })
    }

    pub fn tab_id(&self) -> TabId {
        self.browser().id
    }

    pub fn tree(&self) -> &SchemaTree {
        &self.browser().tree
    }

    pub fn table_view(&self) -> &TableView {
        &self.browser().table
    }

    pub fn console(&self) -> &Console {
        &self.browser().console
    }

    pub fn describe(&self) -> String {
        let s = &self.rt.state;
        let mut text = format!(
            "tabs: {:?}, active: {:?}, home messages: {:?}, dialog: {}, title: {:?}",
            s.test_tabs(),
            s.active,
            s.test_home_messages(),
            s.dialog.is_some(),
            s.window_title,
        );
        if let Some(b) = s.active_browser() {
            let m = &b.tree.model;
            text.push_str(&format!(
                "\nbrowser: main {:?}, pane {:?}, status {:?}\ntree: schemas_loaded {} error {:?} nodes {:?}\nconsole: running {} outcome {:?}",
                b.main,
                b.pane,
                b.status,
                m.schemas_loaded,
                m.schemas_error,
                m.nodes
                    .iter()
                    .map(|n| (n.name.clone(), n.tables.as_ref().map(Vec::len), n.error.clone()))
                    .collect::<Vec<_>>(),
                b.console.is_running(),
                b.console.outcome_text(),
            ));
        }
        text.push_str(&format!("\nfocused: {:?}\nelements:\n", self.focused()));
        for e in self.elements() {
            if e.key.is_some() || e.text.is_some() || e.focused {
                text.push_str(&format!(
                    "  {:?} {} {:?} {:?}{}\n",
                    e.key,
                    e.role,
                    e.label,
                    e.text,
                    if e.focused { " FOCUSED" } else { "" }
                ));
            }
        }
        text
    }

    pub fn connect(&mut self, name: &str) -> TabId {
        if self.rt.state.active.is_some() {
            self.dispatch(Command::GoHome);
        }
        let index = {
            let state = &self.rt.state;
            state
                .home
                .connections
                .iter()
                .position(|connection| connection.name == name)
        };
        let index =
            index.unwrap_or_else(|| panic!("no connection named {name:?}\n{}", self.describe()));
        self.update(move |s, tx| {
            s.home.click_row(index);
            assert!(s.run(Command::Connect, tx), "Connect was not handled");
        });
        self.wait_connected(name)
    }

    pub fn wait_connected(&mut self, name: &str) -> TabId {
        let wanted = name.to_string();
        self.wait_for_state(&format!("connection {name:?}"), move |s| {
            s.active_browser()
                .is_some_and(|b| b.connection.name == wanted)
        });
        self.wait_tree_loaded();
        self.tab_id()
    }

    pub fn wait_tree_loaded(&mut self) {
        self.wait_for_state("the schema tree to load", |s| {
            s.active_browser().is_some_and(|b| {
                let model = &b.tree.model;
                (model.schemas_loaded || model.schemas_error.is_some())
                    && model.schemas_request.is_none()
                    && model.nodes.iter().all(|node| node.request.is_none())
            })
        });
    }

    pub fn open_table(&mut self, table: &str) {
        self.dispatch(Command::FocusTree);
        if self.tree().is_filtering() {
            self.press("escape");
        }
        self.type_text(table);
        self.press("enter");
        self.wait_table_loaded(table);
    }

    pub fn wait_table_loaded(&mut self, table: &str) {
        let wanted = table.to_string();
        self.wait_for_state(&format!("the first page of {table:?}"), move |s| {
            s.active_browser().is_some_and(|b| {
                b.table.grid.as_ref().is_some_and(|grid| {
                    grid.table() == wanted
                        && !grid.is_loading()
                        && (grid.page().is_some() || grid.error().is_some())
                })
            })
        });
    }

    pub fn run_sql(&mut self, sql: &str) -> String {
        self.press("cmd-e");
        self.replace_text(sql);
        assert_eq!(
            self.console().text(),
            sql,
            "the SQL editor did not receive the typed script"
        );
        self.press("cmd-enter");
        self.wait_script_done()
    }

    pub fn wait_script_done(&mut self) -> String {
        self.wait_for_state("the script to finish", |s| {
            s.active_browser()
                .is_some_and(|b| !b.console.is_running() && b.console.outcome.is_some())
        });
        self.console()
            .outcome_text()
            .expect("the script finished with an outcome")
    }
}

pub fn snap_png(rt: &mut Rt, path: &Path) {
    use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer, write_png};
    let gpu = Gpu::headless();
    let scale = rt.env.scale;
    let (w, h) = (
        (rt.viewport.0 * scale).round() as u32,
        (rt.viewport.1 * scale).round() as u32,
    );
    let format = if rt.srgb_blend() {
        OFFSCREEN_FORMAT.remove_srgb_suffix()
    } else {
        OFFSCREEN_FORMAT
    };
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.set_srgb_blend(rt.srgb_blend());
    rt.text.atlas.invalidate();
    renderer.attach_text(&mut rt.text.atlas);
    let target = Offscreen::with_format(&gpu.device, w, h, format);
    renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
    renderer.prepare(
        &gpu.device,
        &gpu.queue,
        &mut rt.text.atlas,
        &rt.instances,
        (w, h),
    );
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, rt.clear_color());
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    write_png(path, w, h, &target.read(&gpu.device)).expect("write the snapshot png");
}

#[cfg(test)]
mod tests;
