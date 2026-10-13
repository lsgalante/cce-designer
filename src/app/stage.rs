//! `State::stage`: what the app has staged for the renderer. Only the
//! engine's hooks see the renderer (`renderer_init`, `App3D::stage_renderer`),
//! so GPU mesh updates are made CPU-side and flushed there
//! (`flush_pending_meshes`), and the raster scene is staged again only when
//! what it is staged from moved (`stage_frame`, by `StageKey`). The
//! persistent meshes, the marker and arrow lists' `StagedMesh` and the key
//! are defined here too.

use glam::Vec3;

use super::Vertex3D;

pub struct SceneStage {
    /// GPU meshes — `None` until `renderer_init`.
    pub meshes: Option<SceneMeshes>,
    /// CPU-staged mesh updates, flushed in `stage_renderer`.
    pub pending_grid: Option<Vec<Vertex3D>>,
    pub pending_origin: Option<Vec<Vertex3D>>,
    pub pending_pivot: Option<Vec<Vertex3D>>,
    pub pending_viewport_bg: Option<Vec<Vertex3D>>,
    /// The spheres mesh needs re-upload from the scene's `rt_sphere_verts`
    /// (or its smooth fill).
    pub spheres_dirty: bool,
    /// How many vertices the scene's fill has as last presented
    /// (`present_scene`) — the count the spheres mesh's draw reads.
    pub vertex_count_spheres: u32,
    /// What the raster scene was last staged from (`stage_frame`): a
    /// frame whose `StageKey` is this one, and that nothing marked
    /// `viewport_dirty`, keeps the staged backdrop.
    pub last_viewport: StageKey,
    /// What the fill mesh was last SORTED for: (geometry version, smooth,
    /// eye in mesh space). The stage pass re-sorts and re-uploads when any
    /// of the three moves while see-through is in effect — which during an
    /// orbit is every frame — and clears it when see-through ends, so the
    /// next entry sorts afresh.
    pub sorted_fill_key: Option<(u64, bool, [f32; 3])>,
    /// The traced scene as it was last handed over: the geometry's version,
    /// the image's, and the world unit's bits, which size the image.
    pub last_rt_scene_key: Option<(u64, u64, u32)>,
    /// Whether `renderer_init` has run before. There is no separate reconnect
    /// callback: the runner calls `renderer_init` once per renderer, so the
    /// first call is this process's own and every later one is a REPLACEMENT
    /// after a reconnect. Remembering is the only way to tell them apart.
    pub seen_renderer: bool,
}

/// Everything the raster scene is staged from, as `stage_frame` compares
/// it frame to frame: the camera's pose, the viewport widget's view and
/// guides, the pane's size in physical px, and the draw-time render
/// settings. A frame whose key equals the last one staged (and that
/// nothing marked `viewport_dirty`) reuses the staged backdrop.
///
/// `show_viewport` is false until the first stage and wherever a hidden or
/// zero-area pane cleared the backdrop, which forces the next stage: the
/// staging branch only runs while the viewport is shown. The rest stay
/// readable — Frame All and the image commands read the pane size and the
/// camera off the last stage.
#[derive(Clone, PartialEq)]
pub struct StageKey {
    pub camera_pos: Vec3,
    pub camera_rx: f32,
    pub camera_ry: f32,
    pub camera_rz: f32,
    pub pivot: Vec3,
    pub zoom: f32,
    pub rotation_x: f32,
    pub rotation_y: f32,
    pub bg_color: [f32; 3],
    pub show_grid: bool,
    pub show_origin: bool,
    pub show_camera_pivot: bool,
    pub width: u32,
    pub height: u32,
    pub active_camera: String,
    pub show_viewport: bool,
    pub wireframe: bool,
    pub geo_opacity: f32,
    pub wire_single_color: bool,
    pub wire_color: [f32; 3],
    pub wire_opacity: f32,
    pub wire_width: f32,
    pub rt_mode: bool,
}

impl Default for StageKey {
    /// Before the first stage: nothing staged, and a key no frame matches.
    fn default() -> Self {
        Self {
            camera_pos: Vec3::ZERO,
            camera_rx: 0.0,
            camera_ry: 0.0,
            camera_rz: 0.0,
            pivot: Vec3::ZERO,
            zoom: 0.0,
            rotation_x: 0.0,
            rotation_y: 0.0,
            bg_color: [0.0; 3],
            show_grid: false,
            show_origin: false,
            show_camera_pivot: false,
            width: 0,
            height: 0,
            active_camera: String::new(),
            show_viewport: false,
            wireframe: false,
            geo_opacity: 1.0,
            wire_single_color: false,
            wire_color: [1.0, 1.0, 1.0],
            wire_opacity: 1.0,
            wire_width: 1.0,
            rt_mode: false,
        }
    }
}

/// One mesh's contents, staged CPU-side for the next flush
/// (`flush_pending_meshes`), since only the engine's hooks see the renderer:
/// the list, whether it changed since the GPU had it, and how many the GPU
/// holds — the count a draw reads, which is the list's length at the last
/// upload rather than now. The markers' lists are INSTANCES (cce-ui's
/// `SceneDraw::instances`: a marker's place and colour, drawn over its
/// sphere); the normals' whiskers and the pull arrows are LINE_LIST
/// vertices.
#[derive(Default)]
pub struct StagedMesh {
    pub verts: Vec<Vertex3D>,
    pub dirty: bool,
    pub count: u32,
}

impl StagedMesh {
    /// Upload the list to `mesh` if it changed since the last upload, and
    /// say whether it did.
    pub fn flush(&mut self, renderer: &mut cce_ui::vk::VkRenderer, mesh: cce_ui::vk::MeshId) -> bool {
        if !std::mem::take(&mut self.dirty) {
            return false;
        }
        renderer.update_mesh(mesh, bytemuck::cast_slice(&self.verts));
        self.count = self.verts.len() as u32;
        true
    }
}

/// The designer's persistent 3D meshes, created in `renderer_init` once the
/// engine's renderer exists.
#[derive(Clone, Copy)]
pub struct SceneMeshes {
    pub viewport_bg: cce_ui::vk::MeshId,
    pub spheres: cce_ui::vk::MeshId,
    /// LINE_LIST edge expansion of `spheres` (vertex pairs per triangle
    /// edge) — the wire pass draws real line primitives, never
    /// PolygonMode::LINE (driver-broken; see cce-ui's scene stage).
    pub sphere_edges: cce_ui::vk::MeshId,
    pub grid: cce_ui::vk::MeshId,
    pub origin: cce_ui::vk::MeshId,
    pub pivot: cce_ui::vk::MeshId,
    /// The marker spheres the marker draws instance (`geometry::marker_sphere`,
    /// white): the group markers' at Group Marker Size — the selected group,
    /// the marked groups and the spreadsheet's rows share it — the points'
    /// at Point Marker Size and the vertices' at `VERTEX_MARKER_SCALE` of
    /// it. Re-uploaded when a size moves (`State::markers.marker_sphere_radii`),
    /// which is 240 vertices whatever the scene.
    pub group_sphere: cce_ui::vk::MeshId,
    pub point_sphere: cce_ui::vk::MeshId,
    pub vertex_sphere: cce_ui::vk::MeshId,
    /// Selected-Group membership markers: while a Group node is selected, one
    /// marker per vertex it tags, so the selection SHOWS the group. This and
    /// the other marker meshes below hold INSTANCES (cce-ui's
    /// `SceneDraw::instances`) — a marker's place and colour — drawn over
    /// their sphere.
    pub group_points: cce_ui::vk::MeshId,
    /// Markers on the points whose rows are selected in the spreadsheet.
    pub row_points: cce_ui::vk::MeshId,
    /// Markers on the members of the marked groups (the Group Markers dialog).
    pub marked_points: cce_ui::vk::MeshId,
    /// The Show Point Markers overlay.
    pub overlay_points: cce_ui::vk::MeshId,
    /// The Show Vertex Markers overlay.
    pub vertex_points: cce_ui::vk::MeshId,
    /// The Show Point Normals overlay (LINE_LIST whiskers).
    pub overlay_normals: cce_ui::vk::MeshId,
    /// Pull arrows (LINE_LIST): while a point-moving Attribute node is
    /// selected, how far and which way it moves a spread of the points.
    pub pull_arrows: cce_ui::vk::MeshId,
}
