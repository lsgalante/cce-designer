//! Undo for edits to the node tree: parameters, and the graph itself.
//!
//! The app has no project-wide history: a text box undoes its own typing
//! and a viewer state its own handles. This is the third. It holds two
//! kinds of step, on ONE stack so that undo takes them back in the order
//! they were made:
//!
//! - **Parameters** — parameters of one node as they stood before
//!   something changed them: a row of the params pane, the row menu,
//!   `set_param`, or a command that rewrites the lot (Reset Parameters).
//!   Recorded by the writers.
//! - **Structure** — nodes added, nodes removed, nodes moved, nodes
//!   renamed, wires made and broken, the display and bypass flags. Recorded by NOTICING: the
//!   tree is compared with how it stood at the last look
//!   (`State::structure_base`) once an event or an action has been
//!   applied, and what differs is the step. There are a dozen writers of
//!   the graph — the widget's own drag read back a frame later, the
//!   keyboard families, paste, the palette, MCP, the image commands,
//!   Arrange — and a recording call in each is one that the thirteenth
//!   would not make.
//!
//! **A step holds what changed and nothing else.** A parameter step names
//! its parameters; a structure step names its nodes, and of a node that
//! stayed only its position, its two flags and its wires. Restoring a
//! node's whole parameter list would take back what is written to it
//! without passing here — a camera node's Rotation under an orbit, a
//! curve's Points under its handles.
//!
//! **One step per gesture.** The pane writes back on every motion of a
//! drag, so records that share a group are one step until the group is
//! broken: by a press or a release, by Enter, Tab or Escape, or by
//! [`GROUP_IDLE`] without a record, which is what ends a run of wheel
//! notches or of alt+hjkl. The graph is not looked at while a drag is
//! held, so a dragged node is one step, from where it was picked up.
//!
//! It is not `cce_ui::history::History` because a step has to be looked at
//! before it is taken: what is filed for redo is the CURRENT state of what
//! the step names, and what that is is in the step.

use crate::app::{FsNode, State};
use crate::param::{ParamDef, ParamKind};
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

/// What a node of a level was, before a structure step.
#[derive(Clone)]
pub enum NodeBefore {
    /// Not there: the step added it.
    Absent { id: String },
    /// There, whole, at this place among its siblings: the step removed it.
    Whole { index: usize, node: FsNode },
    /// There, and these of its fields were otherwise. `wires` holds the
    /// wire parameters that differed, as they were.
    Fields { id: String, position: (f32, f32), geometry_visible: bool, bypassed: bool, wires: Vec<ParamDef> },
}

/// The nodes of one level a structure step changed. `dir_id` is the id of
/// the node whose children they are.
#[derive(Clone)]
pub struct LevelBefore {
    pub dir_id: String,
    pub nodes: Vec<NodeBefore>,
}

#[derive(Clone)]
pub struct StructureStep {
    pub levels: Vec<LevelBefore>,
    /// Nodes that had another name: (id, the name it was). Put back by
    /// renaming, not by writing the name, so that what names the node —
    /// wires, expression paths anywhere in the tree — follows it back.
    pub renames: Vec<(String, String)>,
    pub what: String,
}

#[derive(Clone)]
pub enum Step {
    Params(ParamSnapshot),
    Structure(StructureStep),
}

impl Step {
    pub fn what(&self) -> &str {
        match self {
            Step::Params(p) => &p.what,
            Step::Structure(s) => &s.what,
        }
    }
}

#[derive(Default)]
pub struct EditHistory {
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The group of the last record and when it was made.
    group: Option<(String, Instant)>,
}

impl EditHistory {
    /// File `before` as what the next undo returns to. A new edit forks:
    /// what had been undone cannot be redone over it.
    pub fn record(&mut self, before: Step) {
        self.group = None;
        self.push(before);
    }

    /// [`Self::record`], unless the last record was of the same group and
    /// the group still stands: the earlier step already holds what this
    /// gesture began from.
    pub fn record_grouped(&mut self, before: Step, group: String) {
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

    fn push(&mut self, before: Step) {
        self.redo.clear();
        self.undo.push(before);
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
    }

    /// Take a step off one stack. The caller restores it and files what
    /// the restore replaced with [`Self::file`].
    pub fn take(&mut self, undo: bool) -> Option<Step> {
        self.group = None;
        if undo { self.undo.pop() } else { self.redo.pop() }
    }

    /// File the state a taken step replaced, on the OTHER stack.
    pub fn file(&mut self, undo: bool, replaced: Step) {
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

/// A node's wires: its parameters of the `node` kind.
fn wires(node: &FsNode) -> impl Iterator<Item = &ParamDef> {
    node.params.iter().filter(|p| p.kind() == ParamKind::Node)
}

/// Whether a level's children can be told apart by id. A hand-built tree
/// may carry empty ids or one id twice; such a level is not followed.
fn ids_tell_apart(nodes: &[FsNode]) -> bool {
    let mut seen = std::collections::HashSet::new();
    nodes.iter().all(|n| !n.id.is_empty() && seen.insert(n.id.as_str()))
}

/// What differs between how a tree stood and how it stands.
#[derive(Default)]
pub struct Difference {
    /// The structure that changed, as it WAS: a step's content.
    pub levels: Vec<LevelBefore>,
    /// The nodes renamed: (id, the name it was).
    pub renames: Vec<(String, String)>,
    /// Whether anything differs at all, a parameter's value included —
    /// which is no step, and is when the base has to be taken again.
    pub any: bool,
    added: Vec<String>,
    removed: Vec<String>,
    moved: Vec<String>,
    wired: usize,
    flagged: usize,
}

impl Difference {
    /// The step's name, for the status line.
    pub fn what(&self) -> String {
        let list = |names: &[String]| match names {
            [one] => one.clone(),
            many => format!("{} nodes", many.len()),
        };
        if !self.renames.is_empty() && self.added.is_empty() && self.removed.is_empty() {
            // The wires that named the node changed with it, and are part
            // of the rename.
            let names: Vec<String> = self.renames.iter().map(|(_, was)| was.clone()).collect();
            format!("Rename {}", list(&names))
        } else if !self.added.is_empty() && self.removed.is_empty() {
            format!("Add {}", list(&self.added))
        } else if !self.removed.is_empty() && self.added.is_empty() {
            format!("Delete {}", list(&self.removed))
        } else if !self.added.is_empty() {
            "Edit Nodes".to_string()
        } else if self.wired > 0 {
            "Wire".to_string()
        } else if !self.moved.is_empty() {
            format!("Move {}", list(&self.moved))
        } else {
            "Node Flag".to_string()
        }
    }

    /// The group a run of these belongs to, when it is a move and nothing
    /// else: a run of alt+hjkl is one step.
    pub fn move_group(&self) -> Option<String> {
        let only_moves = self.added.is_empty() && self.removed.is_empty() && self.wired == 0 && self.flagged == 0;
        (only_moves && self.renames.is_empty() && !self.moved.is_empty()).then(|| format!("move\u{0}{}", self.moved.join("\u{0}")))
    }
}

/// Compare a level, and the levels under the nodes that stayed.
pub fn difference(base: &FsNode, now: &FsNode, out: &mut Difference) {
    if !ids_tell_apart(&base.children) || !ids_tell_apart(&now.children) {
        out.any |= base.children.len() != now.children.len();
        return;
    }
    let mut nodes = Vec::new();
    for (index, was) in base.children.iter().enumerate() {
        let Some(is) = now.children.iter().find(|n| n.id == was.id) else {
            out.removed.push(was.name.clone());
            nodes.push(NodeBefore::Whole { index, node: was.clone() });
            continue;
        };
        let changed_wires: Vec<ParamDef> = wires(was)
            .filter(|w| is.params.iter().find(|p| p.name == w.name).is_some_and(|p| !same(p, w)))
            .cloned()
            .collect();
        let moved = was.position != is.position;
        let flagged = was.geometry_visible != is.geometry_visible || was.bypassed != is.bypassed;
        if moved || flagged || !changed_wires.is_empty() {
            if moved {
                out.moved.push(is.name.clone());
            }
            out.flagged += flagged as usize;
            out.wired += changed_wires.len();
            nodes.push(NodeBefore::Fields {
                id: was.id.clone(),
                position: was.position,
                geometry_visible: was.geometry_visible,
                bypassed: was.bypassed,
                wires: changed_wires,
            });
        }
        if was.name != is.name {
            out.renames.push((was.id.clone(), was.name.clone()));
        }
        out.any |= was.name != is.name
            || was.params.len() != is.params.len()
            || was.params.iter().zip(is.params.iter()).any(|(a, b)| a.name != b.name || !same(a, b));
        difference(was, is, out);
    }
    for is in &now.children {
        if !base.children.iter().any(|n| n.id == is.id) {
            out.added.push(is.name.clone());
            nodes.push(NodeBefore::Absent { id: is.id.clone() });
        }
    }
    if !nodes.is_empty() {
        out.any = true;
        out.levels.push(LevelBefore { dir_id: base.id.clone(), nodes });
    }
}

/// Put a tree back as a step says it was, and return the step that would
/// put it back as it is: undo's is redo's and redo's undo's.
pub fn restore(root: &mut FsNode, step: StructureStep) -> StructureStep {
    let mut levels = Vec::new();
    for level in step.levels {
        let Some(dir) = crate::viewer_state::find_node_by_id_mut(root, &level.dir_id) else { continue };
        let mut nodes: Vec<NodeBefore> = Vec::new();
        let mut going: Vec<String> = Vec::new();
        let mut back: Vec<(usize, FsNode)> = Vec::new();
        for node in level.nodes {
            match node {
                NodeBefore::Absent { id } => going.push(id),
                NodeBefore::Whole { index, node } => back.push((index, node)),
                NodeBefore::Fields { id, position, geometry_visible, bypassed, wires } => {
                    let Some(n) = dir.children.iter_mut().find(|n| n.id == id) else { continue };
                    let mut were = Vec::new();
                    for w in wires {
                        if let Some(p) = n.params.iter_mut().find(|p| p.name == w.name) {
                            were.push(std::mem::replace(p, w));
                        }
                    }
                    nodes.push(NodeBefore::Fields {
                        id,
                        position: std::mem::replace(&mut n.position, position),
                        geometry_visible: std::mem::replace(&mut n.geometry_visible, geometry_visible),
                        bypassed: std::mem::replace(&mut n.bypassed, bypassed),
                        wires: were,
                    });
                }
            }
        }
        // What the step added goes, as it stands NOW and from where it
        // stands — the places read before any of them is taken out.
        let mut gone: Vec<(usize, FsNode)> = dir
            .children
            .iter()
            .enumerate()
            .filter(|(_, n)| going.contains(&n.id))
            .map(|(i, n)| (i, n.clone()))
            .collect();
        dir.children.retain(|n| !going.contains(&n.id));
        nodes.extend(gone.drain(..).map(|(index, node)| NodeBefore::Whole { index, node }));
        // What it removed comes back where it was, lowest first, so each
        // lands among the siblings it had.
        back.sort_by_key(|(index, _)| *index);
        for (index, node) in back {
            nodes.push(NodeBefore::Absent { id: node.id.clone() });
            let at = index.min(dir.children.len());
            dir.children.insert(at, node);
        }
        levels.push(LevelBefore { dir_id: level.dir_id, nodes });
    }
    // The names last, and by renaming: every wire and every expression
    // path that names the node is written back with it. After the wires
    // the step holds, or what is filed for redo would be those wires as
    // the rename had just left them. Last renamed, first put back.
    let mut renames = Vec::new();
    for (id, was) in step.renames.into_iter().rev() {
        let Some(is) = crate::viewer_state::find_node_by_id(root, &id).map(|n| n.name.clone()) else { continue };
        // A sibling has taken the name since: two nodes of one name would
        // leave every wire to either naming both. The node keeps the name
        // it has.
        let taken = crate::geometry::find_parent_node(root, &id)
            .is_some_and(|p| p.children.iter().any(|c| c.id != id && c.name == was));
        if !taken && crate::geometry::rename_node_in_tree(root, &id, &was) {
            renames.push((id, is));
        }
    }
    renames.reverse();
    StructureStep { levels, renames, what: step.what }
}

impl State {
    /// Take the tree as it stands for what the next look compares with.
    /// After anything that replaces the tree or that has recorded its own
    /// step, so that it is not noticed as an edit.
    pub fn rebase_structure(&mut self) {
        self.structure_base = Some(self.fs_root.clone());
    }

    /// Whether a drag is held. The graph is not looked at until it is let
    /// go: a dragged node is one step, from where it was picked up.
    fn gesture_held(&self) -> bool {
        self.drag_widget.is_some()
            || self.app_drag.is_some()
            || self.node_drag_group.is_some()
            // A camera orbit or a pan is not an edit, but an orbit with a
            // camera node active writes its rotation every motion — and a
            // changed parameter is a full-tree clone here (`rebase_structure`).
            || self.orbit_drag.is_some()
            || self.pan_drag.is_some()
            || self.is_panning
    }

    /// Look at the tree, and record what of its structure has changed
    /// since the last look. Run once an event or an action has been
    /// applied.
    pub fn record_structure_changes(&mut self) {
        if !self.gesture_held() {
            self.note_structure_changes();
        }
    }

    fn note_structure_changes(&mut self) {
        let Some(base) = &self.structure_base else {
            self.rebase_structure();
            return;
        };
        let mut diff = Difference::default();
        difference(base, &self.fs_root, &mut diff);
        if !diff.any {
            return;
        }
        if !diff.levels.is_empty() || !diff.renames.is_empty() {
            let group = diff.move_group();
            let step =
                Step::Structure(StructureStep { what: diff.what(), levels: diff.levels, renames: diff.renames });
            match group {
                Some(group) => self.edit_history.record_grouped(step, group),
                None => self.edit_history.record(step),
            }
        }
        self.rebase_structure();
    }

    /// Record parameters of a node as they were. For the writers of
    /// parameters, which know what they changed, and call this once they
    /// have changed it.
    pub fn record_params(&mut self, before: ParamSnapshot, grouped: bool) {
        // A wire is structure too. The base is told of these parameters
        // as they are now, so that the look below does not take this edit
        // for one of its own and record it a second time.
        let now: Vec<ParamDef> = crate::viewer_state::find_node_by_id(&self.fs_root, &before.node_id)
            .map(|n| {
                n.params.iter().filter(|p| before.params.iter().any(|b| b.name == p.name)).cloned().collect()
            })
            .unwrap_or_default();
        if let Some(node) = self
            .structure_base
            .as_mut()
            .and_then(|base| crate::viewer_state::find_node_by_id_mut(base, &before.node_id))
        {
            for p in now {
                if let Some(was) = node.params.iter_mut().find(|w| w.name == p.name) {
                    *was = p;
                }
            }
        }
        // What the graph did before this edit is its own step, under it.
        self.note_structure_changes();
        if grouped {
            let group = before.group();
            self.edit_history.record_grouped(Step::Params(before), group);
        } else {
            self.edit_history.record(Step::Params(before));
        }
    }

    /// Record that `pname` of a node was `before` until just now, if it is
    /// not still. For the writers that change one parameter in one go: the
    /// row menu and `set_param`.
    pub fn record_param_edit(&mut self, node_id: &str, before: ParamDef) {
        let now = crate::viewer_state::find_node_by_id(&self.fs_root, node_id)
            .and_then(|n| n.params.iter().find(|p| p.name == before.name));
        if now.is_some_and(|p| !same(p, &before)) {
            self.record_params(
                ParamSnapshot { node_id: node_id.to_string(), what: before.name.clone(), params: vec![before] },
                false,
            );
        }
    }

    /// Undo (or redo) the last edit to the tree. False when there is none,
    /// so the caller can say nothing was taken.
    pub fn history_step(&mut self, undo: bool) -> bool {
        // What has been done and not yet looked at is the step to take.
        self.note_structure_changes();
        let Some(step) = self.edit_history.take(undo) else { return false };
        let verb = if undo { "Undo" } else { "Redo" };
        let what = step.what().to_string();
        match step {
            Step::Params(step) => {
                let Some(node) = crate::viewer_state::find_node_by_id_mut(&mut self.fs_root, &step.node_id) else {
                    // Deleted since. The step names nothing, and is dropped.
                    self.update_status_text(&format!("{verb} {what}: the node is gone"));
                    return false;
                };
                // By name: the step holds the parameters that changed, and
                // the rest of the node is as whatever wrote it last left it.
                let mut replaced = Vec::new();
                for was in step.params {
                    if let Some(p) = node.params.iter_mut().find(|p| p.name == was.name) {
                        replaced.push(std::mem::replace(p, was));
                    }
                }
                let name = node.name.clone();
                self.edit_history.file(
                    undo,
                    Step::Params(ParamSnapshot { node_id: step.node_id, params: replaced, what: step.what }),
                );
                self.sync_grid_settings();
                self.sync_nodes();
                self.rebuild_scene_geometry();
                self.sync_parameters_pane();
                self.update_status_text(&format!("{verb} {what}: {name}"));
            }
            Step::Structure(step) => {
                // Where the editor is and what is selected are slots, which
                // a node coming or going moves: held by id across it.
                let path = self.ids_along(&self.current_path.clone());
                let selected = self
                    .graph()
                    .selected_node()
                    .and_then(|i| self.current_dir().children.get(i))
                    .map(|n| n.id.clone());
                // The active camera is a name, which a rename changes.
                let camera = self
                    .camera_level()
                    .children
                    .iter()
                    .find(|c| c.node_type == "camera" && c.name == self.active_camera)
                    .map(|c| c.id.clone());
                let inverse = restore(&mut self.fs_root, step);
                self.edit_history.file(undo, Step::Structure(inverse));
                self.current_path = self.slots_along(&path);
                let camera = camera
                    .and_then(|id| crate::viewer_state::find_node_by_id(&self.fs_root, &id))
                    .map(|n| n.name.clone());
                if let Some(name) = camera {
                    self.set_active_camera(name);
                }
                let slot = selected.and_then(|id| self.current_dir().children.iter().position(|n| n.id == id));
                self.graph_mut().set_selected_node(slot);
                self.grid_cursor_expanse = None;
                let at = slot.and_then(|i| self.current_dir().children.get(i)).map(|n| n.position);
                if let Some((col, row)) = at {
                    self.grid_cursor_col = col as i32;
                    self.grid_cursor_row = row as i32;
                }
                self.sync_nodes();
                self.rebuild_positions();
                self.apply_layout();
                self.update_panel_bounds();
                self.rebuild_scene_geometry();
                self.sync_parameters_pane();
                self.viewport_dirty = true;
                self.update_status_text(&format!("{verb} {what}"));
            }
        }
        self.rebase_structure();
        if self.syncing_windows() {
            self.needs_autosave = true;
        }
        true
    }

    /// The ids of the nodes a path of slots goes down through.
    fn ids_along(&self, path: &[usize]) -> Vec<String> {
        let mut node = &self.fs_root;
        let mut ids = Vec::new();
        for &slot in path {
            let Some(child) = node.children.get(slot) else { break };
            ids.push(child.id.clone());
            node = child;
        }
        ids
    }

    /// The path of slots that goes down through those nodes now, as far as
    /// they are still there.
    fn slots_along(&self, ids: &[String]) -> Vec<usize> {
        let mut node = &self.fs_root;
        let mut path = Vec::new();
        for id in ids {
            let Some(slot) = node.children.iter().position(|n| n.id == *id) else { break };
            path.push(slot);
            node = &node.children[slot];
        }
        path
    }
}
