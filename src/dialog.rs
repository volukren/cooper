use rune::{Cx, El, Tx};

use crate::{
    browser::{self, confirm::Confirm, ddl},
    form::{self, FormDialog},
    help::{self, Help},
    keys::{Command, Ctx},
    theme::Theme,
    workspace::{self, TabId, Workspace},
};

pub enum Dialog {
    Form(Box<FormDialog>),
    DeleteConnection(String),
    Help(Box<Help>),
    Ddl(TabId),
    Confirm(Box<Confirm>),
}

impl Dialog {
    pub fn tab(&self) -> Option<TabId> {
        match self {
            Dialog::Form(_) | Dialog::DeleteConnection(_) | Dialog::Help(_) => None,
            Dialog::Ddl(tab) => Some(*tab),
            Dialog::Confirm(c) => Some(c.tab),
        }
    }

    pub fn contexts(&self, focused: Option<&str>, out: &mut Vec<Ctx>) {
        match self {
            Dialog::Help(help) => help.contexts(focused, out),
            Dialog::Form(_) | Dialog::DeleteConnection(_) | Dialog::Ddl(_) | Dialog::Confirm(_) => {
            }
        }
    }
}

pub fn view(s: &Workspace, t: &Theme, cx: &mut Cx<Workspace>, layer: &mut Vec<El>) {
    let Some(dialog) = &s.dialog else {
        return;
    };
    match dialog {
        Dialog::Form(session) => form::dialog(s, session, t, cx, layer),
        Dialog::DeleteConnection(name) => workspace::delete_dialog(s, name, t, layer),
        Dialog::Help(help) => help::dialog(s, help, t, cx, layer),
        Dialog::Ddl(tab) => ddl::dialog(s, *tab, t, cx, layer),
        Dialog::Confirm(c) => browser::confirm::dialog(s, c, t, layer),
    }
}

pub fn run(s: &mut Workspace, cmd: Command, tx: &mut Tx) -> Option<bool> {
    match (&s.dialog, cmd) {
        (Some(Dialog::Help(_)), Command::ClearSearch) => Some(help::clear_search(s, tx)),
        _ => None,
    }
}
