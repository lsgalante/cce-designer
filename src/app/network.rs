//! `State::net`: the network editor's own state — the grid cursor and the
//! selection it makes, the lattice and its zoom, the pan and its fling, and
//! the look of the nodes. The widgets are the roster's; this is what the
//! app keeps beside them.

use super::*;

pub struct NetworkView {
    /// The grid cursor's cell — its ANCHOR when it is expanded. The
    /// selection is whatever node stands there
    /// (`State::sync_cursor_and_selection`).
    pub grid_cursor_col: i32,
    pub grid_cursor_row: i32,
    /// The grid cursor EXPANDED over a region: the cell it was anchored at
    /// when a drag began, and the far cell that drag reached.
    ///
    /// Read through [`grid_cursor_region`](crate::app::State::grid_cursor_region), which
    /// hands it back only while its anchor is still the live cursor cell.
    /// That is the whole collapse rule: every OTHER way the cursor moves — a
    /// nav key, a click, a load, a selection following a node — leaves the
    /// anchor behind and the region goes with it, without a line in any of
    /// those places. A flag reset by hand at fifteen call sites is a flag
    /// that gets missed at one of them, and a cursor left stretched across
    /// the sheet is not a subtle wrong.
    pub grid_cursor_expanse: Option<((i32, i32), (i32, i32))>,
    /// The anchor of a LIVE expansion drag — a left press on empty grid,
    /// cleared on release. `Some` is what makes motion grow the region
    /// rather than do nothing; the region it leaves behind outlives it.
    pub grid_cursor_drag: Option<(i32, i32)>,
    /// The grid cell an explicit deselect happened in.
    ///
    /// The selection IS whatever sits in the cursor's cell — that is what
    /// `sync_cursor_and_selection` means — so simply clearing it does not
    /// stick: the sync runs on nearly every frame that changes anything and
    /// puts it straight back. Remembering the cell lets the sync leave that one
    /// alone, and only that one: navigate anywhere else and selection resumes,
    /// which is why this is a cell rather than a flag.
    pub deselected_cell: Option<(i32, i32)>,
    /// A node drag that is carrying the whole selection. `None` for an
    /// ordinary one-node drag, which the widget handles by itself.
    pub node_drag_group: Option<NodeDragGroup>,
    /// The copied nodes, with the positions they were copied FROM — a paste
    /// lays them back out in the same shape, offset to the cursor. A Vec
    /// rather than one node because an expanded cursor selects many, and a
    /// copy that silently took one of them would be a trap.
    pub node_clipboard: Vec<FsNode>,

    /// Whether a dragged node snaps to cells: always, in this app — to a
    /// cell it can land on (cce-ui's `Graph::drag_update`). Not read from
    /// `style.surface.graph.grid_snap`.
    pub grid_snap_enabled: bool,
    /// The configured grid geometry (`configured_grid_geometry`) the live
    /// one is a zoom of — the 100% baseline as it was last read. A config
    /// reload that moves it re-applies the grid at the zoom in hand
    /// (`update_graph_settings_from_config`).
    pub grid_base: GridGeometry,
    /// The network grid at the current zoom. The pitch — centre of one grid
    /// line to the centre of the next, per axis — is the grid's one size,
    /// and a node's (col, row) is the intersection its centre sits on. The
    /// node body's size is its own, not the pitch's; zoom scales the two
    /// together (`scale_grid_geometry`) and nothing else relates them.
    pub grid: GridGeometry,

    /// The pan: where the lattice's (0, 0) crossing stands from the
    /// network pane's top-left, in px. The Graph widget moves it too (its
    /// wheel and trackpad motion), and the app adopts the widget's.
    pub pan_x: f32,
    pub pan_y: f32,
    /// Drag-release fling only (middle / space+left drag over the network
    /// pane): tracked while `is_panning`, integrated by the kinetic slide in
    /// `tick_frame`. Wheel and trackpad panning no longer feed it — that
    /// motion is the Graph widget's own `ScrollMotion` (glide / coast).
    pub pan_velocity_x: f32,
    pub pan_velocity_y: f32,
    /// The pan as the last tick saw it, which the fling's velocity is
    /// measured against.
    pub last_frame_pan_x: f32,
    pub last_frame_pan_y: f32,
    /// A drag pan is held (middle, or space and the left button, over the
    /// network's area), begun with the pointer at `pan_start_cx/cy` and the
    /// pan at `pan_start_x/y`.
    pub is_panning: bool,
    pub pan_start_cx: f32,
    pub pan_start_cy: f32,
    pub pan_start_x: f32,
    pub pan_start_y: f32,
    /// Space is down: the left button pans the network.
    pub space_pressed: bool,
    /// The scroll behaviour config.kdl's `input.inertial` sets: the wheel's
    /// speed, whether a fling coasts, and how fast a coast dies.
    pub graph_scroll_speed: f32,
    pub graph_inertial_scroll: bool,
    pub graph_scroll_friction: f32,
    /// Which of the two a scroll gesture over the plateless network goes
    /// to — the graph (a node was under the pointer when it began) or the
    /// scene — with where and when its last event came. Held for the whole
    /// gesture: a pan slides the node out from under a pointer that does
    /// not move, and the rest of the swipe must not become an orbit.
    pub overlay_wheel: Option<OverlayWheel>,

    /// The network pane's opacity (`style.surface.graph.opacity`), handed
    /// to the graph, its menubar and its breadcrumb.
    pub network_opacity: f32,
    /// Node-domain opacity (style.surface.graph.node.opacity) — independent of
    /// the pane's network_opacity; fades node bodies/wires/ports and node text.
    pub node_opacity: f32,
    /// The node bodies' own backdrop compression (`style.surface.graph.
    /// node_compression`, 0..1), overriding the pane material's for nodes
    /// only — the plates can stay clear while the tablets sitting on them
    /// pull the view toward the tint's key and read as solid. None = the
    /// pane's.
    pub node_compression: Option<f32>,
    /// The node bodies' own tint (`style.surface.graph.node_tint`, rgba,
    /// LINEAR once read), overriding the pane material's for the bodies and
    /// the name floors — what compression pulls the backdrop toward, so a
    /// light tint is a light node. None = the pane's. See
    /// [`State::node_ink`](crate::app::State::node_ink) for what it does to the names.
    pub node_tint: Option<[f32; 4]>,
    /// The node colour the config names (`cce_ui::color::graph_node_color`),
    /// as last handed to the toolkit.
    pub node_color: [f32; 4],
    /// The drop-target glow's animation state: position glides toward the
    /// cell an in-flight node drag will land on, alpha fades in on drag
    /// start and out after release (the glow lingers at its last cell while
    /// fading). None once fully faded. Advanced in
    /// [`State::tick_frame`](crate::app::State::tick_frame), drawn by the
    /// CONTENT branch in render.rs.
    pub drop_glow: Option<DropGlow>,
    /// The box drawn round the wired chain the pointer is in, eased in
    /// and out (see [`State::chain_box_target`](crate::app::State::chain_box_target)).
    /// None once fully faded. Advanced in
    /// [`State::tick_frame`](crate::app::State::tick_frame), drawn by the
    /// CONTENT branch in render.rs under the wires.
    pub chain_box: Option<ChainBox>,
}

/// The animated box round a chain (see [`NetworkView::chain_box`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ChainBox {
    /// The chain's node ids, sorted: what tells one chain from another
    /// while the box is up, so the box glides to a new chain and follows
    /// its own exactly through a pan or a zoom.
    pub ids: Vec<String>,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// 0..1, scaling the outline's and the wash's alpha.
    pub alpha: f32,
}

/// The target a scroll gesture over the plateless network was given at its
/// first event (`State::overlay_wheel_to_graph`).
#[derive(Debug, Clone, Copy)]
pub struct OverlayWheel {
    /// The graph pans; otherwise the scene's camera orbits.
    pub graph: bool,
    pub at: (f32, f32),
    pub last: Instant,
}

/// How long a pause ends a scroll gesture for `OverlayWheel`: longer than
/// the gap between a trackpad's events, shorter than a deliberate second
/// swipe.
pub const OVERLAY_WHEEL_GAP: std::time::Duration = std::time::Duration::from_millis(250);

/// A mouse drag that moves the whole selection, not just the node under the
/// pointer.
///
/// The widget drags ONE node — it has one `dragging_idx` — so the companions
/// are moved here, rigidly, by the offset the dragged node has travelled.
/// Their ORIGINAL cells are kept rather than stepped each frame: a drag is a
/// continuous gesture reported in whole cells, so accumulating the steps would
/// drift the group apart the first time two motion events resolved to the same
/// cell.
#[derive(Debug, Clone)]
pub struct NodeDragGroup {
    /// The slot the pointer grabbed — the widget's own dragged node.
    pub dragged: usize,
    /// The cell it started on; every offset is measured from here.
    pub from: (i32, i32),
    /// The rest of the selection, with the cells they started on.
    pub others: Vec<(usize, (f32, f32))>,
}

/// Animated drop-target glow (see [`NetworkView::drop_glow`]).
#[derive(Debug, Clone, Copy)]
pub struct DropGlow {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// 0..1 — scales the whole feather's alpha profile.
    pub alpha: f32,
}

/// The level's wired CHAINS: the nodes joined by the wires the graph
/// draws (a `node` parameter naming a sibling, as cce-ui's `wire_pairs`
/// reads them), as slots, each chain in slot order and the chains by
/// their first slot. A node wired to nothing is not a chain: its body is
/// already what the overlay claims of it.
pub fn wired_chains(nodes: &[GraphNode]) -> Vec<Vec<usize>> {
    let mut root: Vec<usize> = (0..nodes.len()).collect();
    fn find(root: &mut [usize], mut i: usize) -> usize {
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    }
    let mut wired = vec![false; nodes.len()];
    for (i, n) in nodes.iter().enumerate() {
        for (_, source, kind) in &n.parameters {
            if kind != "node" || source.is_empty() {
                continue;
            }
            let Some(j) = nodes.iter().position(|m| m.name == *source) else { continue };
            if j == i {
                continue;
            }
            wired[i] = true;
            wired[j] = true;
            let (a, b) = (find(&mut root, i), find(&mut root, j));
            root[a.max(b)] = a.min(b);
        }
    }
    let mut chains: Vec<Vec<usize>> = Vec::new();
    let mut at: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for i in (0..nodes.len()).filter(|&i| wired[i]) {
        let r = find(&mut root, i);
        let k = *at.entry(r).or_insert_with(|| {
            chains.push(Vec::new());
            chains.len() - 1
        });
        chains[k].push(i);
    }
    chains
}

// The lattice and the cursor region, as arithmetic on the view alone. A
// window-space answer takes the pane's ORIGIN — the content slot's top-left,
// which is the roster's — and `State` keeps forwarders that pass it.
impl NetworkView {
    /// The node body's size at the current zoom.
    pub fn node_size(&self) -> (f32, f32) {
        (self.grid.node_w, self.grid.node_h)
    }

    /// The window-space centre of lattice cell (col, row) in a pane whose
    /// top-left is `origin`: the origin, plus the pan, plus the cell's
    /// pitches. This is where a node at that position is centred, and what
    /// the grid origin handed to the widget means.
    pub fn cell_center(&self, (px, py): (f32, f32), col: i32, row: i32) -> (f32, f32) {
        (
            px + self.pan_x + col as f32 * self.grid.pitch_x,
            py + self.pan_y + row as f32 * self.grid.pitch_y,
        )
    }

    /// The window-space rect a node body occupies on lattice cell
    /// (col, row) — centred on the intersection.
    pub fn cell_rect(&self, origin: (f32, f32), col: i32, row: i32) -> (f32, f32, f32, f32) {
        let (cx, cy) = self.cell_center(origin, col, row);
        let (w, h) = self.node_size();
        (cx - w * 0.5, cy - h * 0.5, w, h)
    }

    /// The lattice cell nearest a window-space point in the pane.
    /// Nearest, not "the cell whose span contains it": a cell is centred on
    /// its intersection, so a click between two nodes belongs to the closer.
    pub fn cell_at(&self, (px, py): (f32, f32), x: f32, y: f32) -> (i32, i32) {
        let col = if self.grid.pitch_x > 0.0 { ((x - px - self.pan_x) / self.grid.pitch_x).round() } else { 0.0 };
        let row = if self.grid.pitch_y > 0.0 { ((y - py - self.pan_y) / self.grid.pitch_y).round() } else { 0.0 };
        (col as i32, row as i32)
    }

    /// The lattice region the cursor covers: `(col, row, cols, rows)`, never
    /// smaller than one cell. The expanse a drag left behind counts only
    /// while its anchor is still where the cursor is — see
    /// [`grid_cursor_expanse`](NetworkView::grid_cursor_expanse).
    pub fn grid_cursor_region(&self) -> (i32, i32, i32, i32) {
        let anchor = (self.grid_cursor_col, self.grid_cursor_row);
        let far = match self.grid_cursor_expanse {
            Some((a, far)) if a == anchor => far,
            _ => anchor,
        };
        let (c0, c1) = (anchor.0.min(far.0), anchor.0.max(far.0));
        let (r0, r1) = (anchor.1.min(far.1), anchor.1.max(far.1));
        (c0, r0, c1 - c0 + 1, r1 - r0 + 1)
    }

    /// The window-space rect the cursor outline is drawn on: the union of the
    /// region's corner cells, which for the usual one-cell cursor is exactly
    /// `cell_rect` of it.
    pub fn grid_cursor_rect(&self, origin: (f32, f32)) -> (f32, f32, f32, f32) {
        let (col, row, cols, rows) = self.grid_cursor_region();
        let (x0, y0, _, _) = self.cell_rect(origin, col, row);
        let (x1, y1, w, h) = self.cell_rect(origin, col + cols - 1, row + rows - 1);
        (x0, y0, x1 - x0 + w, y1 - y0 + h)
    }

    /// Whether the cursor's region stands on lattice cell `(col, row)`.
    pub fn grid_cursor_covers(&self, col: i32, row: i32) -> bool {
        let (c, r, cols, rows) = self.grid_cursor_region();
        col >= c && col < c + cols && row >= r && row < r + rows
    }

    /// Whether the cursor is expanded past its one cell — what makes a
    /// selection a MULTIPLE selection, and the one test the paint and the
    /// operations share.
    pub fn grid_cursor_expanded(&self) -> bool {
        let (_, _, cols, rows) = self.grid_cursor_region();
        cols > 1 || rows > 1
    }

    /// Move the cursor AND the region it carries by whole cells — what a
    /// selection being dragged across the sheet needs, since stepping the
    /// anchor alone is precisely the thing that collapses the region.
    pub fn shift_grid_cursor(&mut self, dc: i32, dr: i32) {
        self.grid_cursor_col += dc;
        self.grid_cursor_row += dr;
        if let Some((anchor, far)) = self.grid_cursor_expanse {
            self.grid_cursor_expanse =
                Some(((anchor.0 + dc, anchor.1 + dr), (far.0 + dc, far.1 + dr)));
        }
    }
}

// The network editor's methods: navigation and the tree operations it
// drives, the lattice and the cursor region, the grid and scroll settings,
// zoom, the keyboard families, copy and paste, framing, and the read-back
// of the graph widget into the tree and the cursor.
impl State {
    pub fn on_path_changed(&mut self) {
        self.graph_mut().set_selected_node(None);
        self.drag_widget = None;
        self.app_drag = None;
        self.focused_widget = None;
        self.net.pan_x = 0.0;
        self.net.pan_y = 0.0;
        self.net.pan_velocity_x = 0.0;
        self.net.pan_velocity_y = 0.0;
        self.net.last_frame_pan_x = 0.0;
        self.net.last_frame_pan_y = 0.0;
        self.net.grid_cursor_col = 0;
        self.net.grid_cursor_row = 0;
        self.sync_grid_settings();
        self.sync_nodes();
        self.rebuild_scene_geometry();
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        self.sync_cursor_and_selection();
    }

    pub fn move_up(&mut self) -> bool {
        if !self.doc.current_path.is_empty() {
            let exited_idx = self.doc.current_path.pop();
            self.on_path_changed();
            if let Some(idx) = exited_idx {
                let pos = {
                    let dir = self.current_dir();
                    if idx < dir.children.len() {
                        Some(dir.children[idx].position)
                    } else {
                        None
                    }
                };
                if let Some((pos_x, pos_y)) = pos {
                    self.net.grid_cursor_col = pos_x as i32;
                    self.net.grid_cursor_row = pos_y as i32;
                    self.sync_cursor_and_selection();
                }
            }
            true
        } else {
            false
        }
    }

    pub fn find_empty_cell(&self, start_x: f32, start_y: f32, skip_idx: Option<usize>) -> (f32, f32) {
        let x = start_x;
        let mut y = start_y;
        let dir = self.current_dir();
        loop {
            let occupied = dir.children.iter().enumerate().any(|(idx, c)| {
                if Some(idx) == skip_idx {
                    false
                } else {
                    (c.position.0 - x).abs() < 0.01 && (c.position.1 - y).abs() < 0.01
                }
            });
            if occupied {
                y += 1.0;
            } else {
                break;
            }
        }
        (x, y)
    }

    /// Delete the node at `slot` of the current level, splicing it out of
    /// its chain: every sibling wire that named it is rewired to what it
    /// read through its `Input`, so deleting B from A → B → C leaves A → C.
    /// A node with no Input (a generator) leaves those wires as they were.
    /// The rewiring is part of the same undo step as the deletion, since a
    /// structure step holds the wires of every node it touches.
    pub fn delete_node(&mut self, slot: usize) -> bool {
        let len = self.current_dir().children.len();
        if slot < len {
            splice_out(self.current_dir_mut(), slot);
            self.current_dir_mut().children.remove(slot);
            if let Some(sel_idx) = self.graph().selected_node() {
                if sel_idx == slot {
                    self.graph_mut().set_selected_node(None);
                } else if sel_idx > slot {
                    self.graph_mut().set_selected_node(Some(sel_idx - 1));
                }
            }
            self.sync_nodes();
            self.rebuild_positions();
            self.apply_layout();
            self.update_panel_bounds();
            self.rebuild_scene_geometry();
            self.viewport_dirty = true;
            true
        } else {
            false
        }
    }

    /// Bypass the nodes at `slots` of the current level, or stop: the one
    /// way the flag changes, whichever of the key, the node's menu and MCP
    /// asked. Returns how many nodes it changed.
    pub fn set_bypassed(&mut self, slots: &[usize], bypassed: bool) -> usize {
        let mut changed = Vec::new();
        for &slot in slots {
            if let Some(node) = self.current_dir_mut().children.get_mut(slot) {
                if node.bypassed != bypassed {
                    node.bypassed = bypassed;
                    changed.push(node.name.clone());
                }
            }
        }
        if changed.is_empty() {
            return 0;
        }
        self.sync_nodes();
        self.rebuild_scene_geometry();
        self.sync_parameters_pane();
        self.viewport_dirty = true;
        let what = if bypassed { "Bypassed" } else { "No longer bypassing" };
        self.update_status_text(&format!("{what} {}.", changed.join(", ")));
        changed.len()
    }

    /// One level's children as the graph widget's rows. What the widget
    /// reads of a row's parameters is its WIRES — every `node` parameter,
    /// typed `node`, the k-th into input port k ([`node_wires`]) — so that
    /// is what it is handed, and a node has as many input ports as it has
    /// wires where its template declared fewer (Relax's Rest, Collision's
    /// Collider, the Remesh's From).
    fn graph_nodes_of(root: &FsNode, dir: &FsNode, frame: i32) -> Vec<GraphNode> {
        dir.children
            .iter()
            .map(|c| {
                let wires = node_wires_at(root, c, frame);
                GraphNode {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    position: c.position,
                    inputs: c.inputs.max(wires.len()),
                    parameters: wires.into_iter().map(|(name, source)| (name, source, "node".to_string())).collect(),
                    geom_visible: c.geometry_visible,
                    node_type: c.node_type.clone(),
                    outputs: c.outputs,
                }
            })
            .collect()
    }

    /// The pointer connected `output` into input port `port` of the node
    /// `input_id` on the level at `path`: the port's wire is set. False when
    /// the node or the port is not there.
    pub(crate) fn connect_port(&mut self, path: &[usize], input_id: &str, output: String, port: usize) -> bool {
        let dir = self.dir_at_mut(path);
        let Some(child) = dir.children.iter_mut().find(|c| c.id == input_id) else { return false };
        // A node with no wire parameter at all (none that says so) takes
        // it as its Input, as a connection always did.
        let slot = child.params.iter().enumerate().filter(|(_, p)| p.kind() == ParamKind::Node).map(|(i, _)| i).nth(port)
            .or_else(|| child.params.iter().position(|p| p.name == "input"));
        let Some(slot) = slot else { return false };
        child.params[slot].set_text(output);
        self.sync_nodes();
        self.rebuild_scene_geometry();
        self.sync_parameters_pane();
        true
    }

    pub fn sync_nodes(&mut self) {
        let frame = self.sim_frame();
        let graph_nodes = Self::graph_nodes_of(&self.doc.fs_root, self.current_dir(), frame);
        self.graph_mut().set_nodes(&graph_nodes);

        let camera_nodes: Vec<String> = self.camera_level().children.iter()
            .filter(|c| c.node_type == "camera")
            .map(|c| c.name.clone())
            .collect();
        let mut items = vec!["Default Camera".to_string()];
        items.extend(camera_nodes);
        if !items.contains(&self.camera.active_camera) {
            self.set_active_camera("Default Camera");
        }
        self.menu_mut(RIGHT_MENUBAR_IDX).set_menu_items(0, &items);
        for (i, item) in items.iter().enumerate() {
            let checked = item == &self.camera.active_camera;
            self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(0, i, checked);
        }
        let path_strs = self.current_path_names();
        self.path_mut().set_path(&path_strs);

        self.sync_selection_readouts();
        self.sync_pull_arrows();
    }

    /// The node body's size at the current zoom — what `sync_grid_settings`
    /// hands the widget, so the cursor drawn here and the nodes the widget
    /// draws cannot disagree.
    pub fn node_size(&self) -> (f32, f32) {
        self.net.node_size()
    }

    /// Put the grid at a geometry outright (Reset Zoom, Frame All's fit).
    pub fn set_grid_geometry(&mut self, g: GridGeometry) {
        self.net.grid = g;
    }

    /// Scale the grid geometry by one factor — the pitch and the node body
    /// together, which is the only way the two are related.
    pub fn scale_grid_geometry(&mut self, f: f32) {
        self.net.grid = self.net.grid.scaled(f);
    }

    /// Where the network pane's lattice is measured from: the content
    /// slot's top-left, in window space.
    fn network_origin(&self) -> (f32, f32) {
        let (px, py, _, _) = self.positions[CONTENT_IDX];
        (px, py)
    }

    /// [`NetworkView::cell_center`] in the network pane as laid out.
    pub fn cell_center(&self, col: i32, row: i32) -> (f32, f32) {
        self.net.cell_center(self.network_origin(), col, row)
    }

    /// [`NetworkView::cell_rect`] in the network pane as laid out.
    pub fn cell_rect(&self, col: i32, row: i32) -> (f32, f32, f32, f32) {
        self.net.cell_rect(self.network_origin(), col, row)
    }

    /// [`NetworkView::cell_at`] in the network pane as laid out.
    pub fn cell_at(&self, x: f32, y: f32) -> (i32, i32) {
        self.net.cell_at(self.network_origin(), x, y)
    }

    /// [`NetworkView::grid_cursor_region`].
    pub fn grid_cursor_region(&self) -> (i32, i32, i32, i32) {
        self.net.grid_cursor_region()
    }

    /// [`NetworkView::grid_cursor_rect`] in the network pane as laid out.
    pub fn grid_cursor_rect(&self) -> (f32, f32, f32, f32) {
        self.net.grid_cursor_rect(self.network_origin())
    }

    /// [`NetworkView::grid_cursor_covers`].
    pub fn grid_cursor_covers(&self, col: i32, row: i32) -> bool {
        self.net.grid_cursor_covers(col, row)
    }

    /// [`NetworkView::grid_cursor_expanded`].
    pub fn grid_cursor_expanded(&self) -> bool {
        self.net.grid_cursor_expanded()
    }

    /// The nodes the cursor selects, as slots in the current level, in slot
    /// order.
    ///
    /// One cell — the ordinary cursor — defers to the graph's own
    /// `selected_node`, so nothing about a single selection changes here:
    /// that one answer already carries the deselect memory, a click that
    /// arrived from another pane, and a selection made while the network was
    /// not focused. An EXPANDED cursor names every node standing inside it
    /// instead; its anchor is empty grid by construction (a press on a node
    /// drags the node), so there is no single selection to defer to.
    pub fn selected_slots(&self) -> Vec<usize> {
        if !self.grid_cursor_expanded() {
            return self.graph().selected_node().into_iter().collect();
        }
        self.current_dir()
            .children
            .iter()
            .enumerate()
            .filter(|(_, c)| self.grid_cursor_covers(c.position.0 as i32, c.position.1 as i32))
            .map(|(i, _)| i)
            .collect()
    }

    /// [`NetworkView::shift_grid_cursor`].
    pub fn shift_grid_cursor(&mut self, dc: i32, dr: i32) {
        self.net.shift_grid_cursor(dc, dr);
    }

    /// Grow the live expansion drag to the cell under the cursor. `true` when
    /// the region actually changed, which is the redraw.
    pub(super) fn grid_cursor_drag_motion(&mut self) -> bool {
        let Some(anchor) = self.net.grid_cursor_drag else { return false };
        let far = self.cell_at(self.cursor_x, self.cursor_y);
        let next = (anchor != far).then_some((anchor, far));
        if self.net.grid_cursor_expanse == next {
            return false;
        }
        self.net.grid_cursor_expanse = next;
        true
    }

    pub fn sync_grid_settings(&mut self) {
        let active_node_area_y = self.positions[CONTENT_IDX].1;
        let active_node_area_x = self.positions[CONTENT_IDX].0;

        let grid_pitch_x = self.net.grid.pitch_x;
        let grid_pitch_y = self.net.grid.pitch_y;
        let (node_w, node_h) = self.node_size();
        let pan_x = self.net.pan_x;
        let pan_y = self.net.pan_y;
        let grid_snap_enabled = self.net.grid_snap_enabled;
        let graph = self.graph_mut();
        // The lattice is not drawn (since 2026-10-07): the widget is told
        // so, though the designer never asks it to paint the grid either.
        graph.set_show_network_grid(false);
        graph.set_grid_pitch(grid_pitch_x, grid_pitch_y);
        graph.set_node_size(node_w, node_h);
        graph.set_grid_origin(active_node_area_x + pan_x, active_node_area_y + pan_y);
        graph.set_grid_snap_enabled(grid_snap_enabled);
        if let Some(graph) = self.ui_context[self.slots.content].as_any_mut().downcast_mut::<cce_ui::widget::Graph>() {
            graph.set_network_opacity(self.net.network_opacity);
            graph.set_node_opacity(self.net.node_opacity);
        }
        if let Some(menubar) = self.ui_context[self.slots.left_menubar].as_any_mut().downcast_mut::<cce_ui::widget::MenuBar>() {
            menubar.set_network_opacity(self.net.network_opacity);
        }
        if let Some(breadcrumb) = self.ui_context[self.slots.breadcrumb].as_any_mut().downcast_mut::<cce_ui::widget::Breadcrumb>() {
            breadcrumb.set_network_opacity(self.net.network_opacity);
        }

    }

    pub fn update_inertial_settings(&mut self) {
        self.last_config_read = Instant::now();
        let config_path = cce_ui::config::get_config_path();

        let mut enabled = true;
        let mut friction = 0.90;
        let mut speed = 1.0;

        if let Ok(content) = std::fs::read_to_string(&config_path) {
            let val = cce_ui::config::parse_kdl_to_json(&content);
            if let Some(input) = val.get("input") {
                if let Some(inertial) = input.get("inertial") {
                    if let Some(val) = inertial.get("inertial_scroll").and_then(|v| v.as_bool()) {
                        enabled = val;
                    }
                    if let Some(friction_val) = inertial.get("scroll_friction").and_then(|v| v.as_i64()) {
                        friction = (friction_val as f32 / 1000.0).clamp(0.1, 0.999);
                    }
                    if let Some(speed_val) = inertial.get("scroll_speed").and_then(|v| v.as_f64()) {
                        speed = speed_val as f32;
                    }
                }
            }
        }
        
        // Scroll behavior is config-owned (input.inertial in config.kdl) —
        // state.kdl carries no copy, so config edits always take effect.
        // Graph-side these shape the drag-release fling and the unrouted
        // wheel fallback; the routed wheel/trackpad pan is the Graph widget's
        // ScrollMotion, tuned by cce-ui's smooth-scroll keys in input.kdl.
        // The viewport's orbit and zoom are ScrollMotions too: they take the
        // speed and the on/off switch from here, and how long a coast runs
        // from input.kdl's `scroll_friction`, as the network pan does.
        self.net.graph_scroll_speed = speed;
        self.net.graph_inertial_scroll = enabled;
        self.net.graph_scroll_friction = friction;
        self.viewport_mut().scroll_speed = speed;
        self.viewport_mut().inertial_scroll = enabled;
    }

    pub fn update_graph_settings_from_config(&mut self) {
        cce_ui::layout::reload_config();
        
        let opacity = cce_ui::color::graph_opacity();
        let node_opacity = cce_ui::color::graph_node_opacity();
        let node_color = cce_ui::color::graph_node_color();
        let node_compression = cce_ui::config::cached_config()
            .pointer("/style/surface/graph/node_compression")
            .and_then(|v| v.as_f64())
            .map(|k| (k as f32).clamp(0.0, 1.0));
        let node_tint = cce_ui::config::cached_config()
            .pointer("/style/surface/graph/node_tint")
            .and_then(|v| v.as_str())
            .and_then(cce_ui::color::parse_hex_rgba_linear);

        let mut changed = false;

        if self.net.node_tint != node_tint {
            self.net.node_tint = node_tint;
            changed = true;
        }

        if self.net.node_compression != node_compression {
            self.net.node_compression = node_compression;
            changed = true;
        }
        if (self.net.network_opacity - opacity).abs() > 0.001 {
            self.net.network_opacity = opacity;
            changed = true;
        }
        if (self.net.node_opacity - node_opacity).abs() > 0.001 {
            self.net.node_opacity = node_opacity;
            changed = true;
        }
        if self.net.node_color != node_color {
            self.net.node_color = node_color;
            color::set_node_color(node_color);
            changed = true;
        }
        // `graph_grid_snap` is not read: a dragged node always snaps to a
        // cell it can land on (`grid_snap_enabled`, always on — since
        // 2026-10-06; the config's switch, off on the user's machine, had a
        // dragged node float freely and land somewhere else).
        // The grid's spacing and node size: re-applied at the zoom in hand,
        // so a config.kdl edit to `spacing_y` shows at once. Until
        // 2026-10-06 the live geometry was set at startup and only zoomed
        // after it, so an edit waited for Reset Zoom or a restart.
        let base = configured_grid_geometry();
        if base != self.net.grid_base {
            let zoom = if self.net.grid_base.pitch_x > 0.0 { self.net.grid.pitch_x / self.net.grid_base.pitch_x } else { 1.0 };
            self.set_grid_geometry(base.scaled(zoom));
            self.net.grid_base = base;
            changed = true;
        }

        if changed {
            self.sync_grid_settings();
            self.viewport_dirty = true;
        }
    }



    /// Zoom the network view about a window-space point (the cursor cell's
    /// centre when none is given): the pitch scales, and the pan moves so the
    /// lattice point under the anchor stays under it.
    pub fn zoom(&mut self, factor: f32, center: Option<(f32, f32)>) {
        self.net.pan_velocity_x = 0.0;
        self.net.pan_velocity_y = 0.0;
        let old_px = self.net.grid.pitch_x;
        let old_py = self.net.grid.pitch_y;

        // The x pitch is clamped and the y pitch follows by the SAME factor,
        // so the node body — scaled by that factor too — keeps its shape.
        let new_px = (old_px * factor).clamp(MIN_PITCH_X, MAX_PITCH_X);
        let factor = new_px / old_px;
        let new_py = old_py * factor;

        if (new_px - old_px).abs() < 0.01 {
            return;
        }

        let (area_x, area_y, _, _) = self.positions[CONTENT_IDX];
        let (cx, cy) = match center {
            Some(pt) => pt,
            None => self.cell_center(self.net.grid_cursor_col, self.net.grid_cursor_row),
        };

        // The anchor's lattice coordinate, in pitches, is what must not move.
        let col_f = (cx - area_x - self.net.pan_x) / old_px;
        let row_f = (cy - area_y - self.net.pan_y) / old_py;

        self.net.pan_x = cx - area_x - col_f * new_px;
        self.net.pan_y = cy - area_y - row_f * new_py;

        self.scale_grid_geometry(factor);

        self.sync_grid_settings();
    }


    pub fn keep_cursor_in_view(&mut self) {
        self.keep_cell_in_view(self.net.grid_cursor_col, self.net.grid_cursor_row);
    }

    /// Pan the least that brings one lattice cell fully into the pane. The
    /// cursor's own is the usual one; an EXTEND walks the region's far corner
    /// instead, and following the anchor there would scroll the wrong end of
    /// the selection into view — the anchor is the end that is not moving.
    pub fn keep_cell_in_view(&mut self, col: i32, row: i32) {
        let (px, py, pw, ph) = self.positions[CONTENT_IDX];
        let (cx, cy, cw, ch) = self.cell_rect(col, row);

        if cx < px {
            self.net.pan_x -= cx - px;
        } else if cx + cw > px + pw {
            self.net.pan_x -= cx + cw - (px + pw);
        }

        if cy < py {
            self.net.pan_y -= cy - py;
        } else if cy + ch > py + ph {
            self.net.pan_y -= cy + ch - (py + ph);
        }
        self.sync_grid_settings();
    }

    /// Arrange the current level's nodes from their wiring.
    ///
    /// Reports how many moved: an arrange that did nothing — because the
    /// layout was already right — looks identical to one that is broken, and
    /// the status line is the only thing that can tell them apart.
    pub(crate) fn layout_current_level(&mut self) -> bool {
        if self.focused_pane != LEFT_MENUBAR_IDX {
            return false;
        }
        let frame = self.sim_frame();
        let nodes: Vec<crate::layout::LayoutNode> = self
            .current_dir()
            .children
            .iter()
            .map(|c| crate::layout::LayoutNode {
                name: c.name.clone(),
                input: c
                    .params
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case("input"))
                    .map(|p| p.text().to_string()),
                // The rest of the wires the network draws.
                reads: node_wires_at(&self.doc.fs_root, c, frame).into_iter().filter(|(name, _)| !name.eq_ignore_ascii_case("input")).map(|(_, src)| src).collect(),
                position: c.position,
                // Utility trees stay where they were put; see the module doc.
                pinned: false,
            })
            .collect();
        let moved = crate::layout::arrange(&nodes);
        let count = moved.len();
        {
            let dir = self.current_dir_mut();
            for (idx, pos) in moved {
                if let Some(child) = dir.children.get_mut(idx) {
                    child.position = pos;
                }
            }
        }
        if count > 0 {
            self.sync_nodes();
            self.sync_layout();
            // The cursor tracks the selection, which has just moved with its
            // node — otherwise the next keypress navigates from a cell the
            // selected node no longer occupies.
            let sel_pos = self
                .graph()
                .selected_node()
                .and_then(|sel| self.current_dir().children.get(sel).map(|c| c.position));
            if let Some((cx, cy)) = sel_pos {
                self.net.grid_cursor_col = cx as i32;
                self.net.grid_cursor_row = cy as i32;
            }
            self.keep_cursor_in_view();
            self.rebuild_positions();
            self.apply_layout();
            self.update_panel_bounds();
        }
        self.update_status_text(&match count {
            0 => "Layout: every node was already in place.".to_string(),
            1 => "Layout: moved 1 node.".to_string(),
            n => format!("Layout: moved {n} nodes."),
        });
        true
    }

    /// Clear the node selection, and make it stick.
    ///
    /// Returns false when nothing was selected, so Escape can fall through to
    /// meaning nothing rather than reporting that it did something.
    pub(crate) fn deselect_node(&mut self) -> bool {
        // An expanded cursor IS a selection, and of the two this is the one
        // Escape has to be able to clear: the single selection under a plain
        // cursor comes back on the next sync anyway, while a region would
        // stand until the cursor was moved off its anchor.
        if self.grid_cursor_expanded() {
            self.net.grid_cursor_expanse = None;
            self.net.grid_cursor_drag = None;
            self.sync_parameters_pane();
            return true;
        }
        if self.graph().selected_node().is_none() {
            return false;
        }
        self.graph_mut().set_selected_node(None);
        self.net.deselected_cell = Some((self.net.grid_cursor_col, self.net.grid_cursor_row));
        if self.focused_widget == Some(CONTENT_IDX) {
            self.focused_widget = None;
        }
        self.sync_parameters_pane();
        true
    }

    /// Move the grid cursor one cell, taking the selection with it.
    ///
    /// The cursor is the network pane's keyboard position: `sync_cursor_and_
    /// selection` selects whatever node sits in its cell, so navigating IS
    /// selecting. Gated on the network pane having focus — plain h/j/k/l used
    /// to move it from any pane, which drifted the cursor invisibly while you
    /// were looking at the viewport and put it somewhere unexpected when you
    /// came back. Every other network key was already gated; this family was
    /// the one that was not.
    pub(crate) fn network_nav(&mut self, dc: i32, dr: i32) -> bool {
        if self.focused_pane != LEFT_MENUBAR_IDX {
            return false;
        }
        self.net.pan_velocity_x = 0.0;
        self.net.pan_velocity_y = 0.0;
        self.net.grid_cursor_col += dc;
        self.net.grid_cursor_row += dr;
        self.net.deselected_cell = None;
        self.sync_cursor_and_selection();
        self.keep_cursor_in_view();
        true
    }

    /// Lay the drag group's companions out around `dest`, the cell the dragged
    /// node is on (or heading for), keeping the shape the selection had when
    /// the drag began.
    ///
    /// They translate rigidly and are NOT walked off occupied cells the way
    /// the widget walks the node it drags: a whole selection that rearranged
    /// itself around whatever it passed over would not be the selection you
    /// picked up. It is the same bargain `network_move_node` has always made
    /// with alt+hjkl.
    pub(super) fn drag_group_to(&mut self, dest: (i32, i32)) {
        let Some(group) = self.net.node_drag_group.clone() else { return };
        let (dc, dr) = (dest.0 - group.from.0, dest.1 - group.from.1);
        let len = self.current_dir().children.len();
        for (slot, origin) in group.others {
            if slot < len {
                self.current_dir_mut().children[slot].position =
                    (origin.0 + dc as f32, origin.1 + dr as f32);
            }
        }
        self.sync_nodes();
    }

    /// Close an expansion drag by shrinking the region onto what it caught:
    /// the bounding box of the selected nodes, or — with nothing caught — one
    /// cell at the middle of where the region stood.
    ///
    /// A region is a way of POINTING at nodes, and once the pointing is done
    /// the empty margin the pointer swept through is noise: it hides nothing,
    /// it selects nothing, and it makes the next alt+hjkl or Add Node read off
    /// an anchor out in open grid. Settling it also makes the region say what
    /// was selected — a box drawn loosely around two nodes comes back fitted
    /// to them, which is the selection made visible.
    ///
    /// Nothing caught settles to the MIDDLE rather than to the anchor: the
    /// anchor is where the gesture began, and a drag that selected nothing is
    /// most likely aimed at the space it ended up circling. Even spans round
    /// down, toward the region's own first cell.
    ///
    /// The selection is unchanged by all of this — the bounding box of the
    /// selected nodes contains no cell the region did not — which is what
    /// lets it run unconditionally at the end of every drag.
    pub(crate) fn settle_cursor_expansion(&mut self) -> bool {
        if !self.grid_cursor_expanded() {
            return false;
        }
        let (col, row, cols, rows) = self.grid_cursor_region();
        let cells: Vec<(i32, i32)> = self
            .selected_slots()
            .into_iter()
            .map(|i| {
                let p = self.current_dir().children[i].position;
                (p.0 as i32, p.1 as i32)
            })
            .collect();
        match cells.split_first() {
            None => {
                self.net.grid_cursor_col = col + (cols - 1) / 2;
                self.net.grid_cursor_row = row + (rows - 1) / 2;
                self.net.grid_cursor_expanse = None;
            }
            Some((first, rest)) => {
                let (mut c0, mut r0, mut c1, mut r1) = (first.0, first.1, first.0, first.1);
                for &(c, r) in rest {
                    c0 = c0.min(c);
                    r0 = r0.min(r);
                    c1 = c1.max(c);
                    r1 = r1.max(r);
                }
                self.net.grid_cursor_col = c0;
                self.net.grid_cursor_row = r0;
                self.net.grid_cursor_expanse =
                    ((c1, r1) != (c0, r0)).then_some(((c0, r0), (c1, r1)));
            }
        }
        // A settle that lands on a node selects it, and one that lands on
        // empty grid clears the single selection — the ordinary cursor rules,
        // which is what the cursor is again whenever the region collapsed.
        self.net.deselected_cell = None;
        self.sync_cursor_and_selection();
        true
    }

    /// Grow (or shrink) the cursor's region by one cell — the shift+hjkl
    /// family, the plugin's extend-the-selection scheme.
    ///
    /// The ANCHOR never moves: it is the fixed end of the region, exactly as
    /// it is for a drag, so shift+l then shift+h returns to where it started
    /// rather than walking the whole region right and back. A far corner that
    /// meets the anchor again drops the expanse outright, so a shrunk-to-
    /// nothing region is the plain one-cell cursor and not a 1x1 region that
    /// merely behaves like one.
    ///
    /// Extending from a cursor that sits ON a node keeps that node in the
    /// selection — the anchor's cell is part of its own region — which is what
    /// makes the family an EXTEND rather than a second way to start one.
    pub(crate) fn network_extend(&mut self, dc: i32, dr: i32) -> bool {
        if self.focused_pane != LEFT_MENUBAR_IDX {
            return false;
        }
        let anchor = (self.net.grid_cursor_col, self.net.grid_cursor_row);
        let far = match self.net.grid_cursor_expanse {
            Some((a, far)) if a == anchor => far,
            _ => anchor,
        };
        let far = (far.0 + dc, far.1 + dr);
        self.net.grid_cursor_expanse = (far != anchor).then_some((anchor, far));
        self.net.pan_velocity_x = 0.0;
        self.net.pan_velocity_y = 0.0;
        self.keep_cell_in_view(far.0, far.1);
        true
    }

    /// Copy the selected nodes, positions and all.
    pub(crate) fn copy_selected_nodes(&mut self) {
        let slots = self.selected_slots();
        let dir = self.current_dir();
        self.net.node_clipboard = slots
            .into_iter()
            .filter_map(|i| dir.children.get(i).cloned())
            .collect();
    }

    /// Paste the clipboard at the grid cursor, KEEPING THE SHAPE it was
    /// copied in: the set's top-left corner lands on the cursor cell and
    /// every node keeps its offset from it, so a pasted chain arrives wired
    /// the way it was drawn rather than stacked in a column.
    ///
    /// A node whose cell is taken steps aside to the nearest free one, which
    /// is the one case where the shape gives: a paste that silently sat two
    /// nodes on one crossing would be worse than a paste that is a cell out.
    ///
    /// A pasted node whose name is taken takes the next free one
    /// (`transform1` beside a `transform1` becomes `transform2`), and the
    /// wires between pasted nodes follow, so a pasted chain reads itself
    /// and not the chain it was copied from. A wire to a node that was not
    /// copied still names that node.
    ///
    /// Pasted on a free cursor cell a wire runs through, the paste is
    /// spliced into that wire, as Add Node there would be: one node, or a
    /// chain with one head and one tail, goes in whole.
    pub(crate) fn paste_nodes(&mut self) -> bool {
        if self.net.node_clipboard.is_empty() {
            return false;
        }
        // Whole or not at all: a set that half belongs here is pasted
        // nowhere, rather than as the part that does with its wires cut.
        let here = crate::context::context_at(&self.doc.current_path);
        if let Some(why) = self
            .net.node_clipboard
            .iter()
            .find_map(|n| crate::context::refusal(&n.name, &n.node_type, here))
        {
            self.update_status_text(&format!("Not pasted: {why}"));
            return false;
        }
        let origin = self.net.node_clipboard.iter().fold((f32::MAX, f32::MAX), |(x, y), n| {
            (x.min(n.position.0), y.min(n.position.1))
        });
        let (cx, cy) = (self.net.grid_cursor_col as f32, self.net.grid_cursor_row as f32);
        // Asked before the paste, whose nodes' own wires would touch the cell.
        let free = !self.current_dir().children.iter().any(|c| c.position == (cx, cy));
        let wire = if free { self.graph().input_wire_through_cell(cx, cy) } else { None };
        let mut last = (cx, cy);
        let mut renamed: Vec<(String, String)> = Vec::new();
        let mut pasted: Vec<String> = Vec::new();
        for source in self.net.node_clipboard.clone() {
            let mut node = source;
            regenerate_node_ids(&mut node);
            let want = (cx + node.position.0 - origin.0, cy + node.position.1 - origin.1);
            let (nx, ny) = self.find_empty_cell(want.0, want.1, None);
            node.position = (nx, ny);
            // Added = hidden, same as AddNode: a paste of a displayed node
            // must not become a second visible sibling.
            node.geometry_visible = false;
            let old = node.name.clone();
            if self.current_dir().children.iter().any(|c| c.name == old) {
                node.name = self.get_lowest_unused_name(old.trim_end_matches(|c: char| c.is_ascii_digit()));
            }
            renamed.push((old, node.name.clone()));
            pasted.push(node.id.clone());
            self.current_dir_mut().children.push(node);
            last = (nx, ny);
        }
        let dir = self.current_dir_mut();
        for node in dir.children.iter_mut().filter(|c| pasted.contains(&c.id)) {
            for p in node.params.iter_mut().filter(|p| p.kind() == ParamKind::Node && !p.is_expr()) {
                let wired = p.text().trim().to_string();
                if let Some((_, new)) = renamed.iter().find(|(old, _)| *old == wired) {
                    p.set_text(new.clone());
                }
            }
        }
        if let Some((src_id, dest_id)) = wire {
            self.splice_paste(&pasted, &src_id, &dest_id);
        }
        // The cursor lands on the last node pasted, collapsed — the pasted
        // nodes are new, and the region that selected the originals means
        // nothing about them.
        self.net.grid_cursor_col = last.0 as i32;
        self.net.grid_cursor_row = last.1 as i32;
        self.net.grid_cursor_expanse = None;
        self.sync_nodes();
        self.sync_cursor_and_selection();
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        self.rebuild_scene_geometry();
        self.viewport_dirty = true;
        true
    }

    /// Splice the nodes just pasted (`ids`) into the wire from `src_id` to
    /// `dest_id`: as one chain, its head the pasted node whose Input reads
    /// no other pasted node, its tail the one no other pasted node reads.
    /// A paste that is not one such chain is left beside the wire.
    fn splice_paste(&mut self, ids: &[String], src_id: &str, dest_id: &str) {
        let dir = self.current_dir();
        let nodes: Vec<&FsNode> = dir.children.iter().filter(|c| ids.contains(&c.id)).collect();
        let input = |n: &FsNode| crate::geometry::node_param_node(n, "input");
        let heads: Vec<&FsNode> = nodes.iter().copied().filter(|n| !input(n).is_some_and(|i| nodes.iter().any(|m| m.name == i))).collect();
        let tails: Vec<&FsNode> = nodes.iter().copied().filter(|n| !nodes.iter().any(|m| input(m).as_deref() == Some(n.name.as_str()))).collect();
        let ([head], [tail], Some(src)) = (heads.as_slice(), tails.as_slice(), dir.children.iter().find(|c| c.id == src_id)) else { return };
        let (head_id, tail_id, src_name) = (head.id.clone(), tail.id.clone(), src.name.clone());
        if splice_chain_into_wire(self.current_dir_mut(), &head_id, &tail_id, src_name.clone(), dest_id) {
            let dest = self.current_dir().children.iter().find(|c| c.id == dest_id).map(|c| c.name.clone()).unwrap_or_default();
            self.update_status_text(&format!("Pasted between {src_name} and {dest}."));
        }
    }

    /// Move the SELECTED nodes one cell, and the cursor with them — so a run
    /// of alt+h drags them across the sheet rather than leaving them behind on
    /// the first press.
    ///
    /// With an expanded cursor that is every node inside it, and the region
    /// travels too: stepping the anchor alone is exactly what collapses a
    /// region, so a selection would be dropped by the first press that moved
    /// it. `network_nav` cannot do that job — for the plain cursor, stepping
    /// off IS the point.
    pub(crate) fn network_move_node(&mut self, dc: i32, dr: i32) -> bool {
        if self.focused_pane != LEFT_MENUBAR_IDX {
            return false;
        }
        let moving = self.selected_slots();
        if !moving.is_empty() {
            for idx in moving {
                let (x, y) = self.current_dir().children[idx].position;
                self.current_dir_mut().children[idx].position = (x + dc as f32, y + dr as f32);
            }
            self.sync_nodes();
            self.sync_layout();
        }
        if self.grid_cursor_expanded() {
            self.net.pan_velocity_x = 0.0;
            self.net.pan_velocity_y = 0.0;
            self.shift_grid_cursor(dc, dr);
            self.net.deselected_cell = None;
            self.keep_cursor_in_view();
            return true;
        }
        self.network_nav(dc, dr)
    }

    /// Pan the network view by one cell, leaving the cursor and the selection
    /// alone — the ctrl+hjkl family, which had no equivalent here.
    ///
    /// One cell, not a fixed pixel count, so a pan step means the same thing
    /// at every zoom level: the sheet moves by exactly one node.
    pub(crate) fn network_pan_view(&mut self, dc: i32, dr: i32) -> bool {
        if self.focused_pane != LEFT_MENUBAR_IDX {
            return false;
        }
        self.net.pan_velocity_x = 0.0;
        self.net.pan_velocity_y = 0.0;
        self.net.pan_x -= dc as f32 * self.net.grid.pitch_x;
        self.net.pan_y -= dr as f32 * self.net.grid.pitch_y;
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        true
    }

    /// Centre the view on the grid cursor, without changing the zoom — the
    /// counterpart of framing everything, and what `f` means in the plugin
    /// this is replacing.
    ///
    /// CENTRES rather than merely scrolling the cursor into view. The first
    /// version called `keep_cursor_in_view`, which pans only when the cursor
    /// has gone off the edge — so the command did nothing at all in the common
    /// case of a cursor that is already visible but off in a corner, which is
    /// exactly when you press it.
    pub(crate) fn frame_cursor(&mut self) -> bool {
        if self.focused_pane != LEFT_MENUBAR_IDX {
            return false;
        }
        let (_px, _py, pw, ph) = self.positions[CONTENT_IDX];
        if pw <= 0.0 || ph <= 0.0 {
            return true;
        }
        // Pan so the cursor cell's centre — the lattice intersection it names
        // — lands at the pane's centre: the pane's half-extent minus the
        // cell's offset from the origin, which is its column times the pitch.
        self.net.pan_x = pw * 0.5 - self.net.grid_cursor_col as f32 * self.net.grid.pitch_x;
        self.net.pan_y = ph * 0.5 - self.net.grid_cursor_row as f32 * self.net.grid.pitch_y;
        self.net.pan_velocity_x = 0.0;
        self.net.pan_velocity_y = 0.0;
        self.sync_grid_settings();
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        true
    }

    /// The extent a node takes up on the sheet at a grid geometry, relative
    /// to the (0, 0) crossing — `(x_min, x_max, y_min, y_max)`. The body,
    /// AND the name label the widget draws to its right: framing the bodies
    /// alone left the labels of the right-hand column cut off, which is what
    /// Frame All exists to avoid.
    ///
    /// The label's placement is `Graph::node_labels`'s rule, repeated here
    /// because the widget offers no query for it: an 8 px gap and a 14 px
    /// font, both scaled with the body against its 80 px baseline, the font
    /// clamped to 6..48, the width the widget's own estimate — the number it
    /// culls the label against, so the two cannot disagree about where a
    /// label ends. The label is centred on the body and shorter than it at
    /// every zoom Frame All can choose, but its own height is folded in
    /// anyway rather than argued away.
    fn node_extent(name: &str, (col, row): (f32, f32), g: &GridGeometry) -> (f32, f32, f32, f32) {
        let x_min = col * g.pitch_x - g.node_w * 0.5;
        let y_min = row * g.pitch_y - g.node_h * 0.5;
        let scale_f = g.node_w / 80.0;
        let font_size = (14.0 * scale_f).clamp(6.0, 48.0);
        let label_w = TextLabel::estimate_width(name, font_size);
        let cy = row * g.pitch_y;
        (
            x_min,
            x_min + g.node_w + 8.0 * scale_f + label_w,
            y_min.min(cy - font_size * 0.5),
            (y_min + g.node_h).max(cy + font_size * 0.5),
        )
    }

    /// How far a chain's box stands out from its nodes and their names:
    /// a share of the body's height, so it scales with the zoom.
    pub fn chain_box_pad(&self) -> f32 {
        self.net.grid.node_h * 0.55
    }

    /// Every wired chain of the level the network shows, with the
    /// window-space box round it: its bodies and their name labels
    /// (`node_extent`, the rule Frame All fits), padded by
    /// `chain_box_pad`. Read off the graph widget's own nodes, so the boxes
    /// are round what is drawn.
    pub fn chain_boxes(&self) -> Vec<(Vec<usize>, (f32, f32, f32, f32))> {
        let nodes = self.graph().get_nodes();
        let g = &self.net.grid;
        let (ox, oy) = self.network_origin();
        let (ox, oy) = (ox + self.net.pan_x, oy + self.net.pan_y);
        let pad = self.chain_box_pad();
        wired_chains(&nodes)
            .into_iter()
            .map(|chain| {
                let (x0, x1, y0, y1) = chain.iter().fold(
                    (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY),
                    |(ax0, ax1, ay0, ay1), &i| {
                        let (x0, x1, y0, y1) = Self::node_extent(&nodes[i].name, nodes[i].position, g);
                        (ax0.min(x0), ax1.max(x1), ay0.min(y0), ay1.max(y1))
                    },
                );
                let r = (ox + x0 - pad, oy + y0 - pad, x1 - x0 + pad * 2.0, y1 - y0 + pad * 2.0);
                (chain, r)
            })
            .collect()
    }

    /// The chain whose box holds (px, py) while the network overlays the
    /// scene — the smallest, where boxes overlap — or None. Inside it the
    /// pointer is the network's (`overlay_claims`), empty space and all.
    pub fn chain_box_at(&self, px: f32, py: f32) -> Option<(Vec<usize>, (f32, f32, f32, f32))> {
        if !self.network_overlay() {
            return None;
        }
        self.chain_boxes()
            .into_iter()
            .filter(|(_, (x, y, w, h))| px >= *x && px < x + w && py >= *y && py < y + h)
            .min_by(|a, b| (a.1 .2 * a.1 .3).total_cmp(&(b.1 .2 * b.1 .3)))
    }

    /// The box to show: the chain under a free pointer that no floating
    /// pane covers, as its sorted node ids and its rect. A captured pointer
    /// — an orbit, a drag, the dialog — shows none.
    pub(crate) fn chain_box_target(&self) -> Option<(Vec<String>, (f32, f32, f32, f32))> {
        if self.pointer_captured() || self.over_floating_pane_at(self.cursor_x, self.cursor_y) {
            return None;
        }
        let (chain, r) = self.chain_box_at(self.cursor_x, self.cursor_y)?;
        let level = self.current_dir();
        let mut ids: Vec<String> = chain.iter().filter_map(|&i| level.children.get(i).map(|c| c.id.clone())).collect();
        ids.sort();
        Some((ids, r))
    }

    /// Whether the box shown is not the one the pointer asks for: what
    /// pointer motion repaints for, since the tick that eases the box runs
    /// only on a frame something asked for.
    pub(crate) fn chain_box_stale(&self) -> bool {
        let target = self.chain_box_target().map(|(ids, _)| ids);
        match (&self.net.chain_box, target) {
            (Some(b), Some(ids)) => b.ids != ids || b.alpha < 1.0,
            (Some(_), None) => true,
            (None, Some(_)) => true,
            (None, None) => false,
        }
    }

    /// Ease the chain box toward the chain under the pointer: it fades in
    /// over a chain, glides to another, and fades out where it was when
    /// the pointer leaves. Over its own chain it follows the chain exactly,
    /// so a pan or a zoom does not leave it behind. True while it moves.
    pub(crate) fn animate_chain_box(&mut self, dt: f32) -> bool {
        let ease = |k: f32| 1.0 - (-k * dt.max(1e-4)).exp();
        let target = self.chain_box_target();
        match (&mut self.net.chain_box, target) {
            (Some(b), Some((ids, (tx, ty, tw, th)))) => {
                let moving = if b.ids == ids && b.alpha > 0.5 {
                    (b.x, b.y, b.w, b.h) = (tx, ty, tw, th);
                    false
                } else {
                    let f = ease(20.0);
                    b.x += (tx - b.x) * f;
                    b.y += (ty - b.y) * f;
                    b.w += (tw - b.w) * f;
                    b.h += (th - b.h) * f;
                    let near = (tx - b.x).abs() < 0.3 && (ty - b.y).abs() < 0.3 && (tw - b.w).abs() < 0.3 && (th - b.h).abs() < 0.3;
                    if near {
                        (b.x, b.y, b.w, b.h) = (tx, ty, tw, th);
                        b.ids = ids;
                    }
                    !near
                };
                b.alpha += (1.0 - b.alpha) * ease(16.0);
                if b.alpha > 0.995 {
                    b.alpha = 1.0;
                }
                moving || b.alpha < 1.0
            }
            (Some(b), None) => {
                b.alpha -= b.alpha * ease(12.0);
                if b.alpha < 0.02 {
                    self.net.chain_box = None;
                }
                true
            }
            (None, Some((ids, (x, y, w, h)))) => {
                self.net.chain_box = Some(ChainBox { ids, x, y, w, h, alpha: 0.0 });
                true
            }
            (None, None) => false,
        }
    }

    /// The union of `node_extent` over the current level, or None when the
    /// level is empty.
    fn level_extent(&self, g: &GridGeometry) -> Option<(f32, f32, f32, f32)> {
        self.current_dir().children.iter().fold(None, |acc, child| {
            let (x0, x1, y0, y1) = Self::node_extent(&child.name, child.position, g);
            Some(match acc {
                None => (x0, x1, y0, y1),
                Some((ax0, ax1, ay0, ay1)) => (ax0.min(x0), ax1.max(x1), ay0.min(y0), ay1.max(y1)),
            })
        })
    }

    /// Fit every node in the current level — body and name label — into the
    /// network pane.
    ///
    /// Extracted verbatim from the `f` key's inline arm so the command
    /// registry and the key run the same code — the point of the registry
    /// being that there is one implementation behind every way of asking.
    pub(crate) fn frame_all_nodes(&mut self) {
        // The framing baseline is the configured geometry (100% zoom),
        // scaled down until everything fits — Frame All never zooms in
        // past 100%.
        let base = configured_grid_geometry();
        let at = |f: f32| base.scaled(f);
        let (_px, _py, pw, ph) = self.positions[CONTENT_IDX];
        let padding = 40.0;
        let padded_w = (pw - 2.0 * padding).max(10.0);
        let padded_h = (ph - 2.0 * padding).max(10.0);

        if self.level_extent(&base).is_none() {
            self.set_grid_geometry(base);
            self.net.pan_x = 20.0;
            self.net.pan_y = 20.0;
        } else {
            // The extent is not linear in the zoom: a label's font stops
            // shrinking at 6 px and its width is rounded up, so the fit
            // computed at 100% overstates how much a small zoom saves. Each
            // pass refits at the zoom the last one chose; the factor only
            // ever falls, and a pass that changes nothing ends it.
            let mut f = 1.0f32;
            for _ in 0..4 {
                let (x0, x1, y0, y1) = self.level_extent(&at(f)).unwrap();
                let fit = (padded_w / (x1 - x0)).min(padded_h / (y1 - y0)).min(1.0);
                if fit >= 1.0 {
                    break;
                }
                f *= fit;
            }
            f = f.max(MIN_PITCH_X / base.pitch_x);

            self.set_grid_geometry(at(f));
            let (x0, x1, y0, y1) = self.level_extent(&at(f)).unwrap();
            self.net.pan_x = (pw - (x1 - x0)) / 2.0 - x0;
            self.net.pan_y = (ph - (y1 - y0)) / 2.0 - y0;
        }

        self.sync_grid_settings();
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
    }

    pub fn read_panel_offsets(&mut self) {
        let updated_nodes = self.graph().get_nodes();
        let dir = self.current_dir_mut();
        for (i, node) in updated_nodes.iter().enumerate() {
            if let Some(child) = dir.children.get_mut(i) {
                child.position = node.position;
            }
        }
        // Not while a group drag is carrying the selection: the anchor is
        // holding the region still on purpose, and yanking it onto the
        // dragged node's cell would collapse the selection mid-gesture.
        if self.net.node_drag_group.is_some() {
            return;
        }
        if let Some(sel_idx) = self.graph().selected_node() {
            if let Some(node) = updated_nodes.get(sel_idx) {
                self.net.grid_cursor_col = node.position.0 as i32;
                self.net.grid_cursor_row = node.position.1 as i32;
            }
        }
    }

    pub fn sync_cursor_and_selection(&mut self) {
        if self.focused_pane != LEFT_MENUBAR_IDX {
            return;
        }
        let dir = self.current_dir();
        let mut node_at_cursor_idx = None;
        for (i, node) in dir.children.iter().enumerate() {
            if node.position.0 as i32 == self.net.grid_cursor_col && node.position.1 as i32 == self.net.grid_cursor_row {
                node_at_cursor_idx = Some(i);
                break;
            }
        }

        // A selection that arrived from anywhere else — a click, a load, the
        // params pane — spends the remembered deselect. Otherwise clicking the
        // very node you deselected would clear itself again on this sync.
        if self.graph().selected_node().is_some() {
            self.net.deselected_cell = None;
        }

        // An explicit deselect holds for the cell it happened in, and for no
        // other: step away and selection resumes by itself.
        let node_at_cursor_idx = match self.net.deselected_cell {
            Some(cell) if cell == (self.net.grid_cursor_col, self.net.grid_cursor_row) => None,
            _ => node_at_cursor_idx,
        };

        if let Some(idx) = node_at_cursor_idx {
            self.graph_mut().set_selected_node(Some(idx));
            if self.focused_widget != Some(CONTENT_IDX) {
                self.focused_widget = Some(CONTENT_IDX);
            }
        } else {
            self.graph_mut().set_selected_node(None);
            if self.focused_widget == Some(CONTENT_IDX) {
                self.focused_widget = None;
            }
        }
    }

    pub fn sync_cursor_and_selection_from_loaded(&mut self) {
        if let Some(sel_idx) = self.graph().selected_node() {
            let pos = {
                let dir = self.current_dir();
                if sel_idx < dir.children.len() {
                    Some(dir.children[sel_idx].position)
                } else {
                    None
                }
            };
            if let Some((pos_x, pos_y)) = pos {
                self.net.grid_cursor_col = pos_x as i32;
                self.net.grid_cursor_row = pos_y as i32;
            }
        } else {
            self.sync_cursor_and_selection();
        }
    }
}
