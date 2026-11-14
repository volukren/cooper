use std::path::PathBuf;

use color_eyre::{Result, eyre::WrapErr};
use rune::{Chord, ChordKey, Cx, El, KeyEvent, Tx, col, row, text};
use rune_kit::{
    Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, SegmentedControl,
};
use toml_edit::{Array, DocumentMut, Item, Table, value};

use crate::{
    connection::{read_optional, write_private},
    dialog::Dialog,
    icons,
    keys::{self, ACTIONS, Action, Command, Keymap},
    theme::{RADIUS, Theme},
    ui::{self, Modal, ScrollY},
    workspace::Workspace,
};

const THEME_KEY: &str = "theme";
const KEYS_KEY: &str = "keys";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 3] = [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark];

    pub fn id(self) -> &'static str {
        match self {
            ThemeMode::System => "system",
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::System => "System",
            ThemeMode::Light => "Light",
            ThemeMode::Dark => "Dark",
        }
    }

    fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.id() == id)
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Loaded {
    pub theme: ThemeMode,
    pub problems: Vec<String>,
}

#[derive(Debug)]
pub struct SettingsFile {
    path: PathBuf,
}

impl SettingsFile {
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    fn document(&self) -> Result<DocumentMut> {
        match read_optional(&self.path)? {
            Some(data) => data
                .parse()
                .wrap_err_with(|| format!("parsing {}", self.path.display())),
            None => Ok(DocumentMut::new()),
        }
    }

    pub fn load(&self, keymap: &mut Keymap) -> Result<Loaded> {
        let doc = self.document()?;
        let mut loaded = Loaded::default();
        match doc.get(THEME_KEY) {
            None => {}
            Some(item) => match item.as_str().and_then(ThemeMode::parse) {
                Some(theme) => loaded.theme = theme,
                None => loaded.problems.push(format!(
                    "`{THEME_KEY}` must be \"system\", \"light\" or \"dark\""
                )),
            },
        }
        match doc.get(KEYS_KEY) {
            None => {}
            Some(item) => match item.as_table_like() {
                Some(table) => {
                    for (id, item) in table.iter() {
                        let applied = shortcut_specs(item)
                            .ok_or_else(|| format!("\"{id}\" must be a string or a list of them"))
                            .and_then(|specs| keymap.apply(id, &specs));
                        if let Err(problem) = applied {
                            loaded.problems.push(problem);
                        }
                    }
                }
                None => loaded
                    .problems
                    .push(format!("`{KEYS_KEY}` must be a table")),
            },
        }
        Ok(loaded)
    }

    pub fn save(&self, theme: ThemeMode, keymap: &Keymap) -> Result<()> {
        let mut doc = self.document()?;
        doc[THEME_KEY] = value(theme.id());
        let overrides = keymap.overrides();
        if overrides.is_empty() {
            doc.remove(KEYS_KEY);
        } else {
            let mut table = Table::new();
            for (id, specs) in overrides {
                table[id] = match specs.as_slice() {
                    [spec] => value(spec.as_str()),
                    specs => value(specs.iter().map(String::as_str).collect::<Array>()),
                };
            }
            doc[KEYS_KEY] = Item::Table(table);
        }
        write_private(&self.path, &doc.to_string())
    }
}

fn shortcut_specs(item: &Item) -> Option<Vec<String>> {
    if let Some(spec) = item.as_str() {
        return Some(vec![spec.to_string()]);
    }
    item.as_array()?
        .iter()
        .map(|spec| spec.as_str().map(str::to_string))
        .collect()
}

struct Recording {
    cmd: Command,
    slot: usize,
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    General,
    Shortcuts,
}

impl Page {
    const ALL: [Page; 2] = [Page::General, Page::Shortcuts];

    fn id(self) -> &'static str {
        match self {
            Page::General => "general",
            Page::Shortcuts => "shortcuts",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Shortcuts => "Keyboard shortcuts",
        }
    }
}

#[derive(Default)]
pub struct Settings {
    page: Page,
    scroll: ScrollY,
    recording: Option<Recording>,
}

impl Settings {
    pub fn page(&self) -> Page {
        self.page
    }
}

fn open_page(s: &mut Workspace, page: Page) {
    if let Some(settings) = settings_mut(s) {
        settings.page = page;
        settings.recording = None;
    }
}

fn settings_mut(s: &mut Workspace) -> Option<&mut Settings> {
    match &mut s.dialog {
        Some(Dialog::Settings(settings)) => Some(settings),
        _ => None,
    }
}

fn chip_key(action: &Action, slot: usize) -> String {
    format!("shortcut-{}-{slot}", action.id)
}

fn begin_recording(s: &mut Workspace, action: &Action, slot: usize, tx: &mut Tx) {
    if let Some(settings) = settings_mut(s) {
        settings.recording = Some(Recording {
            cmd: action.cmd,
            slot,
            error: None,
        });
        tx.focus(chip_key(action, slot), false);
    }
}

fn stop_recording(s: &mut Workspace) {
    if let Some(settings) = settings_mut(s) {
        settings.recording = None;
    }
}

pub fn record_key(s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(settings) = settings_mut(s) else {
        return false;
    };
    let Some(recording) = &mut settings.recording else {
        return false;
    };
    let Some(chord) = keys::chord_from_event(ev, tx) else {
        return true;
    };
    if chord == Chord::new(ChordKey::Escape) {
        settings.recording = None;
        return true;
    }
    if keys::needs_modifier(&chord) {
        let modifiers = if cfg!(target_os = "macos") {
            "⌘, ⌃ or ⌥"
        } else {
            "Ctrl or Alt"
        };
        recording.error = Some(format!(
            "{} needs a modifier: add {modifiers}",
            chord.label()
        ));
        return true;
    }
    let (cmd, slot) = (recording.cmd, recording.slot);
    settings.recording = None;
    keys::update(|keymap| keymap.set(cmd, slot, chord));
    s.keymap_changed(tx);
    true
}

fn reset(s: &mut Workspace, cmd: Option<Command>, tx: &mut Tx) {
    stop_recording(s);
    keys::update(|keymap| match cmd {
        Some(cmd) => keymap.reset(cmd),
        None => keymap.reset_all(),
    });
    s.keymap_changed(tx);
}

fn section_title(title: &str, extra: Option<El>, t: &Theme) -> El {
    row()
        .w_full()
        .min_h(24.0)
        .pb(4.0)
        .gap(8.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
        .child(
            text(title.to_string())
                .font(t.bold(t.text_sm))
                .color(t.blue)
                .grow(1.0),
        )
        .children(extra)
}

fn appearance(s: &Workspace, t: &Theme) -> El {
    let labels = ThemeMode::ALL.map(ThemeMode::label);
    let selected = ThemeMode::ALL.iter().position(|&mode| mode == s.theme_mode);
    let picker = SegmentedControl::new(&labels, selected)
        .label("Theme")
        .on_pick(|s: &mut Workspace, index: usize, tx: &mut Tx| {
            if let Some(&mode) = ThemeMode::ALL.get(index) {
                s.set_theme(mode, tx);
            }
        })
        .build(&t.k)
        .key("theme-mode");
    col()
        .w_full()
        .gap(8.0)
        .child(section_title("Appearance", None, t))
        .child(
            row()
                .w_full()
                .gap(12.0)
                .center_y()
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(2.0)
                        .child(ui::label("Theme", t.text_sm, t.foreground))
                        .child(ui::label(
                            "System follows the appearance of the OS",
                            t.text_xs,
                            t.muted,
                        )),
                )
                .child(picker),
        )
}

fn chip(action: &'static Action, slot: usize, chord: Chord, recording: bool, t: &Theme) -> El {
    let label = chord.label();
    let (content, color, border) = if recording {
        ("Press keys…".to_string(), t.muted, t.ring)
    } else {
        (label.clone(), t.foreground, t.border)
    };
    row()
        .key(chip_key(action, slot))
        .role("button")
        .label(format!("{}: {label}", action.title))
        .help("Click, then press the new shortcut")
        .focusable(true)
        .focus_ring(2.0, 1.0, t.ring)
        .h(22.0)
        .min_w(28.0)
        .px(6.0)
        .center()
        .radius(4.0)
        .border(1.0, border)
        .hover(t.list_hover)
        .child(text(content).font(t.medium(t.text_xs)).color(color))
        .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| begin_recording(s, action, slot, tx))
        .on_key(record_key)
}

fn shortcut_row(action: &'static Action, settings: &Settings, t: &Theme) -> El {
    let cmd = action.cmd;
    let (chords, modified) = keys::with(|keymap| (keymap.chords(cmd), keymap.is_modified(cmd)));
    let recording = settings
        .recording
        .as_ref()
        .filter(|recording| recording.cmd == cmd);
    let mut notes = Vec::new();
    if let Some(error) = recording.and_then(|recording| recording.error.clone()) {
        notes.push((error, t.danger));
    }
    for (slot, chord) in chords.iter().enumerate() {
        let taken = keys::with(|keymap| keymap.conflicts(cmd, slot));
        if !taken.is_empty() {
            let message = format!("{} is also used by: {}", chord.label(), taken.join(", "));
            notes.push((message, t.warning));
        }
    }
    let chips = chords.iter().enumerate().map(|(slot, &chord)| {
        let active = recording.is_some_and(|recording| recording.slot == slot);
        chip(action, slot, chord, active, t)
    });
    let undo = modified.then(|| {
        Button::new("Reset")
            .variant(ButtonVariant::Ghost)
            .control_size(ControlSize::Mini)
            .help("Back to the default shortcut")
            .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| reset(s, Some(cmd), tx))
            .build(&t.k)
            .key(format!("reset-{}", action.id))
    });
    col()
        .key(format!("shortcut-row-{}", action.id))
        .w_full()
        .py(3.0)
        .gap(2.0)
        .child(
            row()
                .w_full()
                .min_h(22.0)
                .gap(8.0)
                .center_y()
                .child(
                    text(action.title)
                        .font(t.text_sm)
                        .color(t.foreground)
                        .wrap()
                        .grow(1.0)
                        .min_w(0.0),
                )
                .children(undo)
                .children(chips),
        )
        .children(
            notes
                .into_iter()
                .map(|(note, color)| text(note).font(t.text_xs).color(color).wrap().w_full()),
        )
}

fn shortcuts(settings: &Settings, t: &Theme) -> El {
    let mut groups: Vec<(&str, Vec<&'static Action>)> = Vec::new();
    for action in ACTIONS {
        match groups.last_mut() {
            Some((group, actions)) if *group == action.group => actions.push(action),
            _ => groups.push((action.group, vec![action])),
        }
    }
    col()
        .key("settings-shortcuts")
        .w_full()
        .pr(12.0)
        .children(
            groups
                .into_iter()
                .enumerate()
                .map(|(index, (group, actions))| {
                    col()
                        .w_full()
                        .mt(if index > 0 { 12.0 } else { 0.0 })
                        .child(
                            text(group.to_uppercase())
                                .font(t.semibold(t.text_xs))
                                .color(t.muted)
                                .pb(2.0),
                        )
                        .children(
                            actions
                                .into_iter()
                                .map(|action| shortcut_row(action, settings, t)),
                        )
                }),
        )
}

const NAV_WIDTH: f32 = 168.0;

fn nav(settings: &Settings, t: &Theme) -> El {
    let clear = t.background.alpha(0.0);
    col()
        .key("settings-nav")
        .role("tablist")
        .label("Settings pages")
        .w(NAV_WIDTH)
        .h_full()
        .pr(8.0)
        .gap(1.0)
        .border_sides([0.0, 1.0, 0.0, 0.0], t.border)
        .children(Page::ALL.into_iter().map(|page| {
            let active = settings.page == page;
            let mut item = row()
                .key(format!("settings-nav-{}", page.id()))
                .role("tab")
                .label(page.title())
                .focusable(true)
                .focus_selected(active)
                .focus_ring(2.0, -1.0, t.ring)
                .w_full()
                .h(28.0)
                .px(8.0)
                .center_y()
                .radius(RADIUS)
                .fill(if active { t.list_active } else { clear })
                .child(
                    text(page.title())
                        .font(if active {
                            t.medium(t.text_sm)
                        } else {
                            t.text_sm
                        })
                        .color(if active { t.foreground } else { t.muted })
                        .truncate()
                        .min_w(0.0),
                )
                .on_press(move |s: &mut Workspace| open_page(s, page));
            if !active {
                item = item.hover(t.list_hover);
            }
            item
        }))
}

fn general_page(s: &Workspace, t: &Theme) -> El {
    col()
        .key("settings-general")
        .w_full()
        .h_full()
        .child(appearance(s, t))
}

fn shortcuts_page(settings: &Settings, t: &Theme) -> El {
    let reset_all = keys::with(Keymap::any_modified).then(|| {
        Button::new("Reset all")
            .variant(ButtonVariant::Ghost)
            .control_size(ControlSize::Mini)
            .help("Back to the default shortcuts")
            .on_press_tx(|s: &mut Workspace, tx: &mut Tx| reset(s, None, tx))
            .build(&t.k)
            .key("reset-all-shortcuts")
    });
    let list = t.k.scroll_bar(ui::scroll_view(
        "settings-list",
        &settings.scroll,
        shortcuts(settings, t),
    ));
    col()
        .key("settings-keys")
        .w_full()
        .h_full()
        .gap(8.0)
        .child(section_title("Keyboard shortcuts", reset_all, t))
        .child(ui::label(
            "Click a shortcut and press the new keys. Esc cancels.",
            t.text_xs,
            t.muted,
        ))
        .child(
            col()
                .key("settings-box")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(list),
        )
}

pub fn dialog(
    s: &Workspace,
    settings: &Settings,
    t: &Theme,
    cx: &mut Cx<Workspace>,
    layer: &mut Vec<El>,
) {
    let height = cx
        .layout_size("workspace")
        .map_or(480.0, |size| (size[1] - 240.0).clamp(200.0, 480.0));
    let page = match settings.page {
        Page::General => general_page(s, t),
        Page::Shortcuts => shortcuts_page(settings, t),
    };
    let close = IconButton::new(&icons::CLOSE)
        .variant(IconButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .label("Close")
        .on_press_tx(|s: &mut Workspace, tx: &mut Tx| s.close_dialog(tx))
        .build(&t.k)
        .key("close");
    let title = row()
        .w_full()
        .center_y()
        .child(
            text("Settings")
                .font(t.semibold(t.text))
                .color(t.foreground)
                .grow(1.0),
        )
        .child(close);
    Modal::new("settings", "Settings")
        .title(title)
        .width(760.0)
        .child(
            row()
                .key("settings")
                .w_full()
                .h(height)
                .gap(16.0)
                .child(nav(settings, t))
                .child(
                    col()
                        .key("settings-page")
                        .role("tabpanel")
                        .label(settings.page.title())
                        .grow(1.0)
                        .min_w(0.0)
                        .h_full()
                        .child(page),
                ),
        )
        .build(t, layer, |s: &mut Workspace, tx: &mut Tx| {
            s.close_dialog(tx)
        });
}

#[cfg(test)]
mod tests {
    use rune::Chord;

    use super::{Loaded, SettingsFile, ThemeMode};
    use crate::keys::{Command, Keymap};

    fn file(contents: Option<&str>) -> (tempfile::TempDir, SettingsFile) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        if let Some(contents) = contents {
            std::fs::write(&path, contents).unwrap();
        }
        (dir, SettingsFile::at(path))
    }

    fn chord(spec: &str) -> Chord {
        Chord::parse(spec).unwrap()
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let (_dir, file) = file(None);
        let mut keymap = Keymap::default();
        assert_eq!(file.load(&mut keymap).unwrap(), Loaded::default());
        assert!(!keymap.any_modified());
    }

    #[test]
    fn settings_survive_a_save_and_a_load() {
        let (_dir, file) = file(None);
        let mut keymap = Keymap::default();
        keymap.set(Command::NewConnection, 0, chord("mod+k"));
        keymap.set(Command::ShowHelp, 1, chord("f12"));
        file.save(ThemeMode::Light, &keymap).unwrap();

        let mut loaded_keymap = Keymap::default();
        let loaded = file.load(&mut loaded_keymap).unwrap();
        assert_eq!(loaded.theme, ThemeMode::Light);
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
        assert_eq!(loaded_keymap.overrides(), keymap.overrides());
        assert_eq!(
            loaded_keymap.chords(Command::ShowHelp),
            [chord("mod+/"), chord("f12")]
        );
    }

    #[test]
    fn saving_keeps_comments_and_drops_shortcuts_that_went_back() {
        let (dir, file) = file(Some(
            "# my settings\ntheme = \"dark\" # at night\n\n[keys]\nquit = \"ctrl+q\"\n",
        ));
        let mut keymap = Keymap::default();
        assert_eq!(file.load(&mut keymap).unwrap().theme, ThemeMode::Dark);
        assert_eq!(keymap.chords(Command::Quit), [chord("ctrl+q")]);

        keymap.reset_all();
        file.save(ThemeMode::Light, &keymap).unwrap();
        let data = std::fs::read_to_string(dir.path().join("settings.toml")).unwrap();
        assert!(data.starts_with("# my settings\n"), "{data}");
        assert!(data.contains("theme = \"light\""), "{data}");
        assert!(!data.contains("keys"), "{data}");
    }

    #[test]
    fn bad_values_are_reported_and_the_rest_still_loads() {
        let (_dir, file) = file(Some(
            "theme = \"pink\"\n\n[keys]\nquit = \"ctrl+q\"\nnope = \"mod+k\"\nconnect = 5\n",
        ));
        let mut keymap = Keymap::default();
        let loaded = file.load(&mut keymap).unwrap();
        assert_eq!(loaded.theme, ThemeMode::System);
        assert_eq!(loaded.problems.len(), 3, "{:?}", loaded.problems);
        assert_eq!(keymap.chords(Command::Quit), [chord("ctrl+q")]);
    }

    #[test]
    fn a_broken_file_is_an_error_and_is_not_overwritten() {
        let (dir, file) = file(Some("theme = "));
        let mut keymap = Keymap::default();
        assert!(file.load(&mut keymap).is_err());
        assert!(file.save(ThemeMode::Dark, &keymap).is_err());
        let data = std::fs::read_to_string(dir.path().join("settings.toml")).unwrap();
        assert_eq!(data, "theme = ");
    }
}
