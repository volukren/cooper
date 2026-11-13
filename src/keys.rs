use std::sync::LazyLock;

use rune::{Chord, Key, KeyEvent, Tx};

use crate::workspace::Workspace;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    Quit,
    ShowHelp,
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
    FocusSavedQueries,
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
    SaveQuery,
    SaveQueryAs,
    NewQuery,
    OpenQuery,
    RunQuery,
    DeleteQuery,
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
    SavedQueries,
    SavedQueriesInput,
    TableView,
    TableGrid,
    RowEditorInput,
    Console,
    ConsoleGrid,
    ConsoleResult,
    SqlEditorInput,
}

pub struct Binding {
    pub spec: &'static str,
    pub chord: Chord,
    pub ctx: Ctx,
    pub cmd: Command,
}

const TABLE: &[(&str, Ctx, Command)] = {
    use Command::*;
    use Ctx::{
        Browser as B, BrowserInput as BI, Console as C, ConsoleGrid as CG, ConsoleResult as CR,
        Global as G, HelpInput as HI, Home as H, RowEditorInput as REI, SavedQueries as SQ,
        SavedQueriesInput as SQI, SchemaTree as ST, SqlEditorInput as SEI, TableGrid as TG,
        TableView as TV, Workspace as W,
    };
    &[
        ("mod+q", G, Quit),
        ("mod+/", G, ShowHelp),
        ("f1", G, ShowHelp),
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
        ("mod+o", B, FocusSavedQueries),
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
        ("mod+s", C, SaveQuery),
        ("mod+shift+s", C, SaveQueryAs),
        ("mod+shift+n", C, NewQuery),
        ("enter", SQ, OpenQuery),
        ("f5", SQ, RunQuery),
        ("mod+enter", SQ, RunQuery),
        ("mod+enter", SQI, RunQuery),
        ("mod+backspace", SQ, DeleteQuery),
        ("escape", Ctx::Connecting, CancelConnect),
    ]
};

pub static BINDINGS: LazyLock<Vec<Binding>> = LazyLock::new(|| {
    TABLE
        .iter()
        .map(|&(spec, ctx, cmd)| Binding {
            spec,
            chord: Chord::parse(spec).unwrap_or_else(|err| panic!("binding {spec:?}: {err}")),
            ctx,
            cmd,
        })
        .collect()
});

pub fn binding(cmd: Command) -> Option<&'static Binding> {
    BINDINGS.iter().find(|b| b.cmd == cmd)
}

pub fn label(cmd: Command) -> Option<String> {
    binding(cmd).map(|b| b.chord.label())
}

pub fn label_in(cmd: Command, ctx: Ctx) -> Option<String> {
    BINDINGS
        .iter()
        .find(|b| b.cmd == cmd && b.ctx == ctx)
        .map(|b| b.chord.label())
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
    if s.before_key(ev, tx) {
        s.sync_title(tx);
        return true;
    }
    let focused = tx.focused().map(str::to_owned);
    for ctx in contexts(s, focused.as_deref()) {
        for b in BINDINGS.iter().filter(|b| b.ctx == ctx) {
            if b.chord.matches_tx(ev, tx) && s.run(b.cmd, tx) {
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

    #[test]
    fn every_binding_parses_and_the_table_matches_cooper() {
        let gk = TABLE.iter().filter(|(_, ctx, _)| *ctx != Ctx::Connecting);
        assert_eq!(gk.count(), 79);
        assert_eq!(BINDINGS.len(), TABLE.len());
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

    #[test]
    fn labels_use_mac_glyphs() {
        let quit = label(Command::Quit).unwrap();
        assert!(quit.contains('Q'), "{quit}");
        assert!(label(Command::ActivateTab(3)).is_some());
        assert!(label_in(Command::CopyCell, Ctx::ConsoleResult).is_some());
    }
}
