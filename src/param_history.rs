//! Undo for edits to a node's parameters.
//!
//! The app has no project-wide history: a text box undoes its own typing
//! and a viewer state its own handles. This is the third, and it holds one
//! kind of step — parameters of one node as they stood before something
//! changed them: a row of the params pane, the row menu, `set_param`, or a
//! command that rewrites the lot (Reset Parameters).
//!
//! A step holds the parameters that CHANGED, by name, of ONE node, by id.
//! Restoring the whole tree would be less to write and would take back
//! every edit made anywhere since; restoring a node's whole list would take
//! back what is written to it without passing here — a camera node's
//! Rotation under an orbit, a curve's Points under its handles. A step puts
//! back what it replaced and nothing else.
//!
//! **One step per gesture.** The pane writes back on every motion of a
//! drag, so records that share a group — the node and the parameters
//! changed — are one step until the group is broken: by a press or a
//! release, by Enter, Tab or Escape, or by [`GROUP_IDLE`] without a record,
//! which is what ends a run of wheel notches.
//!
//! It is not `cce_ui::history::History` because a step has to be looked at
//! before it is taken: the snapshot filed for redo is the CURRENT state of
//! the node the step names, and which node that is is in the step.

use crate::param::ParamDef;
use std::time::{Duration, Instant};

/// Parameters of a node as they stood, and what changed them.
#[derive(Clone)]
pub struct ParamSnapshot {
    pub node_id: String,
    pub params: Vec<ParamDef>,
    /// What the step was, for the status line: "Reset Parameters",
    /// "Radius".
    pub what: String,
}

impl ParamSnapshot {
    /// The group an edit of these parameters belongs to.
    pub fn group(&self) -> String {
        let names: Vec<&str> = self.params.iter().map(|p| p.name.as_str()).collect();
        format!("{}\u{0}{}", self.node_id, names.join("\u{0}"))
    }
}

/// Whether two states of a parameter are one: what a step would restore.
pub fn same(a: &ParamDef, b: &ParamDef) -> bool {
    a.text() == b.text() && a.is_expr() == b.is_expr() && a.view == b.view
}

pub const LIMIT: usize = 256;
/// How long a group stands with nothing recorded into it.
pub const GROUP_IDLE: Duration = Duration::from_millis(1000);

#[derive(Default)]
pub struct ParamHistory {
    undo: Vec<ParamSnapshot>,
    redo: Vec<ParamSnapshot>,
    /// The group of the last record and when it was made.
    group: Option<(String, Instant)>,
}

impl ParamHistory {
    /// File `before` as what the next undo returns to. A new edit forks:
    /// what had been undone cannot be redone over it.
    pub fn record(&mut self, before: ParamSnapshot) {
        self.group = None;
        self.push(before);
    }

    /// [`Self::record`], unless the last record was of the same group and
    /// the group still stands: the earlier snapshot already holds what this
    /// gesture began from.
    pub fn record_grouped(&mut self, before: ParamSnapshot) {
        let group = before.group();
        let now = Instant::now();
        let standing = matches!(&self.group, Some((g, at)) if *g == group && now.duration_since(*at) < GROUP_IDLE);
        if !(standing && !self.undo.is_empty()) {
            self.push(before);
        }
        self.group = Some((group, now));
    }

    /// The gesture is over: the next grouped record is a step of its own.
    pub fn break_group(&mut self) {
        self.group = None;
    }

    fn push(&mut self, before: ParamSnapshot) {
        self.redo.clear();
        self.undo.push(before);
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
    }

    /// Take a step off one stack. The caller restores it and files what
    /// the restore replaced with [`Self::file`].
    pub fn take(&mut self, undo: bool) -> Option<ParamSnapshot> {
        self.group = None;
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

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Another document: its nodes are not these.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.group = None;
    }
}
