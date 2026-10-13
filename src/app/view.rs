//! `State::view`: the viewport's display settings that are the app's
//! rather than the viewport widget's — the guides' sizes, the square
//! aspect, the element overlays and their markers, the marked groups and
//! the attribute visualizers. Each is persisted in `ViewportSettings`
//! (state.kdl's viewport block, and a project's display block) and read
//! back by `State::apply_display_settings`. The widget keeps the rest of
//! that block (background, grid colour, the guide switches, RT mode).

pub struct ViewDisplay {
    /// The scene is drawn into the largest square centred in the pane
    /// (`toggle_square_viewport`).
    pub square_viewport: bool,
    /// The guide meshes' sizes: the grid's line thickness, the origin
    /// marker's length and the camera pivot marker's (the length of its
    /// beams; their thickness is fixed). Baked into the guide meshes, so a
    /// change re-stages the one it feeds.
    pub grid_thickness: f32,
    pub origin_size: f32,
    pub camera_pivot_size: f32,
    /// World-unit radius of the Show Point Markers overlay.
    pub point_marker_size: f32,
    /// sRGB color of the Show Point Markers overlay.
    pub point_marker_color: [f32; 3],
    /// The element overlay switches, flipped by their palette commands and
    /// persisted in `ViewportSettings`: markers, numbers and normals for
    /// points, numbers and normals for primitives, and numbers, markers and
    /// normals for vertices.
    pub show_point_markers: bool,
    pub show_point_numbers: bool,
    pub show_point_normals: bool,
    pub show_prim_numbers: bool,
    pub show_prim_normals: bool,
    pub show_vertex_numbers: bool,
    pub show_vertex_markers: bool,
    pub show_vertex_normals: bool,
    /// The point groups whose members wear a marker in the scene, by name
    /// — the Group Markers dialog's switches (`group_markers`), persisted
    /// in the viewport block. A name the scene has no group for stays on
    /// the list and marks nothing, so a switch set for a group that comes
    /// and goes with a frame or an edit is not lost with it.
    pub marked_groups: Vec<String>,
    /// The attribute visualizers, applied in order to the displayed scene
    /// (`crate::visualizer`). Persisted in the viewport block.
    pub visualizers: Vec<crate::visualizer::Visualizer>,
}
