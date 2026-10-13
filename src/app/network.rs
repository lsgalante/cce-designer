//! `State::net`: the network editor's own state — the grid cursor and the
//! selection it makes, the lattice and its zoom, the pan and its fling, and
//! the look of the nodes. The widgets are the roster's; this is what the
//! app keeps beside them.

use super::{DropGlow, FsNode, GridGeometry, NodeDragGroup, OverlayWheel};

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
}
