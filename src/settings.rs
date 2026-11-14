use std::path::PathBuf;

use color_eyre::{Result, eyre::WrapErr};
use rune::{Chord, ChordKey, Cx, El, Icon, KeyEvent, Tx, col, row, text};
use rune_kit::{
    Button, ButtonVariant, Combobox, ComboboxOption, ComboboxState, ControlSize, IconButton,
    IconButtonVariant, SegmentedControl,
};
use toml_edit::{Array, DocumentMut, Item, Table, value};

use crate::{
    connection::{read_optional, write_private},
    dialog::Dialog,
    fonts::{Family, Installed},
    icons,
    keys::{self, ACTIONS, Action, Command, Keymap},
    theme::{self, RADIUS, Theme},
    ui::{self, Modal, ScrollY},
    workspace::Workspace,
};

const THEME_KEY: &str = "theme";
const KEYS_KEY: &str = "keys";
const FONTS_KEY: &str = "fonts";

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fonts {
    pub interface: String,
    pub code: String,
}

impl Default for Fonts {
    fn default() -> Self {
        Self {
            interface: theme::FAMILIES.0.to_string(),
            code: theme::FAMILIES.1.to_string(),
        }
    }
}

impl Fonts {
    pub fn get(&self, slot: FontSlot) -> &str {
        match slot {
            FontSlot::Interface => &self.interface,
            FontSlot::Code => &self.code,
        }
    }

    fn get_mut(&mut self, slot: FontSlot) -> &mut String {
        match slot {
            FontSlot::Interface => &mut self.interface,
            FontSlot::Code => &mut self.code,
        }
    }

    pub fn leak(&self) -> (&'static str, &'static str) {
        let leak = |family: &str, default: &'static str| -> &'static str {
            if family == default {
                default
            } else {
                Box::leak(family.to_string().into_boxed_str())
            }
        };
        (
            leak(&self.interface, theme::FAMILIES.0),
            leak(&self.code, theme::FAMILIES.1),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontSlot {
    Interface,
    Code,
}

impl FontSlot {
    const ALL: [FontSlot; 2] = [FontSlot::Interface, FontSlot::Code];

    fn id(self) -> &'static str {
        match self {
            FontSlot::Interface => "interface",
            FontSlot::Code => "code",
        }
    }

    fn default_family(self) -> &'static str {
        match self {
            FontSlot::Interface => theme::FAMILIES.0,
            FontSlot::Code => theme::FAMILIES.1,
        }
    }

    fn default_label(self) -> &'static str {
        match self {
            FontSlot::Interface => "Inter (default)",
            FontSlot::Code => "System monospace (default)",
        }
    }

    fn presets(self) -> &'static [&'static str] {
        match self {
            FontSlot::Interface if cfg!(target_os = "macos") => {
                &["Helvetica Neue", "Avenir Next", "Lucida Grande", "Verdana"]
            }
            FontSlot::Interface if cfg!(windows) => &["Segoe UI", "Arial", "Tahoma", "Verdana"],
            FontSlot::Interface => &[
                "Noto Sans",
                "DejaVu Sans",
                "Liberation Sans",
                "Ubuntu",
                "Cantarell",
            ],
            FontSlot::Code if cfg!(target_os = "macos") => {
                &["Menlo", "Monaco", "Courier New", "Andale Mono"]
            }
            FontSlot::Code if cfg!(windows) => {
                &["Cascadia Mono", "Consolas", "Courier New", "Lucida Console"]
            }
            FontSlot::Code => &[
                "DejaVu Sans Mono",
                "Noto Sans Mono",
                "Liberation Mono",
                "Ubuntu Mono",
            ],
        }
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Loaded {
    pub theme: ThemeMode,
    pub fonts: Fonts,
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
        match doc.get(FONTS_KEY) {
            None => {}
            Some(item) => match item.as_table_like() {
                Some(table) => {
                    for (id, item) in table.iter() {
                        let Some(slot) = FontSlot::ALL.into_iter().find(|slot| slot.id() == id)
                        else {
                            loaded.problems.push(format!(
                                "unknown font \"{id}\" in `{FONTS_KEY}`: use \"interface\" or \"code\""
                            ));
                            continue;
                        };
                        match item
                            .as_str()
                            .map(str::trim)
                            .filter(|family| !family.is_empty())
                        {
                            Some(family) => *loaded.fonts.get_mut(slot) = family.to_string(),
                            None => loaded
                                .problems
                                .push(format!("`{FONTS_KEY}.{id}` must be the name of a font")),
                        }
                    }
                }
                None => loaded
                    .problems
                    .push(format!("`{FONTS_KEY}` must be a table")),
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

    pub fn fonts(&self) -> Fonts {
        self.load(&mut Keymap::default())
            .map(|loaded| loaded.fonts)
            .unwrap_or_default()
    }

    pub fn save(&self, theme: ThemeMode, fonts: &Fonts, keymap: &Keymap) -> Result<()> {
        let mut doc = self.document()?;
        doc[THEME_KEY] = value(theme.id());
        save_fonts(&mut doc, fonts);
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

fn save_fonts(doc: &mut DocumentMut, fonts: &Fonts) {
    if !doc.get(FONTS_KEY).is_some_and(Item::is_table_like) {
        doc.remove(FONTS_KEY);
    }
    let changed: Vec<FontSlot> = FontSlot::ALL
        .into_iter()
        .filter(|&slot| fonts.get(slot) != slot.default_family())
        .collect();
    if changed.is_empty() && doc.get(FONTS_KEY).is_none() {
        return;
    }
    let Some(table) = doc
        .entry(FONTS_KEY)
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
    else {
        return;
    };
    for slot in FontSlot::ALL {
        let family = fonts.get(slot);
        if !changed.contains(&slot) {
            table.remove(slot.id());
            continue;
        }
        match table.get_mut(slot.id()) {
            Some(item) if item.as_str() == Some(family) => {}
            Some(item) => {
                let decor = item.as_value().map(|old| old.decor().clone());
                *item = value(family);
                if let (Some(decor), Some(new)) = (decor, item.as_value_mut()) {
                    *new.decor_mut() = decor;
                }
            }
            None => {
                table.insert(slot.id(), value(family));
            }
        }
    }
    if table.is_empty() {
        doc.remove(FONTS_KEY);
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
    Appearance,
    Shortcuts,
}

impl Page {
    const ALL: [Page; 2] = [Page::Appearance, Page::Shortcuts];

    fn id(self) -> &'static str {
        match self {
            Page::Appearance => "appearance",
            Page::Shortcuts => "shortcuts",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Page::Appearance => "Appearance",
            Page::Shortcuts => "Keyboard shortcuts",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Page::Appearance => "Colors and fonts",
            Page::Shortcuts => "Click a shortcut and press the new keys. Esc cancels.",
        }
    }

    fn icon(self) -> &'static Icon {
        match self {
            Page::Appearance => &icons::PALETTE,
            Page::Shortcuts => &icons::KEYBOARD,
        }
    }
}

#[derive(Default)]
pub struct Settings {
    page: Page,
    scroll: ScrollY,
    recording: Option<Recording>,
    interface_font: ComboboxState,
    code_font: ComboboxState,
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
        settings.interface_font.close();
        settings.code_font.close();
    }
}

fn font_picker(s: &mut Workspace, slot: FontSlot) -> &mut ComboboxState {
    let Workspace {
        dialog,
        scratch_picker,
        ..
    } = s;
    match dialog {
        Some(Dialog::Settings(settings)) => match slot {
            FontSlot::Interface => &mut settings.interface_font,
            FontSlot::Code => &mut settings.code_font,
        },
        _ => scratch_picker,
    }
}

fn set_font(s: &mut Workspace, slot: FontSlot, family: &str, tx: &mut Tx) {
    let family = family.trim();
    if family.is_empty() || family == LOADING_FONTS {
        return;
    }
    let mut fonts = s.fonts.clone();
    *fonts.get_mut(slot) = family.to_string();
    s.set_fonts(fonts, tx);
}

fn font_options(
    slot: FontSlot,
    current: &str,
    query: &str,
    installed: Option<&Installed>,
    open: bool,
) -> Vec<ComboboxOption> {
    let mut options = vec![ComboboxOption::new(
        slot.default_family(),
        slot.default_label(),
    )];
    let mut add = |family: &str, label: String, detail: &str| {
        if !options
            .iter()
            .any(|option| option.id.eq_ignore_ascii_case(family))
        {
            let mut option = ComboboxOption::new(family, label);
            option.detail = detail.to_string();
            options.push(option);
        }
    };
    add(current, current.to_string(), "");
    if !open {
        return options;
    }
    match installed.filter(|installed| !installed.families.is_empty()) {
        Some(installed) => {
            let (first, rest): (Vec<&Family>, Vec<&Family>) = match slot {
                FontSlot::Code => installed.families.iter().partition(|family| family.mono),
                FontSlot::Interface => (installed.families.iter().collect(), Vec::new()),
            };
            for family in first {
                let detail = if slot == FontSlot::Interface && family.mono {
                    "Monospace"
                } else {
                    ""
                };
                add(&family.name, family.name.clone(), detail);
            }
            for family in rest {
                add(&family.name, family.name.clone(), "Proportional");
            }
        }
        None => {
            for &family in slot.presets() {
                add(family, family.to_string(), "");
            }
        }
    }
    let typed = query.trim();
    if !typed.is_empty() {
        let detail = if typed.contains(',') {
            "Font list"
        } else {
            "Custom"
        };
        add(typed, format!("Use “{typed}”"), detail);
    }
    if installed.is_none() {
        let mut loading = ComboboxOption::new(LOADING_FONTS, "Looking for installed fonts…");
        loading.disabled = true;
        options.push(loading);
    }
    options
}

const LOADING_FONTS: &str = "(loading installed fonts)";

fn font_picker_el(
    s: &Workspace,
    settings: &Settings,
    slot: FontSlot,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let state = match slot {
        FontSlot::Interface => &settings.interface_font,
        FontSlot::Code => &settings.code_font,
    };
    let current = s.fonts.get(slot);
    let options = font_options(
        slot,
        current,
        &state.search().text,
        s.installed_fonts,
        state.is_open(),
    );
    let name = match slot {
        FontSlot::Interface => "Interface font",
        FontSlot::Code => "Code font",
    };
    Combobox::new(
        format!("font-{}", slot.id()),
        state,
        &options,
        move |s: &mut Workspace| font_picker(s, slot),
    )
    .label(name)
    .current(Some(current))
    .search_placeholder("Search or type a font name")
    .empty_message("No fonts")
    .width(260.0)
    .max_height(320.0)
    .on_choose(move |s: &mut Workspace, family: &str, tx: &mut Tx| set_font(s, slot, family, tx))
    .build(&t.k, layer)
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

const FONT_NOTE: &str = "Type to search, or enter a list such as “Fira Code, Menlo”. A font that isn't installed falls back to a system font.";

struct Section {
    key: Option<&'static str>,
    title: String,
    extra: Option<El>,
    rows: Vec<El>,
    note: Option<&'static str>,
    dense: bool,
}

fn section(section: Section, t: &Theme) -> El {
    let Section {
        key,
        title,
        extra,
        rows,
        note,
        dense,
    } = section;
    let mut el = col()
        .w_full()
        .child(
            row()
                .w_full()
                .min_h(22.0)
                .pb(10.0)
                .gap(8.0)
                .center_y()
                .child(
                    text(title)
                        .font(t.semibold(t.text_sm))
                        .color(t.foreground)
                        .grow(1.0),
                )
                .children(extra),
        )
        .child(
            col()
                .w_full()
                .gap(if dense { 4.0 } else { 14.0 })
                .children(rows),
        )
        .children(note.map(|note| {
            text(note)
                .font(t.text_xs)
                .color(t.muted)
                .wrap()
                .w_full()
                .pt(8.0)
        }));
    if let Some(key) = key {
        el = el.key(key);
    }
    el
}

fn sections(sections: Vec<Section>, t: &Theme, gap: f32) -> El {
    col()
        .w_full()
        .gap(gap)
        .children(sections.into_iter().map(|item| section(item, t)))
}

fn mini_button(
    label: &str,
    help: &str,
    key: impl Into<String>,
    t: &Theme,
    press: impl Fn(&mut Workspace, &mut Tx) + 'static,
) -> El {
    Button::new(label)
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Mini)
        .help(help)
        .on_press_tx(press)
        .build(&t.k)
        .key(key.into())
}

fn setting_row(title: &str, hint: &str, control: El, t: &Theme) -> El {
    row()
        .w_full()
        .gap(12.0)
        .center_y()
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(ui::label(title, t.text_sm, t.foreground))
                .child(ui::label(hint, t.text_xs, t.muted)),
        )
        .child(control)
}

fn theme_picker(s: &Workspace, t: &Theme) -> El {
    let labels = ThemeMode::ALL.map(ThemeMode::label);
    let selected = ThemeMode::ALL.iter().position(|&mode| mode == s.theme_mode);
    SegmentedControl::new(&labels, selected)
        .label("Theme")
        .on_pick(|s: &mut Workspace, index: usize, tx: &mut Tx| {
            if let Some(&mode) = ThemeMode::ALL.get(index) {
                s.set_theme(mode, tx);
            }
        })
        .build(&t.k)
        .key("theme-mode")
}

fn appearance_sections(
    s: &Workspace,
    settings: &Settings,
    t: &Theme,
    layer: &mut Vec<El>,
) -> Vec<Section> {
    let reset_fonts = (s.fonts != Fonts::default()).then(|| {
        mini_button(
            "Reset",
            "Back to the default fonts",
            "reset-fonts",
            t,
            |s, tx| s.set_fonts(Fonts::default(), tx),
        )
    });
    vec![
        Section {
            key: None,
            title: "Theme".into(),
            extra: None,
            rows: vec![setting_row(
                "Color scheme",
                "System follows the appearance of the OS",
                theme_picker(s, t),
                t,
            )],
            note: None,
            dense: false,
        },
        Section {
            key: Some("settings-fonts"),
            title: "Fonts".into(),
            extra: reset_fonts,
            rows: vec![
                setting_row(
                    "Interface",
                    "Menus, lists, forms and dialogs",
                    font_picker_el(s, settings, FontSlot::Interface, t, layer),
                    t,
                ),
                setting_row(
                    "Code",
                    "The SQL editor, results and table data",
                    font_picker_el(s, settings, FontSlot::Code, t, layer),
                    t,
                ),
            ],
            note: Some(FONT_NOTE),
            dense: false,
        },
    ]
}

fn keycap(action: &'static Action, slot: usize, chord: Chord, recording: bool, t: &Theme) -> El {
    let label = chord.label();
    let c = &t.k.c;
    let (content, border, fill) = if recording {
        ("Press keys…".to_string(), t.ring, t.k.alpha(t.ring, 0.14))
    } else {
        (label.clone(), c.border_base, c.layer_02)
    };
    let mut cap = row()
        .key(chip_key(action, slot))
        .role("button")
        .label(format!("{}: {label}", action.title))
        .help("Click, then press the new shortcut")
        .focusable(true)
        .focus_ring(2.0, 1.0, t.ring)
        .h(22.0)
        .min_w(26.0)
        .px(7.0)
        .center()
        .radius(5.0)
        .fill(fill)
        .border(1.0, border)
        .child(text(content).font(t.medium(t.text_xs)).color(t.foreground))
        .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| begin_recording(s, action, slot, tx))
        .on_key(record_key);
    if !recording {
        cap = cap.shadow(1.0, 0.0, c.border_base).hover(c.layer_03);
    }
    cap
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
    let caps = chords.iter().enumerate().map(|(slot, &chord)| {
        let active = recording.is_some_and(|recording| recording.slot == slot);
        keycap(action, slot, chord, active, t)
    });
    let undo = modified.then(|| {
        mini_button(
            "Reset",
            "Back to the default shortcut",
            format!("reset-{}", action.id),
            t,
            move |s, tx| reset(s, Some(cmd), tx),
        )
    });
    col()
        .key(format!("shortcut-row-{}", action.id))
        .w_full()
        .gap(3.0)
        .child(
            row()
                .w_full()
                .min_h(24.0)
                .gap(6.0)
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
                .children(caps),
        )
        .children(
            notes
                .into_iter()
                .map(|(note, color)| text(note).font(t.text_xs).color(color).wrap().w_full()),
        )
}

fn shortcut_sections(settings: &Settings, t: &Theme) -> Vec<Section> {
    let mut groups: Vec<(&str, Vec<&'static Action>)> = Vec::new();
    for action in ACTIONS {
        match groups.last_mut() {
            Some((group, actions)) if *group == action.group => actions.push(action),
            _ => groups.push((action.group, vec![action])),
        }
    }
    groups
        .into_iter()
        .map(|(group, actions)| Section {
            key: None,
            title: group.to_string(),
            extra: None,
            rows: actions
                .into_iter()
                .map(|action| shortcut_row(action, settings, t))
                .collect(),
            note: None,
            dense: true,
        })
        .collect()
}

const NAV_WIDTH: f32 = 176.0;
const PAGE_PX: f32 = 20.0;

fn nav(settings: &Settings, t: &Theme) -> El {
    let clear = t.background.alpha(0.0);
    col()
        .key("settings-nav")
        .role("tablist")
        .label("Settings pages")
        .w(NAV_WIDTH)
        .h_full()
        .padding(8.0)
        .gap(2.0)
        .border_sides([0.0, 1.0, 0.0, 0.0], t.border)
        .children(Page::ALL.into_iter().map(|page| {
            let active = settings.page == page;
            let color = if active { t.foreground } else { t.muted };
            let mut item = row()
                .key(format!("settings-nav-{}", page.id()))
                .role("tab")
                .label(page.title())
                .focusable(true)
                .focus_selected(active)
                .focus_ring(2.0, -1.0, t.ring)
                .w_full()
                .h(30.0)
                .px(8.0)
                .gap(8.0)
                .center_y()
                .radius(RADIUS)
                .fill(if active { t.list_active } else { clear })
                .child(ui::icon_el(page.icon(), 15.0, color))
                .child(
                    text(page.title())
                        .font(if active {
                            t.medium(t.text_sm)
                        } else {
                            t.text_sm
                        })
                        .color(color)
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

fn page_header(settings: &Settings, t: &Theme) -> El {
    let extra = match settings.page {
        Page::Appearance => None,
        Page::Shortcuts => keys::with(Keymap::any_modified).then(|| {
            mini_button(
                "Reset all",
                "Back to the default shortcuts",
                "reset-all-shortcuts",
                t,
                |s, tx| reset(s, None, tx),
            )
        }),
    };
    row()
        .w_full()
        .px(PAGE_PX)
        .py(16.0)
        .gap(8.0)
        .center_y()
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(
                    text(settings.page.title())
                        .font(t.semibold(t.text))
                        .color(t.foreground),
                )
                .child(ui::label(settings.page.subtitle(), t.text_xs, t.muted)),
        )
        .children(extra)
}

fn appearance_page(s: &Workspace, settings: &Settings, t: &Theme, layer: &mut Vec<El>) -> El {
    col()
        .key("settings-appearance")
        .w_full()
        .h_full()
        .px(PAGE_PX)
        .pt(4.0)
        .child(sections(
            appearance_sections(s, settings, t, layer),
            t,
            24.0,
        ))
}

fn shortcuts_page(settings: &Settings, t: &Theme) -> El {
    let list = t.k.scroll_bar(ui::scroll_view(
        "settings-list",
        &settings.scroll,
        col()
            .key("settings-shortcuts")
            .w_full()
            .px(PAGE_PX)
            .pt(4.0)
            .pb(16.0)
            .child(sections(shortcut_sections(settings, t), t, 20.0)),
    ));
    col().key("settings-keys").w_full().h_full().child(
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
        .map_or(496.0, |size| (size[1] - 224.0).clamp(216.0, 496.0));
    let mut popups = Vec::new();
    let page = match settings.page {
        Page::Appearance => appearance_page(s, settings, t, &mut popups),
        Page::Shortcuts => shortcuts_page(settings, t),
    };
    let close = IconButton::new(&icons::CLOSE)
        .variant(IconButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .label("Close")
        .on_press_tx(|s: &mut Workspace, tx: &mut Tx| s.close_dialog(tx))
        .build(&t.k)
        .key("close");
    let header = row()
        .w_full()
        .px(16.0)
        .py(10.0)
        .gap(8.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
        .child(ui::icon_el(&icons::SETTINGS, 15.0, t.muted))
        .child(
            text("Settings")
                .font(t.semibold(t.text))
                .color(t.foreground)
                .grow(1.0),
        )
        .child(close);
    Modal::new("settings", "Settings")
        .width(760.0)
        .flush()
        .child(
            col().key("settings").w_full().child(header).child(
                row().w_full().h(height).child(nav(settings, t)).child(
                    col()
                        .key("settings-page")
                        .role("tabpanel")
                        .label(settings.page.title())
                        .grow(1.0)
                        .min_w(0.0)
                        .h_full()
                        .child(page_header(settings, t))
                        .child(col().w_full().grow(1.0).min_h(0.0).pb(12.0).child(page)),
                ),
            ),
        )
        .build(t, layer, |s: &mut Workspace, tx: &mut Tx| {
            s.close_dialog(tx)
        });
    layer.extend(popups);
}

#[cfg(test)]
mod tests {
    use rune::Chord;

    use super::{FontSlot, Fonts, LOADING_FONTS, Loaded, SettingsFile, ThemeMode, font_options};
    use crate::fonts::{Family, Installed};
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
        file.save(ThemeMode::Light, &Fonts::default(), &keymap)
            .unwrap();

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
        file.save(ThemeMode::Light, &Fonts::default(), &keymap)
            .unwrap();
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
        assert!(
            file.save(ThemeMode::Dark, &Fonts::default(), &keymap)
                .is_err()
        );
        let data = std::fs::read_to_string(dir.path().join("settings.toml")).unwrap();
        assert_eq!(data, "theme = ");
    }

    fn fonts(interface: &str, code: &str) -> Fonts {
        Fonts {
            interface: interface.to_string(),
            code: code.to_string(),
        }
    }

    #[test]
    fn fonts_survive_a_save_and_a_load_and_defaults_are_left_out() {
        let (dir, file) = file(None);
        let keymap = Keymap::default();
        let chosen = fonts("Helvetica Neue", "JetBrains Mono, Menlo");
        file.save(ThemeMode::System, &chosen, &keymap).unwrap();
        assert_eq!(file.fonts(), chosen);
        let loaded = file.load(&mut Keymap::default()).unwrap();
        assert_eq!(loaded.fonts, chosen);
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);

        let only_code = fonts(Fonts::default().interface.as_str(), "Menlo");
        file.save(ThemeMode::System, &only_code, &keymap).unwrap();
        let data = std::fs::read_to_string(dir.path().join("settings.toml")).unwrap();
        assert!(data.contains("[fonts]\ncode = \"Menlo\"\n"), "{data}");
        assert!(!data.contains("interface"), "{data}");
        assert_eq!(file.fonts(), only_code);

        file.save(ThemeMode::System, &Fonts::default(), &keymap)
            .unwrap();
        let data = std::fs::read_to_string(dir.path().join("settings.toml")).unwrap();
        assert!(!data.contains("fonts"), "{data}");
    }

    #[test]
    fn saving_fonts_keeps_the_comments_in_their_table() {
        let (dir, file) = file(Some(
            "theme = \"dark\"\n\n[fonts]\n# for long nights\ncode = \"Monaco\" # thin\n",
        ));
        let keymap = Keymap::default();
        assert_eq!(file.fonts().code, "Monaco");
        file.save(ThemeMode::Dark, &fonts("Avenir Next", "Monaco"), &keymap)
            .unwrap();
        let data = std::fs::read_to_string(dir.path().join("settings.toml")).unwrap();
        assert!(data.contains("# for long nights\n"), "{data}");
        assert!(data.contains("code = \"Monaco\""), "{data}");
        assert!(data.contains("interface = \"Avenir Next\""), "{data}");
    }

    #[test]
    fn bad_fonts_are_reported_and_fall_back_to_the_defaults() {
        let (_dir, bad) = file(Some(
            "[fonts]\ninterface = \"  \"\ncode = 12\nheadings = \"Georgia\"\n",
        ));
        let loaded = bad.load(&mut Keymap::default()).unwrap();
        assert_eq!(loaded.fonts, Fonts::default());
        assert_eq!(loaded.problems.len(), 3, "{:?}", loaded.problems);

        let (_dir, not_a_table) = file(Some("fonts = \"Menlo\"\n"));
        let loaded = not_a_table.load(&mut Keymap::default()).unwrap();
        assert_eq!(loaded.fonts, Fonts::default());
        assert_eq!(loaded.problems.len(), 1, "{:?}", loaded.problems);
    }

    fn installed() -> Installed {
        let family = |name: &str, mono: bool| Family {
            name: name.to_string(),
            mono,
        };
        Installed {
            families: vec![
                family("Arial", false),
                family("Fira Code", true),
                family("Georgia", false),
                family("Menlo", true),
            ],
        }
    }

    fn rows(options: &[super::ComboboxOption]) -> Vec<(&str, &str)> {
        options
            .iter()
            .map(|option| (option.label.as_str(), option.detail.as_str()))
            .collect()
    }

    #[test]
    fn the_code_picker_lists_monospaced_fonts_first() {
        let installed = installed();
        let options = font_options(FontSlot::Code, "Menlo", "", Some(&installed), true);
        assert_eq!(
            rows(&options),
            [
                ("System monospace (default)", ""),
                ("Menlo", ""),
                ("Fira Code", ""),
                ("Arial", "Proportional"),
                ("Georgia", "Proportional"),
            ]
        );
        let options = font_options(FontSlot::Interface, "Inter", "", Some(&installed), true);
        assert_eq!(
            rows(&options),
            [
                ("Inter (default)", ""),
                ("Arial", ""),
                ("Fira Code", "Monospace"),
                ("Georgia", ""),
                ("Menlo", "Monospace"),
            ]
        );
    }

    #[test]
    fn a_typed_name_is_offered_unless_it_is_listed() {
        let installed = installed();
        let typed = |query: &str| {
            font_options(FontSlot::Code, "Menlo", query, Some(&installed), true)
                .into_iter()
                .find(|option| option.label.starts_with("Use"))
                .map(|option| (option.id, option.detail))
        };
        assert_eq!(typed("fira code"), None, "it's listed already");
        assert_eq!(
            typed(" Iosevka "),
            Some(("Iosevka".into(), "Custom".into()))
        );
        assert_eq!(
            typed("Iosevka, Menlo"),
            Some(("Iosevka, Menlo".into(), "Font list".into()))
        );
    }

    #[test]
    fn until_the_scan_is_done_the_os_fonts_are_offered() {
        let options = font_options(FontSlot::Code, "Menlo", "", None, true);
        let loading = options.last().unwrap();
        assert_eq!(loading.id, LOADING_FONTS);
        assert!(loading.disabled);
        assert!(options.len() > 2, "{:?}", rows(&options));

        let installed = Installed::default();
        let options = font_options(FontSlot::Code, "Menlo", "", Some(&installed), true);
        assert!(
            options.len() > 2 && options.iter().all(|option| !option.disabled),
            "a scan that found nothing falls back to the OS fonts"
        );
    }

    #[test]
    fn a_closed_picker_only_needs_the_current_font() {
        let installed = installed();
        let options = font_options(FontSlot::Code, "Fira Code", "", Some(&installed), false);
        assert_eq!(
            rows(&options),
            [("System monospace (default)", ""), ("Fira Code", "")]
        );
    }
}
