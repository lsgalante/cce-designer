//! `State::doc`: the open project as a document — the node tree, where the
//! editor stands in it, its undo history, and what it was last saved as.
//! Nothing here touches a widget, so it borrows apart from the panes: a
//! `self.doc.current_dir_mut()` leaves the rest of `State` free.

use super::{FsNode, NodeTemplate};

pub struct Document {
    /// The node tree: the root, the object level (`crate::context`).
    pub fs_root: FsNode,
    /// Every node template, flattened (`flatten_node_templates`): what Add
    /// Node offers and what a parameter's default is read from.
    pub node_templates: Vec<NodeTemplate>,
    /// The level the network editor shows, as child indices from the root.
    pub current_path: Vec<usize>,
    /// Undo for edits to the node tree, parameters and structure — see
    /// `src/edit_history.rs`. Consulted after a code row and a viewer
    /// state have had their turn.
    pub edit_history: crate::edit_history::EditHistory,
    /// The tree as it stood when its structure was last looked at, which
    /// the next look is compared with. None until the first.
    pub structure_base: Option<FsNode>,
    /// The project file open, or None for the bundled `default_project.json`.
    pub loaded_project_path: Option<std::path::PathBuf>,
    /// The tree as of the last save, serialized: what
    /// `State::has_unsaved_changes` compares the tree with.
    pub last_saved_root_json: String,
    /// The pane layout as of the last save — [`State::pane_layout_json`] —
    /// so a dragged plate edge or a collapse dirties the title
    /// like an edit to the tree: the save file carries them, so unsaved
    /// they are unsaved changes.
    ///
    /// [`State::pane_layout_json`]: super::State::pane_layout_json
    pub last_saved_layout_json: String,
    /// The projects opened lately, newest first and at most ten
    /// (`State::add_recent_file`): the palette's recent rows.
    pub recent_files: Vec<std::path::PathBuf>,
    /// What one world unit IS — the Guides subnet's "World Unit" choice
    /// (mm / cm / m / in), persisted with the project. Geometry never
    /// converts; this is the declaration that lets the viewport state its
    /// scale against the display metric (`cce_ui::units`) and `View 1:1`
    /// put the pivot plane at true size.
    pub world_unit: cce_ui::units::Unit,
}

impl Document {
    /// The level the network editor shows.
    pub fn current_dir(&self) -> &FsNode {
        let mut node = &self.fs_root;
        for &i in &self.current_path {
            node = &node.children[i];
        }
        node
    }

    pub fn current_dir_mut(&mut self) -> &mut FsNode {
        let mut node = &mut self.fs_root;
        for &i in &self.current_path {
            node = &mut node.children[i];
        }
        node
    }

    /// The node a path addresses, CLAMPED: each step that no longer exists
    /// ends the walk, so a stale second-editor path degrades to the deepest
    /// still-valid ancestor instead of indexing out of bounds after a
    /// structural edit made in the other view.
    pub fn dir_at(&self, path: &[usize]) -> &FsNode {
        let mut node = &self.fs_root;
        for &i in path {
            match node.children.get(i) {
                Some(child) => node = child,
                None => break,
            }
        }
        node
    }

    /// [`Self::dir_at`], mutable — same clamping walk.
    pub fn dir_at_mut(&mut self, path: &[usize]) -> &mut FsNode {
        let mut node = &mut self.fs_root;
        for &i in path {
            if i < node.children.len() {
                node = &mut node.children[i];
            } else {
                break;
            }
        }
        node
    }

    /// The level camera nodes stand on: the root, the object level
    /// (`context`). One level for every view, since 2026-10-02 — a camera
    /// was looked up on the CURRENT level until then, so diving into a
    /// subnet lost it, and with the geometry inside a Geometry node that
    /// would be every working view.
    pub fn camera_level(&self) -> &FsNode {
        &self.fs_root
    }

    pub fn camera_level_mut(&mut self) -> &mut FsNode {
        &mut self.fs_root
    }

    /// Segment names along `path` (skipping steps that no longer resolve —
    /// the same clamping `dir_at` walks with).
    pub fn path_names_at(&self, path: &[usize]) -> Vec<String> {
        let mut node = &self.fs_root;
        let mut names = Vec::new();
        for &i in path {
            if let Some(child) = node.children.get(i) {
                names.push(child.name.clone());
                node = child;
            }
        }
        names
    }
}
