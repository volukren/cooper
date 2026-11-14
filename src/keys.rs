use std::cell::RefCell;

use rune::{Chord, ChordKey, ChordMod, Key, KeyEvent, KeyPlatform, Tx};

use crate::workspace::Workspace;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    Quit,
    ShowHelp,
    ShowSettings,
    CloseTab,
    NextTab,
    PrevTab,
    ActivateTab(usize),
    GoHome,
    NewConnection,
    ToggleTheme,
    CancelConnect,
    Connect,
    ClearSearch,
    EditConnection,
    DeleteConnection,
    DeleteForward,
    SelectNext,
    SelectPrev,
    Refresh,
    OpenConsole,
    FocusTree,
    FocusNextPane,
    FocusPrevPane,
    ShowDdl,
    NextPage,
    PrevPage,
    FirstPage,
    LastPage,
    ToggleRecord,
    CloseRecord,
    NextRecord,
    PrevRecord,
    CopyCell,
    CopyRow,
    EditRow,
    SaveRow,
    CancelEdit,
    NextField,
    PrevField,
    RunScript,
    CancelScript,
    FocusResult,
    FocusEditor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ctx {
    Global,
    Workspace,
    Home,
    Connecting,
    Help,
    HelpInput,
    Browser,
    BrowserInput,
    SchemaTree,
    TableView,
    TableGrid,
    RowEditorInput,
    Console,
    ConsoleGrid,
    ConsoleResult,
    SqlEditorInput,
}

pub struct Binding {
    pub chord: Chord,
    pub ctx: Ctx,
    pub cmd: Command,
    slot: usize,
}

const TABLE: &[(&str, Ctx, Command)] = {
    use Command::*;
    use Ctx::{
        Browser as B, BrowserInput as BI, Console as C, ConsoleGrid as CG, ConsoleResult as CR,
        Global as G, HelpInput as HI, Home as H, RowEditorInput as REI, SchemaTree as ST,
        SqlEditorInput as SEI, TableGrid as TG, TableView as TV, Workspace as W,
    };
    &[
        ("mod+q", G, Quit),
        ("mod+/", G, ShowHelp),
        ("f1", G, ShowHelp),
        ("mod+,", G, ShowSettings),
        ("mod+w", W, CloseTab),
        ("ctrl+tab", W, NextTab),
        ("mod+shift+]", W, NextTab),
        ("ctrl+shift+tab", W, PrevTab),
        ("mod+shift+[", W, PrevTab),
        ("mod+1", W, GoHome),
        ("mod+2", W, ActivateTab(2)),
        ("mod+3", W, ActivateTab(3)),
        ("mod+4", W, ActivateTab(4)),
        ("mod+5", W, ActivateTab(5)),
        ("mod+6", W, ActivateTab(6)),
        ("mod+7", W, ActivateTab(7)),
        ("mod+8", W, ActivateTab(8)),
        ("mod+9", W, ActivateTab(9)),
        ("mod+n", W, NewConnection),
        ("mod+shift+t", W, ToggleTheme),
        ("enter", H, Connect),
        ("mod+e", H, EditConnection),
        ("delete", H, DeleteForward),
        ("mod+backspace", H, DeleteConnection),
        ("ctrl+u", HI, ClearSearch),
        ("down", H, SelectNext),
        ("up", H, SelectPrev),
        ("mod+r", B, Refresh),
        ("f5", B, Refresh),
        ("mod+e", B, OpenConsole),
        ("mod+p", B, FocusTree),
        ("f6", B, FocusNextPane),
        ("shift+f6", B, FocusPrevPane),
        ("mod+.", B, CancelScript),
        ("mod+.", BI, CancelScript),
        ("mod+d", ST, ShowDdl),
        ("mod+]", TV, NextPage),
        ("mod+[", TV, PrevPage),
        ("mod+alt+[", TV, FirstPage),
        ("mod+alt+]", TV, LastPage),
        ("space", TV, ToggleRecord),
        ("enter", TV, ToggleRecord),
        ("escape", TV, CloseRecord),
        ("alt+down", TV, NextRecord),
        ("alt+up", TV, PrevRecord),
        ("mod+c", TV, CopyCell),
        ("mod+shift+c", TV, CopyRow),
        ("escape", TG, CloseRecord),
        ("f2", TV, EditRow),
        ("mod+enter", TV, SaveRow),
        ("mod+enter", REI, SaveRow),
        ("mod+s", TV, SaveRow),
        ("mod+s", REI, SaveRow),
        ("escape", REI, CancelEdit),
        ("tab", REI, NextField),
        ("shift+tab", REI, PrevField),
        ("space", CG, ToggleRecord),
        ("escape", C, CloseRecord),
        ("escape", CG, CloseRecord),
        ("alt+down", C, NextRecord),
        ("alt+up", C, PrevRecord),
        ("mod+c", CG, CopyCell),
        ("mod+shift+c", CG, CopyRow),
        ("mod+c", CR, CopyCell),
        ("mod+shift+c", CR, CopyRow),
        ("mod+enter", C, RunScript),
        ("f5", C, RunScript),
        ("mod+enter", SEI, RunScript),
        ("mod+.", C, CancelScript),
        ("mod+j", C, FocusResult),
        ("mod+j", CR, FocusEditor),
        ("escape", Ctx::Connecting, CancelConnect),
    ]
};

pub struct Action {
    pub id: &'static str,
    pub title: &'static str,
    pub group: &'static str,
    pub cmd: Command,
}

const fn action(
    group: &'static str,
    id: &'static str,
    title: &'static str,
    cmd: Command,
) -> Action {
    Action {
        id,
        title,
        group,
        cmd,
    }
}

pub const ACTIONS: &[Action] = {
    use Command::*;
    const GENERAL: &str = "General";
    const TABS: &str = "Tabs";
    const CONNECTIONS: &str = "Connections";
    const BROWSER: &str = "Database browser";
    const DATA: &str = "Table data";
    const CONSOLE: &str = "SQL console";
    &[
        action(GENERAL, "quit", "Quit Cooper", Quit),
        action(GENERAL, "show-help", "Show keyboard shortcuts", ShowHelp),
        action(GENERAL, "show-settings", "Open settings", ShowSettings),
        action(
            GENERAL,
            "toggle-theme",
            "Switch between light and dark",
            ToggleTheme,
        ),
        action(
            GENERAL,
            "clear-search",
            "Clear the help search",
            ClearSearch,
        ),
        action(TABS, "go-home", "Go to the connection list", GoHome),
        action(TABS, "tab-2", "Go to tab 2", ActivateTab(2)),
        action(TABS, "tab-3", "Go to tab 3", ActivateTab(3)),
        action(TABS, "tab-4", "Go to tab 4", ActivateTab(4)),
        action(TABS, "tab-5", "Go to tab 5", ActivateTab(5)),
        action(TABS, "tab-6", "Go to tab 6", ActivateTab(6)),
        action(TABS, "tab-7", "Go to tab 7", ActivateTab(7)),
        action(TABS, "tab-8", "Go to tab 8", ActivateTab(8)),
        action(TABS, "tab-9", "Go to tab 9", ActivateTab(9)),
        action(TABS, "next-tab", "Next tab", NextTab),
        action(TABS, "prev-tab", "Previous tab", PrevTab),
        action(TABS, "close-tab", "Disconnect and close the tab", CloseTab),
        action(
            CONNECTIONS,
            "new-connection",
            "New connection",
            NewConnection,
        ),
        action(CONNECTIONS, "connect", "Connect", Connect),
        action(
            CONNECTIONS,
            "edit-connection",
            "Edit the connection",
            EditConnection,
        ),
        action(
            CONNECTIONS,
            "delete-connection",
            "Delete the connection",
            DeleteConnection,
        ),
        action(
            CONNECTIONS,
            "delete-forward",
            "Delete the connection (or the next character of the search)",
            DeleteForward,
        ),
        action(
            CONNECTIONS,
            "select-next",
            "Select the next connection",
            SelectNext,
        ),
        action(
            CONNECTIONS,
            "select-prev",
            "Select the previous connection",
            SelectPrev,
        ),
        action(
            CONNECTIONS,
            "cancel-connect",
            "Cancel connecting",
            CancelConnect,
        ),
        action(BROWSER, "refresh", "Refresh the tree and the grid", Refresh),
        action(BROWSER, "open-console", "Open the SQL console", OpenConsole),
        action(BROWSER, "focus-tree", "Focus the schema tree", FocusTree),
        action(
            BROWSER,
            "focus-next-pane",
            "Focus the next pane",
            FocusNextPane,
        ),
        action(
            BROWSER,
            "focus-prev-pane",
            "Focus the previous pane",
            FocusPrevPane,
        ),
        action(BROWSER, "show-ddl", "Show the table's DDL", ShowDdl),
        action(DATA, "next-page", "Next page", NextPage),
        action(DATA, "prev-page", "Previous page", PrevPage),
        action(DATA, "first-page", "First page", FirstPage),
        action(DATA, "last-page", "Last page", LastPage),
        action(
            DATA,
            "toggle-record",
            "Show or hide the row view",
            ToggleRecord,
        ),
        action(DATA, "close-record", "Close the row view", CloseRecord),
        action(DATA, "next-record", "Next row in the row view", NextRecord),
        action(
            DATA,
            "prev-record",
            "Previous row in the row view",
            PrevRecord,
        ),
        action(DATA, "copy-cell", "Copy the selected cell", CopyCell),
        action(DATA, "copy-row", "Copy the selected row", CopyRow),
        action(DATA, "edit-row", "Edit the selected value", EditRow),
        action(DATA, "save-row", "Save the changed values", SaveRow),
        action(DATA, "cancel-edit", "Discard the changes", CancelEdit),
        action(DATA, "next-field", "Edit the next value", NextField),
        action(DATA, "prev-field", "Edit the previous value", PrevField),
        action(CONSOLE, "run-script", "Run the script", RunScript),
        action(
            CONSOLE,
            "cancel-script",
            "Stop the running script",
            CancelScript,
        ),
        action(
            CONSOLE,
            "focus-result",
            "Focus the result grid",
            FocusResult,
        ),
        action(CONSOLE, "focus-editor", "Focus the SQL editor", FocusEditor),
    ]
};

pub fn action_of(cmd: Command) -> Option<&'static Action> {
    ACTIONS.iter().find(|action| action.cmd == cmd)
}

fn default_specs(cmd: Command) -> Vec<&'static str> {
    let mut specs = Vec::new();
    for &(spec, _, other) in TABLE {
        if other == cmd && !specs.contains(&spec) {
            specs.push(spec);
        }
    }
    specs
}

fn default_chord(spec: &str) -> Chord {
    Chord::parse(spec).unwrap_or_else(|err| panic!("binding {spec:?}: {err}"))
}

fn overlap(a: Ctx, b: Ctx) -> bool {
    let wide = |ctx| matches!(ctx, Ctx::Global | Ctx::Workspace);
    a == b || wide(a) || wide(b)
}

pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        let bindings = TABLE
            .iter()
            .map(|&(spec, ctx, cmd)| Binding {
                chord: default_chord(spec),
                ctx,
                cmd,
                slot: default_specs(cmd)
                    .iter()
                    .position(|&other| other == spec)
                    .unwrap_or(0),
            })
            .collect();
        Self { bindings }
    }
}

impl Keymap {
    pub fn chords(&self, cmd: Command) -> Vec<Chord> {
        let mut chords: Vec<Option<Chord>> = vec![None; default_specs(cmd).len()];
        for binding in self.bindings.iter().filter(|b| b.cmd == cmd) {
            chords[binding.slot].get_or_insert(binding.chord);
        }
        chords.into_iter().flatten().collect()
    }

    pub fn set(&mut self, cmd: Command, slot: usize, chord: Chord) {
        for binding in &mut self.bindings {
            if binding.cmd == cmd && binding.slot == slot {
                binding.chord = chord;
            }
        }
    }

    pub fn reset(&mut self, cmd: Command) {
        for (slot, spec) in default_specs(cmd).into_iter().enumerate() {
            self.set(cmd, slot, default_chord(spec));
        }
    }

    pub fn reset_all(&mut self) {
        *self = Self::default();
    }

    pub fn is_modified(&self, cmd: Command) -> bool {
        let defaults = default_specs(cmd);
        self.bindings
            .iter()
            .any(|b| b.cmd == cmd && b.chord != default_chord(defaults[b.slot]))
    }

    pub fn any_modified(&self) -> bool {
        ACTIONS.iter().any(|action| self.is_modified(action.cmd))
    }

    pub fn overrides(&self) -> Vec<(&'static str, Vec<String>)> {
        ACTIONS
            .iter()
            .filter(|action| self.is_modified(action.cmd))
            .map(|action| {
                let chords = self.chords(action.cmd);
                (action.id, chords.iter().map(Chord::to_string).collect())
            })
            .collect()
    }

    pub fn apply(&mut self, id: &str, specs: &[String]) -> Result<(), String> {
        let action = ACTIONS
            .iter()
            .find(|action| action.id == id)
            .ok_or_else(|| format!("unknown shortcut \"{id}\""))?;
        let slots = default_specs(action.cmd).len();
        if specs.len() > slots {
            return Err(format!("\"{id}\" takes at most {slots} shortcuts"));
        }
        let chords = specs
            .iter()
            .map(|spec| Chord::parse(spec).map_err(|err| format!("\"{id}\": {err}")))
            .collect::<Result<Vec<_>, _>>()?;
        for (slot, chord) in chords.into_iter().enumerate() {
            self.set(action.cmd, slot, chord);
        }
        Ok(())
    }

    pub fn conflicts(&self, cmd: Command, slot: usize) -> Vec<&'static str> {
        let platform = KeyPlatform::current();
        let mut titles = Vec::new();
        for mine in self
            .bindings
            .iter()
            .filter(|b| b.cmd == cmd && b.slot == slot)
        {
            for other in &self.bindings {
                if other.cmd != cmd
                    && overlap(mine.ctx, other.ctx)
                    && mine.chord.same_keys(&other.chord, platform)
                    && let Some(action) = action_of(other.cmd)
                    && !titles.contains(&action.title)
                {
                    titles.push(action.title);
                }
            }
        }
        titles
    }

    fn matching(&self, ctx: Ctx, ev: &KeyEvent, tx: &Tx) -> Vec<Command> {
        self.bindings
            .iter()
            .filter(|b| b.ctx == ctx && b.chord.matches_tx(ev, tx))
            .map(|b| b.cmd)
            .collect()
    }
}

thread_local! {
    static KEYMAP: RefCell<Keymap> = RefCell::new(Keymap::default());
}

pub fn with<R>(f: impl FnOnce(&Keymap) -> R) -> R {
    KEYMAP.with_borrow(f)
}

pub fn update<R>(f: impl FnOnce(&mut Keymap) -> R) -> R {
    KEYMAP.with_borrow_mut(f)
}

pub fn label(cmd: Command) -> Option<String> {
    with(|keymap| {
        let binding = keymap.bindings.iter().find(|b| b.cmd == cmd)?;
        Some(binding.chord.label())
    })
}

pub fn label_in(cmd: Command, ctx: Ctx) -> Option<String> {
    with(|keymap| {
        let binding = keymap
            .bindings
            .iter()
            .find(|b| b.cmd == cmd && b.ctx == ctx)?;
        Some(binding.chord.label())
    })
}

pub fn chord_of(cmd: Command, slot: usize) -> Option<Chord> {
    with(|keymap| keymap.chords(cmd).get(slot).copied())
}

pub fn chord_from_event(ev: &KeyEvent, tx: &Tx) -> Option<Chord> {
    let key = match &ev.key {
        Key::Command(c) => ChordKey::Char(*c),
        Key::Text(text) => {
            let mut chars = text.chars();
            let (Some(typed), None) = (chars.next(), chars.next()) else {
                return None;
            };
            ChordKey::Char(tx.key_base().unwrap_or(typed))
        }
        Key::Enter => ChordKey::Enter,
        Key::Tab => ChordKey::Tab,
        Key::Escape => ChordKey::Escape,
        Key::Backspace => ChordKey::Backspace,
        Key::Delete => ChordKey::Delete,
        Key::Insert => ChordKey::Insert,
        Key::Home => ChordKey::Home,
        Key::End => ChordKey::End,
        Key::PageUp => ChordKey::PageUp,
        Key::PageDown => ChordKey::PageDown,
        Key::Left => ChordKey::Left,
        Key::Right => ChordKey::Right,
        Key::Up => ChordKey::Up,
        Key::Down => ChordKey::Down,
        Key::Function(n) => ChordKey::Function(*n),
        _ => return None,
    };
    let mac = KeyPlatform::current() == KeyPlatform::Mac;
    let mods = ev.mods;
    let held = [
        (
            if mac {
                mods.cmd || mods.logo
            } else {
                mods.cmd || mods.ctrl
            },
            ChordMod::Primary,
        ),
        (mac && mods.ctrl, ChordMod::Ctrl),
        (!mac && mods.logo, ChordMod::Super),
        (mods.alt, ChordMod::Alt),
        (mods.shift, ChordMod::Shift),
    ];
    Some(
        held.into_iter()
            .filter(|(on, _)| *on)
            .fold(Chord::new(key), |chord, (_, m)| chord.with(m)),
    )
}

pub fn needs_modifier(chord: &Chord) -> bool {
    let modified = [
        ChordMod::Primary,
        ChordMod::Ctrl,
        ChordMod::Alt,
        ChordMod::Super,
    ]
    .into_iter()
    .any(|m| chord.has(m));
    !modified && matches!(chord.key(), ChordKey::Char(_))
}

pub fn gk_spec(spec: &str) -> String {
    let (body, minus) = match spec.strip_suffix("--") {
        Some(body) => (body, true),
        None if spec == "-" => ("", true),
        None => (spec, false),
    };
    let mut parts: Vec<&str> = if body.is_empty() {
        Vec::new()
    } else {
        body.split('-').collect()
    };
    if minus {
        parts.push("minus");
    }
    parts
        .into_iter()
        .map(|part| if part == "cmd" { "mod" } else { part })
        .collect::<Vec<_>>()
        .join("+")
}

pub fn gk_chord(spec: &str) -> Option<Chord> {
    Chord::parse(&gk_spec(spec)).ok()
}

pub fn contexts(s: &Workspace, focused: Option<&str>) -> Vec<Ctx> {
    let mut out = Vec::new();
    s.contexts(focused, &mut out);
    out.push(Ctx::Global);
    out
}

pub fn is_modifier(ev: &KeyEvent) -> bool {
    matches!(ev.key, Key::Modifier { .. })
}

pub fn route(s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if is_modifier(ev) {
        return false;
    }
    tx.wake_after(std::time::Duration::ZERO);
    if crate::settings::record_key(s, ev, tx) {
        return true;
    }
    if s.before_key(ev, tx) {
        s.sync_title(tx);
        return true;
    }
    let focused = tx.focused().map(str::to_owned);
    for ctx in contexts(s, focused.as_deref()) {
        for cmd in with(|keymap| keymap.matching(ctx, ev, tx)) {
            if s.run(cmd, tx) {
                return true;
            }
        }
    }
    false
}

pub fn field_key(s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx) -> bool {
    route(s, ev, tx)
}

pub fn app_on_key(s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx) -> bool {
    route(s, ev, tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(spec: &str) -> Chord {
        Chord::parse(spec).unwrap()
    }

    #[test]
    fn every_binding_parses_and_has_an_action() {
        let keymap = Keymap::default();
        assert_eq!(keymap.bindings.len(), TABLE.len());
        for &(spec, _, cmd) in TABLE {
            assert!(action_of(cmd).is_some(), "{spec} has no action");
        }
        for action in ACTIONS {
            assert!(!default_specs(action.cmd).is_empty(), "{}", action.id);
            let same = ACTIONS.iter().filter(|other| other.id == action.id);
            assert_eq!(same.count(), 1, "{}", action.id);
        }
    }

    #[test]
    fn the_defaults_are_unmodified_and_free_of_conflicts() {
        let keymap = Keymap::default();
        assert!(!keymap.any_modified());
        assert!(keymap.overrides().is_empty());
        for action in ACTIONS {
            for slot in 0..keymap.chords(action.cmd).len() {
                let conflicts = keymap.conflicts(action.cmd, slot);
                assert!(conflicts.is_empty(), "{}: {conflicts:?}", action.id);
            }
        }
    }

    #[test]
    fn a_slot_changes_in_every_context_and_resets() {
        let mut keymap = Keymap::default();
        keymap.set(Command::CancelScript, 0, chord("mod+k"));
        let changed = keymap
            .bindings
            .iter()
            .filter(|b| b.cmd == Command::CancelScript);
        assert!(changed.clone().count() > 1);
        assert!(changed.clone().all(|b| b.chord == chord("mod+k")));
        assert!(keymap.is_modified(Command::CancelScript));
        assert_eq!(
            keymap.overrides(),
            [("cancel-script", vec!["mod+k".to_string()])]
        );

        keymap.set(Command::ShowHelp, 1, chord("f12"));
        assert_eq!(
            keymap.chords(Command::ShowHelp),
            [chord("mod+/"), chord("f12")]
        );
        keymap.reset(Command::ShowHelp);
        assert!(!keymap.is_modified(Command::ShowHelp));
        keymap.reset_all();
        assert!(!keymap.any_modified());
    }

    #[test]
    fn overrides_apply_by_action_id() {
        let mut keymap = Keymap::default();
        keymap
            .apply("show-help", &["mod+h".into(), "f12".into()])
            .unwrap();
        assert_eq!(
            keymap.chords(Command::ShowHelp),
            [chord("mod+h"), chord("f12")]
        );
        keymap.apply("quit", &["ctrl+q".into()]).unwrap();
        assert_eq!(keymap.chords(Command::Quit), [chord("ctrl+q")]);

        assert!(keymap.apply("nope", &["mod+h".into()]).is_err());
        assert!(keymap.apply("quit", &["mod+".into()]).is_err());
        assert!(
            keymap
                .apply("quit", &["mod+h".into(), "f12".into()])
                .is_err()
        );
        assert_eq!(keymap.chords(Command::Quit), [chord("ctrl+q")]);
    }

    #[test]
    fn a_chord_taken_by_an_overlapping_context_conflicts() {
        let mut keymap = Keymap::default();
        keymap.set(Command::NewConnection, 0, chord("mod+w"));
        assert_eq!(
            keymap.conflicts(Command::NewConnection, 0),
            ["Disconnect and close the tab"]
        );
        keymap.set(Command::EditConnection, 0, chord("mod+r"));
        assert!(keymap.conflicts(Command::EditConnection, 0).is_empty());
    }

    #[test]
    fn a_bare_character_needs_a_modifier() {
        assert!(needs_modifier(&chord("k")));
        assert!(needs_modifier(&chord("shift+k")));
        assert!(!needs_modifier(&chord("mod+k")));
        assert!(!needs_modifier(&chord("alt+k")));
        assert!(!needs_modifier(&chord("f7")));
        assert!(!needs_modifier(&chord("enter")));
    }

    #[test]
    fn gk_specs_convert_to_chords() {
        assert_eq!(gk_spec("cmd-shift-]"), "mod+shift+]");
        assert_eq!(gk_spec("cmd-alt-["), "mod+alt+[");
        assert_eq!(gk_spec("shift-f6"), "shift+f6");
        assert_eq!(gk_spec("cmd--"), "mod+minus");
        assert_eq!(gk_spec("pageup"), "pageup");
        for spec in [
            "cmd-q",
            "cmd-/",
            "ctrl-u",
            "alt-backspace",
            "cmd-.",
            "space",
            "f1",
        ] {
            assert!(gk_chord(spec).is_some(), "{spec}");
        }
    }
}
