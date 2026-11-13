use rune::{Cx, El, KeyEvent, TextEdit, Tx, col, rich, row, text};
use rune_kit::TextInput;

use crate::{
    dialog::Dialog,
    icons,
    keys::{self, Ctx},
    theme::Theme,
    ui::{self, Modal, Scroll, ScrollY},
    workspace::{self, Workspace},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Stroke(&'static str),
    Text(&'static str),
}

struct Binding {
    keys: &'static [Key],
    description: &'static str,
}

struct Section {
    title: &'static str,
    bindings: &'static [Binding],
}

const fn b(keys: &'static [Key], description: &'static str) -> Binding {
    Binding { keys, description }
}

use Key::{Stroke as K, Text as T};

const ELLIPSIS: Key = T("…");

const SECTIONS: &[Section] = &[
    Section {
        title: "Global",
        bindings: &[
            b(
                &[K("cmd-q")],
                "Quit the application (closing the window quits too)",
            ),
            b(&[K("cmd-/"), K("f1")], "Show or hide this help"),
            b(
                &[K("cmd-shift-t")],
                "Switch between the dark and the light theme",
            ),
            b(
                &[K("tab"), K("shift-tab")],
                "Move the focus to the next / previous control",
            ),
            b(&[T("Click")], "Focus a pane, select a row or a field"),
            b(
                &[T("Drag")],
                "Select text in editors, fields and text views",
            ),
            b(&[K("cmd-c")], "Copy the selected text"),
            b(&[T("Wheel")], "Scroll the pane under the pointer"),
        ],
    },
    Section {
        title: "Tabs",
        bindings: &[
            b(&[K("cmd-1")], "Go to the connection list (tab 1)"),
            b(
                &[K("cmd-2"), ELLIPSIS, K("cmd-9")],
                "Go to the connection tab with that number (2 is the first)",
            ),
            b(&[K("ctrl-tab"), K("cmd-shift-]")], "Next tab"),
            b(&[K("ctrl-shift-tab"), K("cmd-shift-[")], "Previous tab"),
            b(
                &[T("Click a tab")],
                "Switch to it; click its × to disconnect and close it",
            ),
            b(&[K("cmd-w")], "Disconnect and close the current tab"),
        ],
    },
    Section {
        title: "Connections",
        bindings: &[
            b(&[K("down")], "Select the next connection"),
            b(&[K("up")], "Select the previous connection"),
            b(&[T("Type")], "Search the connections by name or type"),
            b(
                &[K("cmd-v")],
                "Paste into the search (also from the list; control characters are dropped)",
            ),
            b(&[K("escape"), K("ctrl-u")], "Clear the search"),
            b(&[T("Click")], "Select a connection"),
            b(
                &[K("enter"), T("Double-click")],
                "Connect in a new tab (or go to its open tab)",
            ),
            b(
                &[K("cmd-n")],
                "Create a new connection (also the New connection button)",
            ),
            b(&[K("cmd-e")], "Edit the selected connection"),
            b(
                &[K("cmd-backspace"), K("delete")],
                "Delete the selected connection (asks to confirm; in a search with text, ⌘⌫ \
                 deletes to its start and Delete the character after the caret)",
            ),
        ],
    },
    Section {
        title: "Connection form",
        bindings: &[
            b(
                &[K("tab"), K("shift-tab")],
                "Move to the next / previous field",
            ),
            b(&[T("Type menu")], "Choose PostgreSQL, MySQL or SQLite"),
            b(
                &[T("Store password")],
                "Keep the password in the system keychain or in the config",
            ),
            b(&[T("Eye button")], "Show or hide the password"),
            b(&[T("Browse…")], "Pick the SQLite database file"),
            b(&[K("enter")], "Save the connection (from any field)"),
            b(&[K("escape")], "Cancel and close the form"),
        ],
    },
    Section {
        title: "Database browser",
        bindings: &[
            b(
                &[K("f6"), K("shift-f6")],
                "Cycle the focus: schema tree → data, or SQL editor → result",
            ),
            b(
                &[K("cmd-."), K("escape")],
                "Stop a running script from any pane (the first Esc stops it)",
            ),
            b(&[K("cmd-p")], "Focus the schema tree"),
            b(&[K("up"), K("down")], "Move the selection (tree, grid)"),
            b(
                &[K("left"), K("right")],
                "Collapse / expand a schema (tree), move between cells (grid)",
            ),
            b(
                &[K("enter"), T("Click")],
                "Expand or collapse a schema, open a table (tree)",
            ),
            b(&[T("Type")], "Search the loaded tables by name (tree)"),
            b(
                &[K("cmd-d"), T("Right-click")],
                "Show a table's DDL (tree: the selected table, or Show DDL in its menu)",
            ),
            b(&[K("cmd-]"), K("cmd-[")], "Next / previous page"),
            b(&[K("cmd-alt-["), K("cmd-alt-]")], "First / last page"),
            b(&[T("Pager buttons")], "First, previous, next and last page"),
            b(
                &[K("cmd-r"), K("f5")],
                "Refresh the tree and the grid (F5 runs in the console)",
            ),
            b(
                &[K("space"), K("enter"), T("Double-click")],
                "Show the selected row in full (grid, result; Enter: grid)",
            ),
            b(&[K("cmd-c")], "Copy the selected cell (grid, result)"),
            b(
                &[K("f2")],
                "Edit the selected value in the row view (tables with a primary key)",
            ),
            b(
                &[K("cmd-shift-c")],
                "Copy the selected row as tab-separated values",
            ),
            b(&[K("cmd-e")], "Open the SQL console"),
        ],
    },
    Section {
        title: "Row view",
        bindings: &[
            b(&[T("Wheel")], "Scroll the values"),
            b(&[T("Drag")], "Select a value to copy it"),
            b(&[K("alt-down"), K("alt-up")], "Next / previous row"),
            b(
                &[K("cmd-c"), K("cmd-shift-c")],
                "Copy the cell / the whole row",
            ),
            b(&[K("space"), K("enter"), K("escape")], "Return to the grid"),
        ],
    },
    Section {
        title: "Edit row",
        bindings: &[
            b(
                &[T("Click a value"), K("f2")],
                "Edit the value in place, in the row view (more values: click them too)",
            ),
            b(
                &[T("Click outside")],
                "Back to text (a changed value keeps its change until saved)",
            ),
            b(
                &[K("tab"), K("shift-tab")],
                "Edit the next / previous value",
            ),
            b(
                &[T("NULL box")],
                "Set the value to NULL (typing a value unchecks it)",
            ),
            b(
                &[K("cmd-enter"), K("cmd-s")],
                "Save the changed values (only if the row is still there)",
            ),
            b(&[K("escape")], "Throw the changes away"),
        ],
    },
    Section {
        title: "SQL console",
        bindings: &[
            b(
                &[K("cmd-enter"), K("f5")],
                "Run the script, or only the selected text (editor or result)",
            ),
            b(&[K("cmd-."), K("escape")], "Cancel the running script"),
            b(
                &[K("escape")],
                "Go to the schema tree from the editor or the result grid (no script running)",
            ),
            b(
                &[K("cmd-j")],
                "Move the focus between the editor and the result grid",
            ),
            b(
                &[K("f6"), K("shift-f6")],
                "Cycle the focus: tree → editor → result (once there is one)",
            ),
            b(&[K("cmd-z"), K("cmd-shift-z")], "Undo / redo"),
            b(&[K("cmd-a")], "Select the whole script"),
            b(&[K("cmd-f")], "Find in the script"),
            b(&[K("home"), K("end")], "Start / end of the line"),
            b(&[K("cmd-up"), K("cmd-down")], "Start / end of the script"),
            b(&[K("alt-backspace")], "Delete the previous word"),
            b(&[K("cmd-backspace")], "Delete to the start of the line"),
            b(&[K("up"), K("down")], "Move the selected row (result)"),
            b(
                &[K("space"), T("Double-click")],
                "Show the selected row in full (result)",
            ),
        ],
    },
    Section {
        title: "Help",
        bindings: &[
            b(&[T("Type")], "Filter commands by key or description"),
            b(&[K("ctrl-u"), T("×")], "Clear the search query"),
            b(&[K("up"), K("down")], "Scroll the list"),
            b(&[K("pageup"), K("pagedown")], "Scroll the list by a page"),
            b(&[K("escape")], "Close the help window"),
        ],
    },
];

fn key_label(key: Key) -> String {
    match key {
        Key::Stroke(spec) => match keys::gk_chord(spec) {
            Some(chord) => chord.label(),
            None => spec.to_string(),
        },
        Key::Text(text) => text.to_string(),
    }
}

fn keys_text(keys: &[Key]) -> String {
    keys.iter()
        .map(|&key| match key {
            Key::Stroke(spec) => format!("{spec} {}", key_label(key)),
            Key::Text(text) => text.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn filtered(query: &str) -> Vec<(&'static str, Vec<&'static Binding>)> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    SECTIONS
        .iter()
        .filter_map(|section| {
            let bindings: Vec<&Binding> = section
                .bindings
                .iter()
                .filter(|binding| {
                    let haystack = format!(
                        "{} {} {}",
                        section.title,
                        keys_text(binding.keys),
                        binding.description
                    )
                    .to_lowercase();
                    terms.iter().all(|term| haystack.contains(term.as_str()))
                })
                .collect();
            (!bindings.is_empty()).then_some((section.title, bindings))
        })
        .collect()
}

const LINE_HEIGHT: f32 = 28.0;

pub struct Help {
    pub search: TextEdit,
    pub scroll: ScrollY,
}

impl Help {
    pub fn new() -> Self {
        let mut search = TextEdit::new("");
        search.focused = true;
        Self {
            search,
            scroll: ScrollY::default(),
        }
    }

    pub fn query(&self) -> &str {
        &self.search.text
    }

    pub fn match_count(&self) -> usize {
        filtered(self.query()).iter().map(|(_, b)| b.len()).sum()
    }

    pub fn contexts(&self, _focused: Option<&str>, out: &mut Vec<Ctx>) {
        if self.search.focused {
            out.push(Ctx::HelpInput);
        }
        out.push(Ctx::Help);
    }
}

impl Default for Help {
    fn default() -> Self {
        Self::new()
    }
}

fn help_scroll(s: &Workspace) -> Option<ScrollY> {
    match &s.dialog {
        Some(Dialog::Help(help)) => Some(help.scroll.clone()),
        _ => None,
    }
}

fn to_top(s: &Workspace, tx: &mut Tx) {
    if let Some(scroll) = help_scroll(s) {
        scroll.scroll(tx, "help-list", Scroll::Top, LINE_HEIGHT);
    }
}

pub fn clear_search(s: &mut Workspace, tx: &mut Tx) -> bool {
    let Some(Dialog::Help(help)) = &mut s.dialog else {
        return false;
    };
    help.search.clear();
    help.search.focused = true;
    to_top(s, tx);
    true
}

fn scroll_by(s: &Workspace, tx: &mut Tx, delta: f32) {
    if let Some(scroll) = help_scroll(s) {
        let y = (scroll.offset() + delta).clamp(0.0, scroll.max().max(0.0));
        tx.scroll_to_y("help-list", y);
    }
}

fn page(s: &Workspace) -> f32 {
    help_scroll(s).map_or(LINE_HEIGHT, |scroll| {
        (scroll.viewport() - LINE_HEIGHT).max(LINE_HEIGHT)
    })
}

fn search_key(s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if keys::field_key(s, ev, tx) {
        return true;
    }
    let mods = ev.mods;
    if mods.cmd || mods.ctrl || mods.alt || mods.logo || mods.shift {
        return false;
    }
    match ev.key {
        rune::Key::Up => scroll_by(s, tx, -LINE_HEIGHT),
        rune::Key::Down => scroll_by(s, tx, LINE_HEIGHT),
        rune::Key::PageUp => scroll_by(s, tx, -page(s)),
        rune::Key::PageDown => scroll_by(s, tx, page(s)),
        rune::Key::Enter => {}
        _ => return false,
    }
    true
}

fn chip(spec: &str, t: &Theme) -> El {
    let label = match keys::gk_chord(spec) {
        Some(chord) => chord.label(),
        None => spec.to_string(),
    };
    row()
        .h(20.0)
        .min_w(20.0)
        .px(5.0)
        .center()
        .radius(4.0)
        .border(1.0, t.border)
        .child(text(label).font(t.medium(t.text_xs)).color(t.foreground))
}

fn key_chips(keys: &[Key], t: &Theme) -> El {
    row()
        .gap(4.0)
        .flex_wrap()
        .center_y()
        .children(keys.iter().enumerate().map(|(index, &key)| {
            let separator = index
                .checked_sub(1)
                .is_some_and(|prev| keys[prev] != ELLIPSIS && key != ELLIPSIS)
                .then(|| ui::label("/", t.text_sm, t.muted));
            let chip = match key {
                Key::Stroke(spec) => chip(spec, t),
                Key::Text(words) => rich(
                    words.to_string(),
                    vec![rune::Span::new(0, words.len()).italic()],
                )
                .font(t.text_sm)
                .color(t.muted),
            };
            row().gap(4.0).center_y().children(separator).child(chip)
        }))
}

pub fn dialog(_s: &Workspace, help: &Help, t: &Theme, cx: &mut Cx<Workspace>, layer: &mut Vec<El>) {
    let mut search = TextInput::new(&help.search)
        .placeholder("Search commands…")
        .label("Search commands")
        .leading_icon(&icons::SEARCH)
        .leading_icon_size(14.0)
        .width(None)
        .on_edit(|s: &mut Workspace| {
            workspace::dialog_edit(s, |d| match d {
                Dialog::Help(help) => Some(&mut help.search),
                _ => None,
            })
        })
        .on_key(search_key)
        .on_change(|s: &mut Workspace, tx: &mut Tx| to_top(s, tx));
    if !help.search.text.is_empty() {
        search = search
            .clear_button()
            .on_action(|s: &mut Workspace, tx: &mut Tx| {
                clear_search(s, tx);
            });
    }
    let search = search.build(&t.k).key("help-search");
    let height = cx
        .layout_size("workspace")
        .map_or(560.0, |size| (size[1] - 240.0).clamp(160.0, 560.0));
    let sections = filtered(help.query());
    let body =
        if sections.is_empty() {
            col()
                .key("help-empty")
                .w_full()
                .pt(32.0)
                .items_center()
                .child(ui::label("No matching commands", t.text, t.muted))
        } else {
            col().key("help-sections").w_full().pr(12.0).children(
                sections
                    .into_iter()
                    .enumerate()
                    .map(|(index, (title, bindings))| {
                        col()
                            .w_full()
                            .mt(if index > 0 { 12.0 } else { 0.0 })
                            .child(
                                text(title)
                                    .font(t.bold(t.text_sm))
                                    .color(t.blue)
                                    .w_full()
                                    .pb(4.0)
                                    .mb(4.0)
                                    .border_sides([0.0, 0.0, 1.0, 0.0], t.border),
                            )
                            .children(bindings.into_iter().map(|binding| {
                                row()
                                    .w_full()
                                    .min_h(LINE_HEIGHT)
                                    .gap(12.0)
                                    .center_y()
                                    .child(col().w(220.0).child(key_chips(binding.keys, t)))
                                    .child(
                                        text(binding.description)
                                            .font(t.text_sm)
                                            .color(t.foreground)
                                            .wrap()
                                            .grow(1.0)
                                            .min_w(0.0),
                                    )
                            }))
                    }),
            )
        };
    let list =
        t.k.scroll_bar(ui::scroll_view("help-list", &help.scroll, body));
    let close = rune_kit::IconButton::new(&icons::CLOSE)
        .variant(rune_kit::IconButtonVariant::Ghost)
        .control_size(rune_kit::ControlSize::Small)
        .label("Close")
        .on_press_tx(|s: &mut Workspace, tx: &mut Tx| s.close_dialog(tx))
        .build(&t.k)
        .key("close");
    let title = row()
        .w_full()
        .center_y()
        .child(
            text("Keyboard shortcuts")
                .font(t.semibold(t.text))
                .color(t.foreground)
                .grow(1.0),
        )
        .child(close);
    Modal::new("help", "Keyboard shortcuts")
        .title(title)
        .width(680.0)
        .child(
            col()
                .key("help")
                .w_full()
                .gap(12.0)
                .child(search)
                .child(col().key("help-box").w_full().h(height).child(list)),
        )
        .build(t, layer, |s: &mut Workspace, tx: &mut Tx| {
            s.close_dialog(tx)
        });
}

#[cfg(test)]
mod tests {
    use super::{Key, SECTIONS, filtered, key_label};
    use crate::keys::gk_chord;

    fn count(query: &str) -> usize {
        filtered(query).iter().map(|(_, b)| b.len()).sum()
    }

    #[test]
    fn every_keystroke_parses() {
        for section in SECTIONS {
            for binding in section.bindings {
                for key in binding.keys {
                    if let Key::Stroke(spec) = key {
                        assert!(gk_chord(spec).is_some(), "{spec} in {}", section.title);
                    }
                }
            }
        }
    }

    #[test]
    fn empty_query_shows_everything() {
        let total: usize = SECTIONS.iter().map(|s| s.bindings.len()).sum();
        assert_eq!(count(""), total);
        assert_eq!(count("   "), total);
    }

    #[test]
    fn matches_description_case_insensitively() {
        let sections = filtered("QUIT");
        assert!(sections.iter().all(|(_, b)| {
            b.iter()
                .all(|b| b.description.to_lowercase().contains("quit"))
        }));
        assert!(count("QUIT") >= 1);
    }

    #[test]
    fn matches_keys_and_section_title() {
        assert_eq!(count("cmd-q"), 1);
        assert_eq!(count(&key_label(Key::Stroke("cmd-q"))), 1);

        let sections = filtered("form esc");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, "Connection form");
        assert_eq!(sections[0].1.len(), 1);
    }

    #[test]
    fn every_term_must_match() {
        let sections = filtered("console cancel");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, "SQL console");
        assert_eq!(sections[0].1.len(), 1);
        assert_eq!(count("console definitely-not"), 0);
    }
}
