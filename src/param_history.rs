//! Undo for edits to a node's parameters that are made all at once.
//!
//! The app has no project-wide history: a text box undoes its own typing
//! and a viewer state its own handles. This is the third, and it holds one
//! kind of step — a node's whole parameter list as it stood before a command
//! rewrote it. Reset Parameters is the first to record here.
//!
//! A snapshot is of ONE node, by id. Restoring the whole tree would be less
//! to write and would take back every edit made anywhere since, none of
//! which are recorded; restoring one node's parameters takes back what the
//! command did to that node and leaves the rest of the project alone.
//!
//! It is not `cce_ui::history::History` because a step has to be looked at
//! before it is taken: the snapshot filed for redo is the CURRENT state of
//! the node the step names, and which node that is is in the step.

use crate::param::ParamDef;

/// A node's parameters as they stood, and what changed them.
#[derive(Clone)]
pub struct ParamSnapshot {
    pub node_id: String,
    pub params: Vec<ParamDef>,
    /// What the step was, for the status line: "Reset Parameters".
    pub what: &'static str,
}

pub const LIMIT: usize = 64;

#[derive(Default)]
pub struct ParamHistory {
    undo: Vec<ParamSnapshot>,
    redo: Vec<ParamSnapshot>,
}

impl ParamHistory {
    /// File `before` as what the next undo returns to. A new edit forks:
    /// what had been undone cannot be redone over it.
    pub fn record(&mut self, before: ParamSnapshot) {
        self.redo.clear();
        self.undo.push(before);
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
    }

    /// Take a step off one stack. The caller restores it and files the
    /// node's state from before the restore with [`Self::file`].
    pub fn take(&mut self, undo: bool) -> Option<ParamSnapshot> {
        if undo { self.undo.pop() } else { self.redo.pop() }
    }

    /// File the state a taken step replaced, on the OTHER stack.
    pub fn file(&mut self, undo: bool, replaced: ParamSnapshot) {
        if undo { self.redo.push(replaced) } else { self.undo.push(replaced) }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Another document: its nodes are not these.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}
