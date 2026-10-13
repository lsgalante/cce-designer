//! `State::markers`: what is drawn ON the scene — the marker instances
//! (the selected group's members, the marked groups', the spreadsheet's
//! selected rows, Show Point and Vertex Markers), the pull arrows, the
//! normals' whiskers, and the element numbers with how much of each shows
//! through the fill in front of it. Each `StagedMesh` is a list staged
//! CPU-side and flushed to its mesh in `State::stage`'s
//! `SceneMeshes` (`flush_pending_meshes`); the positions kept beside them
//! let a size change rebuild the markers without an evaluation.

use super::{StagedMesh, Vertex3D};

/// What a list keyed by the selection was built for: the node's id, its
/// parameters' texts, and the geometry version.
pub type SelectionKey = (String, Vec<(String, String)>, u64);

pub struct SceneMarkers {
    /// Selected-Group membership markers: marker vertices staged CPU-side by
    /// `sync_nodes` whenever the selection is a Group node (empty otherwise),
    /// flushed to `meshes.group_points`; its count gates the draw. The key
    /// (`last_group_points_key`) — (node id, params, geometry version) —
    /// spares the re-evaluation on unrelated `sync_nodes` runs.
    pub group_markers: StagedMesh,
    pub last_group_points_key: Option<SelectionKey>,
    /// The selected Group's member positions, kept from the evaluation so
    /// the markers can be re-SIZED without re-evaluating the node — a
    /// marker size change (the viewport menu's slider, per motion of a
    /// drag) only rebuilds the spheres (`rebuild_group_markers`).
    pub group_members: Vec<Vertex3D>,
    /// The marker radius `group_markers` was built at.
    pub last_group_marker_size: f32,
    /// The marked groups' markers, staged by `rebuild_marked_group_markers`,
    /// flushed to `meshes.marked_points`.
    pub marked_group_markers: StagedMesh,
    /// The spreadsheet's selected rows, shown in the scene: a marker on the
    /// point each row is, staged by `rebuild_row_markers` from where the
    /// rows' points were when the table was last filled (`State::sheet`'s
    /// `spreadsheet_points`) and flushed to `meshes.row_points`.
    pub row_markers: StagedMesh,
    /// Pull arrows: while the params pane shows an Attribute node that moves
    /// points (`geometry::moves_points`), an arrow from where each of a
    /// spread of its points was to where the node puts it — staged by
    /// `sync_pull_arrows`, flushed to `meshes.pull_arrows`. Keyed like the
    /// group markers, by (node id, params, geometry version).
    pub pull_arrows: StagedMesh,
    /// The sampled `(before, after)` pairs the arrows were built from, kept
    /// so Pull Arrow Scale can re-draw them without re-evaluating the node
    /// (`rebuild_pull_arrow_verts`).
    pub pull_arrow_pairs: Vec<(glam::Vec3, glam::Vec3)>,
    pub last_pull_arrows_key: Option<SelectionKey>,
    /// The radii the marker spheres were last uploaded at — group, point,
    /// vertex — so the flush re-uploads them when a size moves.
    pub marker_sphere_radii: Option<[f32; 3]>,
    /// The point overlays on the visible scene, rebuilt with it: marker
    /// geometry for Show Point Markers, and (position, vertex index) labels
    /// for Show Point Numbers (`overlay_number_labels`) — the labels project
    /// through `State::camera`'s `last_scene_mvp` into 2D text each frame.
    ///
    /// These were per-node preferences on a hidden `meta` child until
    /// 2026-09-23, so seeing the point numbering of what was on screen meant
    /// diving into each node and flipping its own switch. They are display
    /// settings, and display settings belong to the view: three commands in
    /// the palette (`toggle_point_markers` / `_numbers` / `_normals`) over
    /// the switches on `State::view`, persisted in `ViewportSettings` beside
    /// Show Grid.
    ///
    /// The point markers, the vertex markers and the normals' whiskers go
    /// up together (`mark_overlay_dirty`), as one rebuild stages them.
    pub point_markers: StagedMesh,
    /// The scene's point positions, kept by `rebuild_scene_geometry` while
    /// Show Point Markers is on (empty otherwise), so the markers can be
    /// re-SIZED without re-evaluating the graph — the viewport menu's Point
    /// Marker Size slider does that on every motion of a drag
    /// (`rebuild_overlay_markers`).
    pub overlay_marker_points: Vec<Vertex3D>,
    /// The Show Vertex Markers instances, built beside the points'
    /// (`rebuild_overlay_markers`) and flushed with them.
    pub vertex_markers: StagedMesh,
    /// The same for Show Vertex Markers: where each vertex's marker
    /// stands, inset from its point as its number is.
    pub overlay_vertex_marker_points: Vec<Vertex3D>,
    /// Show Point Normals: LINE_LIST whiskers from each distinct point along
    /// its smooth vertex normal (computed from topology — the kernel outputs
    /// carry only a default up-normal attribute).
    pub overlay_normals: StagedMesh,
    /// Show Point Numbers' labels: (position, point index).
    pub overlay_number_labels: Vec<([f32; 3], u32)>,
    /// How much of each label above shows through the fill in front of its
    /// point (`geometry::point_transmittance`), worked out by the stage
    /// pass for the eye it staged. The labels are 2D text, so this is the
    /// only occlusion they get. Emptied with every rebuild of the labels; a
    /// label with no entry draws whole.
    pub overlay_number_alpha: Vec<f32>,
    /// Show Primitive Numbers and Show Vertex Numbers, the same two lists
    /// each: a primitive's label stands at its centroid, a vertex's inside
    /// its primitive, part of the way from its point to that centroid, so
    /// the vertices that share a point are told apart.
    pub overlay_prim_labels: Vec<([f32; 3], u32)>,
    pub overlay_prim_alpha: Vec<f32>,
    pub overlay_vertex_labels: Vec<([f32; 3], u32)>,
    pub overlay_vertex_alpha: Vec<f32>,
}
